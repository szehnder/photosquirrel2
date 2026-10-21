-- Migration 13 (settings-wiring, ADR-0051): the probe facts the classifier read (`class_facts`, JSON
-- `ps_model::ClassFacts`) and the hash of the classifier settings in force (`classifier_hash`, u64 bit pattern
-- stored as i64), written with the class. A rescan re-classifies an unchanged file whose hash differs from the
-- current settings from these facts alone, without reading the file. NULL for rows probed before this
-- migration: those keep their class until their content changes.
ALTER TABLE file ADD COLUMN class_facts TEXT;
ALTER TABLE file ADD COLUMN classifier_hash INTEGER;
