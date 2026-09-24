use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

pub const MAX_OUTPUT: usize = 1_000_000;
const PROCESS_POLL: Duration = Duration::from_millis(50);
const SENSITIVE_ENVIRONMENT: &[&str] = &[
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "ANTHROPIC_API_KEY",
    "RADY_GEMINI_API_KEY",
    "RADY_CEREBRAS_API_KEY",
    "RADY_XAI_API_KEY",
    "RADY_GROQ_API_KEY",
    "RADY_CLOUDFLARE_API_TOKEN",
    "RADY_CLOUDFLARE_ACCOUNT_ID",
    "RADY_OPENROUTER_API_KEY",
    "RADY_LAYA_API_KEY",
    "RADY_APP_PRIVATE_KEY",
    "RADY_APP_PRIVATE_KEY_FILE",
    "RADY_APP_CLIENT_ID",
    "RADY_APP_ID",
    "RADY_APP_TOKEN_COMMAND",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_HOST",
    "GH_REPO",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "RADY_PUSH_TOKEN",
];

#[derive(Debug)]
pub struct ProcessOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
enum StreamMessage {
    Data(Stream, Vec<u8>),
    Done,
    Failed(String),
}

#[derive(Clone, Copy, Debug)]
enum Stream {
    Stdout,
    Stderr,
}

#[allow(clippy::too_many_arguments)]
pub fn execute<I, S>(
    program: &OsStr,
    arguments: I,
    directory: &Path,
    prompt: &[u8],
    timeout: Duration,
    environment: &BTreeMap<String, String>,
    combine_output: bool,
    cancel_file: Option<&Path>,
) -> Result<ProcessOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if timeout.is_zero() || timeout > Duration::from_secs(86_400) {
        bail!("timeout must be between one second and 24 hours");
    }
    if cancellation_requested(cancel_file)? {
        bail!("run cancelled");
    }
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    scrub_environment(&mut command);
    command.envs(environment);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("could not start {}", program.to_string_lossy()))?;
    let pid = child.id();
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("child input was unavailable"))?;
    let input_data = prompt.to_vec();
    let writer = thread::spawn(move || -> std::io::Result<()> { input.write_all(&input_data) });

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("child output was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("child error output was unavailable"))?;
    let (sender, receiver) = mpsc::sync_channel(16);
    let readers = [
        read_stream(stdout, Stream::Stdout, sender.clone()),
        read_stream(stderr, Stream::Stderr, sender),
    ];
    let deadline = Instant::now() + timeout;
    let mut out = Vec::with_capacity(16_384);
    let mut err = Vec::with_capacity(4_096);
    let mut done = 0;
    let mut status: Option<ExitStatus> = None;

    let result = loop {
        match cancellation_requested(cancel_file) {
            Ok(true) => {
                terminate_group(pid, &mut child);
                break Err(anyhow!("run cancelled"));
            }
            Ok(false) => {}
            Err(error) => {
                terminate_group(pid, &mut child);
                break Err(error);
            }
        }
        if Instant::now() >= deadline {
            terminate_group(pid, &mut child);
            break Err(anyhow!("command exceeded its timeout"));
        }
        match receiver.recv_timeout(PROCESS_POLL) {
            Ok(StreamMessage::Data(stream, bytes)) => {
                if out.len() + err.len() + bytes.len() > MAX_OUTPUT {
                    terminate_group(pid, &mut child);
                    break Err(anyhow!("command produced too much output (limit 1 MB)"));
                }
                if combine_output || matches!(stream, Stream::Stdout) {
                    out.extend_from_slice(&bytes);
                } else {
                    err.extend_from_slice(&bytes);
                }
            }
            Ok(StreamMessage::Done) => done += 1,
            Ok(StreamMessage::Failed(message)) => {
                terminate_group(pid, &mut child);
                break Err(anyhow!(message));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) if done < 2 => {
                terminate_group(pid, &mut child);
                break Err(anyhow!("command output readers stopped unexpectedly"));
            }
            Err(RecvTimeoutError::Disconnected) => {}
        }
        if status.is_none() {
            status = child.try_wait().context("could not query command status")?;
        }
        if status.is_some() && done == 2 {
            break Ok(ProcessOutput {
                code: status.and_then(|value| value.code()).unwrap_or(-1),
                stdout: decode_output(out),
                stderr: decode_output(err),
            });
        }
    };

    drop(receiver);
    if status.is_none() {
        let _ = child.wait();
    }
    let _ = writer.join();
    for reader in readers {
        let _ = reader.join();
    }
    result
}

fn decode_output(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn cancellation_requested(path: Option<&Path>) -> Result<bool> {
    path.map(Path::try_exists)
        .transpose()
        .context("could not read cancellation state")
        .map(Option::unwrap_or_default)
}

fn read_stream(
    mut stream: impl Read + Send + 'static,
    kind: Stream,
    sender: mpsc::SyncSender<StreamMessage>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 32_768];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(StreamMessage::Done);
                    return;
                }
                Ok(length) => {
                    if sender
                        .send(StreamMessage::Data(kind, buffer[..length].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(StreamMessage::Failed(error.to_string()));
                    return;
                }
            }
        }
    })
}

fn terminate_group(pid: u32, child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(pid) {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn scrub_environment(command: &mut Command) {
    for (name, _) in env::vars_os() {
        if sensitive_environment_name(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
}

#[must_use]
pub fn safe_environment() -> BTreeMap<String, String> {
    env::vars()
        .filter(|(name, _)| !sensitive_environment_name(name))
        .collect()
}

fn sensitive_environment_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    SENSITIVE_ENVIRONMENT.contains(&name.as_str())
        || name.ends_with("_API_KEY")
        || name.ends_with("_TOKEN")
        || name.ends_with("_SECRET")
        || name.ends_with("_PASSWORD")
        || name.ends_with("_PRIVATE_KEY")
        || name.ends_with("_CREDENTIAL")
        || name.ends_with("_CREDENTIALS")
        || name.ends_with("_COOKIE")
        || name.starts_with("AWS_")
        || name.starts_with("AZURE_")
        || name.starts_with("GCP_")
        || name.starts_with("GOOGLE_")
        || matches!(
            name.as_str(),
            "API_KEY"
                | "TOKEN"
                | "SECRET"
                | "PASSWORD"
                | "PRIVATE_KEY"
                | "SSH_AUTH_SOCK"
                | "GIT_ASKPASS"
                | "GIT_SSH_COMMAND"
                | "DOCKER_CONFIG"
                | "NETRC"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn output_decode_preserves_valid_and_replaces_malformed_utf8() {
        for (bytes, expected) in [
            (b"ready\n".as_slice(), "ready\n"),
            (&[b'r', 0x80, b'y'], "r\u{fffd}y"),
        ] {
            assert_eq!(decode_output(bytes.to_vec()), expected);
        }
    }

    #[test]
    fn real_process_output_is_bounded_and_collected() -> Result<()> {
        let output = execute(
            OsStr::new("sh"),
            ["-c", "printf out; printf err >&2"],
            Path::new("."),
            b"",
            Duration::from_secs(2),
            &BTreeMap::new(),
            false,
            None,
        )?;
        assert_eq!(output.code, 0);
        assert_eq!(output.stdout, "out");
        assert_eq!(output.stderr, "err");
        let temporary = tempdir()?;
        let cancelled = temporary.path().join("cancelled");
        std::fs::write(&cancelled, b"cancelled\n")?;
        let error = execute(
            OsStr::new("sh"),
            ["-c", "printf should-not-run"],
            Path::new("."),
            b"",
            Duration::from_secs(2),
            &BTreeMap::new(),
            false,
            Some(&cancelled),
        )
        .expect_err("cancelled work must not start");
        assert!(error.to_string().contains("cancelled"));
        let live_marker = temporary.path().join("live-cancelled");
        let writer = {
            let live_marker = live_marker.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(25));
                std::fs::write(live_marker, b"cancelled\n")
            })
        };
        let error = execute(
            OsStr::new("sh"),
            ["-c", "while :; do :; done"],
            Path::new("."),
            b"",
            Duration::from_secs(2),
            &BTreeMap::new(),
            false,
            Some(&live_marker),
        )
        .expect_err("active work must stop when cancelled");
        writer.join().unwrap()?;
        assert!(error.to_string().contains("cancelled"));
        Ok(())
    }

    #[test]
    fn unsafe_environment_values_are_removed() {
        let values = safe_environment();
        for name in SENSITIVE_ENVIRONMENT {
            assert!(!values.contains_key(*name));
        }
        for name in [
            "ACME_API_KEY",
            "RADY_LAYA_API_KEY",
            "CLOUD_TOKEN",
            "AWS_PROFILE",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "SSH_AUTH_SOCK",
            "DOCKER_CONFIG",
        ] {
            assert!(sensitive_environment_name(name), "{name}");
        }
        for name in ["HOME", "PATH", "CARGO_HOME", "TOKENIZERS_PARALLELISM"] {
            assert!(!sensitive_environment_name(name), "{name}");
        }
    }
}
