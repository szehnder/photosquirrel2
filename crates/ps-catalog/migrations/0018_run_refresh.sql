-- Migration 18 (capture-dates gate review; ADR-0082): a scan run remembers whether it refreshes the analysis of
-- unchanged files without stored class facts (ADR-0071), so resuming a paused, cancelled or interrupted refresh
-- run still refreshes.
--
--   refresh_analysis: 0 a plain scan, 1 a scan with the analysis refresh (`scan --refresh-analysis`)
ALTER TABLE scan_run ADD COLUMN refresh_analysis INTEGER NOT NULL DEFAULT 0;
