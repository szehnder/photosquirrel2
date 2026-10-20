# 0045. XMP sidecars have export ledger rows

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
ADR-0022 writes an optional `<stem>.xmp` sidecar next to each exported file with the same partial, fsync and
no-replace rename protocol as a copy, but only copies had ledger rows (ADR-0023). Two failures followed:
- A crash while a sidecar was being written left `<stem>.xmp.ps-partial` behind. Recovery only removes partial
  files of `in_progress` rows, so the partial stayed forever, and the resumed export reported "complete" without
  the sidecar: the bundle's files were already exported, so nothing planned it again.
- When the user deleted a library copy, its sidecar stayed. The next export found the photo's original name free
  but the sidecar name taken by a file it could not prove was its own, and moved the whole bundle to
  `IMG_0042 (2).JPG` instead of restoring `IMG_0042.JPG`.

## Decision
- The ledger gets a `kind` column (catalog migration 10): `file` (0, every existing row) or `sidecar` (1). A
  sidecar row is keyed by the sidecar's library path like any row, and carries the primary file's content and
  file id with the sidecar's own hash and size. Every consumer of "is this content exported" (planner,
  dispositions, completion report) reads `file` rows only.
- The executor records a sidecar `in_progress` before creating its partial and `done` after the rename (or
  `failed`), exactly as for a copy, so recovery removes a crashed sidecar's partial.
- The planner treats a sidecar recorded as `done` for one of a bundle's contents as its own: a re-copied bundle
  keeps that name and the existing sidecar is left as it is (never overwritten).
- When sidecars are on, a bundle whose files are all exported but which has no sidecar of its own is planned as
  a sidecar-only write next to its library copy, if that name is free. This finishes a crashed sidecar and adds
  sidecars when a library first exported without them (Google Photos profile) is exported again with them.

## Alternatives considered
- **Recognise our partials by name** (`*.xmp.ps-partial` next to a ledger-recorded file) - the copy protocol's
  rule is that only partials with an `in_progress` row are ours (ADR-0022, design); a second rule for sidecars is
  weaker and still would not re-plan the missing sidecar.
- **Rewrite sidecars in place** - violates "never overwrite" (ADR-0004).

## Consequences
- One more ledger row per bundle with sidecars. Sidecars count in the plan's sidecar total, not in exported files.
- Libraries exported before migration 10 have no sidecar rows: their sidecars are left as they are and a missing
  one is written once if its name is free.

## Revisit when
Sidecar content needs updating after corrections (a "refresh sidecars" action would then write new versions
under the ledger's control).
