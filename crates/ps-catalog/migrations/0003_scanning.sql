-- Migration 3 (scanning change; ADR-0006, ADR-0007, ADR-0008).
-- `first_seen`: when the file was first catalogued (ms since the Unix epoch). A file moved within its source
-- keeps its row, so its id and first-seen time follow it (incremental-rescan spec). NULL for rows written
-- before this migration.
ALTER TABLE file ADD COLUMN first_seen INTEGER;
