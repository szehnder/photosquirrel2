//! The JSON journal inside a run's quarantine folder (ADR-0061): `journal.jsonl`, one object per line,
//! appended and fsynced, never rewritten. The first line is the run header; a run folder whose header has
//! another `uid` belongs to another catalog's run and is never used.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use ps_model::{CleanupMove, RelPath};
use serde::{Deserialize, Serialize};

/// File name of the journal in `<source root>/.photosquirrel-quarantine/<run id>/`.
pub const JOURNAL_FILE: &str = "journal.jsonl";

/// Journal format version.
pub const JOURNAL_VERSION: u32 = 1;

/// What a journal line records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalEvent {
    /// The header (first line).
    Run,
    /// About to rename the file into quarantine.
    Intent,
    /// In quarantine.
    Done,
    /// Not moved (the note says why).
    Skipped,
    /// About to rename the file back.
    UndoIntent,
    /// Back at its original path.
    Restored,
    /// Undo left the file in quarantine (its original path is occupied, or another reason in the note).
    Conflict,
}

/// One journal line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalLine {
    pub v: u32,
    pub event: JournalEvent,
    pub run: i64,
    /// Milliseconds since the Unix epoch.
    pub at: i64,
    /// Header only: the run's uid and source root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<i64>,
    /// Relative to the source root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine: Option<String>,
    /// Hex of the path bytes when they are not UTF-8 (`original` and `quarantine` are then lossy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_bytes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine_bytes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The catalog content identity (hex) and its kind (`full` or `sampled`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn path_fields(p: &RelPath) -> (String, Option<String>) {
    let s = p.display().into_owned();
    let raw = std::str::from_utf8(p.as_bytes())
        .is_err()
        .then(|| hex_bytes(p.as_bytes()));
    (s, raw)
}

impl JournalLine {
    /// A line about one move.
    pub fn of_move(run: i64, event: JournalEvent, m: &CleanupMove, note: Option<String>) -> Self {
        let (original, original_bytes) = path_fields(&m.rel_path);
        let (quarantine, quarantine_bytes) = path_fields(&m.quarantine_rel_path);
        JournalLine {
            v: JOURNAL_VERSION,
            event,
            run,
            at: crate::now_ms(),
            uid: None,
            source_root: None,
            file: Some(m.file.0),
            original: Some(original),
            quarantine: Some(quarantine),
            original_bytes,
            quarantine_bytes,
            size: Some(m.size),
            content: Some(m.content.to_hex()),
            content_kind: Some(
                match m.content.kind {
                    ps_model::ContentIdKind::Full => "full",
                    ps_model::ContentIdKind::Sampled => "sampled",
                }
                .into(),
            ),
            note,
        }
    }

    fn header(run: i64, uid: &str, source_root: &Path) -> Self {
        JournalLine {
            v: JOURNAL_VERSION,
            event: JournalEvent::Run,
            run,
            at: crate::now_ms(),
            uid: Some(uid.to_owned()),
            source_root: Some(source_root.display().to_string()),
            file: None,
            original: None,
            quarantine: None,
            original_bytes: None,
            quarantine_bytes: None,
            size: None,
            content: None,
            content_kind: None,
            note: None,
        }
    }
}

/// An open journal (appending).
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    file: File,
    run: i64,
}

/// The run folder belongs to another run (another catalog's run with the same number).
pub fn foreign_run(path: &Path, found: Option<&str>) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "{} belongs to another cleanup run (uid {}); refusing to use its folder",
            path.display(),
            found.unwrap_or("unknown")
        ),
    )
}

/// The header of an existing journal.
pub fn read_header(path: &Path) -> io::Result<Option<JournalLine>> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut first = String::new();
    BufReader::new(f).read_line(&mut first)?;
    Ok(serde_json::from_str::<JournalLine>(first.trim())
        .ok()
        .filter(|l| l.event == JournalEvent::Run))
}

/// Every line of a journal (unparseable lines, e.g. a torn last line after a crash, are skipped).
pub fn read_journal(path: &Path) -> io::Result<Vec<JournalLine>> {
    let f = File::open(path)?;
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        if let Ok(l) = serde_json::from_str::<JournalLine>(line?.trim()) {
            out.push(l);
        }
    }
    Ok(out)
}

impl Journal {
    /// Open (or create, with its header) the journal in `run_dir`, which must exist. An existing journal must
    /// carry `uid`.
    pub fn open(run_dir: &Path, run: i64, uid: &str, source_root: &Path) -> io::Result<Journal> {
        let path = run_dir.join(JOURNAL_FILE);
        match OpenOptions::new().append(true).create_new(true).open(&path) {
            Ok(file) => {
                let mut j = Journal { path, file, run };
                j.append(&JournalLine::header(run, uid, source_root))?;
                if let Some(dir) = j.path.parent() {
                    super::fsops::sync_dir_logged(dir);
                }
                Ok(j)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let m = fs::symlink_metadata(&path)?;
                if !m.file_type().is_file() {
                    return Err(foreign_run(&path, None));
                }
                let header = read_header(&path)?;
                let found = header.as_ref().and_then(|h| h.uid.as_deref());
                if found != Some(uid) || header.as_ref().is_some_and(|h| h.run != run) {
                    return Err(foreign_run(&path, found));
                }
                let file = OpenOptions::new().append(true).open(&path)?;
                Ok(Journal { path, file, run })
            }
            Err(e) => Err(e),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one line and fsync; durable when this returns.
    pub fn append(&mut self, line: &JournalLine) -> io::Result<()> {
        let mut s = serde_json::to_string(line).map_err(io::Error::other)?;
        s.push('\n');
        self.file.write_all(s.as_bytes())?;
        self.file.sync_data()
    }

    /// Append a line about one move.
    pub fn record(
        &mut self,
        event: JournalEvent,
        m: &CleanupMove,
        note: Option<String>,
    ) -> io::Result<()> {
        let line = JournalLine::of_move(self.run, event, m, note);
        self.append(&line)
    }
}
