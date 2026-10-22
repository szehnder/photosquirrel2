-- Migration 15 (quarantine-cleanup gate fixes; ADR-0063): tie each cleanup run to its source by more than the
-- source id, and find quarantined files by path.
--
-- `source.id` and `file.id` are plain INTEGER PRIMARY KEYs, which SQLite may hand out again after a removal.
-- A run records the `added_at` of the source it was planned on: a run whose source id now names a source
-- added at another time belongs to a removed source, and acts on the root stored with it. Existing runs take
-- the `added_at` of their source when that source was added before the run was planned (a source added
-- later reuses the id; its runs keep NULL and match no current source).
--
-- Quarantined files are matched to catalog rows by run source and relative path (and stat fingerprint), not
-- by file id alone: `cleanup_move_by_path` serves that lookup.

ALTER TABLE cleanup_run ADD COLUMN source_added_at INTEGER;

UPDATE cleanup_run
SET source_added_at = (
    SELECT s.added_at FROM source s
    WHERE s.id = cleanup_run.source_id AND s.added_at <= cleanup_run.created_at
);

CREATE INDEX cleanup_move_by_path ON cleanup_move (rel_path, state);
