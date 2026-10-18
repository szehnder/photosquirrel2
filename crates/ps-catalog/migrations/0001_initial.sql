-- Catalog schema v1 (catalog change; ADR-0005/0006/0007/0008/0018).
-- Applied in one transaction together with `PRAGMA user_version = 1`. Never edit an applied migration:
-- add a new numbered file instead.
--
-- Stable integer codes (never reuse a value):
--   stage:         0 discovered, 1 probed, 2 analyzed, 3 thumbed, 4 skipped, 5 failed
--   class:         0 photo, 1 likely_photo, 2 thumbnail_cache, 3 graphic, 4 junk (NULL = not classified)
--   media_kind:    0 image, 1 video
--   identity_kind: 0 full, 1 sampled
-- Paths are BLOBs (raw bytes on Unix, WTF-8 on Windows, '/' separators; see ps_model::path).
-- Times are milliseconds since the Unix epoch unless the column says otherwise.

CREATE TABLE source (
    id           INTEGER PRIMARY KEY,
    root_path    BLOB    NOT NULL,
    volume_uuid  TEXT,
    device_hint  TEXT,
    total_bytes  INTEGER,
    -- BLAKE3 hashes of up to 64 sampled files (ADR-0006), concatenated.
    fingerprint  BLOB,
    status       TEXT    NOT NULL DEFAULT 'online' CHECK (status IN ('online', 'offline')),
    added_at     INTEGER NOT NULL,
    -- Overlap folding (ADR-0006): the outer source this one was folded into.
    folded_into  INTEGER REFERENCES source(id)
);

CREATE TABLE scan_run (
    id          INTEGER PRIMARY KEY,
    status      TEXT    NOT NULL
                CHECK (status IN ('running', 'paused', 'cancelled', 'completed', 'interrupted')),
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    sources     TEXT    NOT NULL DEFAULT '[]',   -- JSON array of source ids
    stats       TEXT    NOT NULL DEFAULT '{}'    -- JSON run summary
);

CREATE TABLE file (
    id                 INTEGER PRIMARY KEY,
    source_id          INTEGER NOT NULL REFERENCES source(id),
    rel_path           BLOB    NOT NULL,
    -- Last path component, for the name sort.
    name               BLOB    NOT NULL,
    -- Stat fingerprint (ADR-0007).
    size               INTEGER NOT NULL,
    mtime_ns           INTEGER NOT NULL,
    file_index         INTEGER,
    -- Pipeline stage (ADR-0008); written in the same statement as the stage's results.
    stage              INTEGER NOT NULL DEFAULT 0 CHECK (stage BETWEEN 0 AND 5),
    -- Probe results (stage >= probed).
    class              INTEGER CHECK (class BETWEEN 0 AND 4),
    reasons            TEXT,            -- JSON array
    format             TEXT,            -- ps_model::Format::storage_key
    media_kind         INTEGER CHECK (media_kind IN (0, 1)),
    width              INTEGER,
    height             INTEGER,
    -- Analysis results (stage >= analyzed).
    content_id         BLOB,            -- 32-byte BLAKE3 digest (ADR-0011)
    identity_kind      INTEGER CHECK (identity_kind IN (0, 1)),
    phash              INTEGER,         -- u64 bit pattern stored as i64
    dhash              INTEGER,
    hash_version       INTEGER,
    capture_ts         INTEGER,         -- wall-clock ms ("local time as if UTC"); NULL = no date at all
    capture_offset_min INTEGER,
    capture_provenance TEXT,
    capture_undated    INTEGER NOT NULL DEFAULT 0,
    camera_model       TEXT,
    has_gps            INTEGER NOT NULL DEFAULT 0,
    metadata           TEXT,            -- JSON ps_model::Metadata
    -- Bookkeeping.
    attempts           INTEGER NOT NULL DEFAULT 0,
    missing_since      INTEGER,
    -- Packed for the ID-list indexes: class code (7 = not classified) | media_kind << 3 | missing << 4.
    -- Maintained by the writer in the same statement that changes class, media_kind or missing_since (a
    -- generated column would not be usable from a covering index). One small column instead of three keeps
    -- the indexes narrow and makes the default filter ("not missing") a single comparison: kflags < 16.
    kflags             INTEGER NOT NULL DEFAULT 7,
    UNIQUE (source_id, rel_path)
);

-- Sort-first covering indexes for the gallery ID list (ADR-0018): each is ordered by the sort column then id
-- (ties break by id) and carries every column the filter and the 16-byte sort key read, so the 1M-item
-- query is a single index scan with no table lookups and no sort step.
CREATE INDEX file_by_capture ON file (capture_ts, id, kflags, source_id);
CREATE INDEX file_by_size    ON file (size, id, kflags, source_id);
CREATE INDEX file_by_name    ON file (name, id, kflags, source_id);
-- Exact-duplicate candidates and thumbnail sharing (ADR-0011, ADR-0013).
CREATE INDEX file_by_content ON file (content_id) WHERE content_id IS NOT NULL;
-- No (source_id, stage) index: resume and per-source stage counts walk the UNIQUE (source_id, rel_path)
-- index, which the rescan merge-join reads anyway; every extra index costs ~5 us per inserted row.

CREATE TABLE file_error (
    id       INTEGER PRIMARY KEY,
    file_id  INTEGER NOT NULL REFERENCES file(id),
    run_id   INTEGER REFERENCES scan_run(id),
    stage    INTEGER NOT NULL,          -- stage that was being attempted
    message  TEXT    NOT NULL,
    at       INTEGER NOT NULL
);
CREATE INDEX file_error_by_file ON file_error (file_id);
CREATE INDEX file_error_by_run ON file_error (run_id);
