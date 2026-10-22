-- Migration 14 (quarantine-cleanup; ADR-0024, ADR-0034, ADR-0061): cleanup runs and their moves, the catalog
-- half of the write-ahead journal (the other half is `<source root>/.photosquirrel-quarantine/<run>/journal.jsonl`).
--
-- Stable codes (never reuse a value):
--   cleanup_run.state:        'planned', 'running', 'completed', 'undone'
--   cleanup_move.state:       0 planned, 1 moving (intent journaled, rename may or may not have happened),
--                             2 quarantined, 3 skipped (not moved; `note` says why), 4 restoring (undo intent),
--                             5 restored
--   cleanup_move.disposition: 0 exported, 1 covered (the disposition that made the file eligible)
--   cleanup_move.identity_kind: 0 full, 1 sampled (as file.identity_kind)
-- Paths are BLOBs (see ps_model::path); `rel_path` and `quarantine_rel_path` are relative to `root_path`.
--
-- No foreign keys to `file` or `source`: the record of what was moved must outlive RemoveSource, like the
-- export ledger. A file is quarantined while one of its moves is in state 2 or 4.

CREATE TABLE cleanup_run (
    id          INTEGER PRIMARY KEY,
    source_id   INTEGER NOT NULL,
    root_path   BLOB    NOT NULL,
    library_id  INTEGER NOT NULL REFERENCES export_library(id),
    folder      BLOB,
    uid         TEXT    NOT NULL,
    state       TEXT    NOT NULL CHECK (state IN ('planned', 'running', 'completed', 'undone')),
    created_at  INTEGER NOT NULL,
    started_at  INTEGER,
    ended_at    INTEGER
);

CREATE TABLE cleanup_move (
    run_id                INTEGER NOT NULL REFERENCES cleanup_run(id),
    file_id               INTEGER NOT NULL,
    rel_path              BLOB    NOT NULL,
    quarantine_rel_path   BLOB    NOT NULL,
    size                  INTEGER NOT NULL,
    mtime_ns              INTEGER NOT NULL,
    file_index            INTEGER,
    content_id            BLOB    NOT NULL,
    identity_kind         INTEGER NOT NULL CHECK (identity_kind IN (0, 1)),
    disposition           INTEGER NOT NULL CHECK (disposition IN (0, 1)),
    covering_library_path BLOB    NOT NULL,
    state                 INTEGER NOT NULL DEFAULT 0 CHECK (state BETWEEN 0 AND 5),
    note                  TEXT,
    at                    INTEGER,
    PRIMARY KEY (run_id, file_id)
) WITHOUT ROWID;

-- Dispositions: which files are quarantined now.
CREATE INDEX cleanup_move_by_state ON cleanup_move (state, file_id);
