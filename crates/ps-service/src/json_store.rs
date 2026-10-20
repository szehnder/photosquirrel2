//! Small JSON state files in the data directory (`review-state.json`, `ai-review.json`; ADR-0042).
//!
//! - [`load`] reads a file; a missing file is the default state. A file that does not parse is **never
//!   overwritten**: it is renamed to `<name>.damaged-<unix-ms>` beside it and a warning is logged before the
//!   caller starts afresh, so the user's decisions can still be recovered by hand.
//! - [`save`] writes a temporary file in the same folder, `sync_all`s it, renames it over the old file and
//!   fsyncs the folder (Unix), so a crash leaves either the old or the new state, never a torn file.
//!
//! The catalog owner lock (ADR-0041) guarantees one writer process per data directory; within the process each
//! store serialises its writes behind its own mutex.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The state in `path`, or `T::default()` when there is none or it is damaged (then moved aside).
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> T {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("could not read {}: {e}; starting afresh", path.display());
            }
            return T::default();
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(e) => {
            let aside = damaged_name(path, ps_export::now_ms());
            match std::fs::rename(path, &aside) {
                Ok(()) => log::warn!(
                    "{} is damaged ({e}); kept it as {} and starting afresh",
                    path.display(),
                    aside.display()
                ),
                Err(re) => log::warn!(
                    "{} is damaged ({e}) and could not be moved aside ({re}); starting afresh",
                    path.display()
                ),
            }
            T::default()
        }
    }
}

/// `<name>.damaged-<stamp>` beside `path`.
pub fn damaged_name(path: &Path, stamp: i64) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".damaged-{stamp}"));
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
        assert_eq!(load::<BTreeMap<String, i64>>(&path), m);
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
        let st: BTreeMap<String, i64> = load(&path);
        assert!(st.is_empty());
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
}
