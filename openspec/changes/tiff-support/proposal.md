# Proposal

## Why

TIFF is how scanners, archives and photo editors keep their best copies, and those copies never meet their JPEG
or HEIC twins today:

- **Directories after the pixel data.** libtiff, ImageMagick and most scanner software write the first image
  directory (IFD0) at the end of the file. The probe only sees the first 256 KB (ADR-0010), so such a TIFF has no
  dimensions, orientation, EXIF or capture date, and grouping refuses every look match for it ("dimensions
  unknown"). In a test folder of 16 TIFFs of one picture, 13 had their first IFD beyond 256 KB (one at
  36,000,008), and none of them grouped with the 800 px JPEG of the same picture.
- **Files above 64 MB.** Scans are commonly 100 MB to 1 GB. Files above the 64 MB analysis buffer are streamed for
  their identity only and get "preview unavailable (file too large)": no thumbnail, no perceptual hash, so they
  only ever match exact copies.
- **JPEG-compressed YCbCr TIFFs** (tiled pyramids, some scanners and cameras) fail to decode ("Unhandled TIFF
  color type YCbCr(8)") and end as analysis failures.
- Multipage TIFFs, CMYK, 16-bit, floating-point, planar, tiled and alpha TIFFs must decode to the right colours,
  and the TIFF orientation tag must be applied.

## What Changes

- The probe reads a plain TIFF's directories wherever they are (ADR-0084): from the analysis buffer when the file
  is read in full anyway (<= 64 MB; the scanner then reads it before classifying it, so the read count is
  unchanged), else with bounded targeted reads (at most 1 MiB in at most 64 reads, cycle-safe, blobs such as XMP,
  ICC and Photoshop data skipped). Camera RAW files keep today's prefix-only probe.
- TIFF stills decode strip by strip or tile by tile, downsampled while decoding, through the `tiff` crate the
  `image` crate already uses (ADR-0085): all of grey, RGB, CMYK and JPEG-compressed YCbCr, 1-64-bit integer and
  16/32/64-bit float samples, chunky and planar, strips and tiles, alpha over white, orientation applied; the
  smallest reduced-resolution level of the first image that still reaches the analysis size is preferred; the
  first page of a multipage file is the image. Files in memory fall back to the previous whole-image decode when
  the new decoder cannot handle them (palette images, very large strips).
- TIFFs above 64 MB are analysed in one sequential pass in their I/O lane (ADR-0086): the same read feeds the
  content identity (full up to 256 MB, sampled above, ADR-0011) and the strip decoder, holding at most one 32 MB
  strip plus the downsampled image, counted as one 64 MB buffer against the in-flight cap. They get a thumbnail
  and perceptual hashes like any photo; one that still cannot be decoded keeps its identity and gets a
  placeholder that says why.
- Metadata of such large TIFFs comes from a compact copy of their metadata directories, so EXIF dates, camera and
  orientation are read without the pixel data between them.
- The inspector's large preview and AI previews probe from the whole file, and the large preview of a TIFF above
  64 MB decodes from the file in bounded memory instead of reading it whole.
- The fixture generator writes TIFF test images: directories before or after the pixel data, strips or tiles,
  chunky or planar, 8/16-bit, float, grey/RGB/RGBA/CMYK, JPEG YCbCr, LZW/Deflate with predictors, pages,
  reduced-resolution levels, orientation and camera EXIF. The ground-truth corpus is unchanged.
- Two performance budgets (ADR-0087): the analysis decode of a 12 MP 16-bit LZW TIFF, and the working memory of a
  streamed TIFF decode.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `media-probe`: TIFF directories beyond the 256 KB prefix; the first image of multi-image TIFFs.
- `media-metadata`: TIFF EXIF wherever its directories are, also above 64 MB.
- `thumbnails`: TIFF decoding (layouts, sample formats, colour models, reduced-resolution levels, bounded memory).
- `scanning`: read accounting for TIFF directories; one-pass analysis of TIFFs above 64 MB.
- `content-identity`: identity computed incrementally during another read.
- `duplicate-detection`: TIFF copies group with their JPEG copies by look.
- `test-corpus`: TIFF fixtures.
- `performance-budgets`: TIFF decode time and memory budgets.

## Impact

- ADRs relied on: 0003, 0004, 0008, 0009, 0010, 0011, 0012, 0013, 0015, 0016, 0017, 0019, 0025, 0026, 0036, 0054,
  0056, 0057, 0075. New: 0084 (TIFF directories beyond the probe window; amends 0010), 0085 (strip-wise TIFF
  decoding through the `tiff` crate; amends 0017), 0086 (one-pass analysis of TIFFs above 64 MB; amends 0010,
  0011), 0087 (TIFF decode budgets; extends 0026).
- Crates: `ps-media` (TIFF view and directory loader, metadata block, probe entry points, strip decoder,
  `DecodeError::TooLarge`), `ps-hash` (incremental identity), `ps-scan` (pipeline: TIFF read order, one-pass
  reader), `ps-service` (previews), `ps-fixtures` (TIFF writer), `xtask`/`perf-budgets.toml` (budgets).
- Dependency: `tiff` 0.11 becomes a direct dependency of `ps-media` and `ps-fixtures`; it is already in the build
  (through `image`) at the same version and features, so no new crate, licence or build step (ADR-0085).
- No catalog migration, no file format change. Files analysed before this change keep their rows until their
  content or the analysis version changes; a TIFF that failed before is retried as usual.
- Effort: about 5-7 focused days (probe and loader 1.5, decoder 1.5, one-pass scan 1.5, fixtures and tests 1.5,
  docs and gate runs 1).

## Out of scope

- Palette (indexed) TIFFs beyond what the previous decoder did, CIE L*a*b*, and YCbCr with chroma subsampling
  that is not JPEG-compressed: they keep the previous behaviour (whole-image decode when the file is in memory).
- Colour management (ICC profiles), and tone mapping of floating-point TIFFs whose values exceed 0..1.
- BigTIFF (the 64-bit variant for files above 4 GB): not recognised by the probe, as before.
- Re-analysing already catalogued TIFFs without a content change (no analysis-version bump; a rescan of a changed
  or previously failed file picks the new code up).
- Further pages of a multipage TIFF as separate photos.
- Changing HEIC/libheif packaging (a separate, pending decision).
