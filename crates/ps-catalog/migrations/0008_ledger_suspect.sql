-- Migration 8 (M1 fixes; ADR-0040): the export ledger gains state 3 `suspect` (the file at the library path
-- no longer matched the verified copy when it was re-hashed after the rename; kept, never deleted).
--
-- Stable integer codes (never reuse a value):
--   export_ledger.state: 0 in_progress, 1 done, 2 failed, 3 suspect (ps_model::LedgerState::code)
--
-- SQLite cannot change a CHECK constraint in place: rebuild the table with the same columns and indexes.

CREATE TABLE export_ledger_v8 (
    id               INTEGER PRIMARY KEY,
    library_id       INTEGER NOT NULL REFERENCES export_library(id),
    content_id       BLOB    NOT NULL,
    identity_kind    INTEGER NOT NULL CHECK (identity_kind IN (0, 1)),
    full_hash        BLOB,
    file_id          INTEGER NOT NULL,
    library_rel_path BLOB    NOT NULL,
    size             INTEGER NOT NULL,
    state            INTEGER NOT NULL CHECK (state IN (0, 1, 2, 3)),
    profile          TEXT    NOT NULL CHECK (profile IN ('apple', 'google', 'lightroom')),
    at               INTEGER NOT NULL,
    UNIQUE (library_id, library_rel_path)
);

INSERT INTO export_ledger_v8
    (id, library_id, content_id, identity_kind, full_hash, file_id, library_rel_path, size, state, profile, at)
SELECT id, library_id, content_id, identity_kind, full_hash, file_id, library_rel_path, size, state, profile, at
FROM export_ledger;

DROP TABLE export_ledger;
ALTER TABLE export_ledger_v8 RENAME TO export_ledger;

CREATE INDEX export_ledger_by_content ON export_ledger (library_id, content_id);
CREATE INDEX export_ledger_by_file ON export_ledger (file_id);
