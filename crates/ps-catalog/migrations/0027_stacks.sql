-- Migration 27 (best-of-stacks; ADR-0135): stacks of similar shots are computed, not stored. Only the user's
-- stack decisions and each photo's shot-quality scores are.
--
--   file.shot_quality_version: the version of the scores below (NULL: not scored yet; the background fill scores
--                              it from its stored thumbnail, never from the source)
--   file.sharpness:            variance of the Laplacian of the thumbnail's luminance
--   file.clipped_high/low:     fractions of the thumbnail's pixels with blown highlights / crushed shadows
--   stack_separation:          "taken out of the stack": two contents never stacked together, ordered as
--                              dup_separation's rows are
--   stack_pick:                "pick this one": a content the user picked in its stack
--   export_library.best_of:    the last export's choice, `{"mode": "only" | "both", "window_s", "similarity"}`
--                              (NULL: everything): which shots its dispositions count as not picked
ALTER TABLE file ADD COLUMN shot_quality_version INTEGER;
ALTER TABLE file ADD COLUMN sharpness REAL;
ALTER TABLE file ADD COLUMN clipped_high REAL;
ALTER TABLE file ADD COLUMN clipped_low REAL;

CREATE TABLE stack_separation (
    content_a  BLOB    NOT NULL,
    kind_a     INTEGER NOT NULL CHECK (kind_a IN (0, 1)),
    content_b  BLOB    NOT NULL,
    kind_b     INTEGER NOT NULL CHECK (kind_b IN (0, 1)),
    at         INTEGER NOT NULL,
    PRIMARY KEY (content_a, kind_a, content_b, kind_b)
) WITHOUT ROWID;

CREATE TABLE stack_pick (
    content_id   BLOB    NOT NULL,
    content_kind INTEGER NOT NULL CHECK (content_kind IN (0, 1)),
    at           INTEGER NOT NULL,
    PRIMARY KEY (content_id, content_kind)
) WITHOUT ROWID;

ALTER TABLE export_library ADD COLUMN best_of TEXT;
