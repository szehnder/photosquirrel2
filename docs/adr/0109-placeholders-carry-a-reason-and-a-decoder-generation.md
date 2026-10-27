# 0109. Placeholders carry a reason and a decoder generation

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0055](0055-corrupt-jpegs-fall-back-to-jpeg-decoder.md) (a damaged still is catalogued with a
  placeholder, not failed), [0091](0091-rows-remember-a-missing-decoder.md) (rescans also re-decode outdated
  placeholders), [0007](0007-incremental-rescan-by-stat-fingerprint.md) (unchanged rows re-read once after an
  update), [0016](0016-thumbnail-store.md) (a stored placeholder is replaced by a later analysis)

## Context
The grid drew a file without a thumbnail as a flat grey square. 0.2.0 on Windows could not read TIFFs whose
directory sits at the end of the file, and the owner's library showed rows of blank tiles with nothing saying why.
HEIC files without a system decoder, videos without a poster-frame tool and damaged files looked the same. A user
cannot tell "this computer needs a library", "this version cannot read this kind of file", "the file is damaged" and
"a preview is possible now" apart, and has no cheap way to fill previews in after installing a decoder or updating:
a rescan walks every source, and rows thumbed by an older version are never retried because their stat fingerprint
has not changed. A damaged JPEG was recorded as an analysis failure (ADR-0055), so it was neither grouped by exact
content nor exported.

## Decision
- **Every placeholder says why.** `PlaceholderReason` gains `UnsupportedVariant { detail }` (a layout of a
  recognised format this version does not decode: BigTIFF, a RAW without an embedded preview, a decode too large for
  bounded memory), `Unreadable { message }` (the decoder failed on the data; the error is kept) and
  `NeedsNewerVersion` (`#[serde(other)]`: a reason written by a newer version that this one does not know).
  `ps_media::thumb::reason_for_error` maps decode errors to reasons; `Interrupted`, `Refused` and `Cancelled` say
  nothing about the file and make no placeholder.
- **A damaged still is catalogued** (amends ADR-0055): a recognised still with dimensions whose decode fails keeps
  its identity and metadata, is thumbed with an `Unreadable` placeholder, is never hashed (ADR-0055's point stands:
  no grey padding, no look-based join), is reported as an error and exported as is. A still the probe gave no
  dimensions still fails, as before. BigTIFF is recognised by the probe (its first directory's dimensions) and
  refused by the decoder with `Unsupported("BigTIFF")`, also through the generic decoder fallback.
- **Decoder generations.** `ps_media::decoders::GENERATIONS` holds a `u16` per decoder family (`DecodeFamily`:
  JPEG, PNG with GIF and BMP, TIFF, HEIF, AVIF, WebP, RAW, video poster). A family's generation is bumped by any
  change that makes files of it decodable that were not before. This release: TIFF 1 (`tiff-support`), HEIF 1 and
  AVIF 1 (`heic-system-decoders`), every other family 0. `PlaceholderInfo.generation` (serde default 0) records the
  generation of the run that made the placeholder; `ScanOptions::generations` carries them, so tests simulate an
  update. A unit test lists the constants; `docs/decoder-generations.md` says when to bump.
- **Catalog migration 24** (`0024_preview_gen.sql`): `file.preview_gen INTEGER` and `file.preview_reason TEXT`
  (`decoder_missing`, `no_poster_tool`, `sequence`, `unsupported_variant`, `unreadable`, `needs_newer_version`),
  NULL for rows with a real thumbnail, written in the analysis statement (ADR-0008) and cleared when a re-decode
  stores pixels; a partial index `file_preview (preview_reason, format) WHERE preview_reason IS NOT NULL`;
  `scan_run.kind` (`'scan'` or `'fill'`) and `scan_run.fill_scope` (JSON), so a resumed job fills the same scope.
  **Backfill, exact rule:** rows with `stage = 3 AND phash IS NULL AND media_kind = 0 AND content_id IS NOT NULL`
  (a thumbed still without perceptual hashes has a placeholder: migration 19's signal) get `preview_gen = 0` and a
  reason: `decoder_missing` when `decoder_missing` is set, `unreadable` when `decode_failure` is set,
  `unsupported_variant` for a RAW format or a sampled identity (`identity_kind <> 0`, too large to decode), else
  `unreadable`. Videos have no perceptual hashes with or without a poster frame, so their rows cannot be told apart
  in SQL: they are not marked; their tiles and the dialog take the reason from the stored placeholder record, and
  the video poster generation is 0, so none of them waits. Old placeholders and old failures are both retried once
  at generation 0 and then carry the current generation and their real reason.
- **The waiting rule** (`ps_catalog::WaitingRule`, one SQL predicate and the same rule in Rust): a present, thumbed
  placeholder row can be filled in now when it waits for a decoder this machine has (ADR-0091), or when it has no
  decoder marker and no recorded decode failure (those follow ADR-0091 and ADR-0098) and its `preview_gen` is below
  the running version's generation of its format's family, unless it is a video waiting for a poster-frame tool and
  none is configured. Current-generation `unsupported_variant` and `unreadable` rows wait for a changed file or a
  generation bump.
- **Rescans** (amends ADR-0007, ADR-0091): an unchanged outdated placeholder is re-decoded once (a full read, the
  identity checked) beside the `decoder_missing` rule, or analysed again from `discovered` when it has no
  dimensions, is larger than the analysis buffer (64 MB) or is a video. Counted as `DiffCounts::outdated_previews`.
  A failed re-decode stores the new placeholder with its reason at the current generation, so it is not read again.
- **Fill in previews** is a scan run with `kind = 'fill'` (`Scanner::start_fill(scope, sources)`): its work list is
  the waiting rows within the scope (all, a format or a reason) from the catalog, in path order per source; no
  folder is listed. Rows on sources offline when it starts are counted (`FillSummary::skipped_offline`) and left
  waiting. It uses the scan's lanes, stop checks, sampled progress (`ProgressSnapshot::fill`), pause, resume and
  cancel, the re-decode writer and the regroup at the end. Its summary is `{filled, skipped_offline, failed,
  still_waiting}`. A cancel keeps what was written; the rest stays waiting.
- **The thumbnail store** (amends ADR-0016): an analysis replaces a stored placeholder of the same content (a preview
  now, or a placeholder that says why at this generation); stored pixels are never replaced by a placeholder.

## Alternatives considered
- **Keep damaged stills as failures** - they would stay outside exact-copy groups and the export, and the dialog
  could not explain them; a placeholder keeps ADR-0055's safety (no hashes) and loses nothing.
- **Retry every placeholder on every rescan** - reads every undecodable file again on every run, forever.
- **A version string instead of per-family generations** - every release would re-read every placeholder, although
  most releases make no new file decodable.
- **Join the thumbnail store in the migration** - `thumbs.db` is a separate database; the file row's hashes are the
  signal migration 19 already relied on.
- **Fill in through a rescan of the affected sources** - walks every folder; the job reads only the waiting files.

## Consequences
- A change that makes files of a family decodable must bump that family's generation; a forgotten bump only means
  old placeholders wait for a changed file or a manual re-analysis. Nothing is lost.
- Damaged JPEGs and BigTIFFs are now in the Library with placeholders and are exported; counts of failed files go
  down by them.
- The first rescan (or Fill in previews) after the upgrade re-reads every pre-0.3.0 TIFF, HEIC and AVIF placeholder
  once.

## Revisit when
A format's placeholders can be decoded by a tool outside the app (then the poster-frame rule generalises), or the
thumbnail store moves into the catalog (then the backfill can read the stored kind).
