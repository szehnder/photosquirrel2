-- Migration 16 (cleanup-selection; ADR-0068): a run can be `discarded` and a move `unselected` (code 6).
--
-- Stable codes (never reuse a value):
--   cleanup_run.state:  'planned', 'running', 'completed', 'undone', 'discarded'
--   cleanup_move.state: 0 planned, 1 moving, 2 quarantined, 3 skipped, 4 restoring, 5 restored,
--                       6 unselected (in the plan, left out by the user's selection; never moved as such)
--
-- SQLite cannot change a CHECK in place, so both tables are rebuilt (SQLite's table-rebuild procedure) inside
-- the migration's transaction, keeping every row, both indexes and the foreign key. The old tables are renamed
-- aside first and dropped child-first after the copy: no step ever leaves a row whose parent is missing, so no
-- foreign key is violated at any point (`defer_foreign_keys` is set anyway, as the procedure recommends).
-- No cleanup row is ever deleted (ADR-0068).

PRAGMA defer_foreign_keys = ON;

DROP INDEX cleanup_move_by_state;
DROP INDEX cleanup_move_by_path;

ALTER TABLE cleanup_move RENAME TO cleanup_move_old;
ALTER TABLE cleanup_run RENAME TO cleanup_run_old;

CREATE TABLE cleanup_run (
    id              INTEGER PRIMARY KEY,
    source_id       INTEGER NOT NULL,
    root_path       BLOB    NOT NULL,
    library_id      INTEGER NOT NULL REFERENCES export_library(id),
    folder          BLOB,
    uid             TEXT    NOT NULL,
    state           TEXT    NOT NULL
                    CHECK (state IN ('planned', 'running', 'completed', 'undone', 'discarded')),
    created_at      INTEGER NOT NULL,
    started_at      INTEGER,
    ended_at        INTEGER,
    source_added_at INTEGER
);

INSERT INTO cleanup_run (id, source_id, root_path, library_id, folder, uid, state, created_at, started_at,
                         ended_at, source_added_at)
SELECT id, source_id, root_path, library_id, folder, uid, state, created_at, started_at, ended_at,
       source_added_at
FROM cleanup_run_old;

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
    state                 INTEGER NOT NULL DEFAULT 0 CHECK (state BETWEEN 0 AND 6),
    note                  TEXT,
    at                    INTEGER,
    PRIMARY KEY (run_id, file_id)
) WITHOUT ROWID;

INSERT INTO cleanup_move (run_id, file_id, rel_path, quarantine_rel_path, size, mtime_ns, file_index, content_id,
                          identity_kind, disposition, covering_library_path, state, note, at)
SELECT run_id, file_id, rel_path, quarantine_rel_path, size, mtime_ns, file_index, content_id, identity_kind,
       disposition, covering_library_path, state, note, at
FROM cleanup_move_old;

DROP TABLE cleanup_move_old;
DROP TABLE cleanup_run_old;

CREATE INDEX cleanup_move_by_state ON cleanup_move (state, file_id);
CREATE INDEX cleanup_move_by_path ON cleanup_move (rel_path, state);
