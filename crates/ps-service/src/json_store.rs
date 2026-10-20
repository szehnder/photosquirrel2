//! Small JSON state files in the data directory (`review-state.json`, `ai-review.json`; ADR-0042).
//!
//! - [`load`] reads a file; a missing file is the default state. A file that does not parse, or that cannot be
//!   read at all (EIO, EACCES, ...), is **never overwritten**: it is renamed to `<name>.damaged-<unix-ms>` (or
//!   `<name>.unreadable-<unix-ms>`) beside it and a warning is logged before the caller starts afresh, so the
//!   user's decisions can still be recovered by hand. When it cannot even be moved aside, the load is
//!   [`Loaded::blocked`]: [`JsonFile`] then refuses every save instead of replacing the file.
//! - [`save`] writes a temporary file in the same folder, `sync_all`s it, renames it over the old file and
//!   fsyncs the folder (Unix), so a crash leaves either the old or the new state, never a torn file.
//! - [`JsonFile`] is the in-memory state plus its file: changes that cannot be saved are rolled back and
//!   returned as errors, so the views show them instead of silently keeping a change that is lost on quit.
//!
//! The catalog owner lock (ADR-0041) guarantees one writer process per data directory; within the process each
//! store serialises its writes behind its own mutex.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ps_catalog::CatalogError;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// What [`load`] found.
#[derive(Debug)]
pub struct Loaded<T> {
    pub state: T,
    /// Why the file must not be saved over (it could not be read and could not be moved aside), if so.
    pub blocked: Option<String>,
}

/// The state in `path`, or `T::default()` when there is none or it is damaged or unreadable (then moved
/// aside; when that fails too, the result is [`Loaded::blocked`]).
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> Loaded<T> {
    let fresh = |blocked| Loaded {
        state: T::default(),
        blocked,
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return fresh(None),
        Err(e) => {
            let aside = aside_name(path, "unreadable", ps_export::now_ms());
            return match std::fs::rename(path, &aside) {
                Ok(()) => {
                    log::warn!(
                        "could not read {} ({e}); kept it as {} and starting afresh",
                        path.display(),
                        aside.display()
                    );
                    fresh(None)
                }
                Err(re) => {
                    log::error!(
                        "could not read {} ({e}) or move it aside ({re}); changes will not be saved over it",
                        path.display()
                    );
                    fresh(Some(format!(
                        "it could not be read ({e}) and is left untouched"
                    )))
                }
            };
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(state) => Loaded {
            state,
            blocked: None,
        },
        Err(e) => {
            let aside = damaged_name(path, ps_export::now_ms());
            match std::fs::rename(path, &aside) {
                Ok(()) => {
                    log::warn!(
                        "{} is damaged ({e}); kept it as {} and starting afresh",
                        path.display(),
                        aside.display()
                    );
                    fresh(None)
                }
                Err(re) => {
                    log::error!(
                        "{} is damaged ({e}) and could not be moved aside ({re}); changes will not be saved over it",
                        path.display()
                    );
                    fresh(Some(format!(
                        "it is damaged and could not be moved aside ({re}); it is left untouched"
                    )))
                }
            }
        }
    }
}

/// `<name>.damaged-<stamp>` beside `path`.
pub fn damaged_name(path: &Path, stamp: i64) -> PathBuf {
    aside_name(path, "damaged", stamp)
}

/// `<name>.<what>-<stamp>` beside `path`.
fn aside_name(path: &Path, what: &str, stamp: i64) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{what}-{stamp}"));
    path.with_file_name(name)
}

/// Write `state` to `path` atomically and durably (temp file, `sync_all`, rename, folder fsync).
pub fn save<T: Serialize>(path: &Path, state: &T) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// A state file and its in-memory state (`None` path: nothing persisted, for tests).
#[derive(Debug)]
pub struct JsonFile<S> {
    path: Option<PathBuf>,
    blocked: Option<String>,
    state: Mutex<S>,
}

impl<S: Serialize + DeserializeOwned + Default + Clone> JsonFile<S> {
    /// Load `path` (see [`load`]).
    pub fn open(path: PathBuf) -> Self {
        let loaded = load(&path);
        JsonFile {
            path: Some(path),
            blocked: loaded.blocked,
            state: Mutex::new(loaded.state),
        }
    }

    /// Nothing persisted.
    pub fn in_memory() -> Self {
        JsonFile {
            path: None,
            blocked: None,
            state: Mutex::new(S::default()),
        }
    }

    /// Read the state.
    pub fn read<T>(&self, f: impl FnOnce(&S) -> T) -> T {
        f(&self.state.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Change the state and save it. When the file must not be replaced ([`Loaded::blocked`]) nothing changes;
    /// when the save fails the change is rolled back. Both are errors naming the file (not its full path, which
    /// the views show).
    pub fn update<T>(&self, f: impl FnOnce(&mut S) -> T) -> Result<T, CatalogError> {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(path) = &self.path else {
            return Ok(f(&mut st));
        };
        let name = PathBuf::from(path.file_name().unwrap_or_default());
        if let Some(why) = &self.blocked {
            return Err(CatalogError::Io {
                path: name,
                source: std::io::Error::other(format!("not saved: {why}")),
            });
        }
        let before = st.clone();
        let out = f(&mut st);
        if let Err(source) = save(path, &*st) {
            log::error!("could not save {}: {source}", path.display());
            *st = before;
            return Err(CatalogError::Io { path: name, source });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn save_then_load_round_trips_and_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("state.json");
        let mut m = BTreeMap::new();
        m.insert("a".to_owned(), 1i64);
        save(&path, &m).expect("save");
        assert_eq!(load::<BTreeMap<String, i64>>(&path).state, m);
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("state.json")]);
    }

    #[test]
    fn damaged_file_is_moved_aside_not_overwritten() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("state.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let st: Loaded<BTreeMap<String, i64>> = load(&path);
        assert!(st.state.is_empty());
        assert!(st.blocked.is_none());
        assert!(
            !path.exists(),
            "the damaged file is renamed, not left in place"
        );
        let aside: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].starts_with("state.json.damaged-"), "{aside:?}");
        assert_eq!(
            std::fs::read(tmp.path().join(&aside[0])).unwrap(),
            b"{ not json"
        );
    }

    /// A file the process cannot read (EACCES) is moved aside, so the next save does not replace it.
    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_moved_aside_not_overwritten() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("state.json");
        std::fs::write(&path, b"{\"a\": 1}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            eprintln!("skipping: running as a user that can read mode-000 files (root)");
            return;
        }
        let file: JsonFile<BTreeMap<String, i64>> = JsonFile::open(path.clone());
        assert!(file.read(BTreeMap::is_empty));
        file.update(|m| m.insert("b".into(), 2))
            .expect("saves a fresh file");
        let aside: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("state.json.unreadable-"))
            .collect();
        assert_eq!(aside.len(), 1, "the unreadable file is kept beside");
        let kept = tmp.path().join(&aside[0]);
        std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read(&kept).unwrap(), b"{\"a\": 1}");
        assert_eq!(
            load::<BTreeMap<String, i64>>(&path).state.get("b"),
            Some(&2)
        );
    }

    /// When the unreadable file cannot be moved aside either (read-only folder), saves are refused and the
    /// in-memory state is unchanged.
    #[cfg(unix)]
    #[test]
    fn unreadable_file_that_cannot_move_blocks_saves() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("data");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, b"{\"a\": 1}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let restore = || {
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        };
        if std::fs::read(&path).is_ok() {
            restore();
            eprintln!("skipping: running as a user that can read mode-000 files (root)");
            return;
        }
        let file: JsonFile<BTreeMap<String, i64>> = JsonFile::open(path.clone());
        let err = file
            .update(|m| m.insert("b".into(), 2))
            .expect_err("refuses to save over an unreadable file");
        assert!(err.to_string().contains("state.json"), "{err}");
        assert!(file.read(BTreeMap::is_empty), "nothing changed in memory");
        restore();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"{\"a\": 1}",
            "the file is untouched"
        );
    }

    /// A save that fails (read-only folder) is an error and is rolled back in memory.
    #[cfg(unix)]
    #[test]
    fn failed_save_is_an_error_and_rolled_back() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("data");
        std::fs::create_dir(&dir).unwrap();
        let file: JsonFile<BTreeMap<String, i64>> = JsonFile::open(dir.join("state.json"));
        file.update(|m| m.insert("a".into(), 1))
            .expect("first save");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let r = file.update(|m| m.insert("b".into(), 2));
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        if r.is_ok() {
            eprintln!("skipping: running as a user that can write read-only folders (root)");
            return;
        }
        let err = r.expect_err("the save fails");
        assert!(err.to_string().contains("state.json"), "{err}");
        assert!(
            !err.to_string().contains(&*dir.to_string_lossy()),
            "names the file, not its full path: {err}"
        );
        assert_eq!(file.read(|m| m.get("b").copied()), None, "rolled back");
        assert_eq!(file.read(|m| m.get("a").copied()), Some(1));
    }
}
