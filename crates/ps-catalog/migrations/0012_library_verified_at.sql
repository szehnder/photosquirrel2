-- Migration 12 (M4 gate fixes): when the library's copies were last re-hashed (`report --verify`, the Reports
-- view's Verify copies), so reports can say how fresh the "verified by hash" answer is. NULL: never verified.

ALTER TABLE export_library ADD COLUMN verified_at INTEGER;
