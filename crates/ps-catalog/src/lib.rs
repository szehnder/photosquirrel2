//! SQLite catalog with a single batching writer (ADR-0005).
//!
//! - [`Catalog::open`] creates or opens `catalog.db` in the data directory ([`data_dir`]), applies the
//!   embedded numbered migrations ([`db::CATALOG_MIGRATIONS`], `PRAGMA user_version`) and refuses catalogs
//!   written by a newer build without touching them.
//! - All writes go through [`Writer`]: one thread, typed [`Command`]s, batches of at most 2,000 rows or
//!   100 ms, flush barriers and error reporting.
//! - Queries go through [`Readers`], a pool of read-only connections: the gallery's complete ordered ID list
//!   with 16-byte sort keys, keyset detail pages, per-source keyset pages for the rescan diff.
//! - [`thumbs::ThumbStore`] is the content-addressed thumbnail store `thumbs.db` beside the catalog
//!   (ADR-0016).
//!
//! Nothing here ever writes inside a source tree: the only files created are in the data directory.

pub mod db;
mod error;
mod gallery;
pub mod groups;
mod model;
mod read;
pub mod stores;
pub mod thumbs;
mod writer;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use error::{CatalogError, Result, WriteError, WriteErrors};
pub use gallery::{
    UserDecision, UserDecisionRecord, excluded_classes, is_library, library_classes,
};
pub use groups::{
    ConstraintRows, GroupChanges, GroupOrder, GroupRow, GroupSummary, MemberEdge, MemberRow,
    SuggestionRow,
};
pub use model::{
    AnalysisResult, FileErrorRecord, FileFailure, FileKey, FileRecord, FileRow, NewSource,
    ProbeResult, Results, RunRecord, SourceRecord,
};
pub use read::{PAGE_SIZE, Readers};
pub use stores::{AiCounts, AiItemState, AiQueueRow, AiResultRow, AiRunState, LibraryRecord};
pub use thumbs::{THUMBS_FILE, ThumbStore};
pub use writer::{Command, Writer, WriterConfig, WriterStats};

/// File name of the catalog inside the data directory.
pub const CATALOG_FILE: &str = "catalog.db";

/// Environment variable overriding the data directory (tests, CLI, app).
pub const DATA_DIR_ENV: &str = "PHOTOSQUIRREL_DATA_DIR";

/// Read-only connections in the pool.
pub const READER_POOL_SIZE: usize = 4;

/// The data directory: `PHOTOSQUIRREL_DATA_DIR` when set and non-empty, else the per-user application data
/// directory (`directories::ProjectDirs("app", "PhotoSquirrel", "PhotoSquirrel")`).
pub fn data_dir() -> Result<PathBuf> {
    data_dir_from(std::env::var_os(DATA_DIR_ENV).map(PathBuf::from))
}

/// [`data_dir`] with an explicit override (e.g. the CLI's `--data-dir`).
pub fn data_dir_from(override_dir: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(d) = override_dir.filter(|d| !d.as_os_str().is_empty()) {
        return Ok(d);
    }
    directories::ProjectDirs::from("app", "PhotoSquirrel", "PhotoSquirrel")
        .map(|p| p.data_dir().to_path_buf())
        .ok_or(CatalogError::NoDataDir)
}

/// Options for [`Catalog::open_with`].
#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    pub writer: WriterConfig,
    pub readers: usize,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            writer: WriterConfig::default(),
            readers: READER_POOL_SIZE,
        }
    }
}

/// File next to the catalog that the owning process holds an exclusive OS lock on (ADR-0005: one writer
/// process per catalog).
pub const LOCK_FILE: &str = "catalog.db.lock";

/// An open catalog: the writer handle and the reader pool.
///
/// The process that opens a catalog with [`Catalog::open`] owns it: it holds an exclusive OS file lock on
/// [`LOCK_FILE`] until the catalog is dropped, and a second process (the app or a CLI command) gets
/// [`CatalogError::Locked`]. [`Catalog::open_observer`] opens readers only, without the lock, for read-only
/// commands while another process owns the catalog.
#[derive(Debug)]
pub struct Catalog {
    path: PathBuf,
    // Declared before `readers` so the writer commits and stops before the readers close.
    writer: Writer,
    readers: Arc<Readers>,
    // Released last, after the writer has committed and stopped.
    _lock: Option<std::fs::File>,
}

/// Take the exclusive owner lock for the catalog at `path`.
fn lock_catalog(path: &Path) -> Result<std::fs::File> {
    let lock_path = path.with_file_name(LOCK_FILE);
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(db::io_err(&lock_path))?;
    // A previous owner that just crashed or was killed may still be exiting (its descriptors close a moment
    // after the kill): retry briefly before reporting the catalog as owned by another process.
    let deadline = std::time::Instant::now() + LOCK_RETRY;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(f),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(CatalogError::Locked {
                    path: path.parent().unwrap_or(path).to_path_buf(),
                });
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(CatalogError::Io {
                    path: lock_path,
                    source: e,
                });
            }
        }
    }
}

/// How long opening waits for the owner lock of a catalog whose owner is going away.
pub const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(500);

impl Catalog {
    /// Open (creating and migrating) `catalog.db` in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Catalog> {
        Catalog::open_with(data_dir, OpenOptions::default())
    }

    /// Open the catalog in the default [`data_dir`].
    pub fn open_default() -> Result<Catalog> {
        Catalog::open(&data_dir()?)
    }

    pub fn open_with(data_dir: &Path, opts: OpenOptions) -> Result<Catalog> {
        std::fs::create_dir_all(data_dir).map_err(db::io_err(data_dir))?;
        Catalog::open_file(&db::catalog_path(data_dir), opts)
    }

    /// Open a catalog at an explicit file path, as its owner (exclusive lock; [`CatalogError::Locked`] while
    /// another process owns it).
    pub fn open_file(path: &Path, opts: OpenOptions) -> Result<Catalog> {
        // Refuse before anything opens the file for writing, so a newer catalog stays byte-identical.
        db::refuse_newer(path, db::CATALOG_MIGRATIONS)?;
        let lock = lock_catalog(path)?;
        let mut conn = db::open_writer(path)?;
        db::migrate(&mut conn, path, db::CATALOG_MIGRATIONS, None)?;
        let readers = Arc::new(Readers::open(path, opts.readers)?);
        let writer = Writer::spawn(conn, opts.writer)?;
        Ok(Catalog {
            path: path.to_path_buf(),
            writer,
            readers,
            _lock: Some(lock),
        })
    }

    /// Open the catalog in `data_dir` as an observer: readers only, no lock, every write fails with
    /// [`CatalogError::ReadOnly`]. For read-only commands while another process owns the catalog.
    pub fn open_observer(data_dir: &Path, readers: usize) -> Result<Catalog> {
        let path = db::catalog_path(data_dir);
        db::refuse_newer(&path, db::CATALOG_MIGRATIONS)?;
        Ok(Catalog {
            readers: Arc::new(Readers::open(&path, readers)?),
            path,
            writer: Writer::read_only(),
            _lock: None,
        })
    }

    /// Open as the owner, or as an observer when another process owns the catalog.
    pub fn open_or_observe(data_dir: &Path, opts: OpenOptions) -> Result<Catalog> {
        match Catalog::open_with(data_dir, opts) {
            Err(CatalogError::Locked { .. }) => Catalog::open_observer(data_dir, opts.readers),
            other => other,
        }
    }

    /// Open for read-only use without ever holding the owner lock while someone else could need it: an
    /// observer when the catalog exists at this build's schema version, else (a new or older catalog that
    /// must be created or migrated first) [`Catalog::open_or_observe`]. Read-only commands use this so that
    /// they never make a starting scan or app fail with [`CatalogError::Locked`].
    pub fn open_reader(data_dir: &Path, opts: OpenOptions) -> Result<Catalog> {
        let path = db::catalog_path(data_dir);
        if db::stored_version(&path)? == Some(latest_schema_version())
            && let Ok(c) = Catalog::open_observer(data_dir, opts.readers)
        {
            return Ok(c);
        }
        Catalog::open_or_observe(data_dir, opts)
    }

    /// Opened by [`Catalog::open_observer`]: writes fail.
    pub fn is_observer(&self) -> bool {
        self.writer.is_read_only()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The writer handle (clone it into producer threads).
    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    /// The reader pool.
    pub fn read(&self) -> &Readers {
        &self.readers
    }

    /// A shareable handle to the reader pool, for background query threads.
    pub fn readers(&self) -> Arc<Readers> {
        self.readers.clone()
    }

    /// Flush, stop the writer thread and checkpoint the WAL. Dropping the catalog does the same, but this
    /// reports errors.
    pub fn close(self) -> Result<()> {
        self.writer.shutdown()
    }
}

/// The latest catalog schema version this build supports.
pub fn latest_schema_version() -> u32 {
    db::latest(db::CATALOG_MIGRATIONS)
}
