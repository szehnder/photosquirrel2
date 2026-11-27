# Design

## Context

- **Where exclusion lives today:** a user exclusion is a per-file decision stored in the catalog and set from the
  Library inspector (photo-inspector, "User include and exclude"). Its exclusion category is "excluded by you"
  (ADR-0102). Excluding a group's keeper also excludes the group's copies, because copies follow their keeper
  (ADR-0023).
- **The review list** keeps its loaded groups in memory and re-filters them on a change (ADR-0133). Review state
  (reviewed or not) is stored per group.
- **Cleanup** never plans excluded files (cleanup spec), so an exclusion cannot lead to a move.

## Goals / Non-Goals

**Goals:**
- Exclude a whole group in one keystroke from the review, and take it back as easily.
- Keep the group out of the library when its keeper changes later.

**Non-Goals:**
- A new kind of exclusion or a new catalog table.
- Excluding many groups at once.

## Decisions

1. **Record the exclusion on every copy, not only the keeper.** Excluding only the keeper would rely on copies
   following it (ADR-0023). Then a rescan that adds a better copy would make that copy the keeper, and the group
   would quietly return to the library.
   - Each current copy gets the user decision "exclude" with the optional reason, written in one catalog
     transaction.
   - A copy added later is new and is not excluded. If it becomes the keeper, it is the only file of the group in the
     library, and the review shows the group as partly excluded ("1 of 4 kept").
   - Alternative: a group-level exclusion keyed by the keeper's content. Rejected: groups are derived and re-formed
     on every regroup, so a key on a group would need its own migration rules.
2. **"Include again" and Undo remove only what this action wrote.** The action remembers, for each copy, the
   decision it replaced (normally none). Undo, and Include again in the same session, restore those decisions.
   - Include again in a later session clears the user exclusion from every copy whose reason is the action's own.
     That returns each copy to its automatic state; it does not force "included by you", which would override the
     classifier for good.
   - A copy that the classifier or an accepted AI suggestion excluded keeps that exclusion. The header then names
     that reason.
3. **An excluded group counts as reviewed.** Excluding is a review decision. Marking it reviewed keeps the progress
   card honest and lets **E** advance like **R**. Undo restores the previous reviewed state.
4. **E for the key.** It is free in the Duplicates key map (`down up j k r s space x right left enter escape b = -
   0` and the digits). It is also the first letter of the action, and the inspector's exclude has no key.
5. **The list re-filters; it does not reload (ADR-0133).** The group's excluded state is part of the in-memory
   group summary. An exclusion updates that summary and re-filters, within the `duplicates-list-reload` budget.
6. **Reason prompt:** E excludes at once with no reason, so it stays one keystroke. The header button opens a small
   popover with an optional reason field. This matches the inspector's reason input.

## Risks / Trade-offs

- [Someone presses E by accident] → Undo in the notice; the group stays listed with Include again; nothing on disk
  changes.
- [A group that is part excluded after a rescan reads oddly] → The header says "1 of 4 kept" and offers Include
  again for the rest.
- [Large groups write many rows] → One transaction; the largest corpus group has tens of copies, far below what
  the catalog writer batches.

## Migration Plan

None: no catalog or settings change. Rolling back the release leaves the stored user decisions in place, readable
by the inspector as before.

## Verification

- `just test -p ui-workspace --test duplicates`: the `scenario_duplicate_review_*` tests for exclude, include, undo,
  counting and the keeper change.
- `just test -p photosquirrel-cli`: `scenario_cli_exclude_a_group_from_the_cli`, `scenario_cli_include_a_group_from_the_cli`.
- `just test -p ps-service`: `scenario_duplicate_review_excluded_group_is_not_exported` and the keeper-change
  scenario, on the small corpus.
- `cargo xtask screenshot` surface `duplicates-excluded` in both themes, reviewed.
- `cargo xtask ci` and `openspec validate exclude-group-in-review --strict`.
