# 0074. Keeper reasons compare against the best different copy (amends 0014)

- Status: proposed
- Date: 2026-10-22
- Deciders: maintainers
- Amends: [0014](0014-best-copy-ranking.md)

## Context
ADR-0014 ranks members lexicographically and explains the keeper against the second-ranked member. That member
is often a byte-identical copy of the keeper (same bytes on another drive) or the keeper's RAW+JPEG companion,
so the reasons say "shallower folder" or "identical copies; first catalogued", which explains nothing about
why the keeper beat the resized and recompressed copies. The duplicates view already skips identical copies
for its "Next best" line, so the panel compares the keeper with one member in its reasons and names a different
member below them.

## Decision
- The ranking (order and keeper) is unchanged. The runner-up used for the reasons is the best-ranked member
  outside the keeper set whose content identity differs from the keeper's; if there is none, the best-ranked
  member outside the keeper set; if there is none, the second-ranked member.
- The ranking records that runner-up with the group, and the review UI's "Next best" line uses it instead of
  choosing its own.
- Stored groups without a recorded runner-up are read as before and get one at the next full regroup, which
  happens on the first scan of each app process.

## Alternatives considered
- **Make the UI follow the reasons (second-ranked member)** - consistent, but keeps explaining the keeper
  against an identical copy.
- **List reasons against every member** - long, and the panel is meant to give the one deciding difference.

## Consequences
- Reasons and "Next best" always name the same member; groups whose second member is an identical copy now
  explain the keeper against a real alternative.
- Group digests change, so the first regroup after the upgrade rewrites most group rows once.
- The corpus gate's keeper accuracy is unaffected (keepers do not change).

## Revisit when
The ranking gains criteria that make identical copies differ in quality (for example, sidecar metadata).
