-- Migration 7 (gallery change; ADR-0015, ADR-0018, ADR-0023): user include/exclude decisions and the
-- camera-model filter index.
--
-- `user_inclusion` records what the user decided about a file. The classifier's result stays in `file.class`
-- and `file.reasons`; the user's decision wins in gallery queries and survives rescans, because rescans
-- never touch this table (ADR-0015: nothing silently disappears; ADR-0021: never an AI decision).
--   decision: 'include' (show in the Library although the classifier excluded it) or 'exclude'.
-- `file_id` has no foreign key: RemoveSource deletes file rows, and a stale decision for a vanished id is
-- harmless (ids are not reused while larger ids exist; the gallery joins on present files only).

CREATE TABLE user_inclusion (
    file_id  INTEGER PRIMARY KEY,
    decision TEXT    NOT NULL CHECK (decision IN ('include', 'exclude')),
    reason   TEXT,
    at       INTEGER NOT NULL
);

-- Camera filter + capture-date sort without table lookups (gallery filter bar).
CREATE INDEX file_by_camera ON file (camera_model, capture_ts, id, kflags, source_id)
    WHERE camera_model IS NOT NULL;
