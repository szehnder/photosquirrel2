-- Migration 2: intentionally a no-op. It exists so the upgrade path (v1 -> latest) is exercised from the
-- first release (catalog task 1.3). Later changes add their schemas as 0003 and onwards.
SELECT 1;
