-- Migration 19 (heic-system-decoders; ADR-0091): a file analysed without the decoder its format needs records
-- which one was missing, so the next rescan with that decoder re-decodes it (previews, perceptual hashes).
--
--   decoder_missing: NULL nothing was missing, 'heif' the HEIC/HEIF system decoder, 'avif' the AV1 decoder (only
--                    rows of catalogs from before AVIF was bundled)
ALTER TABLE file ADD COLUMN decoder_missing TEXT;

-- Keeps the run summary's count and the diff cheap: only waiting rows are indexed.
CREATE INDEX file_decoder_missing ON file (source_id) WHERE decoder_missing IS NOT NULL;

-- Catalogs from before this change: thumbed HEIC and AVIF rows with a full identity and no perceptual hashes were
-- analysed without a decoder (sampled identities are files too large to decode, ADR-0010; they keep no marker).
-- stage 3 = thumbed, identity_kind 0 = full.
UPDATE file SET decoder_missing = CASE format WHEN 'heic' THEN 'heif' ELSE 'avif' END
WHERE format IN ('heic', 'avif') AND stage = 3 AND phash IS NULL AND identity_kind = 0;
