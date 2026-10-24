# 0085. TIFF stills decode strip by strip through the `tiff` crate, averaged down while decoding (amends 0017)

- Status: accepted
- Date: 2026-10-24
- Deciders: maintainers
- Amends: [0017](0017-format-support.md)

## Context
ADR-0017 decodes TIFF with the `image` crate. It decodes the whole image at full resolution (a 6000x4000 16-bit
RGB TIFF is 144 MB before conversion to RGBA8), refuses YCbCr ("Unhandled TIFF color type YCbCr(8)": JPEG-
compressed TIFFs and vips pyramids), cannot use a pyramid's reduced-resolution levels and cannot decode without
the whole file in memory. `image` 0.25 decodes TIFF through the `tiff` 0.11 crate, whose `read_chunk` decodes one
strip or tile (none, LZW, Deflate, PackBits, JPEG; horizontal and floating-point predictors; byte order;
WhiteIsZero) and returns raw samples, YCbCr included.

## Decision
TIFF analysis decodes, thumbnails and previews use our own decoder over `tiff::decoder::Decoder` (the crate
becomes a direct dependency at the version `image` already uses; no new crate, licence or build step):

- **Directory:** the first image, or the smallest reduced-resolution directory (NewSubfileType bit 0) of the main
  chain with the first image's aspect ratio (within 1%) whose long edge reaches the requested size. Pages after
  the first are never the image.
- **Scale:** an integer box factor, the largest keeping the long edge at or above the requested size (512 px for
  analysis), applied while decoding.
- **Chunks** in file-offset order, each at most 32 MB decoded and compressed, every sample converted to 8 bits
  (integers by bit depth, signed by offset, floats 0..1 clipped, 1/2/4-bit unpacked) and summed into a `u32`
  accumulator (at most CMYK + alpha channels).
- **Colour** at the end: grey, RGB, CMYK (naive inversion), full-range YCbCr, then alpha over white; the probe's
  orientation is applied once, as for every format.
- For a file in memory, a TIFF this decoder does not handle (palette, Lab, a strip above 32 MB) falls back to the
  `image` decode, so nothing that decoded before stops decoding.

## Alternatives considered
- **Keep `image`, convert YCbCr ourselves** - still full-resolution memory and no streaming.
- **libtiff via FFI** - a C dependency in every build and package for what the pure-Rust crate already decodes.
- **Lanczos after a full decode** - same memory problem; perceptual hashes downscale to 32x32 anyway.

## Consequences
- JPEG-compressed YCbCr TIFFs and pyramids decode; memory per decode is one strip plus the small result.
- Decoding cost is the `tiff` crate's (measured equal to the previous path: 365 ms vs 383 ms for a 6 MP 16-bit
  LZW file); a pyramid decodes from its small level in a few milliseconds.
- Floating-point TIFFs outside 0..1 are clipped, and ICC profiles are ignored, as before.

## Revisit when
The `image` crate supports chunked decoding of TIFF with YCbCr and reduced-resolution levels, or colour management
is added.
