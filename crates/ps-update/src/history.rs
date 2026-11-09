//! `history.jsonl`: every check, download, install, trial, rollback and failure, one JSON line each, the last
//! [`MAX_ENTRIES`] kept (spec "Update history"). The user can read it; support asks for it.

use std::fs;
use std::io;

use serde::{Deserialize, Serialize};

use crate::dir::UpdateDir;
use crate::state::write_atomic;

pub const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Seconds since the Unix epoch.
    pub at: i64,
    /// `check`, `download`, `install`, `trial`, `rollback`, `postponed` or `failure`.
    pub event: String,
    /// The version the event is about, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub detail: String,
}

/// Append an entry, keeping the last [`MAX_ENTRIES`]. A history that cannot be written is logged, never fatal.
pub fn record(dir: &UpdateDir, event: &str, version: Option<&str>, detail: &str) {
    let entry = Entry {
        at: crate::now_secs(),
        event: event.to_owned(),
        version: version.map(str::to_owned),
        detail: detail.to_owned(),
    };
    log::info!("update {event} {}: {detail}", version.unwrap_or("-"));
    if let Err(e) = append(dir, &entry) {
        log::warn!("update history not written: {e}");
    }
}

fn append(dir: &UpdateDir, entry: &Entry) -> io::Result<()> {
    let mut lines: Vec<String> = match fs::read_to_string(dir.history_file()) {
        Ok(text) => text.lines().map(str::to_owned).collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    lines.push(serde_json::to_string(entry).map_err(io::Error::other)?);
    let skip = lines.len().saturating_sub(MAX_ENTRIES);
    let mut text = lines[skip..].join("\n");
    text.push('\n');
    write_atomic(&dir.history_file(), text.as_bytes())
}

/// The entries, oldest first. Lines that do not parse are skipped.
pub fn read(dir: &UpdateDir) -> Vec<Entry> {
    fs::read_to_string(dir.history_file())
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_app_updates_history_records_an_update_s_life() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = UpdateDir::at(tmp.path());
        record(
            &dir,
            "check",
            Some("0.4.1"),
            "PhotoSquirrel 0.4.1 is available",
        );
        record(&dir, "download", Some("0.4.1"), "verified");
        record(&dir, "install", Some("0.4.1"), "installed");
        record(&dir, "trial", Some("0.4.1"), "started");
        let events: Vec<String> = read(&dir).into_iter().map(|e| e.event).collect();
        assert_eq!(events, ["check", "download", "install", "trial"]);
        assert!(
            read(&dir)
                .iter()
                .all(|e| e.version.as_deref() == Some("0.4.1") && e.at > 0)
        );
    }

    #[test]
    fn only_the_last_200_entries_are_kept() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = UpdateDir::at(tmp.path());
        for i in 0..205 {
            record(&dir, "check", None, &i.to_string());
        }
        let entries = read(&dir);
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].detail, "5");
        assert_eq!(entries[MAX_ENTRIES - 1].detail, "204");
    }
}
