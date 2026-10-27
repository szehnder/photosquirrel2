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
    LEDGER_MTIME_MIGRATION,
    LEDGER_SIDECARS_MIGRATION,
    AI_RESULT_SEQ_MIGRATION,
    LIBRARY_VERIFIED_MIGRATION,
    CLASS_FACTS_MIGRATION,
    CLEANUP_MIGRATION,
    CLEANUP_SOURCE_MIGRATION,
    CLEANUP_SELECTION_MIGRATION,
    CAPTURE_DATES_MIGRATION,
    RUN_REFRESH_MIGRATION,
    DECODER_MISSING_MIGRATION,
    DECODE_FAILURE_MIGRATION,
    EXCLUSION_CATEGORY_MIGRATION,
    EXCLUSION_CATEGORY_FIX_MIGRATION,
    FRAME_IDENTITY_MIGRATION,
    PREVIEW_GEN_MIGRATION,
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

/// Migration 9 (ADR-0044: the ledger records each library copy's modification time).
pub const LEDGER_MTIME_MIGRATION: Migration = Migration {
    version: 9,
    name: "ledger_mtime",
    sql: include_str!("../migrations/0009_ledger_mtime.sql"),
};

/// Migration 10 (ADR-0045: sidecars have ledger rows, `export_ledger.kind`).
pub const LEDGER_SIDECARS_MIGRATION: Migration = Migration {
    version: 10,
    name: "ledger_sidecars",
    sql: include_str!("../migrations/0010_ledger_sidecars.sql"),
};

/// Migration 11 (`ai_result.seq`: a write sequence for incremental readers).
pub const AI_RESULT_SEQ_MIGRATION: Migration = Migration {
    version: 11,
    name: "ai_result_seq",
    sql: include_str!("../migrations/0011_ai_result_seq.sql"),
};

/// Migration 12 (`export_library.verified_at`: when the library's copies were last re-hashed).
pub const LIBRARY_VERIFIED_MIGRATION: Migration = Migration {
    version: 12,
    name: "library_verified_at",
    sql: include_str!("../migrations/0012_library_verified_at.sql"),
};

/// Migration 13 (ADR-0051: `file.class_facts` and `file.classifier_hash`, for re-classification without
/// reading files).
pub const CLASS_FACTS_MIGRATION: Migration = Migration {
    version: 13,
    name: "class_facts",
    sql: include_str!("../migrations/0013_class_facts.sql"),
};

/// Migration 14 (quarantine-cleanup, ADR-0061: `cleanup_run`, `cleanup_move`).
pub const CLEANUP_MIGRATION: Migration = Migration {
    version: 14,
    name: "cleanup",
    sql: include_str!("../migrations/0014_cleanup.sql"),
};

/// Migration 15 (ADR-0063: `cleanup_run.source_added_at`, quarantined files found by path).
pub const CLEANUP_SOURCE_MIGRATION: Migration = Migration {
    version: 15,
    name: "cleanup_source_identity",
    sql: include_str!("../migrations/0015_cleanup_source_identity.sql"),
};

/// Migration 16 (ADR-0068: run state `discarded`, move state 6 `unselected`; both cleanup tables rebuilt).
pub const CLEANUP_SELECTION_MIGRATION: Migration = Migration {
    version: 16,
    name: "cleanup_selection",
    sql: include_str!("../migrations/0016_cleanup_selection.sql"),
};

/// Migration 17 (ADR-0069, ADR-0070, ADR-0071: `file.capture_precision`, `file.date_version`; rows whose date
/// cannot change are marked current).
pub const CAPTURE_DATES_MIGRATION: Migration = Migration {
    version: 17,
    name: "capture_dates",
    sql: include_str!("../migrations/0017_capture_dates.sql"),
};

/// Migration 18 (ADR-0082: `scan_run.refresh_analysis`, so a resumed refresh run still refreshes).
pub const RUN_REFRESH_MIGRATION: Migration = Migration {
    version: 18,
    name: "run_refresh",
    sql: include_str!("../migrations/0018_run_refresh.sql"),
};
/// Migration 19 (ADR-0091: `file.decoder_missing` and its partial index; undecoded HEIC and AVIF rows of older
/// catalogs marked).
pub const DECODER_MISSING_MIGRATION: Migration = Migration {
    version: 19,
    name: "decoder_missing",
    sql: include_str!("../migrations/0019_decoder_missing.sql"),
};
/// Migration 20 (ADR-0098: `file.decode_failure` and its partial index; HEIC and AVIF decode failures retried on
/// rescans while transient or after a decoder upgrade).
pub const DECODE_FAILURE_MIGRATION: Migration = Migration {
    version: 20,
    name: "decode_failure",
    sql: include_str!("../migrations/0020_decode_failure.sql"),
};
/// Migration 21 (ADR-0102: `file.exclusion`, `user_inclusion.origin` and `ai_kind`, the partial index
/// `file_exclusion`; existing rows backfilled from their class, reasons and decision reason text).
pub const EXCLUSION_CATEGORY_MIGRATION: Migration = Migration {
    version: 21,
    name: "exclusion_category",
    sql: include_str!("../migrations/0021_exclusion_category.sql"),
};
/// Migration 22 (ADR-0104: `file.exclusion` recomputed with the amended rule, a damaged photo or a file that could
/// not be read is `unreadable`, not `unsupported`; an AI decision without its kind refused).
pub const EXCLUSION_CATEGORY_FIX_MIGRATION: Migration = Migration {
    version: 22,
    name: "exclusion_category_fix",
    sql: include_str!("../migrations/0022_exclusion_category_fix.sql"),
};
/// Migration 23 (ADR-0105: `file.frame_id` and `file.meta_version`; every row marked current except RAW-format rows
/// and Canon camera files of the same capture second as one, whose header a rescan re-reads once).
pub const FRAME_IDENTITY_MIGRATION: Migration = Migration {
    version: 23,
    name: "frame_identity",
    sql: include_str!("../migrations/0023_frame_identity.sql"),
};
/// Migration 24 (ADR-0109: `file.preview_gen`, `file.preview_reason` and the partial index `file_preview`;
/// `scan_run.kind` and `scan_run.fill_scope`; placeholder stills of older catalogs marked at generation 0).
pub const PREVIEW_GEN_MIGRATION: Migration = Migration {
    version: 24,
    name: "preview_gen",
    sql: include_str!("../migrations/0024_preview_gen.sql"),
};
/// SQL that puts a catalog back where migration 23 leaves one written before frame identity (ADR-0105): no frame
/// ids, every row current except those the migration keeps for the re-read. It reuses the migration's own
/// marking statement, so tests that rebuild an old catalog cannot drift from it.
#[doc(hidden)]
pub fn frame_identity_reset_sql() -> String {
    let sql = FRAME_IDENTITY_MIGRATION.sql;
    let at = sql
        .find("UPDATE file SET meta_version = 1")
        .unwrap_or(sql.len());
    format!(
        "UPDATE file SET meta_version = 0, frame_id = NULL;\n{}",
        &sql[at..]
    )
}

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
