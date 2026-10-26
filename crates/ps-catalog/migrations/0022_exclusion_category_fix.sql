-- Migration 22 (excluded-view-filters gate review, ADR-0104 amending ADR-0102): migration 21's rule filed every
-- junk file of no recognised format as 'unsupported' (hidden by default), so a damaged IMG_9999.JPG and a file that
-- could not be read at all were hidden as "not a supported file". The rule now files junk of no recognised format
-- as 'unreadable' (shown by default) when any reason is a probe failure (an I/O error) or the name has a photo or
-- video extension. This recomputes `file.exclusion` for every classifier-excluded row with the new rule; the writer
-- (`writer::exclusion_sql`) writes the same expression, and a test checks both against `ExclusionCategory::of`.
--
-- The extension is the text after the name's last '.', lower case; the list is `ps_model::media_extensions()`.
-- SQL only: it reads no file bytes.
UPDATE file SET exclusion = CASE class
        WHEN 2 THEN 'cache'
        WHEN 3 THEN 'graphic'
        WHEN 4 THEN CASE
            WHEN format IS NOT NULL THEN CASE
                WHEN json_valid(reasons) AND json_extract(reasons, '$[0]') LIKE 'probe failed:%' THEN 'unreadable'
                ELSE 'small' END
            WHEN EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(reasons) THEN reasons ELSE '[]' END) AS r
                         WHERE r.value LIKE 'probe failed:%') THEN 'unreadable'
            WHEN (CASE WHEN instr(CAST(name AS TEXT), '.') > 0 THEN lower(substr(CAST(name AS TEXT),
                      length(rtrim(CAST(name AS TEXT), replace(CAST(name AS TEXT), '.', ''))) + 1)) END)
                 IN ('3fr', '3g2', '3gp', 'apng', 'arw', 'avi', 'avif', 'bmp', 'cr2', 'cr3', 'crw', 'dib', 'dng',
                     'fff', 'gif', 'heic', 'heics', 'heif', 'hif', 'iiq', 'jfif', 'jif', 'jpe', 'jpeg', 'jpg', 'm2t',
                     'm2ts', 'm4v', 'mkv', 'mov', 'mp4', 'mts', 'nef', 'nrw', 'orf', 'pef', 'png', 'qt', 'raf',
                     'raw', 'rw2', 'rwl', 'sr2', 'srf', 'srw', 'tif', 'tiff', 'webm', 'webp', 'x3f') THEN 'unreadable'
            ELSE 'unsupported' END
        END
    WHERE class IN (2, 3, 4);

-- An AI decision always has its kind: the Excluded view files it under that kind's chip, and one without a kind
-- would be in no chip at all. Migration 21's backfill never writes one, but a row from elsewhere is filed as the
-- user's own exclusion (still shown), and the triggers below refuse new ones.
UPDATE user_inclusion SET origin = 'user' WHERE origin = 'ai' AND ai_kind IS NULL;

CREATE TRIGGER user_inclusion_ai_kind_insert BEFORE INSERT ON user_inclusion
    WHEN NEW.origin = 'ai' AND (NEW.ai_kind IS NULL OR NEW.decision != 'exclude')
    BEGIN SELECT RAISE(ABORT, 'an AI decision is an exclusion with its kind'); END;
CREATE TRIGGER user_inclusion_ai_kind_update BEFORE UPDATE ON user_inclusion
    WHEN NEW.origin = 'ai' AND (NEW.ai_kind IS NULL OR NEW.decision != 'exclude')
    BEGIN SELECT RAISE(ABORT, 'an AI decision is an exclusion with its kind'); END;
