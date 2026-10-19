# 0040. Post-rename verification and `suspect` ledger entries

- Status: accepted
- Date: 2026-10-19
- Deciders: maintainers

## Context
ADR-0038 promises that "the ledger plus post-rename verification would detect the replaced file", but the
copier did not verify after the rename, and a failed folder fsync after a successful rename marked the file
`failed`: the next export then copied it again as "IMG_1234 (2).JPG". Separately, the planner took a ledger row
plus a same-size library file as "already exported" without checking the bytes.

## Decision
1. After the no-replace rename the file is in place and verified. A failed folder fsync is logged as a
   warning; the copy is recorded `done`.
2. Post-rename verification: the final file is re-opened, re-stat'ed and re-hashed once (BLAKE3). On a
   mismatch the file is **kept** (never deleted), its ledger row gets the new state **`suspect`** (code 3,
   catalog migration 8), the export report lists it and the CLI warns loudly and exits non-zero. A suspect row
   does not count as exported; the next export copies the photo again under a free name.
3. "Already exported" requires the ledger row, a library file of the same size **and** a matching content
   hash; otherwise the name counts as taken and the keeper is copied under the next free name.

## Alternatives considered
- **Remove the sentence from ADR-0038** - leaves a replaced file undetected.
- **Delete or quarantine a suspect file** - violates ADR-0004; the user decides.
- **Size-only adoption (status quo)** - can count a different same-size file as the user's photo.

## Consequences
- One extra read of every copied file (from the page cache in practice) and a hash of every already-exported
  file when re-planning an export into an existing library: re-plans are I/O-bound on large libraries.
- New ledger state and a table rebuild in migration 8 (rows and indexes kept).

## Revisit when
Re-planning large libraries is too slow: cache (path, size, mtime) -> verified hash in the ledger.
