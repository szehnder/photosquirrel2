# 0093. A better copy not in the library is superseded, not covered

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0092](0092-a-group-already-in-the-library-is-not-copied-again.md) (dispositions of a group covered by an
  earlier copy), [0024](0024-reversible-quarantine-cleanup.md) (what cleanup may move),
  [0023](0023-disposition-ledger.md) (dispositions)

## Context
ADR-0092 made a group whose keeper changed after a lesser member was exported "covered by that earlier copy" and
declared every member of it, the new keeper included, `covered`. Cleanup (ADR-0024) moves `exported` and `covered`
files. Together they let cleanup quarantine the only full-quality original: an 800x533 JPEG from source A is exported;
source B's 3000x2000 HEIC original is decoded later and becomes the keeper; the report said source B was "100%
accounted for (covered 1)" and `cleanup plan --source 2` planned to move the HEIC, "safe in lib". Before ADR-0092,
covered meant that an equal or better copy was in the library, and the owner's first priority is never to lose a
photo.

## Decision
- A new disposition, **`superseded`** ("better copy not in the library yet"): in a group in the library only
  through an earlier copy (ADR-0092), the keeper set and every member ranked above the best-ranked member with a valid
  library copy are superseded, with that copy's library path as the lesser copy. Members ranked below it stay
  `covered` by it; members with their own valid copy are `exported`. Ranking order is the group's member order
  (ADR-0014); the keeper set counts as better whatever its rank (a keeper chosen by the user is the photo they want).
- `covered` keeps its original meaning: an equal or better copy of this file is in the library.
- Superseded files are **not accounted for** (`fully_accounted` is false) and **never eligible for cleanup**.
  Reports show them as their own line, in the CLI report text and `--json` (`counts.files.superseded`), the Reports
  view, the badge ("N better copies not in the library yet") and the inspector, with a pointer to `export run
  --upgrade` and the export panel's option. The inspector never calls the lesser copy "its best copy".
- A quarantined file whose disposition at its path would be superseded is shown as superseded with its quarantine
  path (only possible for files moved before this decision).
- Once `--upgrade` exports the better copy, the keeper set is in the library and the usual rules apply: it is
  exported, the lesser copies covered, and cleanup may consider them.
- The export plan is unchanged: such a group is still not copied by default (ADR-0092); its better copies are
  listed in every plan.

## Alternatives considered
- **Make covered-by-an-earlier-copy groups pending** - the export would then copy them by default, which is what
  ADR-0092 removed.
- **Keep `covered` and exclude these files from cleanup only** - the report would still say "100% accounted for"
  while the library holds a lesser copy.
- **Compare resolutions or bytes instead of rank** - ranking already orders a group by everything the keeper rule
  knows (format tier, resolution, metadata, ADR-0014); a second ordering could disagree with it.

## Consequences
- One more disposition (appended, so stored codes and counters keep their positions); `CompletionCounts` has eight
  entries; JSON reports gain a `superseded` key.
- A library holding lesser copies is visibly not complete until the user upgrades; cleanup cannot remove the better
  originals meanwhile.

## Revisit when
A reversible "replace in the library" flow exists (ADR-0092's revisit), or per-member quality comparisons beyond
ranking are wanted.
