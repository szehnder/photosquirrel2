# Proposal

## Why

During duplicate review you can choose which copy to keep but not "none of them". A group of photos you don't want
(blurry shots, a wrong upload, someone else's pictures) can only be left out of the library from the Library
inspector, by excluding its keeper. That means leaving the review and finding the photo again. The owner asked for a
way to say "keep none" while reviewing.

## What Changes

- The Duplicates review gets an **Exclude group** action (key **E**, and a button in the group's header). It
  records "excluded by you" (with an optional reason) on every copy in the group, so the whole group leaves the
  library and stays out even if a rescan later makes another copy its keeper. It also marks the group reviewed and
  opens the next unreviewed group, like **R**.
- An excluded group stays in the list with an "Excluded, none kept" state, so the review progress card still counts
  it. Its header offers **Include again**, which removes that exclusion from every copy. A toast offers Undo
  right away.
- `photosquirrel-cli groups exclude <group> [--reason <text>]` and `groups include <group>` do the same from the
  command line. `groups list --json` reports `excluded: true` on such groups.
- Nothing on disk changes. Excluded files are never eligible for cleanup (cleanup spec, "Excluded files not
  eligible"), so "keep none" cannot lead to any file being moved.

Out of scope:
- Deleting or quarantining the files.
- Excluding several groups at once.
- Excluding a single copy from inside a group. That copy is already covered by the keeper; to drop a copy that is
  not the same photo, use **S** (separate).
- Similar-shot suggestions and stacks (`best-of-stacks`).

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `duplicate-review`: a group can be excluded and included again from the review, with its own state in the list
  and in the progress card.
- `cli`: `groups exclude` / `groups include`, and `excluded` in `groups list --json`.

## Impact

- **Code:** the Duplicates view (action, key, header button, excluded state, toast with Undo); the review service
  (a group's excluded state, derived from its keeper's user decision); the CLI `groups` subcommands.
- **No catalog change:** it reuses the stored user decision of the inspector (photo-inspector, "User include and
  exclude").
- **ADRs:** ADR-0004 (non-destructive), ADR-0014 (keeper overrides), ADR-0023 (disposition ledger: an excluded
  keeper excludes its group), ADR-0102 (exclusion categories: "excluded by you"), ADR-0133 (the review list keeps
  its loaded groups in memory; an exclusion re-filters, it does not reload).
