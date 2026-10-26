# 0098. HEIC and AVIF decode failures are retried when transient or after a decoder change (catalog migration 20)

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0091](0091-rows-remember-a-missing-decoder.md) (which unchanged rows a rescan re-decodes),
  [0095](0095-heic-and-avif-decode-failures-keep-the-file.md) (decode failures keep a placeholder)

## Context
ADR-0095 keeps a HEIC or AVIF whose decode fails with a placeholder, and stored it like a real placeholder: no
`decoder_missing` marker, so the stat-fingerprint diff never read it again (ADR-0091 re-decodes only marked rows).
That is right for a corrupt file, but a watchdog timeout, "AV1 decoding is paused", a thread that could not start,
or a later libheif upgrade that can decode the file left it without perceptual hashes (no grouping, no preview)
forever.

## Decision
- **Migration 20** adds `file.decode_failure TEXT` and a partial index: `<key>|<t or p>|<attempts>|<backend>`
  (`heif|p|1|libheif 1.23.3`). Analysis writes it when a HEIC or AVIF decode fails while its decoder is present,
  and clears it otherwise (with the analysis statement, ADR-0008); a filled-in re-decode clears it too.
- **Transient or permanent.** `DecodeError::Interrupted` (timeout, paused, no thread, ADR-0097) is transient;
  every other failure (the decoder rejected the data, a size refusal) is permanent.
- **Retry rule** (the diff, for unchanged `thumbed` rows without a `decoder_missing` marker): re-decode (one full
  read, ADR-0091's re-decode path) when the decoder for its key is available and either its backend differs from
  the recorded one or the failure is transient with fewer than 3 attempts. Attempts count on only for the same
  backend and a transient failure; a new backend starts at 1. A permanent failure with the same backend is not read
  again until the file changes.
- **Backend** is the decoder's name with the version detection found ("libheif 1.23.3", "ImageIO", "WIC
  <version>"), and for AVIF a constant naming rav1d and the wrapper revision, bumped with any change that decodes
  more files.
- Run summaries count these re-decodes (`diff.decode_retries`, also in `diff.redecode`).
- **Compatibility with decoder generations** (`preview-placeholders`, design D1): that change's
  `preview_gen`/`preview_reason` describe placeholders and a per-format generation; this record describes one
  failed decode and its backend. Its fill rule may treat a row with `decode_failure` like an `unreadable`
  placeholder at the current generation; a generation bump retries such rows too. Neither replaces the other.
  That change and `excluded-view-filters` take the next migration numbers (21 and 22).

## Alternatives considered
- **Re-decode every placeholder on every rescan** - re-reads corrupt files forever.
- **Reuse `decoder_missing` for failures** - mixes "waiting for a decoder" (counted and reported as such) with
  "the decoder failed".
- **Retry transient failures without a bound** - a file that always hangs would be read and abandoned on every
  scan.

## Consequences
- One more column; downgrade unsupported (ADR-0005). Rows from before migration 20 carry no record: their
  failures are retried only when the file changes, or by `preview-placeholders`' generation rule.
- Upgrading libheif (or the OS decoder) costs one read of each file that failed before.

## Revisit when
`preview-placeholders` lands (fold both into one "needs work" rule), or failures of other formats are catalogued.
