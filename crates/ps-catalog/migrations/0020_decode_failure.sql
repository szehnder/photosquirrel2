-- Migration 20 (heic-system-decoders, gate re-review; ADR-0098): a HEIC or AVIF whose decode failed while its
-- decoder was present records how, so a rescan decodes it again when the failure was transient (a watchdog
-- timeout, AV1 decoding paused, a thread that could not start; up to 3 attempts) or the decoder backend changed (a
-- libheif upgrade, another rav1d), and never re-reads it for a permanent data error otherwise.
--
--   decode_failure: NULL no failure recorded; else '<key>|<t|p>|<attempts>|<backend>', for example
--                   'heif|p|1|libheif 1.23.3' (key heif or avif; t transient, p permanent; the backend last)
--
-- Rows of catalogs from before this migration have none: a failure recorded by migration 19's code cannot be told
-- from a real placeholder without a re-read, and the coming decoder generations (`preview-placeholders`) retry
-- them once.
ALTER TABLE file ADD COLUMN decode_failure TEXT;

-- Keeps the rescan diff's lookups cheap: only rows with a recorded failure are indexed.
CREATE INDEX file_decode_failure ON file (source_id) WHERE decode_failure IS NOT NULL;
