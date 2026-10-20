-- Migration 10 (M3/M4 gate fixes; ADR-0045): the export ledger also records XMP sidecars, so a sidecar write
-- interrupted by a crash is cleaned up and retried like a copy, and a sidecar PhotoSquirrel wrote is known to be
-- its own.
--
-- Stable integer codes (never reuse a value):
--   export_ledger.kind: 0 file (a copied or adopted photo or video), 1 sidecar (`<stem>.xmp`)
-- A sidecar row's content_id, identity_kind and file_id are those of the primary file it describes; its
-- full_hash and size are those of the sidecar's own bytes.

ALTER TABLE export_ledger ADD COLUMN kind INTEGER NOT NULL DEFAULT 0 CHECK (kind IN (0, 1));
