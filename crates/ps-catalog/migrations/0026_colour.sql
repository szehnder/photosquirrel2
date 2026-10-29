-- Migration 26 (adobe-rgb-color; ADR-0113): the colour space of a file's pixels, which the grid thumbnail and the
-- large preview are converted from (perceptual hashes never are).
--
--   colour: 'srgb', 'adobe_rgb', 'display_p3' or 'profile:<description>' (written by every analysis from the probe);
--           'unchecked' (analysed before colour spaces were read and not re-checked: the inspector shows nothing);
--           NULL (not analysed yet, or due the one-time re-check below)
--
-- The owner's choice (2026-10-27): of the rows analysed before, only thumbed JPEG and RAW stills with a real thumbnail
-- whose name starts with `_` (DCF's marker for an Adobe RGB file, `_MG_1234.JPG`) stay NULL. The next rescan of their
-- source reads each one's header once (the probe window, read-only) and makes its thumbnail again when it is not
-- sRGB. Every other analysed row is marked 'unchecked' here without reading it; it is recorded when it is analysed
-- again. SQL only: it reads no file bytes. stage 3 = thumbed; X'5F' = '_' (`name` is a BLOB).
ALTER TABLE file ADD COLUMN colour TEXT;
UPDATE file SET colour = 'unchecked'
WHERE content_id IS NOT NULL
  AND NOT COALESCE(stage = 3
                   AND preview_reason IS NULL
                   AND (format = 'jpeg' OR format LIKE 'raw:%')
                   AND substr(name, 1, 1) = X'5F',
                   0);
