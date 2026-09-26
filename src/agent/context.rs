use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::Result;

const MAX_FILE_BYTES: u64 = 64 * 1024;
const MAX_TOTAL_BYTES: usize = 256 * 1024;
const MAX_SKILLS: usize = 16;
const MAX_MCP_SERVERS: usize = 8;
const MAX_MCP_ARGS: usize = 32;
const BROWSER_COMMAND: &str = "playwright-mcp";
const BROWSER_ARGS: &[&str] = &[
    "--headless",
    "--isolated",
    "--sandbox",
    "--no-webmcp",
    "--block-service-workers",
    "--timeout-action",
    "5000",
    "--timeout-navigation",
    "15000",
    "--idle-timeout",
    "30000",
    "--output-max-size",
    "10485760",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpConfiguration {
    servers: Vec<McpServer>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct McpServer {
    name: String,
    command: String,
    args: Vec<String>,
}

impl McpConfiguration {
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.servers.iter().any(|server| server.name == name)
    }

    #[must_use]
    pub fn claude_json(&self) -> String {
        let servers: Map<String, Value> = self
            .servers
            .iter()
            .map(|server| {
                (
                    server.name.clone(),
                    json!({"command": server.command, "args": server.args}),
                )
            })
            .collect();
        serde_json::to_string(&json!({"mcpServers": servers}))
            .expect("MCP configuration is serializable")
    }

    #[must_use]
    pub fn codex_inline_toml(&self) -> String {
        let servers = self
            .servers
            .iter()
            .map(|server| {
                let args = server
                    .args
                    .iter()
                    .map(|argument| {
                        serde_json::to_string(argument).expect("argument is serializable")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{}={{command={},args=[{}]}}",
                    server.name,
                    serde_json::to_string(&server.command).expect("command is serializable"),
                    args
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("mcp_servers={{{servers}}}")
    }

    #[must_use]
    pub fn claude_allowed_tools(&self) -> String {
        self.servers
            .iter()
            .map(|server| format!("mcp__{}", server.name))
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepositoryContext {
    guidance: String,
    files: Vec<String>,
    mcp: Option<McpConfiguration>,
}

impl RepositoryContext {
    pub fn load(root: &Path, selected_mcp: &[String]) -> Result<Self> {
        let mut total = 0;
        let mut files = Vec::new();
        let mut sections = Vec::new();
        for path in ["AGENTS.md", "DESIGN.md"] {
            if let Some(text) = read_optional(root, Path::new(path), &mut total)? {
                files.push(path.to_owned());
                sections.push((path.to_owned(), text));
            }
        }

        let configuration_path = Path::new(".pekin/context.json");
        let configuration = match read_optional(root, configuration_path, &mut total)? {
            Some(source) => {
                files.push(configuration_path.to_string_lossy().into_owned());
                serde_json::from_str::<ContextFile>(&source)
                    .context(".pekin/context.json must use the supported context schema")?
            }
            None => ContextFile {
                schema: 1,
                ..ContextFile::default()
            },
        };
        if configuration.schema != 1 {
            bail!(".pekin/context.json requires schema 1");
        }
        if configuration.skills.len() > MAX_SKILLS
            || configuration.mcp_servers.len() > MAX_MCP_SERVERS
        {
            bail!(".pekin/context.json exceeds its guidance or MCP server limit");
        }
        validate_servers(&configuration.mcp_servers)?;

        let mut seen_skills = BTreeSet::new();
        for skill in &configuration.skills {
            validate_relative_path(skill)?;
            if !seen_skills.insert(skill) {
                bail!(".pekin/context.json repeats a skill path");
            }
            let text = read_required(root, Path::new(skill), &mut total)?;
            files.push(skill.clone());
            sections.push((skill.clone(), text));
        }

        let mcp = if selected_mcp.is_empty() {
            None
        } else {
            Some(select_mcp(&configuration.mcp_servers, selected_mcp)?)
        };

        Ok(Self {
            guidance: render_guidance(&sections),
            files,
            mcp,
        })
    }

    #[must_use]
    pub fn guidance(&self) -> &str {
        &self.guidance
    }

    #[must_use]
    pub fn files(&self) -> &[String] {
        &self.files
    }

    #[must_use]
    pub fn mcp(&self) -> Option<&McpConfiguration> {
        self.mcp.as_ref()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextFile {
    #[serde(default)]
    schema: u8,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default, alias = "mcpServers")]
    mcp_servers: BTreeMap<String, McpServerFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpServerFile {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

fn select_mcp(
    servers: &BTreeMap<String, McpServerFile>,
    selected: &[String],
) -> Result<McpConfiguration> {
    let mut names = BTreeSet::new();
    let mut selected_servers = Vec::with_capacity(selected.len());
    for name in selected {
        if !valid_name(name) || !names.insert(name) {
            bail!("selected MCP server name is invalid or repeated");
        }
        let server = servers
            .get(name)
            .ok_or_else(|| anyhow!("selected MCP server {name:?} is not configured"))?;
        selected_servers.push(McpServer {
            name: name.clone(),
            command: server.command.clone(),
            args: server.args.clone(),
        });
    }
    Ok(McpConfiguration {
        servers: selected_servers,
    })
}

fn validate_servers(servers: &BTreeMap<String, McpServerFile>) -> Result<()> {
    for (name, server) in servers {
        if name == "browser" {
            if server.command != BROWSER_COMMAND {
                bail!("reserved MCP server \"browser\" must use command \"{BROWSER_COMMAND}\"");
            }
            if server
                .args
                .iter()
                .map(String::as_str)
                .ne(BROWSER_ARGS.iter().copied())
            {
                bail!(
                    "reserved MCP server \"browser\" must use the fixed sandboxed Playwright arguments"
                );
            }
        }
        if !valid_name(name)
            || !valid_command(&server.command)
            || server.args.len() > MAX_MCP_ARGS
            || server.args.iter().any(|argument| !valid_argument(argument))
        {
            bail!("MCP server {name:?} is invalid");
        }
    }
    Ok(())
}

fn read_optional(root: &Path, relative: &Path, total: &mut usize) -> Result<Option<String>> {
    let path = root.join(relative);
    match fs::symlink_metadata(&path) {
        Ok(_) => read_checked(root, relative, total).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("could not inspect guidance path {}", relative.display())),
    }
}

fn read_required(root: &Path, relative: &Path, total: &mut usize) -> Result<String> {
    read_checked(root, relative, total)
}

fn read_checked(root: &Path, relative: &Path, total: &mut usize) -> Result<String> {
    inspect_path(root, relative)?;
    let path = root.join(relative);
    let metadata = fs::metadata(&path)
        .with_context(|| format!("could not inspect guidance file {}", relative.display()))?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        bail!(
            "guidance file {} must be a regular file no larger than 64 KiB",
            relative.display()
        );
    }
    let bytes = fs::read(&path)
        .with_context(|| format!("could not read guidance file {}", relative.display()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES || total.saturating_add(bytes.len()) > MAX_TOTAL_BYTES {
        bail!("repository guidance exceeds its size limit");
    }
    *total += bytes.len();
    String::from_utf8(bytes)
        .with_context(|| format!("guidance file {} must be valid UTF-8", relative.display()))
}

fn inspect_path(root: &Path, relative: &Path) -> Result<()> {
    let mut current = PathBuf::from(root);
    for component in relative.components() {
        match component {
            Component::Normal(part) => current.push(part),
            _ => bail!("guidance path must be a relative, traversal-free file path"),
        }
        if fs::symlink_metadata(&current)
            .with_context(|| format!("could not inspect guidance path {}", relative.display()))?
            .file_type()
            .is_symlink()
        {
            bail!("guidance path {} must not use symlinks", relative.display());
        }
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 512 || path.chars().any(char::is_control) {
        bail!("skill path is invalid");
    }
    inspect_path_components(Path::new(path))
}

fn inspect_path_components(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("guidance path must be a relative, traversal-free file path");
    }
    Ok(())
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'_' | b'-'))
        })
}

fn valid_command(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_argument(value: &str) -> bool {
    value.len() <= 4_096 && !value.chars().any(char::is_control)
}

fn render_guidance(sections: &[(String, String)]) -> String {
    sections
        .iter()
        .map(|(path, text)| format!("## {path}\n\n{text}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{BROWSER_ARGS, RepositoryContext};

    #[test]
    fn loads_bounded_guidance_and_explicit_mcp() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("AGENTS.md"), "be careful").unwrap();
        fs::write(root.path().join("DESIGN.md"), "be kind").unwrap();
        fs::create_dir(root.path().join(".pekin")).unwrap();
        fs::create_dir(root.path().join("skills")).unwrap();
        fs::write(root.path().join("skills/review.md"), "review narrowly").unwrap();
        fs::write(
            root.path().join(".pekin/context.json"),
            r#"{"schema":1,"skills":["skills/review.md"],"mcp_servers":{"docs":{"command":"npx","args":["-y","docs-mcp"]}}}"#,
        )
        .unwrap();

        let selected = vec!["docs".to_owned()];
        let context = RepositoryContext::load(root.path(), &selected).unwrap();
        assert_eq!(
            context.files(),
            [
                "AGENTS.md",
                "DESIGN.md",
                ".pekin/context.json",
                "skills/review.md"
            ]
        );
        assert!(context.guidance().contains("## skills/review.md"));
        let mcp = context.mcp().unwrap();
        assert_eq!(
            mcp.claude_json(),
            r#"{"mcpServers":{"docs":{"args":["-y","docs-mcp"],"command":"npx"}}}"#
        );
        assert_eq!(
            mcp.codex_inline_toml(),
            r#"mcp_servers={docs={command="npx",args=["-y", "docs-mcp"]}}"#
        );
        assert_eq!(mcp.claude_allowed_tools(), "mcp__docs");
        assert!(mcp.contains("docs"));
        assert!(!mcp.contains("browser"));
        assert!(RepositoryContext::load(tempdir().unwrap().path(), &[]).is_ok());
    }

    #[test]
    fn rejects_invalid_configuration_without_executing_it() {
        for (name, source, selected) in [
            ("unknown-field", r#"{"schema":1,"extra":true}"#, vec![]),
            (
                "traversal",
                r#"{"schema":1,"skills":["../secret.md"]}"#,
                vec![],
            ),
            (
                "unknown-server",
                r#"{"schema":1}"#,
                vec!["missing".to_owned()],
            ),
            (
                "bad-command",
                r#"{"schema":1,"mcp_servers":{"bad":{"command":"npx run"}}}"#,
                vec!["bad".to_owned()],
            ),
        ] {
            let root = tempdir().unwrap();
            fs::create_dir(root.path().join(".pekin")).unwrap();
            fs::write(root.path().join(".pekin/context.json"), source).unwrap();
            assert!(
                RepositoryContext::load(root.path(), &selected).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn reserves_browser_for_the_sandboxed_playwright_command() {
        let valid_args = BROWSER_ARGS
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>();
        for (name, command, args, valid) in [
            ("valid", "playwright-mcp", valid_args.clone(), true),
            ("command", "sh", valid_args.clone(), false),
            (
                "missing",
                "playwright-mcp",
                valid_args[..12].to_vec(),
                false,
            ),
            (
                "extra",
                "playwright-mcp",
                [valid_args.clone(), vec!["--unsafe".to_owned()]].concat(),
                false,
            ),
        ] {
            let root = tempdir().unwrap();
            fs::create_dir(root.path().join(".pekin")).unwrap();
            fs::write(
                root.path().join(".pekin/context.json"),
                json!({"schema": 1, "mcp_servers": {"browser": {"command": command, "args": args}}}).to_string(),
            )
            .unwrap();
            let result = RepositoryContext::load(root.path(), &[]);
            assert_eq!(result.is_ok(), valid, "{name}");
            if !valid {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("reserved MCP server \"browser\""),
                    "{name}"
                );
            }
        }
    }
}
