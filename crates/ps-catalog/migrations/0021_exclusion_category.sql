-- Migration 21 (excluded-view-filters, ADR-0102): each file's exclusion category, and where each user decision came
-- from, so the Excluded view filters and counts by category in SQL.
--
--   file.exclusion:         the classifier's category of an excluded class: 'cache' (thumbnail cache), 'graphic',
--                           'small' (junk for its size or video length), 'unreadable' (junk of a recognised format
--                           whose probe failed), 'unsupported' (junk of no recognised format); NULL for the classes
--                           the Library shows and for unclassified files. Written with `class` and `reasons`.
--   user_inclusion.origin:  'user', or 'ai' for an exclusion recorded by accepting an AI suggestion.
--   user_inclusion.ai_kind: the suggestion's kind for 'ai' decisions: 'screenshot', 'document', 'graphic',
--                           'not_photo'.
--
-- The effective category is the decision's ('you' or 'ai') when the file has an exclude decision, none when it
-- has an include decision, else `file.exclusion`. The backfill below is SQL only: it reads no file bytes.
ALTER TABLE file ADD COLUMN exclusion TEXT;
ALTER TABLE user_inclusion ADD COLUMN origin TEXT NOT NULL DEFAULT 'user' CHECK (origin IN ('user', 'ai'));
ALTER TABLE user_inclusion ADD COLUMN ai_kind TEXT
    CHECK (ai_kind IS NULL OR ai_kind IN ('screenshot', 'document', 'graphic', 'not_photo'));

-- Stored reasons are a JSON array of display texts; a probe failure's text starts with 'probe failed:'. The same
-- expression is written by the catalog's writer (`writer::exclusion_sql`).
UPDATE file SET exclusion = CASE class
        WHEN 2 THEN 'cache'
        WHEN 3 THEN 'graphic'
        WHEN 4 THEN CASE
            WHEN format IS NULL THEN 'unsupported'
            WHEN json_valid(reasons) AND json_extract(reasons, '$[0]') LIKE 'probe failed:%' THEN 'unreadable'
            ELSE 'small' END
        END
    WHERE class IN (2, 3, 4);

-- Exclusions recorded by accepting an AI suggestion carry the reason '<label> (AI suggestion)'. Only that exact
-- suffix and the four labels match; anything else stays 'user', which is still shown.
UPDATE user_inclusion SET origin = 'ai', ai_kind = CASE substr(reason, 1, length(reason) - 16)
        WHEN 'screenshot' THEN 'screenshot'
        WHEN 'document' THEN 'document'
        WHEN 'graphic' THEN 'graphic'
        WHEN 'not a photo' THEN 'not_photo'
        END
    WHERE decision = 'exclude' AND length(reason) > 16 AND substr(reason, -16) = ' (AI suggestion)'
      AND substr(reason, 1, length(reason) - 16) IN ('screenshot', 'document', 'graphic', 'not a photo');

-- The Excluded view's lists and counts scan only the classifier-excluded rows, from the index alone: the sort
-- columns (name, size, path) and the filter columns (kflags, source) are in it.
CREATE INDEX file_exclusion ON file (exclusion, id, kflags, source_id, size, name, rel_path)
    WHERE exclusion IS NOT NULL;
