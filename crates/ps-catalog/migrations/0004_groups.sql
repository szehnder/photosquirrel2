-- Migration 4 (duplicate-groups change; ADR-0013, ADR-0014, ADR-0033, ADR-0035): duplicate groups, their
-- members with the edge each joined by, similar-shot suggestions, and the user's constraints.
--
-- Groups are derived data, rewritten by the regroup after each scan (only the groups that changed). User
-- constraints are keyed by content identity (ADR-0011: digest + identity kind), never by file id, so they
-- survive rescans, moves, re-links and regrouping.
--
-- Stable integer codes (never reuse a value):
--   dup_member.edge_kind:      0 keeper (root of the explanation tree), 1 exact, 2 derivative, 3 user merge
--   *.kind_* / identity kind:  0 full, 1 sampled (as file.identity_kind)
-- Times are milliseconds since the Unix epoch.
--
-- File ids have no foreign keys here: RemoveSource deletes file rows and the next regroup removes their
-- groups; until then a stale member row for a vanished id is harmless (queries join on present files).

-- One duplicate group (two or more members). Its id is its smallest member file id: deterministic, equal for
-- full and incremental regroups, and stable while that member stays in the group.
CREATE TABLE dup_group (
    id                INTEGER PRIMARY KEY,
    keeper_file_id    INTEGER NOT NULL,
    -- The member to show (the JPEG of a RAW+JPEG keeper set, else the keeper).
    display_file_id   INTEGER NOT NULL,
    keeper_set        TEXT    NOT NULL,   -- JSON array of file ids (the keeper and its RAW+JPEG companion)
    reasons           TEXT    NOT NULL,   -- JSON array of {"text": ..., "reason": ps_dedup::KeeperReason}
    member_count      INTEGER NOT NULL CHECK (member_count >= 2),
    reclaimable_bytes INTEGER NOT NULL,
    -- 32-byte digest of the group's stored form: the regroup rewrites a group only when it changed.
    digest            BLOB    NOT NULL,
    reviewed_at       INTEGER
);
CREATE INDEX dup_group_by_keeper ON dup_group (keeper_file_id);
-- Largest groups / most reclaimable first (review queue).
CREATE INDEX dup_group_by_reclaimable ON dup_group (reclaimable_bytes, id);

-- A file is in at most one group.
CREATE TABLE dup_member (
    file_id      INTEGER PRIMARY KEY,
    group_id     INTEGER NOT NULL REFERENCES dup_group(id) ON DELETE CASCADE,
    -- Position in the best-copy ranking, 0 = keeper.
    rank         INTEGER NOT NULL,
    edge_kind    INTEGER NOT NULL CHECK (edge_kind BETWEEN 0 AND 3),
    -- The member this one joined through (NULL for the keeper).
    via_file_id  INTEGER,
    -- Hash distances to `via_file_id` for derivative edges.
    phash_d      INTEGER,
    dhash_d      INTEGER,
    in_keeper_set INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX dup_member_by_group ON dup_member (group_id, rank);

-- Similar-shot suggestions (ADR-0013/0035): pairs that passed the hash test but whose metadata contradicts.
-- Never collapsed into a group.
CREATE TABLE dup_suggestion (
    file_a   INTEGER NOT NULL,            -- file_a < file_b
    file_b   INTEGER NOT NULL,
    phash_d  INTEGER NOT NULL,
    dhash_d  INTEGER NOT NULL,
    reason   TEXT    NOT NULL,            -- JSON {"text": ..., "reason": ps_dedup::Contradiction}
    PRIMARY KEY (file_a, file_b),
    CHECK (file_a < file_b)
) WITHOUT ROWID;
CREATE INDEX dup_suggestion_by_b ON dup_suggestion (file_b);

-- Keeper overrides (ADR-0014): the content the user chose as keeper of its group. `chosen_file_id` records
-- which copy was clicked (informational: any file of that content ranks first).
CREATE TABLE keeper_override (
    content_id     BLOB    NOT NULL,
    content_kind   INTEGER NOT NULL CHECK (content_kind IN (0, 1)),
    chosen_file_id INTEGER,
    at             INTEGER NOT NULL,
    PRIMARY KEY (content_id, content_kind)
) WITHOUT ROWID;

-- "Never pair" (ADR-0033), stored with the smaller (digest, kind) first.
CREATE TABLE dup_separation (
    content_a  BLOB    NOT NULL,
    kind_a     INTEGER NOT NULL CHECK (kind_a IN (0, 1)),
    content_b  BLOB    NOT NULL,
    kind_b     INTEGER NOT NULL CHECK (kind_b IN (0, 1)),
    at         INTEGER NOT NULL,
    PRIMARY KEY (content_a, kind_a, content_b, kind_b)
) WITHOUT ROWID;

-- "Always pair" (ADR-0033 user merges), stored with the smaller (digest, kind) first. Separations win.
CREATE TABLE dup_merge (
    content_a  BLOB    NOT NULL,
    kind_a     INTEGER NOT NULL CHECK (kind_a IN (0, 1)),
    content_b  BLOB    NOT NULL,
    kind_b     INTEGER NOT NULL CHECK (kind_b IN (0, 1)),
    at         INTEGER NOT NULL,
    PRIMARY KEY (content_a, kind_a, content_b, kind_b)
) WITHOUT ROWID;
