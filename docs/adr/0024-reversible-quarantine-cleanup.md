# 0024. Cleanup by reversible quarantine (later phase)

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Priority three: optionally clean up already-scanned backups to reclaim space and to make it obvious what is left.

## Decision
Cleanup moves files whose disposition is `exported` or `covered` - re-verified by re-hashing the source file immediately before the move and confirming the library copy still exists and matches - into `<source root>/.photosquirrel-quarantine/<run id>/<relative path>` on the same volume (a rename, no data copy). A journal (catalog + a JSON file inside the quarantine folder) records every move; **Undo** restores any run fully or partially. Emptying the quarantine is left to the user's file manager; PhotoSquirrel never deletes. Cleanup ships after export and the ledger are proven.

## Alternatives considered
- **Delete to OS trash** - trash on external drives is unreliable and per-OS.

## Consequences
- This is the only feature that changes source trees, and only by reversible renames.

## Revisit when
Never relaxed toward deletion.
