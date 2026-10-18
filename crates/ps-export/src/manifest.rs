//! `<library>/.photosquirrel/manifest.jsonl` (ADR-0022): one JSON line per exported (or adopted) file, appended
//! and synced after the file is in place. Never rewritten. A crash between the append and the ledger update
//! can repeat a line on resume (the file is then adopted); readers key records by `library_path`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};

use ps_model::{ExportProfile, RelPath};
use serde::{Deserialize, Serialize};

use crate::LIBRARY_META_DIR;
use crate::library::LibraryRoot;

/// Manifest file name inside [`LIBRARY_META_DIR`].
pub const MANIFEST_FILE: &str = "manifest.jsonl";

/// One manifest line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRecord {
    /// Library-relative path, `/`-separated.
    pub library_path: String,
    /// BLAKE3 of every byte (hex).
    pub full_hash: String,
    /// Catalog identity (hex) and its kind (`full` / `sampled`).
    pub content_id: String,
    pub identity_kind: String,
    pub size: u64,
    pub file_id: i64,
    /// The file the bytes were copied from.
    pub source_path: String,
    /// Source paths of every copy of this content (the duplicates), the copied one included.
    pub source_paths: Vec<String>,
    pub keeper_reasons: Vec<String>,
    pub profile: ExportProfile,
    pub at_ms: i64,
    /// An identical file was already at the path; nothing was copied.
    pub adopted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar: Option<String>,
}

/// An open manifest for appending.
#[derive(Debug)]
pub struct Manifest {
    file: File,
}

impl Manifest {
    pub fn rel_path() -> RelPath {
        RelPath::from_bytes(format!("{LIBRARY_META_DIR}/{MANIFEST_FILE}").into_bytes())
            .unwrap_or_else(|| unreachable!("constant path is valid"))
    }

    /// Open (creating) the manifest. Appends only; existing lines are never touched.
    pub fn open(lib: &LibraryRoot) -> io::Result<Manifest> {
        let path = lib.prepare(&Self::rel_path())?;
        if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_file()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not a regular file", path.display()),
            ));
        }
        let file = OpenOptions::new().append(true).create(true).open(&path)?;
        Ok(Manifest { file })
    }

    /// Append one record and sync it.
    pub fn append(&mut self, rec: &ManifestRecord) -> io::Result<()> {
        let mut line = serde_json::to_vec(rec).map_err(io::Error::other)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()
    }
}

/// Read every record of a library's manifest (tests, reports).
pub fn read_manifest(lib: &std::path::Path) -> io::Result<Vec<ManifestRecord>> {
    let path = Manifest::rel_path().under(lib);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(io::Error::other))
        .collect()
}
