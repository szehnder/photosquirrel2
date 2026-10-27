-- Migration 24 (preview-placeholders; ADR-0109): a placeholder row records why it has no preview and the decoder
-- generation that made it, so Fill in previews and the next rescan re-decode exactly the rows a newer decoder can
-- fill in; and a scan run records whether it is a Fill in previews job.
--
--   preview_gen:    the decoder generation of the format's family when the placeholder was made; NULL for rows with
--                   a real thumbnail (and rows never thumbed)
--   preview_reason: 'decoder_missing', 'no_poster_tool', 'sequence', 'unsupported_variant', 'unreadable' or
--                   'needs_newer_version'; NULL with preview_gen
--   scan_run.kind:  'scan' a walk of the sources, 'fill' a Fill in previews job (no walk)
--   scan_run.fill_scope: the job's scope as JSON ({"format":"heic","reason":null}), NULL for scans
ALTER TABLE file ADD COLUMN preview_gen INTEGER;
ALTER TABLE file ADD COLUMN preview_reason TEXT;
ALTER TABLE scan_run ADD COLUMN kind TEXT NOT NULL DEFAULT 'scan' CHECK (kind IN ('scan', 'fill'));
ALTER TABLE scan_run ADD COLUMN fill_scope TEXT;

-- The No preview filter, the counts by format and reason and the waiting rule read only placeholder rows.
CREATE INDEX file_preview ON file (preview_reason, format) WHERE preview_reason IS NOT NULL;

-- Catalogs from before this change: every thumbed still without perceptual hashes has a placeholder (the signal
-- migration 19 used for HEIC and AVIF). It gets generation 0, so the formats this version decodes at a newer
-- generation (TIFF, HEIF and AVIF are at 1) are filled in once. The reason, as far as the row tells it: the missing
-- decoder, a recorded decode failure, a RAW file (no embedded preview) or a sampled identity (too large to decode)
-- is a variant not supported, anything else could not be read. A re-decode records the real reason.
-- Videos have no perceptual hashes with or without a poster frame, so their rows say nothing: they are not marked,
-- and their tiles take the reason from the stored placeholder record. stage 3 = thumbed, media_kind 0 = image,
-- identity_kind 0 = full.
UPDATE file SET preview_gen = 0,
    preview_reason = CASE
        WHEN decoder_missing IS NOT NULL THEN 'decoder_missing'
        WHEN decode_failure IS NOT NULL THEN 'unreadable'
        WHEN format LIKE 'raw:%' OR identity_kind <> 0 THEN 'unsupported_variant'
        ELSE 'unreadable' END
WHERE stage = 3 AND phash IS NULL AND media_kind = 0 AND content_id IS NOT NULL;
