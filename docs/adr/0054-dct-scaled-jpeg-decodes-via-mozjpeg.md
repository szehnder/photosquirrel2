# 0054. DCT-scaled JPEG decodes via mozjpeg (libjpeg-turbo core, plain C)

- Status: accepted
- Date: 2026-10-21
- Deciders: maintainers

## Context
ADR-0017 picked `jpeg-decoder` for DCT-scaled JPEG decodes (1/2, 1/4, 1/8) and `zune-jpeg` for full decodes.
The analysis decode of a 12 MP JPEG is a 1/4-scale decode (1000x750, the smallest scale reaching 512 px,
ADR-0010), and it dominates the per-file cost of a scan. ADR-0026 budgets analysis at >= 150 files/s on 16 cores
(`scan-analysis-12mp` <= 6.67 ms per file, warm cache). On the reference machine (Ryzen 7 4800H, balanced power
profile) the scan measured 8.6-9.0 ms per file (111-116 files/s):

- `jpeg-decoder` 1/4 decode: 34 ms wall but 54 ms CPU per file on one thread, because it spawns one worker
  thread per colour component per decode and sends every MCU row over a channel. With 15 analysis workers it
  reached 92 ms CPU per file (heavy sys time from thread churn) and 6.9 ms per file wall for the decode alone,
  above the whole-pipeline budget. No public API selects its single-threaded worker.
- `zune-jpeg` cannot scale in the DCT domain; a full 12 MP decode plus a box downscale cost 57 ms CPU and was
  memory-bandwidth bound in parallel (16.5 ms per file at 15 threads).

## Decision
DCT-scaled decodes of RGB and greyscale JPEGs use `mozjpeg` (the libjpeg-turbo decoder core, via `mozjpeg-sys`)
on the calling thread, built from C sources by the `cc` crate with `default-features = false` plus `unwinding`
and `parallel`: no nasm, no cmake, no system library, so it builds the same on Linux, macOS and Windows CI and in
the packaging image. libjpeg errors unwind into the existing `contain` guard. `jpeg-decoder` stays for CMYK/YCCK
JPEGs and as the fallback when mozjpeg fails or returns unexpected dimensions; `zune-jpeg` stays for full-size
decodes. Measured after the change: 32.5 ms CPU per 1/4 decode on one thread, 55.6 ms at 15 threads (3.97 ms
per file wall for the decode alone), and `scan-analysis-12mp` 4.64 ms per file (215 files/s).

Perceptual hashes are not re-versioned: over 40 synthetic 12 MP photos the two decoders differ by a mean of
0.17 per channel value (max 4), at most 2 bits of pHash and 1 bit of dHash, well inside the near-duplicate
thresholds; the medium ground-truth corpus gate (ADR-0025) scores precision, recall and keeper accuracy 1.0 with
no must-not-merge violations. Catalogs hashed before this change therefore regroup correctly with new rows.

## Alternatives considered
- **`turbojpeg` (libjpeg-turbo 3 TurboJPEG API)** - needs cmake, and nasm for its default `require-simd`, or a
  system library through pkg-config; more CI and packaging moving parts for the same decoder core.
- **mozjpeg with nasm SIMD** - SIMD mostly speeds up full-size IDCT and colour conversion, not the entropy
  decoding that dominates a 1/4 decode; it would add nasm to every build environment.
- **Patch or fork `jpeg-decoder` to use its immediate worker** - removes the thread churn but keeps the slower
  entropy decoder; measured headroom would be thin, and we would maintain a fork.
- **Decode at 1/8 scale** - 500 px long edge breaks ADR-0010's >= 512 px analysis rule, and the entropy decode
  cost is the same.

## Consequences
- A C dependency (IJG/BSD-3/Zlib licensed) compiled from source; builds need a C compiler, which every target
  already has for other crates.
- libjpeg pads truncated JPEGs with grey and warns instead of failing; such files now get a (partly grey)
  analysis decode and thumbnail rather than a decode failure.
- On-demand large previews and grid thumbnails that use DCT scaling get the faster decoder too.

## Revisit when
A pure-Rust decoder offers DCT-domain scaling at comparable speed, or `scan-analysis-12mp` regresses past its
baseline again.
