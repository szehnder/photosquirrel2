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
