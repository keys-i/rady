use std::env;
#[cfg(test)]
use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};

use crate::Result;
use crate::github;
use crate::github::apps;

use super::ServeArgs;

const APP_TOKEN_REFRESH: Duration = Duration::from_secs(50 * 60);
enum AppPrivateKey {
    Environment,
    File(PathBuf),
}

impl AppPrivateKey {
    fn read(&self) -> Result<String> {
        match self {
            Self::Environment => env::var("PEKIN_APP_PRIVATE_KEY")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("PEKIN_APP_PRIVATE_KEY is empty")),
            Self::File(path) => read_app_private_key(path),
        }
    }
}

struct AppCredentials {
    issuer: String,
    private_key: AppPrivateKey,
    owner: Option<String>,
    slug: String,
}

pub(super) struct ServiceTokenProvider {
    credentials: AppCredentials,
    tokens: Vec<String>,
    refreshed_at: Option<Instant>,
    scope: github::InstallationTokenScope,
    installation_seed: usize,
}

impl ServiceTokenProvider {
    pub(super) fn new(
        arguments: &ServeArgs,
        scope: github::InstallationTokenScope,
    ) -> Result<Self> {
        let private_key_environment = env::var("PEKIN_APP_PRIVATE_KEY")
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        Ok(Self {
            credentials: app_credentials(arguments, private_key_environment)?,
            tokens: Vec::new(),
            refreshed_at: None,
            scope,
            installation_seed: installation_seed()?,
        })
    }

    pub(super) fn tokens(&mut self) -> Result<&[String]> {
        let refresh = self
            .refreshed_at
            .is_none_or(|refreshed| refreshed.elapsed() >= APP_TOKEN_REFRESH);
        if refresh {
            let private_key = self.credentials.private_key.read()?;
            let app = github::authenticated_app(&private_key, &self.credentials.issuer)?;
            apps::require_app_identity(&app, &self.credentials.slug)?;
            let tokens = github::mint_installation_tokens(
                &private_key,
                &self.credentials.issuer,
                self.credentials.owner.as_deref(),
                self.scope,
                self.installation_seed,
            )?;
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

    /// Mint a short-lived write token for one installed repository
    pub(super) fn delivery_token(&self, repository: &str) -> Result<String> {
        let private_key = self.credentials.private_key.read()?;
        let app = github::authenticated_app(&private_key, &self.credentials.issuer)?;
        apps::require_app_identity(&app, &self.credentials.slug)?;
        let token = github::mint_repository_installation_token(
            &private_key,
            &self.credentials.issuer,
            repository,
        )?;
        validate_service_token(&token)?;
        Ok(token)
    }
}

fn app_credentials(arguments: &ServeArgs, private_key_environment: bool) -> Result<AppCredentials> {
    let private_key_file = arguments.app_private_key_file.clone();
    if private_key_file.is_some() && private_key_environment {
        bail!("use either --app-private-key-file or PEKIN_APP_PRIVATE_KEY, not both");
    }
    let issuer = arguments
        .app_client_id
        .as_deref()
        .filter(|value| !value.is_empty());
    let issuer =
        issuer.ok_or_else(|| anyhow!("provide --app-client-id with the GitHub App private key"))?;
    let private_key = match (private_key_file, private_key_environment) {
        (Some(path), false) => AppPrivateKey::File(path),
        (None, true) => AppPrivateKey::Environment,
        (None, false) => bail!("provide --app-private-key-file with the GitHub App client ID"),
        (Some(_), true) => {
            bail!("use either --app-private-key-file or PEKIN_APP_PRIVATE_KEY, not both")
        }
    };
    Ok(AppCredentials {
        issuer: issuer.to_owned(),
        private_key,
        owner: arguments.owner.clone(),
        slug: app_slug()?,
    })
}

fn app_slug() -> Result<String> {
    app_slug_from(env::var("PEKIN_APP_SLUG").ok().as_deref())
}

fn app_slug_from(value: Option<&str>) -> Result<String> {
    let slug = value
        .filter(|value| !value.is_empty())
        .unwrap_or(apps::PEKIN_SLUG)
        .to_owned();
    apps::validate_slug(&slug)?;
    if slug != apps::PEKIN_SLUG {
        bail!("PEKIN_APP_SLUG must be {}", apps::PEKIN_SLUG);
    }
    Ok(slug)
}

fn installation_seed() -> Result<usize> {
    installation_seed_from(env::var("PEKIN_INSTALLATION_SEED").ok().as_deref())
}

fn installation_seed_from(value: Option<&str>) -> Result<usize> {
    value
        .filter(|value| !value.is_empty())
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|_| anyhow!("PEKIN_INSTALLATION_SEED must be a non-negative integer"))
        .map(Option::unwrap_or_default)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Harness;

    #[test]
    fn service_token_validation_is_bounded() {
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
            (None, None, false, -1),
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
                app_private_key_file: key_file.map(PathBuf::from),
                once: true,
            };
            let source = app_credentials(&arguments, environment_key);
            match expected {
                1 => assert!(matches!(
                    source,
                    Ok(AppCredentials { owner: Some(owner), .. }) if owner == "keys-i"
                )),
                -1 => assert!(source.is_err()),
                _ => panic!("invalid test case"),
            }
        }
    }

    #[test]
    fn service_identity_inputs_are_strict() -> Result<()> {
        assert_eq!(app_slug_from(None)?, apps::PEKIN_SLUG);
        assert_eq!(installation_seed_from(Some("42"))?, 42);
        assert!(app_slug_from(Some("rad duck")).is_err());
        assert!(app_slug_from(Some("another-app")).is_err());
        assert!(installation_seed_from(Some("nope")).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn app_private_key_file_must_be_small_private_and_regular() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temporary = tempfile::tempdir()?;
        let key = temporary.path().join("pekin.pem");
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
