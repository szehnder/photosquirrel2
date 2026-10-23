-- Migration 17 (capture-dates; ADR-0069, ADR-0070, ADR-0071): the precision of a file's capture date and the
-- version of the date rules that chose it.
--
--   capture_precision: 0 second, 1 day, 2 month (a folder-name date: `2019-07-04 Beach`, `2019-07 Italy`)
--   date_version:      1 = the rules before ADR-0069/0070, 2 = folder-name dates and local days of instants
--
-- A rescan re-dates its source's unchanged, analysed rows whose date_version is below the build's
-- DATE_LOGIC_VERSION from the stored facts alone (path, mtime_ns, metadata JSON), without reading any file.
-- Rows whose date cannot change under version 2 are marked current here: an EXIF DateTimeOriginal or CreateDate
-- (wall clocks, never shifted), or a container date with a non-zero offset (a recorded wall clock).
ALTER TABLE file ADD COLUMN capture_precision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE file ADD COLUMN date_version INTEGER NOT NULL DEFAULT 1;
UPDATE file SET date_version = 2
WHERE capture_provenance IN ('DateTimeOriginal', 'CreateDate')
   OR (capture_provenance = 'container' AND capture_offset_min <> 0);
