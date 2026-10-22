# 0067. An excluded quarantined file needs a valid library copy too (amends 0066)

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
ADR-0066 counts a quarantined file as accounted for while its prior disposition is exported, covered or
excluded. Excluded was meant for files that were never in the library, but cleanup only moves exported and
covered files, so a quarantined file is excluded only when it became excluded after the move: the user
excluded it (or its group's keeper), or a classifier change set it aside. If its library copy is then deleted
or changed, the rules derive `excluded` for it (an excluded file with a valid copy is still exported or
covered), it stayed `quarantined`, and the drive read 100% accounted for although the file's only copy was in
the quarantine folder. The README tells users that a drive at 100% means every file in its quarantine is also
in their library; deleting the quarantine folder would have lost the photo.

## Decision
- A quarantined file is `quarantined` only while its prior disposition is exported or covered, that is while a
  verified library copy covers it, whatever its exclusion state. Otherwise it is `pending`, with its quarantine
  path, no exclusion reason and no library path, and counts in `CompletionCounts::pending_in_quarantine`
  exactly as in ADR-0066.
- The reason shown for such a file says to export again (including it first if it is excluded) or undo the
  cleanup; export still skips excluded files, so an excluded one stays pending until the user includes it or
  undoes the run.

## Alternatives considered
- **Keep counting excluded quarantined files as safe** - the user's exclusion says "I don't want this in the
  library", not "it may be lost"; an undo must still be able to bring it back, and the report is the only
  guard before a manual delete.
- **Export excluded files that are only in quarantine** - overrides the user's decision; pending already
  tells them.

## Consequences
- "100% accounted for" again implies that every file in the drive's quarantine folder has a valid library
  copy.

## Revisit when
Exclusion gains a "may be deleted" meaning, or cleanup can move excluded files (neither planned).
