use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{anyhow, bail};
use getrandom::fill;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Result;

const RUNS_DIRECTORY: &str = "KOELU_RUNS_DIR";
const MEMORY_DIRECTORY: &str = "KOELU_MEMORY_DIR";
const CANCELLED: &str = "cancelled";
const MAX_LISTED_RUNS: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Run {
    id: String,
    path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunStore {
    root: PathBuf,
}

impl RunStore {
    pub fn open() -> Result<Self> {
        Self::at(default_root())
    }

    pub fn memory() -> Result<Self> {
        Self::at(memory_root())
    }

    pub fn at(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        match fs::symlink_metadata(&root) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&root)?;
            }
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata(&root)?;
        if !metadata.is_dir() {
            bail!("Koelu run root is not a directory: {}", root.display());
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create(&self) -> Result<Run> {
        for _ in 0..32 {
            let id = new_id()?;
            let path = self.root.join(&id);
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Run { id, path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not allocate a unique Koelu run ID")
    }

    pub fn load(&self, id: &str) -> Result<Run> {
        validate_id(id)?;
        let path = self.root.join(id);
        if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            bail!("Koelu run does not exist: {id}");
        }
        Ok(Run {
            id: id.to_owned(),
            path,
        })
    }

    pub fn list(&self) -> Result<Vec<Run>> {
        let mut newest = BinaryHeap::with_capacity(MAX_LISTED_RUNS + 1);
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str() else {
                continue;
            };
            if validate_id(id).is_ok() && entry.file_type()?.is_dir() {
                let modified = entry.metadata()?.modified().unwrap_or(UNIX_EPOCH);
                newest.push(Reverse((modified, id.to_owned(), entry.path())));
                if newest.len() > MAX_LISTED_RUNS {
                    newest.pop();
                }
            }
        }
        let mut newest = newest
            .into_iter()
            .map(|Reverse(value)| value)
            .collect::<Vec<_>>();
        newest.sort_unstable_by(|left, right| right.cmp(left));
        Ok(newest
            .into_iter()
            .map(|(_, id, path)| Run { id, path })
            .collect())
    }
}

impl Run {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(value)?;
        self.write_bytes(name, &bytes)
    }

    pub fn read_json<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let path = self.file(name)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1_000_000 {
            bail!("Koelu run JSON must be a regular file no larger than 1 MB");
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    pub fn write_text(&self, name: &str, value: &str) -> Result<()> {
        self.write_bytes(name, value.as_bytes())
    }

    pub fn read_text(&self, name: &str) -> Result<String> {
        let path = self.file(name)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4_000_000 {
            bail!("Koelu run text must be a regular file no larger than 4 MB");
        }
        Ok(fs::read_to_string(path)?)
    }

    pub fn cancel(&self) -> Result<()> {
        self.write_text(CANCELLED, "cancelled\n")
    }

    pub fn is_cancelled(&self) -> Result<bool> {
        match fs::symlink_metadata(self.file(CANCELLED)?) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
            Ok(_) => bail!("Koelu cancellation marker is not a regular file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn file(&self, name: &str) -> Result<PathBuf> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            bail!(
                "Koelu run file names may contain only letters, numbers, dots, underscores and hyphens"
            );
        }
        Ok(self.path.join(name))
    }

    fn write_bytes(&self, name: &str, bytes: &[u8]) -> Result<()> {
        atomic_write(&self.file(name)?, bytes)
    }
}

fn default_root() -> PathBuf {
    root_from(|name| env::var_os(name), env::temp_dir())
}

fn memory_root() -> PathBuf {
    if let Some(root) = env::var_os(MEMORY_DIRECTORY).filter(|value| !value.is_empty()) {
        return PathBuf::from(root);
    }
    default_root().parent().map_or_else(
        || env::temp_dir().join("koelu-memory"),
        |root| root.join("memory"),
    )
}

fn root_from<F>(variable: F, temporary: PathBuf) -> PathBuf
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(root) = variable(RUNS_DIRECTORY).filter(|value| !value.is_empty()) {
        return PathBuf::from(root);
    }
    if let Some(root) = variable("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("koelu").join("runs");
    }
    if let Some(home) = variable("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".local/state/koelu/runs");
    }
    if let Some(root) = variable("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("Koelu").join("runs");
    }
    temporary.join("koelu-runs")
}

fn new_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    fill(&mut bytes).map_err(|error| anyhow!("could not generate Koelu run ID: {error}"))?;
    Ok(format!("run_{}", hex(&bytes)))
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() == 36
        && id.starts_with("run_")
        && id[4..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    bail!("invalid Koelu run ID")
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(DIGITS[(byte >> 4) as usize] as char);
        value.push(DIGITS[(byte & 15) as usize] as char);
    }
    value
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Koelu run file has no parent directory"))?;
    for _ in 0..32 {
        let temporary = parent.join(format!(
            ".{}.tmp-{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            new_id()?
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let outcome = (|| -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            sync_directory(parent)?;
            Ok(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return outcome;
    }
    bail!("could not allocate an atomic Koelu run write")
}

#[cfg(target_os = "linux")]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use serde_json::Value;
    use tempfile::tempdir;

    use super::{MAX_LISTED_RUNS, RunStore, root_from, validate_id};

    #[test]
    fn validates_opaque_run_ids() {
        let valid = "run_0123456789abcdef0123456789abcdef";
        for (id, accepted) in [
            (valid, true),
            ("run_0123456789abcdef0123456789abcdeg", false),
            ("run_0123456789abcdef0123456789abcdef/child", false),
            ("../run_0123456789abcdef0123456789abcdef", false),
            ("run_short", false),
        ] {
            assert_eq!(validate_id(id).is_ok(), accepted, "{id}");
        }
    }

    #[test]
    fn writes_lists_and_cancels_runs() {
        let temporary = tempdir().unwrap();
        let store = RunStore::at(temporary.path()).unwrap();
        let run = store.create().unwrap();
        run.write_text("note.txt", "first").unwrap();
        run.write_text("note.txt", "second").unwrap();
        run.write_json("run.json", &BTreeMap::from([("stage", "checked")]))
            .unwrap();

        assert_eq!(run.read_text("note.txt").unwrap(), "second");
        assert_eq!(
            run.read_json::<BTreeMap<String, String>>("run.json")
                .unwrap()["stage"],
            "checked"
        );
        assert!(!run.is_cancelled().unwrap());
        run.cancel().unwrap();
        assert!(run.is_cancelled().unwrap());
        assert_eq!(store.list().unwrap(), vec![run.clone()]);
        assert_eq!(store.load(run.id()).unwrap(), run);
        assert_eq!(
            fs::metadata(run.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(run.path().join("run.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        symlink(run.path().join("run.json"), run.path().join("linked.json")).unwrap();
        assert!(run.read_json::<Value>("linked.json").is_err());
        let linked_id = "run_ffffffffffffffffffffffffffffffff";
        symlink(run.path(), temporary.path().join(linked_id)).unwrap();
        assert!(store.load(linked_id).is_err());
        assert_eq!(store.list().unwrap(), vec![run.clone()]);
        assert!(!fs::read_dir(temporary.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")
        }));
    }

    #[test]
    fn bounds_retained_run_listing() {
        let temporary = tempdir().unwrap();
        let store = RunStore::at(temporary.path()).unwrap();
        for index in 0..=MAX_LISTED_RUNS {
            fs::create_dir(temporary.path().join(format!("run_{index:032x}"))).unwrap();
        }

        let runs = store.list().unwrap();
        assert_eq!(runs.len(), MAX_LISTED_RUNS);
        assert!(
            !runs
                .iter()
                .any(|run| run.id() == "run_00000000000000000000000000000000")
        );
    }

    #[test]
    fn selects_override_without_mutating_process_environment() {
        let variables = BTreeMap::from([(
            "KOELU_RUNS_DIR".to_owned(),
            OsString::from("/custom/koelu-runs"),
        )]);
        let root = root_from(|name| variables.get(name).cloned(), "/temporary".into());
        assert_eq!(root, std::path::PathBuf::from("/custom/koelu-runs"));
    }
}
