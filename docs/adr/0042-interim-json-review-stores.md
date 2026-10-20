# 0042. Interim JSON stores for review state and AI corrections

- Status: accepted
- Date: 2026-10-19
- Deciders: maintainers

## Context
The `duplicate-review` and `ai-insights` changes need durable user decisions that the catalog cannot hold yet:

- **Review state** (which duplicate groups the user has reviewed) and **dismissed similar-shot suggestions**.
  The catalog has `dup_group.reviewed_at`, but group ids change on regrouping and there is no
  `suggestion.dismissed_at` column.
- **AI corrections** (the user's category, people, pets, tags ... over the model's values) and **suggestion
  decisions** (accepted or rejected exclusion suggestions). Migration 5 created `ai_correction` and
  `ai_suggestion`, but the catalog exposes no write API for them (its writer's `exec` is crate-private and
  ADR-0005 routes every write through typed commands).

Adding catalog commands and a migration for each was more work than the M2 views needed, and the shape of the
data was still moving. These are user decisions: losing them silently is a data-loss bug even though no photo is
touched.

## Decision
Two small JSON files in the data directory, next to `catalog.db`, written only by `ps-service`:

- `review-state.json` (`ps_service::review::ReviewStore`): `{version, reviewed: {membership key -> ms},
  dismissed: {"contentA:contentB" -> ms}}`.
- `ai-review.json` (`ps_service::insights::InsightStore`): `{version, corrections: {content hex -> fields},
  decisions: {"content:kind" -> status}, revision}`.

Rules:

1. **Keyed by content, never by row id.** A reviewed group is keyed by an FNV-1a hash of the sorted multiset of
   its members' BLAKE3 content ids, so it survives restarts and regrouping while the group is unchanged and
   becomes unreviewed when a member is added or removed. Dismissed pairs and AI corrections are keyed by
   content id, so they survive rescans, moves and renames.
2. **Atomic, durable writes.** Every change writes `<name>.tmp` in the same folder, `sync_all`s it, renames it
   over the old file and fsyncs the folder (`ps_service::json_store::save`). A crash leaves the old or the new
   state, never a torn file.
3. **Single writer.** Only the process that owns the catalog (ADR-0041 owner lock) opens the stores, and each
   store serialises its writes behind a mutex. Read-only CLI observers do not open them.
4. **Damaged files are kept.** A file that does not parse is renamed to `<name>.damaged-<unix-ms>` and a warning
   is logged before the store starts empty (`json_store::load`); it is never overwritten.
5. **Advisory and non-destructive.** Neither file decides grouping or export on its own (ADR-0021); accepting an
   AI exclusion suggestion records an ordinary user exclusion in the catalog.

## Alternatives considered
- **Catalog tables now** (`suggestion.dismissed_at`, membership-keyed review rows, writes to `ai_correction`) -
  the right end state, but each needs typed writer commands and a migration while the data shape was still
  changing during M2.
- **Keep them in `ui-state.json`** - that file is UI layout, rewritten freely and safe to delete; user
  decisions need stronger guarantees.
- **SQLite side database** - durability for free, but a second database file to migrate and lock for a few
  kilobytes of state.

## Consequences
- Two more files to back up with the catalog; deleting the data directory removes them too.
- The whole file is rewritten on each change. Both stay small (one entry per reviewed group or corrected
  photo); at 100k entries a write is a few MB, still well under the latency a click tolerates on a background
  thread.
- Review state is not visible to SQL queries; the CLI reads `dup_group.reviewed_at`, which `ps-service` keeps in
  step.

## Revisit when
A catalog migration adds membership-keyed review rows, `suggestion.dismissed_at` and writer commands for
`ai_correction`/`ai_suggestion`. Then import both files once at open (renaming them to `<name>.imported`) and
supersede this ADR.
