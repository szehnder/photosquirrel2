-- Migration 9 (M3/M4 gate fixes; ADR-0044): the export ledger records the modification time of each library
-- copy when the row is written, so a re-plan or a report can trust a row whose copy still has its recorded size
-- and modification time without reading the copy. NULL in rows written before this migration: those are
-- trusted on their size, as before.

ALTER TABLE export_ledger ADD COLUMN library_mtime_ns INTEGER;
