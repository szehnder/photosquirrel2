-- Migration 23 (burst-frame-identity; ADR-0105): the frame a file records and the version of the metadata rules
-- its stored metadata was read under.
--
--   frame_id:     `raw:IMG_0034 num:101-0034` (the parts present: a DNG's OriginalRawFileName stem, the Canon file
--                 number), NULL when the file records none; the metadata JSON holds it too (`fields.frame_id`)
--   meta_version: 0 = before ADR-0105, 1 = frame identity and maker notes kept in DNGPrivateData
--
-- A rescan re-reads the header (the probe window, read-only, no decode, no hashing) of each unchanged analysed
-- RAW-format row below the build's META_LOGIC_VERSION and regroups it. Every other row is marked current here
-- without reading it: a JPEG burst carries sub-seconds, and a JPEG's file number is filled in when it is next
-- analysed. SQL only: it reads no file bytes.
ALTER TABLE file ADD COLUMN frame_id TEXT;
ALTER TABLE file ADD COLUMN meta_version INTEGER NOT NULL DEFAULT 0;
UPDATE file SET meta_version = 1 WHERE format IS NULL OR format NOT LIKE 'raw:%';
