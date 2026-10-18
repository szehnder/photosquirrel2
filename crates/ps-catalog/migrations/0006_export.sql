-- Migration 6 (export change; ADR-0022, ADR-0023): export libraries and the export ledger.
--
-- Stable integer codes (never reuse a value):
--   export_ledger.state:         0 in_progress, 1 done, 2 failed (ps_model::LedgerState::code)
--   export_ledger.identity_kind: 0 full, 1 sampled (as file.identity_kind)
-- Paths are BLOBs (see ps_model::path). Times are milliseconds since the Unix epoch.
--
-- `file_id` deliberately has no foreign key: a ledger row records what was copied out and must outlive the
-- removal of its source's catalog records (RemoveSource), otherwise the library could no longer be
-- reconciled. Rows are keyed by (library, library path): one row per file in the library.

CREATE TABLE export_library (
    id          INTEGER PRIMARY KEY,
    root_path   BLOB    NOT NULL UNIQUE,
    volume_uuid TEXT,
    created_at  INTEGER NOT NULL
);

CREATE TABLE export_ledger (
    id               INTEGER PRIMARY KEY,
    library_id       INTEGER NOT NULL REFERENCES export_library(id),
    content_id       BLOB    NOT NULL,             -- 32-byte catalog identity (ADR-0011)
    identity_kind    INTEGER NOT NULL CHECK (identity_kind IN (0, 1)),
    full_hash        BLOB,                         -- 32-byte BLAKE3 of every byte, computed while copying
    file_id          INTEGER NOT NULL,
    library_rel_path BLOB    NOT NULL,
    size             INTEGER NOT NULL,
    state            INTEGER NOT NULL CHECK (state IN (0, 1, 2)),
    profile          TEXT    NOT NULL CHECK (profile IN ('apple', 'google', 'lightroom')),
    at               INTEGER NOT NULL,
    UNIQUE (library_id, library_rel_path)
);

-- "Is this content already exported to this library?" (planner) and dispositions by content.
CREATE INDEX export_ledger_by_content ON export_ledger (library_id, content_id);
-- Dispositions and the inspector by file.
CREATE INDEX export_ledger_by_file ON export_ledger (file_id);
