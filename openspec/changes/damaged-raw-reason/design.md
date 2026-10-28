# Design

## Context

- **The RAW decode.** `ps_media::decode::decode_raw_preview(bytes, probe, min_long)` is the only RAW decode. It is
  reached from `decode_scaled_held`'s `Format::Raw(_)` branch, so the analysis, re-decodes, Fill in previews and the
  large preview all go through it. It tries `probe.preview` first: `range.slice(bytes)`, a JPEG SOI, `decode_jpeg`.
  If any step fails it silently tries rawler inside `util::contain`:
  `get_decoder(&src).ok()?`, then `preview_image(..).ok().flatten()`, then `thumbnail_image(..).ok().flatten()`.
  Every failure becomes `DecodeError::NoRawPreview`.
- **The probe hides the evidence.** `probe::tiff_previews` skips a preview candidate when
  `abs + len > file_size` (`continue`). `probe_raf` records `ProbeError::Corrupt` "RAF preview offset outside the
  file". So a CR2 cut inside its preview has `probe.preview == None`, and nothing downstream knows why.
- **rawler 0.8's errors** (`rawler/src/lib.rs` `RawlerError`) come in two kinds. `Unsupported { what, make, model,
  mode }` is returned by `check_supported_with_everything` ("Unknown camera") and by `get_decoder` for an unknown
  make. `DecoderFailed(String)` covers everything else: I/O errors (`From<io::Error>`, which also logs a backtrace
  at error level), TIFF parse errors, "Missing tag", JPEG errors ("Failed to read JPEG image"), and a panic caught
  in `decode`. Our `contain` also turns panics into `None`: `IFD::singlestrip_data_rawsource` `assert!`s. So rawler's
  own error cannot tell a cut file from a parse quirk. Matching on message text would be fragile.
- **The mapping and the pipeline.** `thumb::reason_for_error`: `NoRawPreview` -> `UnsupportedVariant { "RAW files
  without an embedded preview" }`, `Failed(m)` -> `Unreadable { m }`, `Unsupported(d)` -> `UnsupportedVariant { d }`.
  The analysis in `ps-scan/src/pipeline.rs` has a dedicated `Err(e @ DecodeError::NoRawPreview)` arm (a placeholder,
  no error). The generic arm `Err(e) if probe.display.is_some() || metadata.fields.width.is_some()` reports an
  error unless the error is `Unsupported` or `TooLarge`. The last arm fails the file (`Persist::Failed`). A RAW with
  `Failed` and no dimensions would therefore fail instead of being catalogued, which is why D3 adds a RAW arm.
- **The damaged-still precedent.** ADR-0055 says a damaged JPEG is never hashed, so it gets no grey padding and no
  look-based join. ADR-0109 says a damaged still is catalogued with an `unreadable` placeholder, reported as an error
  and exported as is. ADR-0095 does the same for HEIC/AVIF decode failures.
- **Generations.** `ps_media::decoders::GENERATIONS` has RAW at 0. `ps_catalog::WaitingRule` (`ps-catalog/src/
  previews.rs`) and `ps_scan::diff::outdated_placeholder` treat a thumbed placeholder row
  (`preview_reason IS NOT NULL`) with no `decoder_missing`, no `decode_failure` and `preview_gen < generation of its
  family` as waiting. They apply only to placeholder rows of that family. Rows with real thumbnails have
  `preview_reason NULL` and are never touched. A re-read that fails goes through `Redecode::Undecodable`
  (`pipeline.rs`), which stores the new placeholder at the current generation and always calls
  `ctx.progress.error(..)`.
- **Summaries.** `RunSummary` (`ps-scan/src/report.rs`) has `errors`, which counts this run's `progress.error` calls,
  and `undecoded`, which is filled at the end of the run by one aggregate (`CatalogReader::undecoded_counts`,
  `pipeline::undecoded`). The CLI prints it in `cmd/scan.rs` `print_summary` and `undecoded_lines`. The app's run
  summary is `ui-workspace/src/views/scan_dashboard.rs` (the `kit::stat` row and the notices), fed by
  `ps_service::scans::RunOverview` (`scans/summary.rs` `run_overview`).
- **The completion report.** `ps-export/src/disposition.rs`: `unreadable` means failed after retries
  (`ps-service/src/export.rs` `derive`: `f.stage == Failed && f.attempts >= 3`). A catalogued damaged still has
  content identity, so it is grouped by exact content and exported as is. It is `pending`, then `exported`, and
  nothing in the report says it is damaged. `Badge::label` reads "100% accounted for" or
  "N pending, ..., M unreadable". The CLI report lives in `cmd/export.rs` (`report_out`, `print_report`) and the
  app's in `ui-workspace/src/views/reports.rs`.
- **The fixture.** `crates/photosquirrel-cli/tests/decoder_logs.rs` `truncated_card()` writes the first eighth of
  `ps_fixtures::dng::cr2_unknown_lens(11)`. The directories and maker note are kept and the preview JPEG is cut.
  `scenario_scan_progress_damaged_raw_file_still_reported_under_the_default_filter` pins `unsupported_variant` today.
  The intact `cr2_unknown_lens` deliberately carries an IFD0 private tag (0xC5D8) whose count runs past the end of
  the file (quiet-decoder-logs, Deviations). A truncation check must not treat it as evidence.

## Decisions

### D1. Truncation is judged from the file's own structure, by our code

New `ps_media::raw_check::truncation(bytes, probe) -> Option<Truncation>`, where `Truncation { file_size, needed,
what }`. It runs at the top of `decode_raw_preview` over the full buffer, and only when
`bytes.len() as u64 == probe.file_size`. A partial buffer is never judged. It looks only at ranges a reader must
have:

- **TIFF-based RAWs** (CR2, NEF, NRW, ARW, SR2, DNG, ORF, PEF, RW2, ERF, KDC, DCR, MEF, IIQ, 3FR, SRW): it walks the
  IFD0 chain, SubIFDs (0x014A) and the EXIF IFD pointer with the probe's `crate::tiff` parser. The evidence is an
  IFD offset past the end, or a strip, tile or JPEG range that ends past it: `StripOffsets`/`StripByteCounts`
  (0x0111/0x0117), `TileOffsets`/`TileByteCounts` (0x0144/0x0145), and `JPEGInterchangeFormat`/`Length`
  (0x0201/0x0202). Both tags must be present, and a count of 0 is ignored. Maker notes and the values of other tags
  are not evidence. That covers the 0xC5D8 tag of the intact `cr2_unknown_lens`, and the private tags real cameras
  write.
- **CR3** (BMFF): it walks the top-level boxes with `crate::bmff`. A box whose header-declared size ends past the
  file is evidence; the usual cut is `mdat` cut short. A `size = 0` box (it runs to the end) is not.
- **RAF**: the header's JPEG and CFA offset and length pairs.
- Other RAW formats (MRW, X3F, CRW/CIFF, ...) get no check and fall through to D2.

Any byte past the end counts, with no tolerance. A false positive mislabels a good photo as damaged. That is
visible and loses nothing, because the file is still catalogued and exported as is. A false negative is the
priority-1 bug. False positives are guarded against by tests over every `ps-fixtures` RAW writer, the read-only
sample folder and the owner's Canon card (tasks 1.3, 6.4). `what` names the range ("image data", "preview",
"directory"). The message reads "the file ends at 3.1 MB but its image data runs to 24.6 MB: it was cut short (a
card removed or a copy stopped while writing)".

**Rejected: classify by rawler's error.** `DecoderFailed` holds I/O errors and parse quirks alike, as strings. An
`assert!` panics instead of returning an error. And when the probe's preview decodes, rawler is never called, so a
cut sensor strip would never be seen.
**Rejected: record it in the probe.** The probe sees only the first 256 KB and cannot walk CR3 boxes beyond it. Its
`ProbeRecord` is not stored, so the decode would recompute it anyway.

### D2. The outcomes of `decode_raw_preview`, in order

1. `truncation` is `Some`: `DecodeError::Failed(message)`. This happens even when the preview would decode: the
   photo is the sensor data, and a cut RAW must not look fine.
2. The probe's preview decodes: `Ok`, as today.
3. The probe's preview lies in the file, starts with SOI, and `decode_jpeg` (strict zune, then jpeg-decoder) fails,
   and rawler gives no image either: `Failed(jpeg error)`. The embedded JPEG is damaged, as with a damaged JPEG
   file (ADR-0055).
4. rawler: `get_decoder` returns `Err(Unsupported { make, model, .. })`: `Unsupported("RAW files from this camera
   (Canon EOS R100)")`. This is really not supported, and a newer rawler may add the camera.
5. rawler: a decoder, and both `preview_image` and `thumbnail_image` return `Ok(None)`: `NoRawPreview`, unchanged
   ("RAW files without an embedded preview").
6. rawler: any other `Err` (`DecoderFailed`) or a caught panic, with no truncation evidence:
   `Unsupported("RAW files whose preview this version cannot extract: {message}")`. Nothing shows this file is
   damaged, so it is not called damaged. The message is kept, so a bug report has it.

The `contain` closure returns a small enum (image, unsupported camera, no preview, failed with a message) instead
of `Option`, so no outcome collapses into another. `thumb::reason_for_error` keeps its mapping. Only the detail
texts are new.

### D3. One RAW arm in the analysis

`ps-scan/src/pipeline.rs`: the `Err(e @ DecodeError::NoRawPreview)` arm becomes `Err(e) if probe.format.is_some_and(
Format::is_raw)`. Every RAW decode error makes a placeholder (`placeholder_of`), whether or not the probe found
dimensions. Only `Failed` is reported through `ctx.progress.error(..)`, with "could not be read (...); it is
catalogued with a placeholder and exported as is", the text of the generic arm. A RAW then never reaches
`Persist::Failed`, so a cut CR2 is never an "analysis failed" row retried twice and then `unreadable` in the report
with no identity. It keeps its identity, its metadata (whatever the probe and nom-exif read) and its exact-copy
grouping. The arm sits before the HEIC/AVIF arm and the generic arm and changes neither.

No perceptual hash, as for every damaged still (ADR-0055, ADR-0109). This matters here: with a hash, an intact
preview of a cut CR2 would join its camera JPEG by look, and the RAW would outrank it as keeper. Without one, the
RAW+JPEG companion rule (`ps-dedup` `edges::raw_jpeg_companions`) applies only inside a group, so the damaged CR2
stays its own group. Its intact JPEG keeps its own keeper and export. No `ps-dedup` change.

### D4. The RAW generation goes to 1: one re-read of RAW placeholders only

`GENERATIONS` RAW 0 -> 1, the `decoder_generations_of_this_build` test and `docs/decoder-generations.md` change
with it. ADR-0115 widens ADR-0109's rule: a family is also bumped when a change corrects the reason its decoder
gives, because the re-read writes the corrected reason.

- What is re-read: rows with `preview_reason IS NOT NULL`, a RAW format, `preview_gen 0`, and no `decoder_missing`
  or `decode_failure` (RAW rows never have either: `failure_record` is HEIC/AVIF only). That is every RAW
  placeholder made before this version, each once, by the next rescan of its source (counted in
  `DiffCounts::outdated_previews`) or by Fill in previews. RAW files with a thumbnail, and every other format, are
  not opened. A RAW placeholder over 64 MB, or one without dimensions, takes `needs_full_analysis` (analysed again
  from `discovered`). It is one read, and it ends at generation 1 again.
- No catalog change is needed to find them: the `file_preview (preview_reason, format)` index and the waiting rule
  exist (migration 24). Before 0.3.0 a RAW without a preview was an analysis failure, and failures are retried
  through the stage rules anyway.
- **Rejected: a migration that resets `preview_gen` of `unsupported_variant` RAW rows.** It is the same effect with
  a migration number, and it would need `adobe-rgb-color` (migration 26) to land first. A generation bump is
  ADR-0109's own tool for exactly this.
- **Rejected: re-read only rows whose detail is "RAW files without an embedded preview".** The detail is in
  `thumbs.db`, not in the row. Every RAW placeholder of 0.3.0 has that detail or "files over 64 MB" anyway.

### D5. A re-read that is still "not supported" is not an error

`Redecode::Undecodable` always reports an error today. For a RAW, it reports one only when the new placeholder's
reason is `Unreadable`, the analysis rule of D3. Otherwise the generation bump would turn every unknown-camera CR2
on a card into a run error once. HEIC and AVIF keep today's behaviour (ADR-0095, ADR-0098), and their tests are
unchanged. The Fill summary's `failed` count is unchanged: a re-read that made no preview is still `failed`.

### D6. The run summary counts damaged files from the catalog

`RunSummary.damaged: u64` (`#[serde(default)]`, so old `stats` JSON reads as 0). It is the number of present files
of the run's sources with `preview_reason = 'unreadable'` at the end of the run, from one aggregate
`CatalogReader::damaged_count(sources)` over the `file_preview` index, beside `undecoded` in `pipeline::summary`.
That covers damaged JPEGs, TIFFs, HEICs and AVIFs too: "damaged" means the catalog's `unreadable` placeholder
reason, whatever the format. Files that failed analysis after retries stay in `stages.failed`.

`errors` stays this run's errors. A first scan of a card with one cut CR2 reports `errors 1, damaged 1`. An
unchanged rescan reports `errors 0, damaged 1`, so the warning does not vanish on the second scan, which is the
scan a user runs before erasing. The count is an aggregate, never per-file events (ADR-0019).

- **CLI** (`cmd/scan.rs` `print_summary`): after the main line, when `damaged > 0`: "1 damaged file could not be
  read (cut short or corrupt). It is catalogued and copied as is; keep the original card or drive until you have
  checked it. List: `photosquirrel-cli report`". `--json` carries `damaged`.
- **App** (`scan_dashboard.rs`): `RunOverview.damaged` (`scans/summary.rs`, the same aggregate, in both branches of
  `run_overview`). It adds a `kit::stat("summary-damaged", "damaged", ..)`, and when it is above 0 a notice with
  `colors.warning`, the same text and a "Show" button that opens the Library filtered to No preview, reason "could
  not be read", for those sources (`no_preview_filter`). Ids and aria labels follow the `summary-undecoded-*`
  notices.

### D7. The completion report flags damaged files; dispositions unchanged

The owner chose on 2026-10-28: keep the badge with ", N damaged", and cleanup never moves a damaged original
(D10).

A damaged file is copied as is, so its bytes are as safe in the library as on the card, and its disposition stays
what the rules say. What the report owes the user is the warning that the photo itself may be incomplete, and that
the card's free space may still hold the rest.

- `FileFacts.damaged: bool` (`ps-service/src/export.rs` `derive`: `f.preview.is_some_and(|p| p.reason ==
  PreviewReason::Unreadable)`). `CompletionCounts.damaged: u64` is counted per source and folder and merged like
  `pending_in_quarantine`. `fully_accounted()` does not read it.
- `Badge::label` appends ", N damaged" to both variants: "100% accounted for, 1 damaged" and
  "2 pending, 1 damaged". `ps_export::DAMAGED_HINT` reads "copied as is, but the photo may be incomplete (cut short or
  corrupt). Keep the original card or drive until you have checked them: a recovery tool may still find the rest".
- CLI `report`: a line per source with damaged files, "    1 damaged file: {hint}", then up to 20 paths and
  "and N more". `--json`: `counts.damaged` and `damaged_files` (every path) per source.
- Reports view: the same line on the source card, with a drill-down to the gallery's No preview filter for that
  source and reason.

### D8. The dialog's sentence for an outdated placeholder

`ps-service/src/previews.rs` says "This version of PhotoSquirrel reads {label} files that an older version could
not, so a preview can be made now." After D4 that is false for most RAW placeholders: an unknown camera stays
unsupported, and a cut file becomes damaged. The new wording is "This version of PhotoSquirrel reads {label} files
better than the version that made this placeholder, so it can look at this file again." The button
(Fill in previews) is unchanged.

### D9. Fixtures and tests

- `ps_fixtures::dng::cut(bytes, Cut)` with `Cut::InPreview`, the existing first-eighth cut, and
  `Cut::AfterPreview`, which keeps the whole preview JPEG and cuts halfway through the sensor strip. For CR2-like
  writers the sensor strip comes after the preview. `ps-fixtures` has no CR3 or RAF writer, so the CR3 (`mdat`
  past the end) and RAF (CFA past the end) checks are `raw_check` unit tests over minimal hand-built headers in
  `ps-media`. The scenarios use CR2.
- Unit tests in `ps-media` for each D2 outcome. "Unknown camera" uses a CR2-like file with a make and model missing
  from rawler's camera table and no probe preview.
- The existing CLI test switches its pin from `unsupported_variant` to `unreadable`, `errors 1`, `damaged 1`.

### D10. Cleanup keeps damaged originals (owner's choice)

The cleanup planner (ADR-0096/0099 re-derive coverage before each move) skips a file whose `FileFacts.damaged` is
true and lists it as kept with the reason "damaged: keep the original for recovery". The run re-checks it before the
move like every other condition, so a plan made before the file was found damaged does not move it either. The
damaged file's disposition in the report is unchanged; only cleanup leaves it in place.

## Risks / Trade-offs

- [A real camera writes a strip count past the end of a good file] -> it shows as damaged with no thumbnail. It is
  still catalogued and exported, and nothing is lost. Guards: the owner's 124-file Canon card and the sample folder
  must show 0 damaged (6.4). A camera found to do this gets a narrow exception (make and tag) and a fixture.
- [A cut RAW loses its intact preview in the grid] -> accepted for now. It shows a damaged tile with the reason, and
  the bytes, preview included, are exported. Showing the preview needs a new thumbnail kind (owner question 2).
- [The generation bump re-reads every 0.3.0 RAW placeholder once] -> usually a handful per card. RAWs over 64 MB
  are analysed again once. It is bounded, and it happens once per row.
- [Mislabelling the other way: a corrupt-but-complete RAW with no rawler preview stays "not supported"] -> without
  evidence we do not claim damage (D2.6). rawler's message is kept in the detail.
- [`errors` and `damaged` disagree on a rescan] -> intended (D6). The text says what each means.

## Owner decisions (2026-10-28)

1. Keep the "100% accounted for" badge with ", N damaged" and the hint; cleanup never moves a damaged original
   (D10).
2. Showing the intact embedded preview of a cut RAW, marked damaged, comes later as its own change (ROADMAP row 34).
