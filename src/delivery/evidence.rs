use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use anyhow::{anyhow, bail};
use nix::errno::Errno;
use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::Mode;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::quality;

use super::git::git;
use super::hex;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub(super) enum Fingerprint {
    Missing,
    File { mode: u32, sha256: String },
    Symlink { mode: u32, target: String },
}

pub(super) const MAX_CREDENTIAL_SCAN_BYTES: usize = 1_000_000;
pub(super) const CREDENTIAL_PATTERN: &str = r"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----|(?:github_pat_[A-Za-z0-9_]{20,255}|gh[pousr]_[A-Za-z0-9]{20,255}|(?:AKIA|ASIA)[0-9A-Z]{16}|(?:AIza[A-Za-z0-9_-]{35}|AQ\.[A-Za-z0-9_-]{20,255}|csk-[A-Za-z0-9_-]{20,255}|xai-[A-Za-z0-9_-]{20,255})|sk-(?:ant-[A-Za-z0-9_-]{20,255}|proj-[A-Za-z0-9_-]{20,255}|[A-Za-z0-9_-]{32,255}))";

pub(super) fn changed_files(
    directory: &Path,
    reference: &str,
    cancel_file: Option<&Path>,
) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    for name in git(
        directory,
        &["diff", "--name-only", "-z", reference, "--"],
        cancel_file,
    )?
    .split('\0')
    {
        if !name.is_empty() {
            names.insert(name.to_owned());
        }
    }
    for name in git(
        directory,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        cancel_file,
    )?
    .split('\0')
    {
        if !name.is_empty() {
            names.insert(name.to_owned());
        }
    }
    let credential = Regex::new(CREDENTIAL_PATTERN)?;
    for name in &names {
        if !quality::relative_path(name) {
            bail!("refusing a path outside the task workspace");
        }
        let path = Path::new(name);
        let basename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let suffix = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if basename == ".env"
            || (basename.starts_with(".env.") && !matches!(suffix.as_str(), "example" | "sample"))
            || basename == "auth.json"
            || matches!(suffix.as_str(), "pem" | "p12" | "pfx")
        {
            bail!("potential credential file requires manual review: {name}");
        }
        scan_credentials(directory, path, name, &credential)?;
    }
    Ok(names.into_iter().collect())
}

pub(super) fn scan_credentials(
    directory: &Path,
    path: &Path,
    name: &str,
    credential: &Regex,
) -> Result<()> {
    let Some(file) = open_scannable_file(directory, path, name)? else {
        return Ok(());
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() > MAX_CREDENTIAL_SCAN_BYTES as u64
        || metadata.nlink() > 1
    {
        bail!("credential scan requires a unique file no larger than 1 MB: {name}");
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
    file.take((MAX_CREDENTIAL_SCAN_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CREDENTIAL_SCAN_BYTES {
        bail!("credential scan file grew beyond 1 MB: {name}");
    }
    let content = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow!("credential scan requires text or an approved binary path: {name}"))?;
    if credential.is_match(content) {
        bail!("credential material requires manual review: {name}");
    }
    Ok(())
}

fn open_scannable_file(directory: &Path, path: &Path, name: &str) -> Result<Option<File>> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("refusing a path outside the task workspace");
    }
    let directory_flags = OFlag::O_RDONLY
        | OFlag::O_CLOEXEC
        | OFlag::O_DIRECTORY
        | OFlag::O_NOFOLLOW
        | OFlag::O_NONBLOCK;
    let mut parent = open(directory, directory_flags, Mode::empty())
        .map_err(|error| anyhow!("cannot safely open the task workspace: {error}"))?;
    let mut components = path.components().peekable();
    while let Some(Component::Normal(component)) = components.next() {
        let final_component = components.peek().is_none();
        let flags = if final_component {
            OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK
        } else {
            directory_flags
        };
        match openat(&parent, component, flags, Mode::empty()) {
            Ok(file) if final_component => return Ok(Some(file.into())),
            Ok(directory) => parent = directory,
            Err(Errno::ENOENT) => return Ok(None),
            Err(Errno::ELOOP) if final_component => {
                bail!("changed symlink requires manual review: {name}")
            }
            Err(error) => bail!("cannot safely scan changed file {name}: {error}"),
        }
    }
    bail!("refusing a path outside the task workspace")
}

pub(super) fn snapshot(
    directory: &Path,
    names: &[String],
) -> Result<BTreeMap<String, Fingerprint>> {
    names
        .iter()
        .map(|name| {
            let path = directory.join(name);
            let value = match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Fingerprint::Missing,
                Err(error) => return Err(error.into()),
                Ok(metadata) if metadata.file_type().is_symlink() => Fingerprint::Symlink {
                    mode: metadata.mode(),
                    target: fs::read_link(&path)?.to_string_lossy().into_owned(),
                },
                Ok(metadata) if metadata.is_file() => {
                    let file = open_scannable_file(directory, Path::new(name), name)?.ok_or_else(
                        || anyhow!("changed file disappeared while capturing evidence: {name}"),
                    )?;
                    let metadata = file.metadata()?;
                    if !metadata.is_file() {
                        bail!("unsupported changed file requires manual review: {name}");
                    }
                    Fingerprint::File {
                        mode: metadata.mode(),
                        sha256: hash_reader(file)?,
                    }
                }
                Ok(_) => bail!("unsupported changed file requires manual review: {name}"),
            };
            Ok((name.clone(), value))
        })
        .collect()
}

pub(super) fn hash_file(path: &Path) -> Result<String> {
    hash_reader(File::open(path)?)
}

fn hash_reader(mut file: File) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn evidence(
    directory: &Path,
    start: &str,
    max_files: usize,
    max_lines: usize,
    cancel_file: Option<&Path>,
) -> Result<(Vec<String>, String, BTreeMap<String, Fingerprint>)> {
    let names = changed_files(directory, start, cancel_file)?;
    if names.is_empty() {
        bail!("no changes were produced");
    }
    if names.len() > max_files {
        bail!(
            "efficiency gate: {} changed files exceeds {max_files}",
            names.len()
        );
    }
    let mut diff = git(
        directory,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=3",
            start,
            "--",
        ],
        cancel_file,
    )?;
    for name in git(
        directory,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        cancel_file,
    )?
    .split('\0')
    .filter(|name| !name.is_empty())
    {
        let Some(file) = open_scannable_file(directory, Path::new(name), name)? else {
            bail!("changed file disappeared while capturing evidence: {name}");
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            bail!("unsupported changed file requires manual review: {name}");
        }
        if metadata.len() > 64_000 {
            bail!("review context exceeds 64000 bytes; split the task");
        }
        let mut content = String::new();
        file.take(64_001).read_to_string(&mut content)?;
        if content.len() > 64_000 {
            bail!("review context exceeds 64000 bytes; split the task");
        }
        diff.push_str(&format!("\nNew file: {name}\n"));
        for line in content.lines() {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
    }
    if diff.len() > 64_000 {
        bail!("review context exceeds 64000 bytes; split the task");
    }
    let changed_lines = diff
        .lines()
        .filter(|line| {
            (line.starts_with('+') || line.starts_with('-'))
                && !line.starts_with("+++")
                && !line.starts_with("---")
        })
        .count();
    if changed_lines > max_lines {
        bail!("efficiency gate: {changed_lines} changed lines exceeds {max_lines}");
    }
    Ok((names.clone(), diff, snapshot(directory, &names)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_scan_is_bounded_and_never_follows_symlinks() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let credential = Regex::new(CREDENTIAL_PATTERN)?;
        for (name, content, rejected) in [
            ("safe.txt", "ordinary text", false),
            ("secret.txt", "-----BEGIN PRIVATE KEY-----", true),
            (
                "token.txt",
                "github_pat_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                true,
            ),
            ("temporary-key.txt", "ASIAAAAAAAAAAAAAAAAA", true),
            (
                "gemini-legacy.txt",
                "AIzaAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                true,
            ),
            ("gemini-current.txt", "AQ.AAAAAAAAAAAAAAAAAAAA", true),
            ("cerebras.txt", "csk-AAAAAAAAAAAAAAAAAAAA", true),
            ("xai.txt", "xai-AAAAAAAAAAAAAAAAAAAA", true),
        ] {
            let path = temporary.path().join(name);
            fs::write(&path, content)?;
            assert_eq!(
                scan_credentials(temporary.path(), Path::new(name), name, &credential).is_err(),
                rejected
            );
        }
        let external = tempfile::tempdir()?;
        let outside = external.path().join("outside.txt");
        fs::write(&outside, "-----BEGIN PRIVATE KEY-----")?;
        std::os::unix::fs::symlink(&outside, temporary.path().join("linked.txt"))?;
        assert!(
            scan_credentials(
                temporary.path(),
                Path::new("linked.txt"),
                "linked.txt",
                &credential
            )
            .is_err()
        );
        std::os::unix::fs::symlink(external.path(), temporary.path().join("linked-directory"))?;
        assert!(
            scan_credentials(
                temporary.path(),
                Path::new("linked-directory/outside.txt"),
                "linked-directory/outside.txt",
                &credential
            )
            .is_err()
        );
        let large = temporary.path().join("large.txt");
        fs::write(&large, vec![b'x'; MAX_CREDENTIAL_SCAN_BYTES + 1])?;
        assert!(
            scan_credentials(
                temporary.path(),
                Path::new("large.txt"),
                "large.txt",
                &credential
            )
            .is_err()
        );
        let socket = temporary.path().join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket)?;
        assert!(
            scan_credentials(temporary.path(), Path::new("socket"), "socket", &credential).is_err()
        );
        Ok(())
    }

    #[test]
    fn snapshot_uses_safe_workspace_descriptors() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        fs::write(temporary.path().join("regular.txt"), "safe")?;
        std::os::unix::fs::symlink("regular.txt", temporary.path().join("linked.txt"))?;
        let external = tempfile::tempdir()?;
        fs::write(external.path().join("outside.txt"), "outside")?;
        std::os::unix::fs::symlink(external.path(), temporary.path().join("linked-directory"))?;
        let socket = temporary.path().join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket)?;
        for (name, accepted) in [
            ("regular.txt", true),
            ("linked.txt", true),
            ("linked-directory/outside.txt", false),
            ("socket", false),
        ] {
            assert_eq!(
                snapshot(temporary.path(), &[name.to_owned()]).is_ok(),
                accepted,
                "{name}"
            );
        }
        assert!(matches!(
            snapshot(temporary.path(), &["linked.txt".to_owned()])?.get("linked.txt"),
            Some(Fingerprint::Symlink { .. })
        ));
        Ok(())
    }
}
