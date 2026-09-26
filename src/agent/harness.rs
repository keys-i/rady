use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use super::context::McpConfiguration;
use super::process::execute;
use super::{Harness, ProcessOutput, Usage};

pub fn executable(harness: Harness) -> Result<PathBuf> {
    if harness == Harness::Command {
        let arguments = split_command(&env::var("KOELU_AGENT_COMMAND").unwrap_or_default())?;
        let first = arguments.first().ok_or_else(|| {
            anyhow!("set KOELU_AGENT_COMMAND to an installed non-interactive agent command")
        })?;
        return which(first).ok_or_else(|| {
            anyhow!("set KOELU_AGENT_COMMAND to an installed non-interactive agent command")
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
        bail!("run {login} before using Koelu");
    }
    Ok(binary)
}

#[derive(Debug)]
pub struct AgentCommand {
    pub program: OsString,
    pub arguments: Vec<OsString>,
}

#[allow(clippy::too_many_arguments)]
pub fn command(
    directory: &Path,
    instructions: &str,
    model: Option<&str>,
    agents: usize,
    read_only: bool,
    harness: Harness,
    evidence_only: bool,
    mcp: Option<&McpConfiguration>,
) -> Result<AgentCommand> {
    if !directory.is_dir() {
        bail!("coding directory must be a directory");
    }
    if !(1..=8).contains(&agents) {
        bail!("use between 1 and 8 Koelu agents");
    }
    if harness == Harness::Command {
        let setting = command_setting(read_only);
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
    let binary = executable(harness)?;
    let mut arguments = Vec::<OsString>::new();
    if harness == Harness::Claude {
        let mcp_json = mcp.map_or_else(
            || "{\"mcpServers\":{}}".to_owned(),
            McpConfiguration::claude_json,
        );
        arguments.extend(os_strings(&[
            "--print",
            "--no-session-persistence",
            "--append-system-prompt",
            instructions,
            "--strict-mcp-config",
            "--mcp-config",
            &mcp_json,
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
            if let Some(mcp) = mcp {
                arguments.extend(os_strings(&["--allowedTools", &mcp.claude_allowed_tools()]));
            }
        }
    } else {
        let mcp = mcp.map_or_else(
            || "mcp_servers={}".to_owned(),
            McpConfiguration::codex_inline_toml,
        );
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
            "-c",
            &mcp,
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
    let command_environment = if harness == Harness::Command {
        command_environment(
            environment,
            env::var("KOELU_COMMAND_ALLOW_GEMINI").as_deref() == Ok("1"),
            env::var("KOELU_GEMINI_API_KEY").ok(),
        )?
    } else {
        environment.clone()
    };
    let result = execute(
        &command.program,
        &command.arguments,
        directory,
        prompt.as_bytes(),
        timeout,
        &command_environment,
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

const MAX_GEMINI_API_KEY_BYTES: usize = 1024;

fn command_setting(read_only: bool) -> &'static str {
    if read_only {
        "KOELU_REVIEW_COMMAND"
    } else {
        "KOELU_AGENT_COMMAND"
    }
}

fn command_environment(
    environment: &BTreeMap<String, String>,
    allow_gemini: bool,
    gemini_api_key: Option<String>,
) -> Result<BTreeMap<String, String>> {
    let mut environment = environment.clone();
    environment.remove("GEMINI_API_KEY");
    if allow_gemini {
        let key = gemini_api_key
            .ok_or_else(|| anyhow!("KOELU_COMMAND_ALLOW_GEMINI requires KOELU_GEMINI_API_KEY"))?;
        if key.is_empty()
            || key.len() > MAX_GEMINI_API_KEY_BYTES
            || key.chars().any(char::is_control)
        {
            bail!("KOELU_GEMINI_API_KEY is not safe to pass to the command harness");
        }
        environment.insert("GEMINI_API_KEY".to_owned(), key);
    }
    Ok(environment)
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

pub(crate) fn command_usage(output: &str) -> Result<(Value, Option<BTreeMap<String, u64>>)> {
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
    fn command_paths_and_gemini_forwarding_are_explicit() -> Result<()> {
        for (read_only, expected) in [
            (false, "KOELU_AGENT_COMMAND"),
            (true, "KOELU_REVIEW_COMMAND"),
        ] {
            assert_eq!(command_setting(read_only), expected);
        }
        for (allowed, key, forwards) in [
            (false, Some("safe-key"), false),
            (true, Some("safe-key"), true),
        ] {
            let environment = BTreeMap::from([("GEMINI_API_KEY".to_owned(), "ignored".to_owned())]);
            let result = command_environment(&environment, allowed, key.map(ToOwned::to_owned))?;
            assert_eq!(result.contains_key("GEMINI_API_KEY"), forwards);
        }
        for key in [None, Some(""), Some("line\nbreak")] {
            assert!(
                command_environment(&BTreeMap::new(), true, key.map(ToOwned::to_owned)).is_err()
            );
        }
        Ok(())
    }
}
