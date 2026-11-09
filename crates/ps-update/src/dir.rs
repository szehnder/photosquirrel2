//! The update folder: the only place, besides the installation it updates, where the updater writes (ADR-0126).
//!
//! Per user and per machine, beside PhotoSquirrel's other local data and independent of `--data-dir`:
//! `<platform local data dir>/updates` (`directories::ProjectDirs` for `io.github.szehnder.photosquirrel`).

use std::path::{Path, PathBuf};

/// Moves the update folder (tests).
pub const DIR_ENV: &str = "PHOTOSQUIRREL_UPDATE_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDir {
    root: PathBuf,
}

impl UpdateDir {
    /// The update folder of this user (or `PHOTOSQUIRREL_UPDATE_DIR`, which tests use to stay out of the real
    /// one), or `None` when the platform has no home directory.
    pub fn for_user() -> Option<Self> {
        if let Some(dir) = std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
            return Some(Self::at(dir));
        }
        directories::ProjectDirs::from("io.github", "szehnder", "photosquirrel")
            .map(|d| Self::at(d.data_local_dir().join("updates")))
    }

    /// An update folder at `root` (tests, and the helper, which is told where it is).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `state.json`: what the updater knows ([`crate::state::UpdateState`]).
    pub fn state_file(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// `history.jsonl`: the last 200 events ([`crate::history`]).
    pub fn history_file(&self) -> PathBuf {
        self.root.join("history.jsonl")
    }

    /// Partial downloads (`<name>.part`).
    pub fn download_dir(&self) -> PathBuf {
        self.root.join("download")
    }

    /// The verified payload of a version, ready to install.
    pub fn ready_dir(&self, version: &str) -> PathBuf {
        self.root.join("ready").join(version)
    }

    /// The kept copy of the version an install replaced.
    pub fn previous_dir(&self, version: &str) -> PathBuf {
        self.root.join("previous").join(version)
    }

    /// The copy of the installed CLI that performs an install after the app exits.
    pub fn helper_dir(&self, version: &str) -> PathBuf {
        self.root.join("helper").join(version)
    }

    /// Per-installation running locks.
    pub fn installs_dir(&self) -> PathBuf {
        self.root.join("installs")
    }
}
