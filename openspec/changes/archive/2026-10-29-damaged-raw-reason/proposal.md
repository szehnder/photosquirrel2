# Proposal

## Why

A gate review of `quiet-decoder-logs` found that PhotoSquirrel calls a damaged RAW file "not supported yet". A CR2
that was cut short (the card was pulled mid-write, the power failed, or a copy stopped) goes this way:

- `ps_media::decode::decode_raw_preview` (`crates/ps-media/src/decode.rs`) tries the probe's preview first. The probe
  (`probe::tiff_previews`) drops any preview whose range runs past the end of the file, so the RAW has none.
  rawler is tried next. Every rawler outcome, whether `get_decoder` failed, `preview_image` returned an error, it
  returned `Ok(None)` or it panicked, collapses into `.ok()?` and becomes `DecodeError::NoRawPreview`.
- `thumb::reason_for_error` (`crates/ps-media/src/thumb.rs`) maps `NoRawPreview` to
  `UnsupportedVariant { detail: "RAW files without an embedded preview" }`.
- The pipeline's `Err(e @ DecodeError::NoRawPreview)` arm (`ps-scan/src/pipeline.rs`, analysis) makes the placeholder
  and reports no error.
- The "Why no preview?" dialog (`ps-service/src/previews.rs`) offers `PreviewAction::UpdateLater`. The tile says
  "This CR2 file is a variant this version of PhotoSquirrel does not decode", and `previews status` says
  "not supported yet".
- The scan summary says `errors: 0`. `RunSummary` (`ps-scan/src/report.rs`) has no count of damaged files, and the
  completion report counts the file as pending, then as exported. A source can read "100% accounted for" with a cut
  photo in it.

A user who reads that will believe the photo is fine and only needs a newer version, and may then erase the card.
On a card pulled mid-write, the rest of the file may still be in the card's free space and recoverable with a
recovery tool, but only until the card is formatted or reused. Priority 1 ("never lose a photo") makes this the
next change. The same mistake hides a subtler case: a CR2 whose embedded JPEG is intact but whose sensor data is
cut. Today it shows a perfect thumbnail. It is just as damaged.

## What Changes

- **A RAW file cut short is damaged.** Before it decodes, `decode_raw_preview` checks the whole buffer for our own
  evidence of truncation. That is a strip, tile, preview or IFD that the file's structure records but that ends past
  the end of the file (for TIFF-based RAWs, CR3 and RAF). When the check finds it, the result is
  `DecodeError::Failed` with a plain message ("the file ends at 3.1 MB but its image data runs to 24.6 MB: it was
  cut short"), and the reason is `unreadable`. This holds even when the embedded preview is intact.
- **The other rawler outcomes are told apart, not all called damaged.** An unknown camera (rawler `Unsupported`)
  stays not supported and names the camera. A file that has no embedded preview stays not supported. A rawler
  decoder failure with no evidence of truncation also stays not supported, with rawler's message, because nothing
  shows the file is damaged. An embedded JPEG that lies inside the file but fails to decode counts as damaged, as a
  damaged JPEG does (ADR-0055, ADR-0109).
- **The pipeline handles RAW decode errors in one arm.** A damaged RAW is catalogued with its identity, metadata
  and an `unreadable` placeholder. It is never hashed, so it cannot outrank its intact camera JPEG. It is reported
  as a run error and exported as is, whether or not the probe found its dimensions. "Not supported" outcomes make no
  error, as before.
- **Old rows are re-classified once.** The RAW decoder generation goes from 0 to 1 (ADR-0109's mechanism, widened by
  ADR-0115 to cover a change that corrects a reason). The first rescan or Fill in previews after the update re-reads
  each RAW placeholder of generation 0 once: RAW placeholders only, nothing else. A re-read that is still "not
  supported" is not reported as an error.
- **The scan summary counts damaged files.** `RunSummary.damaged` counts the files of the run's sources that are
  catalogued as damaged (`preview_reason = 'unreadable'`) at the end of the run. It comes from one aggregate over
  the existing `file_preview` index. The CLI prints a line after the counts, `--json` carries it, and the app's run
  summary shows a "damaged" stat and a warning notice. An unchanged rescan still shows the count, because it comes
  from the catalog and not from this run's errors.
- **The completion report flags damaged files as at risk.** Dispositions do not change, because a damaged file is
  still copied as is. `CompletionCounts.damaged` counts them per source and folder. The badge label adds
  ", 1 damaged". The text report says to keep the original card or drive until the files are checked, and lists
  their paths (all of them with `--json`). The Reports view shows the same line and drills down to them.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `thumbnails`: damaged RAW files are told apart from unsupported ones.
- `incremental-rescan`: RAW placeholders are re-classified once after the update.
- `scan-progress`: the run summary counts damaged files; the damaged RAW scenario now pins `unreadable`.
- `cli`: the scan summary line and `--json` carry the damaged count.
- `completion-report`: damaged files are counted and flagged as at risk.
- `cleanup`: damaged originals are never planned or moved (the owner's choice).

## Impact

- `ps-media`: `decode.rs` (`decode_raw_preview`), a new `raw_check.rs` (truncation evidence over the full buffer,
  reusing `tiff.rs` and `bmff.rs`), `thumb.rs` (`reason_for_error` unchanged for `Failed`; new detail texts),
  `decoders.rs` (`GENERATIONS` RAW 1, the generation test).
- `ps-scan`: `pipeline.rs` (one RAW arm in the analysis; error reporting in the `Redecode::Undecodable` arm),
  `report.rs` (`RunSummary.damaged`).
- `ps-catalog`: `previews.rs` or `read.rs` (`damaged_count(sources)`).
- `ps-service`: `scans/summary.rs` (`RunOverview.damaged`), `export.rs` (`FileFacts.damaged`), `previews.rs` (the
  generation-outdated sentence).
- `ps-export`: `disposition.rs` (`CompletionCounts.damaged`, `Badge` label, `DAMAGED_HINT`).
- `ps-model`: `disposition.rs` (`CompletionCounts.damaged`).
- `photosquirrel-cli`: `cmd/scan.rs` (`print_summary`), `cmd/export.rs` (report text and JSON).
- `ui-workspace`: `views/scan_dashboard.rs` (stat and notice), `views/reports.rs` (damaged line and drill-down).
- `ps-fixtures`: a truncation helper with two cuts (inside the preview; after the preview, inside the sensor data).
- Docs: `docs/decoder-generations.md` (RAW 1, and a re-classification is a reason to bump), release notes.
- **New ADR-0115** "A RAW file cut short is damaged; damaged files are counted and flagged" (amends 0109, 0023).
  **No migration**: `file.preview_reason` and the `file_preview` index exist (migration 24), and the new summary and
  report fields are serde-default JSON. This change does not depend on `adobe-rgb-color` (migration 26).

## Out of scope

- Showing the intact embedded preview of a damaged RAW. That needs a thumbnail that is both pixels and damaged,
  which is a new thumbnail kind and a catalog column. The owner wants it later, as its own change (ROADMAP row 34).
- Truncation evidence for RAW files over the 64 MB analysis buffer (they get a "files over 64 MB" placeholder
  without a decode) and for RAW formats other than TIFF-based, CR3 and RAF (MRW, X3F, CRW).
- Repairing or recovering files. PhotoSquirrel never writes to sources.
- Holding back the "100% accounted for" badge for damaged files (the owner chose to keep it, with the damaged count).
- Truncation checks for plain TIFF, PNG or WebP. Their decoders already fail on cut data, and the failure becomes
  `unreadable`.
