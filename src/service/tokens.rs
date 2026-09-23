use std::collections::BTreeMap;
use std::env;
#[cfg(test)]
use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};

use crate::Result;
use crate::agent;
use crate::github;

use super::ServeArgs;

const APP_TOKEN_REFRESH: Duration = Duration::from_secs(50 * 60);
const SERVICE_TOKEN_ENVIRONMENT: &[&str] = &[
    "RADY_APP_PRIVATE_KEY",
    "RADY_APP_PRIVATE_KEY_FILE",
    "RADY_APP_CLIENT_ID",
    "RADY_APP_ID",
    "RADY_APP_SLUG",
];

enum AppPrivateKey {
    Environment,
    File(PathBuf),
}

impl AppPrivateKey {
    fn read(&self) -> Result<String> {
        match self {
            Self::Environment => env::var("RADY_APP_PRIVATE_KEY")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("RADY_APP_PRIVATE_KEY is empty")),
            Self::File(path) => read_app_private_key(path),
        }
    }
}

enum ServiceTokenSource {
    App {
        issuer: String,
        private_key: AppPrivateKey,
        owner: Option<String>,
    },
    Command {
        program: PathBuf,
        arguments: Vec<String>,
    },
    Static,
}

pub(super) struct ServiceTokenProvider {
    source: ServiceTokenSource,
    tokens: Vec<String>,
    refreshed_at: Option<Instant>,
}

impl ServiceTokenProvider {
    pub(super) fn new(arguments: &ServeArgs) -> Result<Self> {
        if let Some(command) = arguments.token_command.as_deref() {
            let parts = agent::split_command(command)?;
            let (program, arguments) = parts
                .split_first()
                .ok_or_else(|| anyhow!("token command is empty"))?;
            let program = agent::which(program)
                .ok_or_else(|| anyhow!("token command executable is not installed"))?;
            return Ok(Self {
                source: ServiceTokenSource::Command {
                    program,
                    arguments: arguments.to_vec(),
                },
                tokens: Vec::new(),
                refreshed_at: None,
            });
        }

        let private_key_environment = env::var("RADY_APP_PRIVATE_KEY")
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        if let Some(source) = app_service_token_source(arguments, private_key_environment)? {
            return Ok(Self {
                source,
                tokens: Vec::new(),
                refreshed_at: None,
            });
        }

        let token = env::var("GH_TOKEN").unwrap_or_default();
        validate_service_token(&token).with_context(
            || "provide App credentials with --app-client-id and --app-private-key-file",
        )?;
        Ok(Self {
            source: ServiceTokenSource::Static,
            tokens: vec![token],
            refreshed_at: None,
        })
    }

    pub(super) fn tokens(&mut self) -> Result<&[String]> {
        let refresh = match &self.source {
            ServiceTokenSource::App { .. } => self
                .refreshed_at
                .is_none_or(|refreshed| refreshed.elapsed() >= APP_TOKEN_REFRESH),
            ServiceTokenSource::Command { .. } => true,
            ServiceTokenSource::Static => false,
        };
        if refresh {
            let tokens = match &self.source {
                ServiceTokenSource::App {
                    issuer,
                    private_key,
                    owner,
                } => {
                    let private_key = private_key.read()?;
                    github::mint_installation_tokens(&private_key, issuer, owner.as_deref())?
                }
                ServiceTokenSource::Command { program, arguments } => {
                    let output = agent::execute(
                        program.as_os_str(),
                        arguments,
                        Path::new("."),
                        b"",
                        Duration::from_secs(30),
                        &service_token_environment(),
                        false,
                        None,
                    )?;
                    if output.code != 0 {
                        bail!("token command failed; no GitHub request was made");
                    }
                    vec![output.stdout.trim().to_owned()]
                }
                ServiceTokenSource::Static => {
                    bail!("static GitHub token was unexpectedly selected for refresh")
                }
            };
            for token in &tokens {
                validate_service_token(token)?;
            }
            if tokens.is_empty() {
                bail!("GitHub App has no installations");
            }
            self.tokens = tokens;
            self.refreshed_at = Some(Instant::now());
        }
        if self.tokens.is_empty() {
            bail!("GitHub App token is unavailable");
        }
        Ok(&self.tokens)
    }
}

fn app_service_token_source(
    arguments: &ServeArgs,
    private_key_environment: bool,
) -> Result<Option<ServiceTokenSource>> {
    let private_key_file = arguments.app_private_key_file.clone();
    if private_key_file.is_some() && private_key_environment {
        bail!("use either --app-private-key-file or RADY_APP_PRIVATE_KEY, not both");
    }
    let issuer = arguments
        .app_client_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            arguments
                .app_id
                .as_deref()
                .filter(|value| !value.is_empty())
        });
    if private_key_file.is_none() && !private_key_environment && issuer.is_none() {
        return Ok(None);
    }
    let issuer =
        issuer.ok_or_else(|| anyhow!("provide --app-client-id with the GitHub App private key"))?;
    let private_key = match (private_key_file, private_key_environment) {
        (Some(path), false) => AppPrivateKey::File(path),
        (None, true) => AppPrivateKey::Environment,
        (None, false) => bail!("provide --app-private-key-file with the GitHub App client ID"),
        (Some(_), true) => {
            bail!("use either --app-private-key-file or RADY_APP_PRIVATE_KEY, not both")
        }
    };
    Ok(Some(ServiceTokenSource::App {
        issuer: issuer.to_owned(),
        private_key,
        owner: arguments.owner.clone(),
    }))
}

fn validate_service_token(token: &str) -> Result<()> {
    if token.is_empty() || token.len() > 8_192 || !token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        bail!("GitHub App installation token is invalid");
    }
    Ok(())
}

fn read_app_private_key(path: &Path) -> Result<String> {
    const MAX_PRIVATE_KEY_BYTES: u64 = 64 * 1024;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path).with_context(|| {
        format!(
            "could not securely open App private key at {}",
            path.display()
        )
    })?;
    let metadata = file
        .metadata()
        .context("could not inspect the opened App private key")?;
    if !metadata.is_file() {
        bail!("App private key must be a regular, non-symlink file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("App private key must be readable only by its owner; run chmod 600");
        }
    }
    if metadata.len() == 0 || metadata.len() > MAX_PRIVATE_KEY_BYTES {
        bail!("App private key is empty or too large");
    }
    let mut private_key = String::with_capacity(metadata.len() as usize);
    file.take(MAX_PRIVATE_KEY_BYTES + 1)
        .read_to_string(&mut private_key)
        .context("App private key is not valid UTF-8")?;
    if private_key.len() as u64 > MAX_PRIVATE_KEY_BYTES {
        bail!("App private key is empty or too large");
    }
    Ok(private_key)
}

fn service_token_environment() -> BTreeMap<String, String> {
    SERVICE_TOKEN_ENVIRONMENT
        .iter()
        .copied()
        .filter_map(|name| env::var(name).ok().map(|value| (name.to_owned(), value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Harness;

    #[test]
    fn service_token_boundaries_are_narrow() {
        assert!(SERVICE_TOKEN_ENVIRONMENT.contains(&"RADY_APP_PRIVATE_KEY"));
        assert!(SERVICE_TOKEN_ENVIRONMENT.contains(&"RADY_APP_PRIVATE_KEY_FILE"));
        assert!(!SERVICE_TOKEN_ENVIRONMENT.contains(&"RADY_GEMINI_API_KEY"));
        assert!(!SERVICE_TOKEN_ENVIRONMENT.contains(&"GH_TOKEN"));
        for (token, valid) in [
            ("ghs_valid", true),
            ("", false),
            ("bad token", false),
            ("line\nbreak", false),
        ] {
            assert_eq!(validate_service_token(token).is_ok(), valid, "{token:?}");
        }
        assert!(validate_service_token(&"x".repeat(8_192)).is_ok());
        assert!(validate_service_token(&"x".repeat(8_193)).is_err());
    }

    #[test]
    fn built_in_app_credentials_are_resolved_as_one_complete_source() {
        for (client_id, key_file, environment_key, expected) in [
            (Some("Iv1.client"), Some("key.pem"), false, 1),
            (None, None, false, 0),
            (Some("Iv1.client"), None, false, -1),
            (None, Some("key.pem"), false, -1),
            (Some("Iv1.client"), Some("key.pem"), true, -1),
        ] {
            let arguments = ServeArgs {
                owner: Some("keys-i".to_owned()),
                harness: Harness::Codex,
                interval: 30,
                max_reviews: 4,
                app_client_id: client_id.map(str::to_owned),
                app_id: None,
                app_private_key_file: key_file.map(PathBuf::from),
                token_command: None,
                once: true,
            };
            let source = app_service_token_source(&arguments, environment_key);
            match expected {
                1 => assert!(matches!(
                    source,
                    Ok(Some(ServiceTokenSource::App { owner: Some(owner), .. })) if owner == "keys-i"
                )),
                0 => assert!(matches!(source, Ok(None))),
                -1 => assert!(source.is_err()),
                _ => panic!("invalid test case"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn app_private_key_file_must_be_small_private_and_regular() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temporary = tempfile::tempdir()?;
        let key = temporary.path().join("radyybot.pem");
        fs::write(&key, "private key")?;
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600))?;
        assert_eq!(read_app_private_key(&key)?, "private key");

        fs::set_permissions(&key, fs::Permissions::from_mode(0o644))?;
        assert!(read_app_private_key(&key).is_err());
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600))?;
        let link = temporary.path().join("linked.pem");
        symlink(&key, &link)?;
        assert!(read_app_private_key(&link).is_err());

        let empty = temporary.path().join("empty.pem");
        fs::write(&empty, "")?;
        fs::set_permissions(&empty, fs::Permissions::from_mode(0o600))?;
        assert!(read_app_private_key(&empty).is_err());
        let oversized = temporary.path().join("oversized.pem");
        fs::write(&oversized, vec![b'x'; 64 * 1024 + 1])?;
        fs::set_permissions(&oversized, fs::Permissions::from_mode(0o600))?;
        assert!(read_app_private_key(&oversized).is_err());
        Ok(())
    }
}
