-- Migration 5 (ai-insights change; ADR-0020, ADR-0021). AI queue, results, user corrections and advisory
-- suggestions. Keyed by content identity (ADR-0011): byte-identical copies share one queue entry and one result.
-- Nothing here is read by grouping, keeper choice or export planning (ADR-0021: advisory only).
--
-- Stable codes (never reuse a value):
--   ai_queue.state:        0 pending, 1 in_flight, 2 done, 3 failed, 4 deferred (source offline)
--   ai_run.state:          0 paused, 1 running, 2 finished    (a 'running' row becomes 'paused' at start-up)
--   ai_suggestion.status:  0 open, 1 accepted, 2 rejected
-- content_id is the 32-byte BLAKE3 digest (file.content_id); times are ms since the Unix epoch.

CREATE TABLE ai_queue (
    content_id   BLOB    PRIMARY KEY,
    -- Lower runs first: keepers of class photo/likely_photo, then other keepers, by capture date;
    -- non-keepers only when the user opted in.
    priority     INTEGER NOT NULL,
    state        INTEGER NOT NULL DEFAULT 0 CHECK (state BETWEEN 0 AND 4),
    -- Requests spent so far (including retries and the repair round).
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    enqueued_at  INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) WITHOUT ROWID;
-- claim_next: first pending item by priority.
CREATE INDEX ai_queue_by_state ON ai_queue (state, priority);

-- One row per content, provider, model and schema version (spec "One result per content", "Provenance").
CREATE TABLE ai_result (
    content_id      BLOB    NOT NULL,
    provider        TEXT    NOT NULL,
    model           TEXT    NOT NULL,
    schema_version  INTEGER NOT NULL,
    prompt_version  INTEGER NOT NULL,
    insight         TEXT    NOT NULL,   -- JSON ps_model::ImageInsight
    latency_ms      INTEGER NOT NULL,
    requests        INTEGER NOT NULL DEFAULT 1,
    at              INTEGER NOT NULL,
    PRIMARY KEY (content_id, provider, model, schema_version)
) WITHOUT ROWID;
-- The review view lists results newest first.
CREATE INDEX ai_result_by_time ON ai_result (at);

-- User corrections win over AI values (spec "User corrections win"); the AI result stays stored.
CREATE TABLE ai_correction (
    content_id  BLOB    NOT NULL,
    field       TEXT    NOT NULL CHECK (field IN ('category', 'also', 'people', 'pets', 'tags', 'subject', 'setting')),
    value       TEXT    NOT NULL,       -- JSON
    at          INTEGER NOT NULL,
    PRIMARY KEY (content_id, field)
) WITHOUT ROWID;

-- Advisory suggestions such as "probably a screenshot" (is_photo false, confidence >= 0.7). Accepting one is a
-- user exclusion recorded elsewhere with the AI's reason; nothing here excludes anything by itself.
CREATE TABLE ai_suggestion (
    id          INTEGER PRIMARY KEY,
    content_id  BLOB    NOT NULL,
    kind        TEXT    NOT NULL,       -- e.g. 'exclude_screenshot', 'exclude_document', 'exclude_graphic'
    reason      TEXT    NOT NULL,
    confidence  REAL    NOT NULL,
    status      INTEGER NOT NULL DEFAULT 0 CHECK (status BETWEEN 0 AND 2),
    created_at  INTEGER NOT NULL,
    decided_at  INTEGER,
    UNIQUE (content_id, kind)
);
CREATE INDEX ai_suggestion_open ON ai_suggestion (status, created_at);

-- The queue's run state survives restarts; a single row.
CREATE TABLE ai_run (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    state       INTEGER NOT NULL DEFAULT 0 CHECK (state BETWEEN 0 AND 2),
    provider    TEXT,
    model       TEXT,
    updated_at  INTEGER NOT NULL
);
