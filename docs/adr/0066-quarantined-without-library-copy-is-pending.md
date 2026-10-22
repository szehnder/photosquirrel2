# 0066. A quarantined file without a valid library copy is pending (amends 0034)

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
ADR-0034 counts a quarantined file as accounted for. If the library copy that made it safe is later deleted
or changed, the file's only copy is in the quarantine folder, yet the completion report still said "100%
accounted for" - the very report that tells users the quarantine folder can be emptied. Export could not
repair it either: it read the keeper from its original path and failed.

## Decision
- A quarantined file is `quarantined` (accounted for, with its prior disposition) only while that prior
  disposition is exported, covered or excluded. Otherwise it is `pending`, with its quarantine path, and is
  counted in `CompletionCounts::pending_in_quarantine`; the badge, the CLI report and the drill-down say that
  its only copy is in the quarantine folder and that export copies it again or undo puts it back.
- The export input reads such a keeper from its quarantine path (`KeeperFile::read_from`), read-only like
  every source read; the file stays in quarantine. Once its library copy is back it is `quarantined` again.
- PhotoSquirrel never empties or deletes a quarantine folder (ADR-0024); the README tells users to run a
  verified report first.

## Alternatives considered
- **A separate "quarantined, uncovered" disposition** - every disposition consumer would need it; `pending`
  already means "not safe yet" and drives export.
- **Refuse export until undo** - leaves the user to discover why; reading the quarantined copy is safe.

## Consequences
- The completion report never calls a drive accounted for while a file's only copy is in its quarantine.

## Revisit when
Cleanup supports deleting quarantined files (never planned, ADR-0024).
