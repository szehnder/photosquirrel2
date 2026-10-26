# Proposal

## Why

The owner scanned a real Canon SD card (2,632 files: CR2 originals, camera JPEGs and Adobe-converted DNGs of the
same shots, in `DCIM/` and dated copy folders). Five burst frames were merged into the group of the frame shot just
before them. None of those five frames would be exported, and a cleanup could have quarantined their originals as
"covered" by a copy of a different photo. ADR-0013 and ADR-0035 promise the opposite: bursts are suggestions only.

The cause:
- Canon records the first frame of a burst with sub-second time `00` and the next frame with `15`, `16`, `13`.
- Adobe's DNG converter drops `SubSecTimeOriginal` when it is `00`. So frame N's DNG has no sub-second time, and the
  rule "different sub-seconds when both have them" cannot tell it from frame N+1.
- The second guard, "two camera originals with the same model and dimensions are two shots", needs maker notes on
  both files. Adobe moves Canon's maker notes into `DNGPrivateData`, so the DNGs look like non-originals.
- Frames 130 ms apart have pHash distance 2, so DNG N joins frame N+1's group, and, as the larger file, becomes the
  keeper. Frame N+1 (its CR2, its DNG and both JPEG copies) is then a "duplicate" of frame N.

Both files say which frame they are: the DNGs carry `OriginalRawFileName` (`IMG_0034.CR2` vs `IMG_0035.CR2`), and
the Canon maker notes carry a file number (`101-0034` vs `101-0035`). The synthetic corpus never modelled converted
RAW files, so the gate (precision 1.0000) could not catch it.

This ships in 0.3.0, before `preview-placeholders`, because it can keep a photo out of the library.

## What Changes

- **Frame identity is read.** Metadata gains a frame identity where the file records one: the DNG
  `OriginalRawFileName` stem, and the maker-note file number of Canon (and other makers where it is cheap), including
  maker notes kept in `DNGPrivateData`. It is read from the bytes the probe already reads.
- **Different frame identities are never one image.** Two files that both record a frame identity, for the same
  camera, and whose identities differ, are a contradiction: they become a similar-shot suggestion, not a group.
- **RAW files are camera originals.** A RAW-format file (CR2, NEF, ARW, DNG, ...) counts as a camera original for the
  "two camera originals are two shots" guard, with or without parsed maker notes; `DNGPrivateData` maker notes count
  as maker notes.
- **Existing catalogs are corrected.** RAW and DNG rows analysed before this change have their metadata re-read
  (header bytes only, read-only) on the next rescan and are regrouped. The separated frames become pending, the next
  export copies them, and a quarantined file whose library copy belonged to the wrong frame stops counting as
  accounted for (the existing cleanup rule) and is reported with a restore hint.
- **The corpus models converted RAW bursts.** The generator writes burst sets as CR2-like originals, camera JPEGs
  and converted DNGs whose first frame lost its `00` sub-second, all in must-not-merge sets. The gate baseline is
  re-recorded with the new files and must stay at precision, recall and keeper accuracy 1.0000 with 0 violations.

## Capabilities

### Modified Capabilities
- `media-metadata`: frame identity; maker notes inside `DNGPrivateData`.
- `duplicate-detection`: different frame identities contradict; RAW files are camera originals.
- `incremental-rescan`: RAW rows analysed before frame identity existed are re-read and regrouped.
- `cleanup`: quarantined files covered by a copy of another frame stop being accounted for and say why.
- `test-corpus`: converted RAW bursts with a dropped sub-second.

## Impact

- Crates: `ps-model` (frame identity in metadata), `ps-media` (DNG tag 0xC68B, Canon maker-note file number,
  `DNGPrivateData` maker notes), `ps-dedup` (the contradiction and the RAW-original rule), `ps-catalog` (migration 23,
  `file.frame_id` and a metadata version for the re-read), `ps-scan` (the re-read on rescan), `ps-fixtures` (the
  converted-burst sets), `ps-service`/CLI (the restore hint in the report).
- ADRs relied on: 0013, 0035, 0039, 0007, 0024, 0025, 0096, 0099. New ADR (next free number): "Recorded frame
  identities keep frames apart" (amends 0035).
- Migration 23. `preview-placeholders` moves to migration 24.

## Out of scope

- Colour management of Adobe RGB JPEGs and previews (the `_MG_` files): a separate, cosmetic follow-up.
- Quieting rawler's lens-database warnings in the CLI: a separate small fix.
- Frame identity from file names alone: names are renamed by import tools and are not evidence.
