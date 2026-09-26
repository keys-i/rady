use std::collections::BTreeMap;
use std::env;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail};

use crate::Result;
use crate::agent;

pub(super) const MAX_PUSH_TOKEN_BYTES: usize = 8_192;

pub(super) struct GitNetworkAuth {
    pub(super) environment: BTreeMap<String, String>,
}

pub(super) fn git(
    directory: &Path,
    arguments: &[&str],
    cancel_file: Option<&Path>,
) -> Result<String> {
    let binary = agent::which("git").ok_or_else(|| anyhow!("install Git"))?;
    let output = agent::execute(
        binary.as_os_str(),
        arguments,
        directory,
        b"",
        Duration::from_secs(120),
        &BTreeMap::from([("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())]),
        false,
        cancel_file,
    )?;
    if output.code != 0 {
        bail!(
            "Git {} failed; inspect the retained workspace before retrying",
            arguments.first().copied().unwrap_or("command")
        );
    }
    Ok(output.stdout.trim_end_matches('\n').to_owned())
}

pub(super) fn checkpoint(
    workspace: &Path,
    names: &[String],
    label: &str,
    cancel_file: Option<&Path>,
) -> Result<Option<String>> {
    if git(workspace, &["status", "--porcelain"], cancel_file)?.is_empty() {
        return Ok(None);
    }
    let mut add = vec!["--literal-pathspecs", "add", "--"];
    add.extend(names.iter().map(String::as_str));
    git(workspace, &add, cancel_file)?;
    git(workspace, &["diff", "--cached", "--check"], cancel_file)?;
    let label = label
        .lines()
        .next()
        .unwrap_or("checkpoint")
        .trim_start_matches(['#', ' '])
        .chars()
        .take(64)
        .collect::<String>();
    let message = format!(
        "koelu: {}",
        if label.is_empty() {
            "checkpoint"
        } else {
            &label
        }
    );
    git(workspace, &["commit", "-m", &message], cancel_file)?;
    if !git(workspace, &["status", "--porcelain"], cancel_file)?.is_empty() {
        bail!("workspace changed during checkpoint; publishing is blocked");
    }
    Ok(Some(git(workspace, &["rev-parse", "HEAD"], cancel_file)?))
}

pub(super) fn git_network(
    directory: &Path,
    arguments: &[&str],
    cancel_file: Option<&Path>,
    auth: Option<&GitNetworkAuth>,
) -> Result<String> {
    let binary = agent::which("git").ok_or_else(|| anyhow!("install Git"))?;
    let mut environment = BTreeMap::from([("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())]);
    if let Some(auth) = auth {
        environment.extend(auth.environment.clone());
    }
    let output = agent::execute(
        binary.as_os_str(),
        arguments,
        directory,
        b"",
        Duration::from_secs(120),
        &environment,
        false,
        cancel_file,
    )?;
    if output.code != 0 {
        bail!(
            "Git {} failed; inspect the retained workspace before retrying",
            arguments.first().copied().unwrap_or("command")
        );
    }
    Ok(output.stdout.trim_end_matches('\n').to_owned())
}

pub(super) fn require_remote_base(
    directory: &Path,
    remote: &str,
    base: &str,
    expected_start: &str,
    cancel_file: Option<&Path>,
    auth: Option<&GitNetworkAuth>,
) -> Result<()> {
    let reference = format!("refs/heads/{base}");
    let output = git_network(
        directory,
        &["ls-remote", "--exit-code", remote, &reference],
        cancel_file,
        auth,
    )?;
    if parse_remote_ref(&output, &reference)? != expected_start {
        bail!("the base branch advanced during the task; start a fresh run");
    }
    Ok(())
}

pub(super) fn parse_remote_ref<'a>(output: &'a str, reference: &str) -> Result<&'a str> {
    let (object_id, received_reference) = output
        .split_once('\t')
        .ok_or_else(|| anyhow!("Git returned an invalid remote reference"))?;
    if output.matches('\t').count() != 1
        || received_reference != reference
        || !matches!(object_id.len(), 40 | 64)
        || !object_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("Git returned an invalid remote reference");
    }
    Ok(object_id)
}

pub(super) fn git_network_auth(remote: &str) -> Result<Option<GitNetworkAuth>> {
    match env::var("KOELU_PUSH_TOKEN") {
        Ok(token) => git_network_auth_for_token(remote, &token).map(Some),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => bail!("KOELU_PUSH_TOKEN must contain valid text"),
    }
}

pub(super) fn git_network_auth_for_token(remote: &str, token: &str) -> Result<GitNetworkAuth> {
    Ok(GitNetworkAuth {
        environment: git_network_environment(remote, token)?,
    })
}

pub(super) fn git_network_environment(
    remote: &str,
    token: &str,
) -> Result<BTreeMap<String, String>> {
    if token.is_empty()
        || token.len() > MAX_PUSH_TOKEN_BYTES
        || !token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        bail!("KOELU_PUSH_TOKEN must be 1 to {MAX_PUSH_TOKEN_BYTES} printable ASCII characters");
    }
    if remote
        .strip_prefix("https://github.com/")
        .is_none_or(str::is_empty)
    {
        bail!("KOELU_PUSH_TOKEN requires an HTTPS github.com origin");
    }
    let credentials = format!("x-access-token:{token}");
    Ok(BTreeMap::from([
        ("GIT_CONFIG_COUNT".to_owned(), "1".to_owned()),
        (
            "GIT_CONFIG_KEY_0".to_owned(),
            "http.https://github.com/.extraheader".to_owned(),
        ),
        (
            "GIT_CONFIG_VALUE_0".to_owned(),
            format!("AUTHORIZATION: Basic {}", base64(credentials.as_bytes())),
        ),
    ]))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        encoded.push(ALPHABET[(chunk[0] >> 2) as usize] as char);
        encoded.push(
            ALPHABET[(((chunk[0] & 3) << 4) | (chunk.get(1).copied().unwrap_or(0) >> 4)) as usize]
                as char,
        );
        match chunk {
            [_, second, third] => {
                encoded.push(ALPHABET[(((second & 15) << 2) | (third >> 6)) as usize] as char);
                encoded.push(ALPHABET[(third & 63) as usize] as char);
            }
            [_, second] => {
                encoded.push(ALPHABET[((second & 15) << 2) as usize] as char);
                encoded.push('=');
            }
            [_] => encoded.push_str("=="),
            _ => unreachable!("chunks are never empty"),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_base_reference_parser_fails_closed() {
        let reference = "refs/heads/main";
        for (output, expected, accepted) in [
            (
                "0123456789abcdef0123456789abcdef01234567\trefs/heads/main",
                "0123456789abcdef0123456789abcdef01234567",
                true,
            ),
            (
                "0123456789abcdef0123456789abcdef01234567\trefs/heads/main",
                "fedcba9876543210fedcba9876543210fedcba98",
                false,
            ),
            (
                "0123456789abcdef0123456789abcdef01234567\trefs/heads/other",
                "0123456789abcdef0123456789abcdef01234567",
                false,
            ),
            (
                "0123456789abcdef0123456789abcdef01234567\trefs/heads/main\nextra",
                "0123456789abcdef0123456789abcdef01234567",
                false,
            ),
            (
                "short\trefs/heads/main",
                "0123456789abcdef0123456789abcdef01234567",
                false,
            ),
        ] {
            assert_eq!(
                parse_remote_ref(output, reference).is_ok_and(|object_id| object_id == expected),
                accepted
            );
        }
    }

    #[test]
    fn network_auth_is_ephemeral_and_limited_to_github_https() -> Result<()> {
        let environment = git_network_environment("https://github.com/owner/repo.git", "token")?;
        assert_eq!(environment["GIT_CONFIG_COUNT"], "1");
        assert_eq!(
            environment["GIT_CONFIG_KEY_0"],
            "http.https://github.com/.extraheader"
        );
        assert_eq!(
            environment["GIT_CONFIG_VALUE_0"],
            "AUTHORIZATION: Basic eC1hY2Nlc3MtdG9rZW46dG9rZW4="
        );
        assert!(environment.values().all(|value| !value.contains("token")));
        let hosted = git_network_auth_for_token("https://github.com/owner/repo.git", "app-token")?;
        assert_eq!(
            hosted.environment["GIT_CONFIG_KEY_0"],
            "http.https://github.com/.extraheader"
        );
        assert!(
            hosted
                .environment
                .values()
                .all(|value| !value.contains("app-token"))
        );
        for (remote, token) in [
            ("git@github.com:owner/repo.git", "token"),
            ("https://example.com/owner/repo.git", "token"),
            ("https://github.com/owner/repo.git", ""),
        ] {
            assert!(git_network_environment(remote, token).is_err());
        }
        assert!(
            git_network_environment(
                "https://github.com/owner/repo.git",
                &"x".repeat(MAX_PUSH_TOKEN_BYTES + 1)
            )
            .is_err()
        );
        Ok(())
    }
}
