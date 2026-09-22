use std::borrow::Cow;
use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use clap::ValueEnum;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempfile::tempdir;

use crate::Result;

pub const MAX_OUTPUT: usize = 1_000_000;
const MAX_STRUCTURED_RESPONSE: usize = 64_000;
const PROCESS_POLL: Duration = Duration::from_millis(50);
const SENSITIVE_ENVIRONMENT: &[&str] = &[
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "ANTHROPIC_API_KEY",
    "RADY_GEMINI_API_KEY",
    "RADY_CEREBRAS_API_KEY",
    "RADY_XAI_API_KEY",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_HOST",
    "GH_REPO",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "RADY_PUSH_TOKEN",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Codex,
    Claude,
    Command,
}

impl Harness {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Command => "command",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageRecord {
    pub harness: String,
    pub known: bool,
    #[serde(flatten)]
    pub values: BTreeMap<String, u64>,
}

#[derive(Debug, Default)]
pub struct Usage {
    max_tokens: Option<u64>,
    records: Vec<UsageRecord>,
    missing: bool,
}

impl Usage {
    pub fn new(max_tokens: Option<u64>) -> Result<Self> {
        if max_tokens == Some(0) {
            bail!("token budget must be a positive whole number");
        }
        Ok(Self {
            max_tokens,
            records: Vec::new(),
            missing: false,
        })
    }

    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.records
            .iter()
            .filter_map(|record| record.values.get("total_tokens"))
            .sum()
    }

    pub fn before_call(&self) -> Result<()> {
        let Some(maximum) = self.max_tokens else {
            return Ok(());
        };
        if self.missing {
            bail!("token usage is unavailable, so Rady cannot enforce the budget");
        }
        if self.total_tokens() >= maximum {
            bail!("token budget is exhausted");
        }
        Ok(())
    }

    pub fn record(
        &mut self,
        harness: Harness,
        values: Option<BTreeMap<String, u64>>,
    ) -> Result<()> {
        let known = values.is_some();
        if !known {
            self.missing = true;
        }
        self.records.push(UsageRecord {
            harness: harness.as_str().to_owned(),
            known,
            values: values.unwrap_or_default(),
        });
        if !known && self.max_tokens.is_some() {
            bail!("token usage is unavailable, so Rady cannot enforce the budget");
        }
        if self
            .max_tokens
            .is_some_and(|maximum| self.total_tokens() > maximum)
        {
            bail!("token budget was exceeded");
        }
        Ok(())
    }

    pub fn record_unknown(&mut self, harness: Harness) {
        let _ = self.record(harness, None);
    }

    #[must_use]
    pub fn complete(&self, agents: usize) -> bool {
        !self.missing && agents == 1
    }

    #[must_use]
    pub fn records(&self) -> &[UsageRecord] {
        &self.records
    }

    #[must_use]
    pub const fn maximum(&self) -> Option<u64> {
        self.max_tokens
    }
}

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
    for name in SENSITIVE_ENVIRONMENT {
        command.env_remove(name);
    }
}

#[must_use]
pub fn safe_environment() -> BTreeMap<String, String> {
    env::vars()
        .filter(|(name, _)| !SENSITIVE_ENVIRONMENT.contains(&name.as_str()))
        .collect()
}

pub fn executable(harness: Harness) -> Result<PathBuf> {
    if harness == Harness::Command {
        let arguments = split_command(&env::var("RADY_AGENT_COMMAND").unwrap_or_default())?;
        let first = arguments.first().ok_or_else(|| {
            anyhow!("set RADY_AGENT_COMMAND to an installed non-interactive agent command")
        })?;
        return which(first).ok_or_else(|| {
            anyhow!("set RADY_AGENT_COMMAND to an installed non-interactive agent command")
        });
    }
    let name = harness.as_str();
    let binary = which(name).ok_or_else(|| anyhow!("install {name}, then sign in with its CLI"))?;
    let arguments: &[&str] = if harness == Harness::Codex {
        &["login", "status"]
    } else {
        &["auth", "status", "--text"]
    };
    let result = execute(
        binary.as_os_str(),
        arguments,
        Path::new("."),
        b"",
        Duration::from_secs(30),
        &BTreeMap::new(),
        false,
        None,
    )?;
    let logged_in = result.code == 0
        && (harness != Harness::Codex
            || format!("{}{}", result.stdout, result.stderr)
                .to_ascii_lowercase()
                .contains("chatgpt"));
    if !logged_in {
        let login = if harness == Harness::Codex {
            "codex login with ChatGPT"
        } else {
            "claude auth login"
        };
        bail!("run {login} before using Rady");
    }
    Ok(binary)
}

#[derive(Debug)]
pub struct AgentCommand {
    pub program: OsString,
    pub arguments: Vec<OsString>,
}

pub fn command(
    directory: &Path,
    instructions: &str,
    model: Option<&str>,
    agents: usize,
    read_only: bool,
    harness: Harness,
    evidence_only: bool,
) -> Result<AgentCommand> {
    if !directory.is_dir() {
        bail!("coding directory must be a directory");
    }
    if !(1..=8).contains(&agents) {
        bail!("use between 1 and 8 Rady agents");
    }
    let binary = executable(harness)?;
    if harness == Harness::Command {
        let setting = if read_only {
            "RADY_REVIEW_COMMAND"
        } else {
            "RADY_AGENT_COMMAND"
        };
        let arguments = split_command(&env::var(setting).unwrap_or_default())?;
        let (program, rest) = arguments
            .split_first()
            .ok_or_else(|| anyhow!("set {setting} to an installed agent command"))?;
        let program =
            which(program).ok_or_else(|| anyhow!("set {setting} to an installed agent command"))?;
        return Ok(AgentCommand {
            program: program.into_os_string(),
            arguments: rest.iter().map(OsString::from).collect(),
        });
    }
    let mut arguments = Vec::<OsString>::new();
    if harness == Harness::Claude {
        arguments.extend(os_strings(&[
            "--print",
            "--no-session-persistence",
            "--append-system-prompt",
            instructions,
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
        ]));
        if read_only {
            arguments.extend(os_strings(&[
                "--permission-mode",
                "plan",
                "--tools",
                if evidence_only { "" } else { "Read,Glob,Grep" },
            ]));
        } else {
            let tools = if agents > 1 {
                "Read,Glob,Grep,Edit,Write,Agent"
            } else {
                "Read,Glob,Grep,Edit,Write"
            };
            arguments.extend(os_strings(&[
                "--permission-mode",
                "acceptEdits",
                "--tools",
                tools,
            ]));
        }
    } else {
        arguments.extend(os_strings(&[
            "exec",
            "--sandbox",
            if read_only {
                "read-only"
            } else {
                "workspace-write"
            },
            "--ephemeral",
            "--cd",
            directory.to_string_lossy().as_ref(),
            "-c",
            "model_provider=\"openai\"",
            "-c",
            &format!(
                "developer_instructions={}",
                serde_json::to_string(instructions)?
            ),
            "-c",
            if agents > 1 {
                "agents.enabled=true"
            } else {
                "agents.enabled=false"
            },
        ]));
        if agents > 1 {
            arguments.extend(os_strings(&[
                "-c",
                &format!("agents.max_concurrent_threads_per_session={}", agents - 1),
            ]));
        }
    }
    if let Some(model) = model {
        arguments.extend(os_strings(&["--model", model]));
    }
    Ok(AgentCommand {
        program: binary.into_os_string(),
        arguments,
    })
}

pub fn run(
    command: AgentCommand,
    prompt: &str,
    directory: &Path,
    harness: Harness,
    timeout: Duration,
    environment: &BTreeMap<String, String>,
    usage: Option<&mut Usage>,
) -> Result<ProcessOutput> {
    run_cancellable(
        command,
        prompt,
        directory,
        harness,
        timeout,
        environment,
        usage,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_cancellable(
    mut command: AgentCommand,
    prompt: &str,
    directory: &Path,
    harness: Harness,
    timeout: Duration,
    environment: &BTreeMap<String, String>,
    usage: Option<&mut Usage>,
    cancel_file: Option<&Path>,
) -> Result<ProcessOutput> {
    if let Some(usage) = usage.as_deref() {
        usage.before_call()?;
    }
    if harness == Harness::Codex && !contains_argument(&command.arguments, "--json") {
        command.arguments.push("--json".into());
    }
    if harness == Harness::Claude && !contains_argument(&command.arguments, "--output-format") {
        command
            .arguments
            .extend(os_strings(&["--output-format", "json"]));
    }
    let result = execute(
        &command.program,
        &command.arguments,
        directory,
        prompt.as_bytes(),
        timeout,
        environment,
        false,
        cancel_file,
    );
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            if let Some(usage) = usage {
                usage.record_unknown(harness);
            }
            return Err(error);
        }
    };
    let values = match harness {
        Harness::Codex => codex_usage(&output.stdout),
        Harness::Claude => claude_usage(&output.stdout),
        Harness::Command => command_usage(&output.stdout).map(|(_, usage)| usage),
    };
    let values = match values {
        Ok(value) => value,
        Err(error) => {
            if let Some(usage) = usage {
                usage.record_unknown(harness);
            }
            return Err(error);
        }
    };
    if let Some(usage) = usage {
        usage.record(harness, values)?;
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    prompt: &str,
    schema: &Value,
    directory: &Path,
    instructions: &str,
    model: Option<&str>,
    evidence_only: bool,
    harness: Harness,
    timeout: Duration,
    usage: Option<&mut Usage>,
) -> Result<Value> {
    evaluate_cancellable(
        prompt,
        schema,
        directory,
        instructions,
        model,
        evidence_only,
        harness,
        timeout,
        usage,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn evaluate_cancellable(
    prompt: &str,
    schema: &Value,
    directory: &Path,
    instructions: &str,
    model: Option<&str>,
    evidence_only: bool,
    harness: Harness,
    timeout: Duration,
    usage: Option<&mut Usage>,
    cancel_file: Option<&Path>,
) -> Result<Value> {
    let mut command = command(
        directory,
        instructions,
        model,
        1,
        true,
        harness,
        evidence_only,
    )?;
    let scratch = tempdir().context("could not create response workspace")?;
    let schema_path = scratch.path().join("schema.json");
    let output_path = scratch.path().join("result.json");
    fs::write(&schema_path, serde_json::to_vec(schema)?)?;
    let mut adjusted_prompt = Cow::Borrowed(prompt);
    if harness == Harness::Codex {
        command.arguments.extend(os_strings(&[
            "--ignore-user-config",
            "-c",
            "web_search=\"disabled\"",
            "-c",
            "mcp_servers={}",
        ]));
        if evidence_only {
            command.arguments.extend(os_strings(&[
                "--skip-git-repo-check",
                "-c",
                "features.shell_tool=false",
            ]));
        }
        command.arguments.extend([
            OsString::from("--output-schema"),
            schema_path.into_os_string(),
            OsString::from("--output-last-message"),
            output_path.clone().into_os_string(),
            OsString::from("-"),
        ]);
    } else if harness == Harness::Claude {
        command.arguments.extend(os_strings(&[
            "--output-format",
            "json",
            "--json-schema",
            &serde_json::to_string(schema)?,
        ]));
    } else {
        adjusted_prompt = Cow::Owned(format!(
            "{instructions}\nReturn only JSON matching this schema:\n{}\n{prompt}",
            serde_json::to_string(schema)?
        ));
    }
    let mut environment = BTreeMap::new();
    if harness == Harness::Command {
        environment.insert(
            "RADY_MODEL".to_owned(),
            model.unwrap_or_default().to_owned(),
        );
        environment.insert("RADY_READ_ONLY".to_owned(), "1".to_owned());
    }
    let result = run_cancellable(
        command,
        &adjusted_prompt,
        directory,
        harness,
        timeout,
        &environment,
        usage,
        cancel_file,
    )?;
    if result.code != 0 {
        bail!(
            "{} stopped before completing its review; nothing was published",
            harness.as_str()
        );
    }
    let text = if harness == Harness::Codex {
        read_structured_response(&output_path)?
    } else {
        result.stdout
    };
    if text.len() > MAX_STRUCTURED_RESPONSE {
        bail!("agent response is too large; nothing was published");
    }
    let mut response: Value = serde_json::from_str(&text)
        .context("agent returned an invalid or missing review; nothing was published")?;
    if harness == Harness::Claude {
        if response.get("is_error").and_then(Value::as_bool) == Some(true)
            || response.get("subtype").and_then(Value::as_str) != Some("success")
        {
            bail!("Claude returned an incomplete result; nothing was published");
        }
        response = response
            .get("structured_output")
            .cloned()
            .ok_or_else(|| anyhow!("Claude returned no structured result"))?;
    }
    if !response.is_object() {
        bail!("agent response must be a JSON object");
    }
    Ok(response)
}

fn read_structured_response(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .context("agent returned an invalid or missing review; nothing was published")?
        .take((MAX_STRUCTURED_RESPONSE + 1) as u64)
        .read_to_end(&mut bytes)
        .context("agent returned an invalid or missing review; nothing was published")?;
    if bytes.len() > MAX_STRUCTURED_RESPONSE {
        bail!("agent response is too large; nothing was published");
    }
    String::from_utf8(bytes)
        .context("agent returned an invalid or missing review; nothing was published")
}

pub fn worker_message(output: &ProcessOutput, harness: Harness) -> Result<String> {
    if harness == Harness::Claude {
        let value: Value = serde_json::from_str(&output.stdout)?;
        return Ok(value
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned());
    }
    if harness == Harness::Command {
        let (result, _) = command_usage(&output.stdout)?;
        return Ok(match result {
            Value::String(value) => value,
            _ => output.stdout.clone(),
        });
    }
    for line in output.stdout.lines().rev() {
        let event: Value = serde_json::from_str(line).context("Codex emitted invalid JSONL")?;
        if let Some(text) = event
            .get("item")
            .and_then(Value::as_object)
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("agent_message"))
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str)
        {
            return Ok(text.to_owned());
        }
    }
    Ok(String::new())
}

fn normalise_usage(raw: &Value, harness: Harness) -> Result<BTreeMap<String, u64>> {
    let object = raw
        .as_object()
        .ok_or_else(|| anyhow!("invalid agent usage"))?;
    let input = token_count(object.get("input_tokens"), "input_tokens")?;
    let output = token_count(object.get("output_tokens"), "output_tokens")?;
    let cached = optional_token_count(object.get("cached_input_tokens"), "cached_input_tokens")?;
    let mut values = BTreeMap::from([
        ("input_tokens".to_owned(), input),
        ("cached_input_tokens".to_owned(), cached),
        ("output_tokens".to_owned(), output),
    ]);
    let total = if harness == Harness::Claude {
        let creation = optional_token_count(
            object.get("cache_creation_input_tokens"),
            "cache_creation_input_tokens",
        )?;
        let read = optional_token_count(
            object.get("cache_read_input_tokens"),
            "cache_read_input_tokens",
        )?;
        values.insert("cache_creation_input_tokens".to_owned(), creation);
        values.insert("cache_read_input_tokens".to_owned(), read);
        input + output + creation + read
    } else {
        input + output
    };
    values.insert("total_tokens".to_owned(), total);
    Ok(values)
}

fn token_count(value: Option<&Value>, name: &str) -> Result<u64> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("invalid {name} in agent usage"))
}

fn optional_token_count(value: Option<&Value>, name: &str) -> Result<u64> {
    match value {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| anyhow!("invalid {name} in agent usage")),
    }
}

fn codex_usage(output: &str) -> Result<Option<BTreeMap<String, u64>>> {
    let mut total = BTreeMap::new();
    let mut found = false;
    for line in output.lines() {
        let event: Value = serde_json::from_str(line).context("Codex emitted invalid JSONL")?;
        if event.get("type").and_then(Value::as_str) == Some("turn.completed") {
            let values =
                normalise_usage(event.get("usage").unwrap_or(&Value::Null), Harness::Codex)?;
            for (name, value) in values {
                *total.entry(name).or_insert(0) += value;
            }
            found = true;
        }
    }
    Ok(found.then_some(total))
}

fn claude_usage(output: &str) -> Result<Option<BTreeMap<String, u64>>> {
    let value: Value = serde_json::from_str(output).context("Claude emitted invalid JSON")?;
    value
        .get("usage")
        .map(|usage| normalise_usage(usage, Harness::Claude))
        .transpose()
}

fn command_usage(output: &str) -> Result<(Value, Option<BTreeMap<String, u64>>)> {
    let Ok(value) = serde_json::from_str::<Value>(output) else {
        return Ok((Value::String(output.to_owned()), None));
    };
    let Some(object) = value.as_object() else {
        return Ok((Value::String(output.to_owned()), None));
    };
    let (Some(result), Some(usage)) = (object.get("result"), object.get("usage")) else {
        return Ok((Value::String(output.to_owned()), None));
    };
    Ok((
        result.clone(),
        Some(normalise_usage(usage, Harness::Command)?),
    ))
}

pub fn split_command(value: &str) -> Result<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escape = false;
    for character in value.chars() {
        if escape {
            current.push(character);
            escape = false;
        } else if character == '\\' && quote != Some('\'') {
            escape = true;
        } else if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            } else {
                current.push(character);
            }
        } else if character.is_whitespace() && quote.is_none() {
            if !current.is_empty() {
                arguments.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escape || quote.is_some() {
        bail!("agent command contains an incomplete quote or escape");
    }
    if !current.is_empty() {
        arguments.push(current);
    }
    Ok(arguments)
}

#[must_use]
pub fn which(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 && candidate.is_file() {
        return Some(candidate.to_owned());
    }
    env::split_paths(&env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

fn contains_argument(arguments: &[OsString], needle: &str) -> bool {
    arguments.iter().any(|value| value == needle)
}

fn os_strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_budget_fails_closed() -> Result<()> {
        let mut usage = Usage::new(Some(4))?;
        usage
            .record(
                Harness::Command,
                Some(BTreeMap::from([("total_tokens".to_owned(), 5)])),
            )
            .expect_err("budget must reject overshoot");
        assert_eq!(usage.total_tokens(), 5);
        Ok(())
    }

    #[test]
    fn command_splitter_preserves_quoted_arguments() -> Result<()> {
        assert_eq!(
            split_command("tool --name 'two words'")?,
            ["tool", "--name", "two words"]
        );
        Ok(())
    }

    #[test]
    fn command_usage_accepts_plain_and_structured_output() -> Result<()> {
        let (plain, usage) = command_usage("hello")?;
        assert_eq!(plain, "hello");
        assert!(usage.is_none());
        let (result, usage) =
            command_usage(r#"{"result":"ok","usage":{"input_tokens":2,"output_tokens":3}}"#)?;
        assert_eq!(result, "ok");
        assert_eq!(
            usage.and_then(|value| value.get("total_tokens").copied()),
            Some(5)
        );
        Ok(())
    }

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
    fn structured_response_file_has_a_fixed_limit() -> Result<()> {
        let temporary = tempdir()?;
        for (name, length, accepted) in [
            ("at-limit", MAX_STRUCTURED_RESPONSE, true),
            ("over-limit", MAX_STRUCTURED_RESPONSE + 1, false),
        ] {
            let path = temporary.path().join(name);
            fs::write(&path, vec![b'x'; length])?;
            assert_eq!(read_structured_response(&path).is_ok(), accepted, "{name}");
        }
        Ok(())
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
        fs::write(&cancelled, b"cancelled\n")?;
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
                fs::write(live_marker, b"cancelled\n")
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
        for name in [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "ANTHROPIC_API_KEY",
            "RADY_GEMINI_API_KEY",
            "RADY_CEREBRAS_API_KEY",
            "RADY_XAI_API_KEY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_HOST",
            "GH_REPO",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "RADY_PUSH_TOKEN",
        ] {
            assert!(SENSITIVE_ENVIRONMENT.contains(&name));
            assert!(!values.contains_key(name));
        }
    }

    #[test]
    fn plan_schema_can_be_serialised() {
        let value = serde_json::json!({"type": "object"});
        assert_eq!(value["type"], "object");
    }
}
