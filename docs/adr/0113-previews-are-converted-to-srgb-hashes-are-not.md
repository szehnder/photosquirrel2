# 0113. Previews are converted to sRGB; hashes are not

- Status: accepted
- Date: 2026-10-29
- Deciders: maintainers
- Amends: [0016](0016-thumbnail-store.md) (thumbnails and large previews are converted to
  sRGB; the large-preview cache key), [0012](0012-perceptual-hashing-and-index.md) (the hash input is stated: the unconverted
  analysis decode)

## Context
Canon bodies set to Adobe RGB (1998) write `_MG_` JPEGs and CR2s whose pixels are Adobe RGB. They mark it the DCF
way: EXIF `ColorSpace` 0xFFFF ("uncalibrated"), the Interop IFD's `InteropIndex` `R03`, the Canon maker note's
`ColorSpace` (0x00B4) = 2, and usually no ICC profile. Phones write Display P3 JPEGs with an embedded ICC profile.
PhotoSquirrel decoded every pixel as sRGB, so these previews looked flat next to their sRGB neighbours. The files
themselves are fine: export copies bytes.

`moxcms` 0.8 (pure Rust, no native code) is already in the build through `image` 0.25. It parses ICC profiles,
has built-in sRGB, Adobe RGB and Display P3 profiles and builds 8-bit RGB transforms.

## Decision
- **The probe records the colour space** of a still from bytes it already reads, first answer wins:
  1. a JPEG's own `APP2 ICC_PROFILE` chunks (sequence order, inside the probe window). The profile is classified by
     what it does: a profile whose transform to sRGB moves no sample by more than one code value is sRGB; one that
     matches moxcms' Adobe RGB or Display P3 the same way is named so; any other is "profile: <description>". A
     profile moxcms rejects, or chunks beyond the window, are ignored with a probe warning;
  2. a DNG's `PreviewColorSpace` (0xC71A): 2 sRGB, 3 Adobe RGB (other values: unknown);
  3. EXIF: `InteropIndex` `R03` is Adobe RGB, `ColorSpace` 1 is sRGB; 0xFFFF alone is not evidence;
  4. the Canon maker note's `ColorSpace` (0x00B4): 2 is Adobe RGB (CR2 previews carry no EXIF of their own);
  5. otherwise sRGB.
  A RAW's preview takes the container's answer (2-4); a JPEG takes 1, 3, 4. TIFF, PNG, WebP, HEIC and AVIF are sRGB
  until their profiles are read (a follow-up). The file name is never evidence.
- **Only display images are converted, after resizing**: the 384 px grid thumbnail (`thumbnail_from_image`) and
  the 2048 px large preview (`make_large_preview`), with perceptual intent (relative colorimetric when a profile has
  no perceptual table). sRGB images are not touched, so their thumbnails stay byte-identical. The dominant colour
  is taken from the converted thumbnail. Built-in Adobe RGB and Display P3 transforms are built once; transforms of
  embedded profiles are cached (8, keyed by the profile bytes).
- **Hashes are not converted.** Perceptual hashes, quality and every grouping input come from the analysis decode
  as decoded. Grouping, keepers and the ground-truth gate do not move.
- **`moxcms` is a direct dependency** of `ps-media` (and of `ps-fixtures`, which encodes test profiles), pinned to
  the 0.8 series `image` uses.
- **Catalog migration 26** adds `file.colour`: `srgb`, `adobe_rgb`, `display_p3`, `profile:<description>`, or
  `unchecked`. New analyses write the probe's answer. Existing thumbed JPEG and RAW rows with real thumbnails whose
  name starts with `_` (DCF's Adobe RGB marker) stay NULL; every other existing row is `unchecked` (the owner's
  choice of 2026-10-27).
- **One re-check per NULL row**, on the next rescan of its source, shares the one-time header re-read of ADR-0105
  and ADR-0117: a row due both is opened once and read once (the probe window); `meta_version` and `colour` are
  stamped independently. An sRGB answer is written and nothing more is read. Any other answer reads the file once
  (identity checked), makes the thumbnail again and replaces the stored one for that content id; `colour` is written
  for every row of that content. Hashes, metadata and the dedup stamp are not touched; nothing is regrouped.
- **Large previews** are cached under `hex(blake3(identity hex ‖ ":srgb-1"))`, so previews cached before are never
  served again and age out under the cache cap.
- The inspector's File section shows a "Colour" row for Adobe RGB, Display P3 and embedded profiles, from
  `file.colour`; sRGB, unchecked and unknown show none.

## Alternatives considered
- **Convert the analysis decode** - every perceptual hash of an Adobe RGB file would change, and an Adobe RGB
  original and its sRGB export would hash further apart; grouping would move for a display problem.
- **lcms2** - a C library and a build dependency on every platform, for what moxcms already does in the build.
- **Treat `ColorSpace` 0xFFFF as Adobe RGB** - "uncalibrated" says only "not sRGB"; editors write it for wide-gamut
  profiles of every kind.
- **Re-read the header of every JPEG in an old catalog** - 100k camera JPEGs x 256 KB for a setting few use; the
  `_` name selects the candidates, renamed files wait for their next analysis.
- **A writer command to replace the thumbnail** - the thumbnail store is written directly by the scanner, as the
  ADR-0117 placeholder is; the catalog only gets the `colour` stamp.

## Consequences
- Adobe RGB, Display P3 and profiled JPEGs and Canon/DNG RAW previews look right in the grid, the filmstrip and the
  loupe. The conversion costs well under a millisecond at 384 px.
- The first rescan after the update reads the probe window of each `_`-named JPEG and RAW once, and the whole file
  of those that are not sRGB. Renamed Adobe RGB files keep muted thumbnails until analysed again.
- Large previews are made again on first view after the update.

## Revisit when
HEIC or AVIF decoders hand us profiles, TIFF/PNG/WebP profiles are read, the display itself is colour managed, or
moxcms changes behaviour on an `image` upgrade.
