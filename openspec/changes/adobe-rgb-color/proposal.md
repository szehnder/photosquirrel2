# Proposal

## Why

On the owner's Canon card, the `_MG_` JPEGs and the previews of their CR2s look flat and muted in the grid and the
loupe next to the same shots' `IMG_` files. The camera recorded them in Adobe RGB (1998). Canon marks this the DCF
way: EXIF `ColorSpace` = 0xFFFF ("uncalibrated"), the Interop IFD's `InteropIndex` = `R03`, and a file name that
starts with `_`. There is usually no embedded ICC profile. PhotoSquirrel decodes the pixels and shows them as if
they were sRGB, so the wider-gamut primaries are squeezed toward grey. The README lists this under "Known
limitations", and ROADMAP row 30 asks for the fix.

The files themselves are fine: export copies them byte for byte, and Lightroom or Apple Photos show them correctly.
Only our previews are wrong.

## What Changes

- **The colour space is read in the probe.** It comes from bytes the probe already reads:
  - an embedded ICC profile (JPEG `APP2 ICC_PROFILE`)
  - a DNG `PreviewColorSpace`
  - EXIF `InteropIndex` `R03` with `ColorSpace` 0xFFFF
  - the Canon maker note's colour-space tag

  The result is `sRGB` (also the default when nothing is recorded), `Adobe RGB`, `Display P3` or "other profile".
- **Thumbnails and the loupe are converted to sRGB.** The grid thumbnail and the 2048 px large preview are
  converted after resizing:
  - by a fixed Adobe RGB (1998) or Display P3 transform
  - or by a transform built from the embedded profile, cached by profile

  sRGB files take no conversion step and their thumbnails stay byte-identical. The dominant colour is taken from the
  converted thumbnail.
- **Grouping does not move.** Perceptual hashes, quality and every other analysis value come from the unconverted
  analysis decode, as today. Grouping, keepers and the corpus gate are unchanged.
- **Uses `moxcms`** (0.8, already in the build through `image`) directly for ICC parsing and transforms. It is pure
  Rust and there is no new native code. This is a dependency of consequence, so it gets an ADR.
- **Old catalogs.** Catalog migration 26 adds `file.colour`. Thumbed JPEG and RAW stills whose file name starts
  with `_` (the DCF marker for Adobe RGB) are left unrecorded; every other existing row is marked as not re-checked.
  On the next rescan of their source, each unrecorded unchanged row's header is re-read (the probe window,
  read-only). When it is not sRGB, its thumbnail is made again. Large previews cached before the change are never
  served again (new cache key) and age out under the cap.
- **The inspector shows the colour space** in the File section ("Adobe RGB (1998)", "Display P3",
  "Embedded profile: <description>"). sRGB and unknown show nothing.

## Capabilities

### Modified Capabilities
- `thumbnails`: colour-space detection, conversion to sRGB for thumbnails and large previews, hashes unaffected,
  and old thumbnails redone once.
- `photo-inspector`: the colour space in the File section; the loupe shows the converted preview.

## Impact

- `ps-model`:
  - `ColourSpace` (`analysis.rs`)
  - `ProbeRecord.colour` and the ICC byte ranges (serde default)
  - `Metadata.fields.colour_space` for the inspector
- `ps-media`:
  - `probe.rs`: the APP2 ICC segments, and `ColorSpace` 0xA001 and `InteropIndex` 0x0001 in the Exif and Interop
    IFDs it already walks
  - `jpeg.rs` `scan`: APP2 marker ranges
  - `frame.rs`: Canon maker-note tag 0x00B4 beside `FileNumber`
  - DNG `PreviewColorSpace` 0xC71A on the preview IFD
  - new `colour.rs`: `to_srgb(&mut RgbImage, &ColourSource)` with cached transforms
  - `thumb.rs`: `thumbnail_from_image` gets a colour argument, and so do `make_large_preview`, `analyze_still_held`
    and `make_thumbnail`
  - `decode.rs`: `decode_preview` and `decode_raw_preview` report which stream's colour applies
- `ps-scan/src/pipeline.rs`: the analysis persist (thumbnail from the analysis or preview decode;
  `still_hashes(&d.image)` stays on the unconverted image) and the TIFF one-pass path, which passes sRGB.
  `ps-scan/src/diff.rs`: the colour re-check of unchanged rows.
- `ps-catalog`: migration 26 `0026_colour.sql` (`file.colour`), the writer's analysis statement, a re-thumb
  command that replaces a stored image thumbnail for a content id, and `read.rs` `FILE_COLUMNS`.
- `ps-service/src/library.rs` (the large preview: conversion and the new cache key), `ui-workspace` (inspector
  File row), README (remove the Adobe RGB limitation and say TIFF and HEIC profiles are not applied yet).
- **New ADR-0113** "Previews are converted to sRGB; hashes are not" (amends 0016, 0012; records the `moxcms`
  dependency). **Migration 26.**

## Out of scope

- HEIC and AVIF colour (iPhone Display P3). The system decoders (`ps-sysdecode`) do not hand us the profile yet; a
  follow-up.
- TIFF, PNG and WebP embedded profiles (the README's TIFF limitation stays).
- Colour management of the display itself (we target sRGB, as GPUI does).
- Changing exported files: export copies bytes and never converts.
