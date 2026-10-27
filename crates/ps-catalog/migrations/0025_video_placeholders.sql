-- Migration 25 (preview-placeholders; ADR-0111): videos catalogued with a placeholder before migration 24 get their
-- reason from the thumbnail store, which lives in a separate database (thumbs.db), so the owner does it once, at
-- open, after this migration, and then removes the task's row.
--
--   catalog_task: one-time catalog tasks left for the engine to finish (name); a row is removed when its task is
--                 done.
--     'video_placeholder_reasons': thumbed video rows without a placeholder reason; those whose stored thumbnail
--                                 is a placeholder are stamped with its reason (no reason recorded: no poster-frame
--                                 tool) and generation.
CREATE TABLE catalog_task (
    name TEXT PRIMARY KEY
);

-- stage 3 = thumbed, media_kind 1 = video.
INSERT INTO catalog_task (name)
SELECT 'video_placeholder_reasons'
WHERE EXISTS (SELECT 1 FROM file
              WHERE stage = 3 AND media_kind = 1 AND preview_reason IS NULL AND content_id IS NOT NULL);
