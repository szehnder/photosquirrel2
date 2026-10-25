# 0092. A group already in the library is not copied again when its keeper changes; better copies are offered

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0023](0023-disposition-ledger.md) (covered dispositions), [0071](0071-date-logic-version-and-existing-libraries.md)
  (existing libraries keep their layout)

## Context
The export ledger records each copy by content and planning asks only whether the current keeper set's contents are
in the library (ADR-0022, ADR-0044). When a group's keeper changes to a different content after the old keeper was
exported (a HEIC original decoded at last becomes keeper by format tier, ADR-0014; the user chooses another keeper; a
better copy appears on a new drive), the group turns pending and the next export copies the new keeper beside the old
one. Import tools see two photos. With system HEIC decoding this happens for many groups at once.

## Decision
- A group none of whose keeper-set files is in the library, but one of whose other members has a `done` ledger copy
  still at its recorded path, is **covered by that earlier copy**: the export copies nothing for it by default. The
  check is the stat-only ledger check of ADR-0044 (a re-hash only with verify). A missing earlier copy does not
  cover the group; a copy changed by another app does (it is reported as in ADR-0046).
- A keeper set partly in the library is still completed beside its exported files (ADR-0071); that is not a keeper
  change.
- Plans (`ps-export`, CLI dry run and run, the app's export panel, `--json`) list the **better copies available**:
  group, earlier library copy, current keeper set, with totals.
- On request only (`--upgrade`, or the export panel's option, per export, never a stored default) the better copies
  are copied into the earlier copy's folder under their own names, with the normal collision suffix, pairing (RAW+JPEG
  sets, Live Photo still and video together) and verified copy; a sidecar written for the group is shared, not
  rewritten; manifest lines name the earlier copy (`better_copy_of`).
- The earlier copy is **never removed, replaced or renamed** (ADR-0004).
- Dispositions: in such a group the members with a valid library copy are `exported` and every other member, the
  keeper included, `covered` by that copy; the group counts as accounted for.

## Alternatives considered
- **Copy the new keeper by default** (the old behaviour) - duplicates in the library at scale.
- **Replace the library copy** - breaks copy-only export and any import already made from the library.
- **Ask per group during the export** - does not scale and does not work headless.
- **Ignore better copies silently** - hides that the library holds a lesser copy.

## Consequences
- A library may hold a lesser copy of a photo until the user opts in; every plan says how many.
- After an upgrade the library holds both copies, side by side; removing the lesser one stays the user's choice in
  their photo app.
- `ExportUnit` copies carry their contents; planning costs one lookup per member (the re-plan budget is unchanged).
- The completion report agrees with the plan: a covered group is accounted for.

## Revisit when
A reversible "replace in the library" flow (quarantine-based, ADR-0024) is wanted, or libraries are shared between
catalogs.
