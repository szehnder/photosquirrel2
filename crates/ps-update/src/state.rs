//! What the updater remembers between runs: `state.json` in the update folder (ADR-0124, ADR-0125, ADR-0126).
//!
//! Kept outside `settings.toml`, which the running app owns (ADR-0049). Written atomically (a temporary file,
//! then a rename); a missing or unreadable file reads as the empty state, which only means "check again".

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Manifest;
use crate::dir::UpdateDir;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// The last check that ended (successfully or not).
    pub last_check: Option<CheckRecord>,
    /// When a check last succeeded.
    pub last_success_at: Option<i64>,
    /// Seconds added to the next scheduled check, drawn when the last check completed (0-3600).
    pub jitter_s: u64,
    /// Scheduled checks that failed in a row, and since when.
    pub failures_since: Option<i64>,
    /// The newest version whose manifest verified: nothing older is offered again.
    pub highest_accepted: Option<String>,
    /// The newest verified manifest, while it offers a version newer than this build.
    pub available: Option<Manifest>,
    /// A verified payload waiting to be installed.
    pub ready: Option<Ready>,
    /// The copy of the version the last install replaced, for rollback.
    pub kept: Option<Kept>,
    /// The first starts of a newly installed version.
    pub trial: Option<Trial>,
    /// Versions never offered again automatically (failed to install or start, rolled back, or skipped).
    pub skipped: Vec<String>,
    /// Versions whose notice the user dismissed.
    pub dismissed: Vec<String>,
    /// Versions whose manual-install fallback was already offered (macOS bundle swap refused).
    pub fallback_offered: Vec<String>,
    /// Data directories (catalogs) this installation has used, for the rollback guard. At most 16.
    pub catalogs: Vec<PathBuf>,
    /// The version that last showed the one-time "Updated to" notice.
    pub announced: Option<String>,
    /// The last install or rollback that failed, shown until dismissed.
    pub last_failure: Option<Failure>,
    /// In notify mode, the user chose to install the available version at the next quit.
    pub install_on_quit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub at: i64,
    /// The version that could not be installed.
    pub version: String,
    /// The version the user is still on.
    pub current: String,
    pub message: String,
    /// What failed: the install, or the new version's first starts.
    #[serde(default)]
    pub reason: FailureReason,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    #[default]
    Install,
    /// The new version did not start; the previous one was put back.
    Start,
    /// The new version had trouble starting but the catalog was already upgraded, so it was kept.
    StartKept,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRecord {
    pub at: i64,
    pub ok: bool,
    /// "up to date", "PhotoSquirrel 0.4.1 is available", or the failure.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    pub version: String,
    /// The package kind (`appimage`, `app.tar.gz`, `setup`, `deb`).
    pub kind: String,
    pub file: PathBuf,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub version: String,
    pub path: PathBuf,
    /// The newest catalog format (`PRAGMA user_version`) the kept version opens.
    pub max_catalog_schema: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trial {
    pub version: String,
    pub starts: u32,
    pub ok: bool,
}

/// The most catalogs remembered for the rollback guard.
pub const MAX_CATALOGS: usize = 16;

impl UpdateState {
    /// Read the state, or the empty state when there is none or it cannot be read.
    pub fn load(dir: &UpdateDir) -> Self {
        match fs::read(dir.state_file()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("update state unreadable ({e}); starting from an empty state");
                Self::default()
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log::warn!("update state unreadable ({e}); starting from an empty state");
                Self::default()
            }
        }
    }

    /// Write the state atomically.
    pub fn save(&self, dir: &UpdateDir) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(&dir.state_file(), &bytes)
    }

    /// Remember a data directory this installation used.
    pub fn remember_catalog(&mut self, data_dir: &Path) -> bool {
        let p = data_dir.to_path_buf();
        if self.catalogs.contains(&p) {
            return false;
        }
        self.catalogs.push(p);
        if self.catalogs.len() > MAX_CATALOGS {
            self.catalogs.remove(0);
        }
        true
    }

    /// Forget an offer or a staged payload that is not newer than `installed` (left by the update that installed
    /// it, or by an install done by hand). Returns whether anything changed.
    pub fn forget_not_newer(&mut self, installed: &semver::Version) -> bool {
        let mut changed = false;
        if self
            .available
            .as_ref()
            .is_some_and(|a| !crate::version::newer(&a.version, installed))
        {
            self.available = None;
            changed = true;
        }
        if self
            .ready
            .as_ref()
            .is_some_and(|r| !crate::version::newer(&r.version, installed))
        {
            self.ready = None;
            self.install_on_quit = false;
            changed = true;
        }
        changed
    }

    pub fn is_skipped(&self, version: &str) -> bool {
        self.skipped.iter().any(|v| v == version)
    }

    pub fn skip(&mut self, version: &str) {
        if !self.is_skipped(version) {
            self.skipped.push(version.to_owned());
        }
    }
}

/// Write `bytes` to `path` through `<path>.tmp` and a rename, so a reader sees the old file or the new one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_a_torn_temporary_file_is_ignored() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = UpdateDir::at(tmp.path());
        assert_eq!(UpdateState::load(&dir), UpdateState::default());
        let mut s = UpdateState {
            highest_accepted: Some("0.4.1".into()),
            ..Default::default()
        };
        s.skip("0.4.0");
        s.save(&dir).expect("save");
        // A write that died half way left only the temporary file.
        fs::write(tmp.path().join("state.json.tmp"), b"{\"highest_acc").expect("torn");
        assert_eq!(UpdateState::load(&dir), s);
        // A corrupt state file reads as empty instead of failing.
        fs::write(dir.state_file(), b"not json").expect("corrupt");
        assert_eq!(UpdateState::load(&dir), UpdateState::default());
    }

    #[test]
    fn at_most_sixteen_catalogs_are_remembered() {
        let mut s = UpdateState::default();
        for i in 0..20 {
            s.remember_catalog(Path::new(&format!("/data/{i}")));
        }
        assert!(!s.remember_catalog(Path::new("/data/19")));
        assert_eq!(s.catalogs.len(), MAX_CATALOGS);
        assert_eq!(s.catalogs[0], PathBuf::from("/data/4"));
    }
}
