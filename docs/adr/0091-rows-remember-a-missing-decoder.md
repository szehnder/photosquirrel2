# 0091. Rows remember a missing decoder; rescans re-decode them (catalog migration 19)

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0007](0007-incremental-rescan-by-stat-fingerprint.md) (unchanged files can be processed again)

## Context
A file analysed without a decoder is `thumbed` with a placeholder and no perceptual hashes. The stat-fingerprint diff
never revisits it, so installing a decoder would change nothing for files already catalogued, and old catalogs have
no record of why a file has no hashes.

## Decision
- Migration 19 adds `file.decoder_missing TEXT` (the decoder's key, `heif` or `avif`) and a partial index; it
  back-fills `thumbed` HEIC and AVIF rows with a full identity and no `phash`.
- Analysis sets the column when the decode reports a missing decoder and clears it otherwise.
- The diff queues an unchanged, marked row for a **re-decode** when the run's decoder registry can decode it (one full
  read, ADR-0010), and otherwise counts it as awaiting a decoder without reading it. No opt-in is needed.
- A re-decode recomputes the identity (a mismatch is handled as a changed file), writes hashes, clears the marker,
  bumps the dedup stamp and replaces the thumbnail; stage, class, metadata and dates are untouched.
- A re-decode never fails an analysed file (as ADR-0082): a read error keeps the row and the marker; a decode error
  clears the marker and keeps the exact-copy analysis; both are reported.
- Run summaries carry per-format counts of rows still awaiting a decoder, from one aggregate query.

## Alternatives considered
- **Opt-in like `--refresh-analysis`** - the user who installed a decoder expects previews; the work is bounded to the
  files without one.
- **Treat `phash IS NULL` as the marker** - also true of RAW files without a preview and of oversized files; cannot say
  which decoder to wait for.
- **Re-analyse marked rows from scratch (reset to discovered)** - would fail the row on a read error and lose its
  group during the run.

## Consequences
- One more column; downgrade unsupported (ADR-0005). Installing a decoder costs one read of each affected file on the
  next rescan.

## Revisit when
More per-row "needs work" reasons accumulate (then a general pending-work table).
