# 0088. A TIFF decode is capped at 4 GiB of work and checks pause and cancel before every strip or tile (amends 0085, 0086)

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0085](0085-strip-wise-tiff-decoding.md), [0086](0086-one-pass-analysis-of-large-tiffs.md)

## Context
ADR-0085 bounds the *memory* of a TIFF decode (one 32 MB chunk plus the accumulator) but not its *work*: the
decoder accepts any dimensions and decodes every strip or tile, and chunks may share one data offset. A gate review
measured a 16.8 MB uncompressed file declaring 81920x81920 pixels in 400 tiles sharing one 16 MB block: 29.6 s of
decoding and an image (the `image` crate's 768 MB allocation cap refused the same file in 110 us). With 1 MB of
offset arrays (about 262,000 tiles) the work reaches hours per file, and Deflate makes such a file a few hundred
KB. The decode also had no pause or cancel checkpoint: a CPU worker decoding a TIFF from memory never looked at
the scan control, and the one-pass reader (ADR-0086) checks it only on physical reads, which chunks served from the
probe's prefix or segments skip. ADR-0008 requires pause and cancel within 250 ms.

## Decision
- **Work cap.** Before any chunk is read, the decoder computes the work of the chosen directory from its header: the
  decoded bytes of every strip or tile (tile padding included, a sample below 8 bits counted as the byte it is
  unpacked to), and separately the compressed bytes its byte counts claim, a block shared by several chunks counted
  once per chunk. Each must be at most **4 GiB** (`TIFF_MAX_WORK_BYTES`), else the decode fails with
  `DecodeError::TooLarge` naming the dimensions and the size, so the scanner's placeholder gives the reason and the
  identity is kept (ADR-0086). 4 GiB is the most a classic (non-BigTIFF) file can hold, so every uncompressed TIFF
  is admitted (a 14000x10000 16-bit RGB scan is 0.8 GiB) and a compressed or shared-block file costs at most what
  the largest uncompressed one does: measured 18 s of one core for exactly 4 GiB of shared uncompressed tiles.
  Duplicate and overlapping offsets are counted, not refused: writers that share one blank tile stay decodable.
- **Levels first.** The smallest reduced-resolution level reaching the requested size is still preferred; when
  none does and the first image exceeds the cap, the largest level within the cap is decoded (a smaller preview
  beats a placeholder).
- **Stop check.** `tiff_decode::decode_with(reader, min_long, stop)` calls `stop` before every strip or tile (at
  most 32 MB of decoding apart) and returns `DecodeError::Cancelled` when it returns true; the decode of a file in
  memory then skips the `image` fallback. The scanner passes its `Busy::checkpoint` (which parks while paused) in
  the CPU worker (`analyze_still_with`) and in the one-pass lane decode; a cancelled CPU job is dropped and its file
  stays `probed`, like a queued job; a cancelled one-pass decode is a cancelled read.
- **Arithmetic.** The box factor is at most 4104 (`4104^2 x 255 < 2^32`, so the `u32` accumulator cannot
  overflow); along an edge shorter than the factor the box spans that edge; chunk counts use checked
  multiplication; and when the accumulator would exceed its 256 MB the factor grows until it fits, so a 2048 px
  preview of a 5-channel image just under 4096 px decodes at factor 2 (2047 px) instead of failing.

## Alternatives considered
- **A pixel cap (1 gigapixel)** - redundant with the byte cap for 8-bit and wider samples and too strict for
  greyscale scans; the work is what costs time.
- **The `image` crate's 768 MB cap** - refuses the 0.8-1 GB scans ADR-0086 exists for.
- **Refuse shared or overlapping chunks** - breaks writers that share a blank tile; counting them bounds the work.
- **A wall-clock deadline** - not reproducible across machines; the work cap is deterministic.

## Consequences
- A hostile TIFF costs at most about 18 s of one core (4 GiB), interruptible within one chunk; the reviewer's file
  is refused before any chunk is read.
- A legitimate classic TIFF decoding to more than 4 GiB (a 30000x24000 16-bit RGB LZW scan) gets a placeholder
  naming the reason unless it has a reduced-resolution level; its identity and metadata are kept.
- `decode_with`, `decode_tiff_reader_with`, `decode_scaled_with` and `analyze_still_with` are new public entry
  points; the existing ones behave as before with a stop check that never fires.

## Revisit when
BigTIFF is supported, or a real archive of TIFFs above the cap without reduced-resolution levels turns up.
