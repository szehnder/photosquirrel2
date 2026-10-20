-- Migration 11 (M3 gate fixes): every write of an AI result takes the next sequence number, so a reader that
-- remembers the highest number it has seen finds every new or re-described result, also one whose `at` is not
-- newer (a clock step, or the same millisecond). The gallery's insight index uses it (ps_service::library).
-- Rows written before this migration have seq 0 and are covered by the index's full build.

ALTER TABLE ai_result ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;
CREATE INDEX ai_result_by_seq ON ai_result (seq);
