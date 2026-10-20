//! Connections, pragmas and numbered migrations (`PRAGMA user_version`, ADR-0005).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::error::{CatalogError, Result};

/// One embedded migration: its number (the schema version after it), a name and the SQL.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Catalog migrations, in order. Numbers are contiguous from 1.
pub const CATALOG_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "reserved",
        sql: include_str!("../migrations/0002_reserved.sql"),
    },
    Migration {
        version: 3,
        name: "scanning",
        sql: include_str!("../migrations/0003_scanning.sql"),
    },
    GROUPS_MIGRATION,
    AI_MIGRATION,
    EXPORT_MIGRATION,
    GALLERY_MIGRATION,
    LEDGER_SUSPECT_MIGRATION,
];

/// Migration 4 (duplicate-groups change: `dup_group`, `dup_member`, `dup_suggestion`, `keeper_override`,
/// `dup_separation`, `dup_merge`).
pub const GROUPS_MIGRATION: Migration = Migration {
    version: 4,
    name: "groups",
    sql: include_str!("../migrations/0004_groups.sql"),
};

/// Migration 5 (ai-insights change: `ai_queue`, `ai_result`, `ai_correction`, `ai_suggestion`, `ai_run`).
pub const AI_MIGRATION: Migration = Migration {
    version: 5,
    name: "ai_insights",
    sql: include_str!("../migrations/0005_ai_insights.sql"),
};

/// Migration 6 (export change: `export_library`, `export_ledger`).
pub const EXPORT_MIGRATION: Migration = Migration {
    version: 6,
    name: "export",
    sql: include_str!("../migrations/0006_export.sql"),
};

/// Migration 7 (gallery change: `user_inclusion`, camera-filter index). The gallery queries still detect a
/// missing table ([`crate::Readers::has_user_decisions`]) for catalogs opened read-only at an older version.
pub const GALLERY_MIGRATION: Migration = Migration {
    version: 7,
    name: "gallery",
    sql: include_str!("../migrations/0007_gallery.sql"),
};

/// Migration 8 (ADR-0040: export ledger state 3 `suspect`).
pub const LEDGER_SUSPECT_MIGRATION: Migration = Migration {
    version: 8,
    name: "ledger_suspect",
    sql: include_str!("../migrations/0008_ledger_suspect.sql"),
};

/// `thumbs.db` migrations.
pub const THUMB_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "thumbs",
    sql: include_str!("../migrations/thumbs/0001_thumbs.sql"),
}];

/// The schema version this build creates and understands.
pub fn latest(migrations: &[Migration]) -> u32 {
    migrations.last().map_or(0, |m| m.version)
}

pub(crate) fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> CatalogError + '_ {
    move |source| CatalogError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Refuse a database written by a newer build, without modifying it: the version is read through a
/// read-only connection before anything opens the file for writing.
pub(crate) fn refuse_newer(path: &Path, migrations: &[Migration]) -> Result<u32> {
    if !path.exists() {
        return Ok(0);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let found: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let supported = latest(migrations);
    if found > supported {
        return Err(CatalogError::NewerSchema {
            path: path.to_path_buf(),
            found,
            supported,
        });
    }
    Ok(found)
}

/// The schema version of an existing catalog file, read through a read-only connection (`None` when the file
/// does not exist). Never creates or migrates anything (doctor, diagnostics).
pub fn stored_version(path: &Path) -> Result<Option<u32>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(Some(conn.pragma_query_value(
        None,
        "user_version",
        |r| r.get(0),
    )?))
}

/// Open the read-write connection with the writer pragmas (WAL, `synchronous=NORMAL`, ...).
pub(crate) fn open_writer(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let mode: String = conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(CatalogError::Corrupt(format!(
            "could not enable WAL on {} (journal_mode={mode})",
            path.display()
        )));
    }
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA temp_store = MEMORY;
         PRAGMA mmap_size = 268435456;
         PRAGMA cache_size = -131072;
         PRAGMA wal_autocheckpoint = 16384;",
    )?;
    conn.set_prepared_statement_cache_capacity(64);
    Ok(conn)
}

/// Open a read-only query connection (`SQLITE_OPEN_READ_ONLY`, `query_only=1`).
pub(crate) fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        // Readers do not memory-map the file and keep a 2 MiB page cache each. Mapped pages count in the
        // process's RSS once per reader that touched them: with a 256 MiB map per reader the idle app showed
        // ~440 MB of file-backed RSS on a 1M-file catalog (692 MB in all, budget 250 MB, ADR-0026). Without the
        // map the same reads come from the OS page cache, which the kernel shares and reclaims; the 1M ID list
        // (126 ms vs 132 ms), the first gallery frame (median 495 ms vs 548 ms) and the no-change rescan held.
        "PRAGMA query_only = 1;
         PRAGMA temp_store = MEMORY;
         PRAGMA mmap_size = 0;
         PRAGMA cache_size = -2048;",
    )?;
    conn.set_prepared_statement_cache_capacity(32);
    rusqlite::vtab::array::load_module(&conn)?;
    Ok(conn)
}

/// Apply every migration newer than the database's version, each in its own transaction together with its
/// `user_version` bump. Stops after `up_to` when given (test fixtures of old versions).
pub(crate) fn migrate(
    conn: &mut Connection,
    path: &Path,
    migrations: &[Migration],
    up_to: Option<u32>,
) -> Result<u32> {
    let mut version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let supported = latest(migrations);
    if version > supported {
        return Err(CatalogError::NewerSchema {
            path: path.to_path_buf(),
            found: version,
            supported,
        });
    }
    for m in migrations {
        if m.version <= version || up_to.is_some_and(|u| m.version > u) {
            continue;
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(m.sql)?;
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
        log::info!(
            "{}: migrated to version {} ({})",
            path.display(),
            m.version,
            m.name
        );
        version = m.version;
    }
    Ok(version)
}

/// Create (or upgrade) a database at `path` with migrations up to `version` only. For tests and fixtures
/// that need an old schema.
#[doc(hidden)]
pub fn create_catalog_at_version(path: &Path, version: u32) -> Result<()> {
    let mut conn = open_writer(path)?;
    migrate(&mut conn, path, CATALOG_MIGRATIONS, Some(version))?;
    Ok(())
}

/// The catalog file inside a data directory.
pub fn catalog_path(data_dir: &Path) -> PathBuf {
    data_dir.join(crate::CATALOG_FILE)
}
