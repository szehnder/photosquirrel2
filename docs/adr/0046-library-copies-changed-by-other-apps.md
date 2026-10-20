# 0046. A library copy changed by another app stays the export of its photo

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
The library folder is meant to be imported by Apple Photos, Google Photos or Lightroom, and some of them write
into the files they find there (Lightroom's "write metadata to file", tagging tools, an editor saving in place).
The size of such a copy changes. The planner then saw a ledger row whose copy no longer matched and planned the
keeper again; the original name was taken (by the changed file), so every re-export added `IMG_0042 (2).JPG`, and
the next app run could change that one too. The library filled with duplicates of exactly the photos the user had
worked on.

## Decision
- A `done` ledger row whose copy is still at its recorded path, as a regular file, but with another size (or, when
  checked, other bytes) is **changed in the library**. The planner lists it (`Plan::modified`, CLI dry run, the
  Export view) and does not copy the keeper again. Nothing is overwritten or removed (ADR-0004).
- Copying it again is an explicit choice: `export --recopy-modified` or "Copy them again" in the Export view makes
  a fresh verified copy under a free name beside the changed file.
- A copy that is gone (deleted, moved away) is not "changed": its keeper is planned again, as before.
- Completion reports are unchanged and stay conservative: the changed copy is no longer the verified bytes, so the
  keeper and its group are pending and the copy is listed as a mismatch ("changed in the library, probably by
  another app"). Cleanup (ADR-0024) can therefore never rely on a copy another program rewrote.

## Alternatives considered
- **Copy again automatically** (the old behaviour) - silent duplicates on every export.
- **Count the changed copy as exported** - the report would vouch for bytes PhotoSquirrel never verified.
- **Update the ledger to the new bytes** - would make "exported" mean "whatever is in the library now".

## Consequences
- After an app rewrites copies, the Export view shows "N changed in the library" and the report shows them pending
  until the user chooses to copy them again (or accepts them by exporting nothing).

## Revisit when
Users routinely let their photo app write into the library; then a per-library "the app may edit copies" setting
could count changed copies as exported.
