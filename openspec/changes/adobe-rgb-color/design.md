# Design

## Context

- Decoding: `ps-media/src/decode.rs` `decode_scaled_held` decodes JPEG (`decode_jpeg`: jpeg-decoder or mozjpeg
  scaled), TIFF, PNG/GIF/WebP/BMP, RAW (`decode_raw_preview`: the probe's preview JPEG, else rawler's preview or
  thumbnail), HEIC and AVIF into an 8-bit `RgbImage`. Nothing looks at colour profiles. `decode_preview` decodes
  the probe's embedded JPEG preview.
- Analysis: `ps-media/src/thumb.rs` `analyze_still_held` does one decode of at least 512 px (`Decoded`) and takes
  the thumbnail from an embedded preview of at least 320 px when there is one, else from the analysis image
  (`thumbnail_from_image`: resize to 384 px, JPEG q82, `dominant_colour`).
  - In `ps-scan/src/pipeline.rs` the persist step takes `still_hashes(&decoded.image)` (pHash/dHash, ADR-0012) from
    the same unconverted analysis image.
  - The TIFF one-pass path (around line 1579) calls `thumbnail_from_image` itself.
- Large previews: `ps-service/src/library.rs` re-probes the file (`probe_buffer`) and calls
  `thumb::make_large_preview` (or `make_large_tiff_preview`). The result is cached in `PreviewCache` under the
  content id's hex (`valid_key`: hex, at most 64 characters).
- The probe already walks IFD0, Exif, GPS, Interop and SubIFDs (`tiff.rs` `IfdKind::Interop` from tag 0xA005), the
  JPEG markers (`jpeg.rs` `scan`), and Canon maker notes (`frame.rs`, `burst-frame-identity`).
- `moxcms` 0.8.1 is in `Cargo.lock` as a dependency of `image` 0.25. It parses ICC
  (`ColorProfile::new_from_slice`), has built-in `new_srgb`, `new_adobe_rgb` and `new_display_p3`, and builds 8-bit
  RGB transforms (`create_transform_8bit`). It is pure Rust.

## Decisions

### D1. What the colour space is, in order (new ADR-0113)

`ColourSpace { Srgb, AdobeRgb, DisplayP3, Profile { description } }` and `ColourSource` (the ICC bytes when the
answer came from a profile). For each decoded stream, the first that applies:

1. **The stream's own ICC profile:** `APP2 "ICC_PROFILE\0"` chunks, reassembled in sequence order, within the probe
   window. A profile that is sRGB is `Srgb`: its description says sRGB, or its colorants and TRC match moxcms'
   sRGB within 1e-3. A profile moxcms rejects is ignored, with a metadata warning.
2. **DNG `PreviewColorSpace`** (0xC71A) for a DNG's preview: 2 = sRGB, 3 = Adobe RGB. 4 = ProPhoto maps to
   `Profile` through moxcms' ProPhoto, if it has one; otherwise it is treated as unknown.
3. **EXIF:** `InteropIndex` = `R03` -> `AdobeRgb` (the DCF option file). `ColorSpace` = 1 -> `Srgb`. `ColorSpace`
   = 0xFFFF alone is not enough: "uncalibrated" says only "not sRGB".
4. **Canon maker note** `ColorSpace` (0x00B4): 2 -> `AdobeRgb`. It covers CR2 files, whose preview JPEG carries no
   EXIF of its own.
5. Otherwise `Srgb`.

A RAW's embedded preview takes the container's answer (2-4). A JPEG main image takes 1, then 3 and 4. The file name
is never evidence (it only selects candidates for the old-catalog re-check, D4).

### D2. Convert for display only, after resizing

- `colour::to_srgb(img: &mut RgbImage, src: &ColourSource)`:
  - `Srgb` returns at once, so no pixel is touched and sRGB output stays byte-identical
  - `AdobeRgb` and `DisplayP3` use one cached transform each (`OnceLock`)
  - `Profile` uses an LRU of 8 transforms keyed by the BLAKE3 of the ICC bytes
  - perceptual intent, falling back to relative colorimetric when the profile has no perceptual table
- It runs after `resize_to_long_edge`:
  - in `thumbnail_from_image` (about 110k pixels at 384 px)
  - in `make_large_preview` (about 3 MP at 2048 px)

  The analysis decode is never converted. Converting after the resize means the resize filters encoded values,
  exactly as it does today for every file; the difference from converting first is below JPEG noise.
- `dominant_colour` reads the converted thumbnail.
- Cost: at 384 px the conversion is well under 1 ms, against `analysis-decode-thumb`'s 100 ms budget for a 12 MP
  file. The bench gets an Adobe RGB case. `gallery-scroll-p95` is unaffected (the grid paints stored JPEGs).

### D3. Hashes, grouping and the corpus

pHash and dHash keep their input, the analysis image as decoded, so ADR-0012 is unchanged. Two copies of one
picture with different colour handling (an Adobe RGB original and an sRGB export from an editor) hash as they do
today. Grouping, keepers, quality and the corpus gate are unchanged. The corpus fixtures are sRGB and untagged, so
their thumbnails are byte-identical too. A test asserts that the hashes of an Adobe RGB fixture are the same with
and without the change's conversion. It runs the analysis with a test hook that disables conversion and compares.

### D4. Old catalogs (migration 26)

The owner chose this on 2026-10-27: only `_`-named files are re-checked; renamed Adobe RGB files wait for their next analysis.

- `0026_colour.sql`: `ALTER TABLE file ADD COLUMN colour TEXT`. Values are `srgb`, `adobe_rgb`, `display_p3`,
  `profile:<description>` or `unchecked`. New analyses write the probe's answer.
- Backfill:
  - thumbed still rows with a content id, format `jpeg` or a RAW format, and `name` starting with `_` (DCF's
    Adobe RGB marker) stay NULL: re-check them
  - every other existing row gets `unchecked`: analysed before colour was read, not re-checked, and the inspector
    shows nothing
- **Rescan re-check:** an unchanged thumbed row with `colour IS NULL` on an online source is read once (open
  read-only, probe window only) and its colour is recorded:
  - `srgb`: written, done
  - otherwise: the file is read and its thumbnail made again (the embedded preview of a RAW, or a 384 px JPEG
    decode), replacing the stored image thumbnail for that content id (a new writer command)
  - `colour` is written for every row with that content id, so copies are not read again
  - perceptual hashes, metadata and the dedup stamp are not touched, and nothing is regrouped
  - the summary counts "N previews recoloured"
- Files renamed away from `_` by an import tool stay `unchecked`. Analysing again (a changed file, or an analysis
  refresh) records them. This is the cost of not reading the header of every JPEG in a library once (100k camera
  JPEGs x 256 KB).
- **Large previews:** the cache key becomes `hex(blake3(identity_hex ‖ ":srgb-1"))`, 64 hex characters, so
  `valid_key` holds. Previews cached before are never served and age out by the LRU cap.

### D5. Inspector

The File section gets a "Colour" row from `Metadata.fields.colour_space`: "Adobe RGB (1998)", "Display P3", or
"Embedded profile: <description>". sRGB, unchecked and unknown show no row, so the common case adds nothing. The
loupe shows the converted large preview (D2) and the filmstrip the converted thumbnails.

### D6. Testing

- `ps-fixtures` writers: a JPEG with EXIF `ColorSpace` 0xFFFF + `InteropIndex` R03 and no profile; the same with an
  APP2 Display P3 profile (bytes from moxcms' profile encoder, or a small CC0 profile under `ps-fixtures/data`); a
  CR2-style file with the Canon maker-note colour space 2 and a preview JPEG; and a DNG with
  `PreviewColorSpace` 3. Each is a saturated colour patch whose expected sRGB value is known.
- `ps-media` tests: detection order, conversion of the patch within ΔE 2 of the reference, an sRGB file's
  thumbnail byte-identical to before (a golden file from the current build), hashes unchanged (D3), and malformed
  ICC ignored.
- `ps-scan` tests: the re-check reads only `_` rows, once, only their probe window when sRGB; it replaces the
  thumbnail and does not change the dedup stamp.

## Risks / Trade-offs

- [moxcms behaviour changes on an `image` upgrade] -> we pin the same 0.8 series and own the call. The ADR records
  it, and the tests pin the patch values.
- [A wrong "not sRGB" answer over-saturates a preview] -> only explicit evidence converts (ICC, R03, DNG tag,
  Canon tag); "uncalibrated" alone does not.
- [Old renamed Adobe RGB files stay muted] -> documented; recorded on their next analysis.

## Deviations (as built)

- **The colour comes from the probe inside `ps-media`.** `thumbnail_from_image` takes a `&ColourSource`;
  `make_thumbnail`, `analyze_still_held` and `make_large_preview` build it from the probe they already receive
  (`ColourSource::of(bytes, probe)`), so their signatures did not change, and `decode_preview` and
  `decode_raw_preview` report nothing new: a JPEG's embedded preview and a RAW's preview both take the file's (the
  container's) answer, which is the probe's. The TIFF one-pass path passes `ColourSource::srgb()`.
- **ICC profiles are classified by what they do.** Besides a description containing "sRGB", a profile whose
  transform to sRGB moves none of 125 samples (five levels per channel) by more than 2 code values is sRGB, and one
  that agrees the same way with moxcms' Adobe RGB or Display P3 is named so (the inspector says "Display P3", not
  "Embedded profile: Display P3"). The conversion still uses the embedded profile itself. A grey or CMYK profile does
  not apply to the RGB the decoders hand over and is ignored silently; a rejected or incomplete one (missing chunk,
  beyond the probe window) is ignored with a probe warning.
- **The profile cache is keyed by the profile bytes** (compared, 8 most recent), not their BLAKE3: `ps-media` has no
  BLAKE3 dependency, and the effect is the same.
- **DNG `PreviewColorSpace` 4 (ProPhoto) is not mapped**: only 2 (sRGB) and 3 (Adobe RGB) are evidence; other values
  fall through to EXIF. CR3 (`CMT2` Exif and Interop, `CMT3` maker note) and RAF (its JPEG's EXIF) read the same tags,
  beyond D1's list.
- **The colour space lives in `file.colour` only**, not in `Metadata.fields.colour_space`: the inspector reads the
  column (`ItemDetail.colour_space`), and the re-check writes the column only, so the metadata JSON and the dedup
  stamp never change for it. Formats whose profiles are not read yet (HEIC, AVIF, TIFF, PNG, WebP, videos) record
  `srgb`, the colour they are shown in; the follow-up that reads their profiles needs its own one-time re-check
  (selected by format).
- **No writer command replaces the thumbnail.** The scanner stores the new thumbnail through the thumbnail store
  (`ThumbStore::put`), as ADR-0117's "could not be read" placeholder is stored; the catalog gets
  `Command::SetColour`, which stamps every row of the content (`None` writes `unchecked`).
- **The re-check rides on the one-time header re-read** of ADR-0105/0117 (`Todo.colour_check` beside
  `meta_reread`), so a `_` RAW due both is opened once: the head read is the probe window (256 KB) instead of the
  structure check's 64 KB, and `meta_version` and `colour` are stamped independently
  (`a_raw_due_both_checks_is_opened_once`). The diff also checks the `_` name and the format (JPEG or RAW) besides
  the NULL colour, so no other row can ever be read for it, and leaves placeholder rows out (no thumbnail to
  recolour; a placeholder that a newer decoder fills in is analysed and records its colour then). A file not in sRGB
  is read whole once (the decode-memory budget is taken), its BLAKE3 identity compared with the catalog's (other
  content under an unchanged fingerprint: `unchecked`, its stored thumbnail untouched) and its thumbnail made by
  `make_thumbnail` (the embedded preview, else a 384 px decode) in the I/O lane, not as a CPU job: it happens once per
  file. A RAW found cut by the same read, a file over 64 MB (or with a sampled identity), and a file whose thumbnail
  cannot be made again record their colour and keep their thumbnail. A read error is reported once and leaves the
  row NULL for the next rescan.
- **The counts** are `DiffCounts.colour_checked` and `recoloured` (in the run summary JSON), printed by the CLI
  ("N files checked once for their colour space ..., M recoloured"); the app's dashboard does not show them.
- **Migration 26** also leaves rows never analysed NULL (their analysis writes the colour) and marks analysed rows
  that are not thumbed (`stage` 2) `unchecked`; a changed file's `colour` is cleared with its other analysis results.
- **The bench case** is a fourth file, a 12 MP Canon Adobe RGB JPEG, in `analysis-decode-thumb`'s own test
  (`scenario_thumbnails_decode_within_per_file_budget`), not a separate budget: 35.0 ms median for it, 35.39 ms for
  the budget (baseline 37.46).
- **Goldens:** `ps-media/tests/data/colour/` holds eight thumbnails (an untagged 1000x750 photo and the green patch,
  each without and with an embedded sRGB profile, from both the thumbnail and the analysis path), written by
  `ps-media` at 2212ccf before any change of this one (`PHOTOSQUIRREL_WRITE_COLOUR_GOLDENS=1`), compared byte for byte
  on x86_64 Linux, where they were written, and within 2 code values on other targets (their SIMD decode and resize
  paths may differ by one); on every target the test also compares the thumbnails with the conversion hook off, which
  must be byte-identical.
