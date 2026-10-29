# 0115. A RAW file cut short is damaged; damaged files are counted and flagged

- Status: accepted
- Date: 2026-10-28
- Deciders: maintainers
- Amends: [0109](0109-placeholders-carry-a-reason-and-a-decoder-generation.md) (RAW decode outcomes; a generation
  is also bumped when a change corrects a family's placeholder reason), [0023](0023-disposition-ledger.md)
  (the report's damaged count; dispositions unchanged), [0024](0024-reversible-quarantine-cleanup.md) (cleanup
  never moves a damaged original)

## Context
A CR2 cut short (a card pulled mid-write, a power loss, a copy that stopped) was called "not supported yet". The
probe drops a preview whose range runs past the end of the file, every rawler outcome collapsed into
`DecodeError::NoRawPreview`, and that became an `unsupported_variant` placeholder with no error. The scan said
`errors: 0`, the report "100% accounted for". A user who reads that may erase the card, although its free space may
still hold the rest of the file. A CR2 whose embedded JPEG is intact but whose sensor data is cut showed a perfect
thumbnail. rawler's errors cannot tell a cut file from a parse quirk (`DecoderFailed(String)` holds both, and some
of its paths `assert!`), and when the probe's preview decodes rawler is never called.

## Decision
- **Truncation is judged by our code, from the file's own structure** (`ps_media::raw_check::truncation`), over
  the whole buffer only (`bytes.len() == probe.file_size`): for TIFF-based RAWs the IFD chain, SubIFDs and the EXIF
  pointer, with an IFD offset past the end, or a strip (0x0111/0x0117), tile (0x0144/0x0145) or JPEG
  (0x0201/0x0202) range that ends past it, as evidence; for CR3 a top-level box whose declared size ends past the
  file (`size = 0` is not); for RAF the header's JPEG and CFA ranges. Maker notes and other tag values are never
  evidence. Any byte past the end counts.
- **The outcomes of `decode_raw_preview`, in order:** truncation -> `Failed` ("the file ends at X but its image
  data runs to Y: it was cut short ...", reason `unreadable`), even when the preview would decode; the probe's
  preview decodes -> the image; an in-file preview that fails to decode with no rawler image -> `Failed` (as a
  damaged JPEG, ADR-0055); rawler `Unsupported` -> `Unsupported` naming make and model; no preview -> `NoRawPreview`;
  any other rawler error or panic -> `Unsupported` with rawler's message. Only evidence makes a file damaged.
- **The analysis has one RAW arm:** every RAW decode error makes a placeholder (never `Persist::Failed`), only
  `Failed` is reported as an error, and no perceptual hash is stored (ADR-0055, ADR-0109), so a cut RAW cannot
  join or outrank its intact camera JPEG.
- **Generations (amends 0109):** a family is also bumped when a change corrects the reason its decoder gives.
  RAW goes 0 -> 1: every RAW placeholder of generation 0 is re-read once. A re-read RAW is reported as an error
  only when it is now `unreadable` (HEIC and AVIF re-reads unchanged).
- **Counted, not per file (ADR-0019):** `RunSummary.damaged` and `RunOverview.damaged` are the present files of the
  run's sources with `preview_reason = 'unreadable'` at the end of the run, one aggregate over the `file_preview`
  index, so an unchanged rescan still shows them. The CLI prints a line, `--json` carries the count, the dashboard
  shows a stat and a warning notice with Show.
- **The report flags them (amends 0023):** `FileFacts.damaged`, `CompletionCounts.damaged` per source and folder,
  the badge adds ", N damaged" ("100% accounted for, 1 damaged"), and reports list the files with a keep-the-
  original hint (`ps_export::DAMAGED_HINT`). Dispositions do not change: the file is copied as is.
- **Cleanup keeps damaged originals (amends 0024; owner's choice):** a damaged file is not eligible, is listed as
  kept ("damaged: keep the original for recovery") and the run's re-check skips it, so a plan made before it was
  found damaged does not move it.
- No migration: `file.preview_reason` and its index are migration 24's; the new fields are serde-default JSON.

## Alternatives considered
- **Classify by rawler's error** - its failures are strings for I/O and parse quirks alike, a panic is not an
  error, and a cut sensor strip behind an intact preview is never seen.
- **Record truncation in the probe** - the probe sees 256 KB and its record is not stored.
- **A migration resetting `preview_gen` of RAW placeholders** - the same effect as the generation bump, with a
  migration number and an ordering constraint on another change.
- **Hold back "100% accounted for" while a file is damaged** - the owner kept the badge with the count: the bytes
  are as safe in the library as on the card.

## Consequences
- A camera that writes a strip count past the end of a good file would show its files as damaged (catalogued and
  exported all the same); it gets a narrow exception and a fixture when found.
- The first rescan or Fill in previews after the update re-reads every RAW placeholder once.
- `errors` and `damaged` differ on a rescan by design: one is this run's errors, the other the catalog's state.
- A cut RAW loses its intact embedded preview in the grid until a later change adds a damaged-with-pixels kind.

## Revisit when
The thumbnail store can hold pixels for a damaged file, or a RAW format without a check (MRW, X3F, CRW) shows up
cut in real libraries.
