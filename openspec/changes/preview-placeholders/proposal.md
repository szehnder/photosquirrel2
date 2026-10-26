# Proposal

## Why

The owner scanned a library on a Windows laptop running 0.2.0. That version cannot read TIFF files whose image
directory sits at the end of the file. The TIFF tiles in the Library showed nothing: the grid draws a file with no
thumbnail as a flat grey square. The inspector had details but no picture, and nothing said why. HEIC files without
a system decoder look the same, and so do videos without a poster-frame tool and files that are damaged.

The user cannot tell four cases apart:
- "this is a photo, but this computer needs a library to show it"
- "this version of PhotoSquirrel cannot read this kind of file yet"
- "the file is damaged"
- "a preview is possible now and only needs filling in"

They also have no efficient way to fill in previews after installing a decoder or updating the app. A full rescan
walks every source, and rows thumbed by an older version are never retried (a known `tiff-support` follow-up: old
TIFF rows over 64 MB that got a placeholder are not re-queued).

`heic-system-decoders` adds install guidance for HEIC to the scan summary and the inspector, and re-decodes files
waiting for a decoder on the next rescan. This change makes that general, for every format and every reason. It adds
a placeholder that looks deliberate, an explanation one click away, and a targeted way to fill in previews.

## What Changes

- **A designed placeholder tile** replaces the flat square in the grid, the Excluded grid, the list thumbnail
  column and the loupe. It shows:
  - a media glyph (photo or video)
  - the format badge ("TIFF", "HEIC")
  - "No preview"
  - a status mark for the reason: needs a library, not supported yet, damaged, or ready to fill in
  - a capture date or duration when known

  It is drawn with ui-theme tokens and reduces to the glyph and badge at small tile sizes.
- **Placeholder reasons for every case.** `PlaceholderReason` gains `UnsupportedVariant { detail }` (for example
  BigTIFF), `Unreadable { message }` and `NeedsNewerVersion`, alongside `heic-system-decoders`' `DecoderMissing`,
  `NoPosterTool` and `Sequence`.
- **An info button** on placeholder tiles (shown on hover, focus and selection) and in the inspector and loupe opens
  a **"Why no preview?" dialog** for that file. The dialog shows:
  - the format and the reason in plain words
  - what still works: the file is catalogued, exact copies are found, and it is exported
  - what to do, by reason:
    - install steps from the decoder report, with Copy buttons and click-only Store links (ADR-0089 guidance)
    - "update PhotoSquirrel"
    - "the file may be damaged: try opening it in another app"
    - a **Fill in previews** button when a preview is possible now
  - "N other files have no preview for this reason" with **Show them**, which filters the Library to those files.
- **Library filter "No preview"**, by reason and format.
- **Fill in previews:** a background job that re-decodes only files whose placeholder could now become a preview,
  for everything or for one format or reason. It reads each waiting file once, skips offline sources and counts
  them, regroups by look (ADR-0007 dedup stamp), can be paused and cancelled, and reports progress in the scan
  dashboard. It does not walk any folder. The CLI has `previews fill [--format F] [--json]` and `previews status`.
- **Re-queue after an update:** every placeholder records the decoder generation that produced it, for each format.
  When the running version decodes that format at a newer generation (TIFF in 0.3.0 versus 0.2.0), those files count
  as waiting. A one-line notice offers Fill in previews, and the next rescan of their source fills them in.

## Capabilities

### Modified Capabilities
- `thumbnails`: the new placeholder reasons; a decoder generation for each placeholder.
- `gallery`: the placeholder tile, the info button, the "No preview" filter, and Fill in previews from the Library.
- `photo-inspector`: the "Why no preview?" dialog for every reason.
- `incremental-rescan`: a targeted fill-in job; re-queue after an update.
- `cli`: `previews status` and `previews fill`.

## Impact

- `ps-model` (placeholder reasons, decoder generations), `ps-media` (generation constants for each decoder, and the
  reason classification of decode failures), `ps-catalog` (migration 23 `file.preview_gen`, preview-status queries),
  `ps-scan` and `ps-service` (fill-in job on the scan lanes and their stop checks), `ui-gallery` (placeholder paint
  and info hit target), `ui-workspace` (dialog, filter, notice, dashboard row), and `photosquirrel-cli`. There is
  one new ADR.
- **Depends on `heic-system-decoders`** (decoder registry and report, `decoder_missing`, migration 19) and comes
  after `excluded-view-filters` (migrations 21 and 22; the Excluded grid and list draw the same placeholder).
- Read-only on user files as always. The job only reads sources.
- **Release:** ships in 0.3.0 together with `tiff-support` and `heic-system-decoders` (owner decision, 2026-10-25). "What's new in 0.3.0" in `packaging/release-notes.md` gains this change's user-facing line; the version bump and tag happen after all four are archived, with the owner's go-ahead.
