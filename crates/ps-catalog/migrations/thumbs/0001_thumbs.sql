-- thumbs.db schema v1 (ADR-0016): one grid thumbnail per content identity, shared by duplicates.
-- A separate database from the catalog so thumbnail blobs never bloat catalog pages or its WAL.
--
-- kind: 'image' | 'poster_frame' | 'placeholder' (ps_model::ThumbKind::as_str)

CREATE TABLE thumb (
    content_id    BLOB    PRIMARY KEY CHECK (length(content_id) = 32),
    identity_kind INTEGER NOT NULL CHECK (identity_kind IN (0, 1)),  -- 0 full, 1 sampled
    kind          TEXT    NOT NULL CHECK (kind IN ('image', 'poster_frame', 'placeholder')),
    width         INTEGER NOT NULL,
    height        INTEGER NOT NULL,
    dominant_rgb  INTEGER NOT NULL,     -- 0xRRGGBB
    jpeg          BLOB,                 -- NULL for placeholders
    placeholder   TEXT,                 -- JSON ps_model::PlaceholderInfo
    created_at    INTEGER NOT NULL
);

-- Index of the 2048 px on-demand preview LRU (files `<content-id-hex>.jpg` in a cache directory).
CREATE TABLE preview (
    content_id  BLOB    PRIMARY KEY CHECK (length(content_id) = 32),
    bytes       INTEGER NOT NULL,
    last_used   INTEGER NOT NULL
);
CREATE INDEX preview_by_last_used ON preview (last_used);
