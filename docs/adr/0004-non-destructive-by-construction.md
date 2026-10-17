# 0004. Non-destructive by construction

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The collection contains irreplaceable family photos. The single worst outcome is losing one. Earlier attempts mixed "find" and "clean" and made the user nervous.

## Decision
Source trees are **read-only** to the application:
1. The scanner opens source files read-only and never writes into source trees (no marker files, no sidecars, no thumbnails next to files).
2. The primary output is **copy-only export** into a separate library folder; every copy is written to a temporary name, fsynced, re-hashed against the source hash and only then renamed into place (ADR-0022).
3. No code path deletes a user file. The later cleanup feature (ADR-0024) only *moves* files that the ledger proves are safely exported or duplicates of exported files, into a quarantine folder on the same volume, with a journal that can undo every move.
4. Destructive-adjacent operations always show a dry-run plan first and require explicit confirmation.
Source code enforces this: file-system mutation of user paths is only allowed in `ps-export` (copy, quarantine move, undo), and a test asserts the scanner opens no file for writing.

## Alternatives considered
- **Delete duplicates to trash** - rejected; space savings do not justify any risk.
- **Hard-link dedupe in place** - modifies sources, filesystem-specific.

## Consequences
- The user can always re-run safely; the worst case of a bug is a wasted copy, not a lost photo.
- Freeing space is a separate, later, explicit step.

## Revisit when
Never relaxed. Extensions must keep the copy-verify-rename and journal guarantees.
