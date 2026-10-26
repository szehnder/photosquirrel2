# 0095. HEIC and AVIF decode failures keep the file; decode memory is budgeted; libheif's limit by version

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0091](0091-rows-remember-a-missing-decoder.md) (decode errors of new files), [0089](0089-heic-through-the-operating-systems-decoder.md)
  (the libheif size limit), [0009](0009-threaded-pipeline-with-io-lanes.md) (a decode budget beside the buffer budget)

## Context
The gate review of `heic-system-decoders` found three gaps:
- A **new** HEIC or AVIF the decoder rejects (truncated, corrupt, or declaring more pixels than the decode cap) failed
  analysis and became `unreadable`, so it was never exported once a decoder was installed. In 0.2.0, without a
  decoder, the same file got a placeholder and was exported. A re-decode already keeps the exact-copy analysis
  (ADR-0091).
- A decode's working memory was not budgeted: the scan bounds file buffers (ADR-0009), but a HEIC decode holds the
  system decoder's image plus our copy of it, and an AVIF decode rav1d's picture plus the RGB output. Parallel
  decodes of hostile files declaring huge sizes could use several gigabytes at once.
- `heif_context_set_maximum_image_size_limit` takes a **per-side** limit up to libheif 1.17 and the square root of an
  **area** from 1.18. The loader passed the square root of the area cap (16,384) to every version, so on Ubuntu 22.04
  (libheif 1.12) panoramas wider than 16,384 px were refused.

## Decision
- **HEIC and AVIF**: a decode failure or a size refusal during analysis keeps the file's identity and metadata with a
  placeholder ("preview unavailable (could not be decoded: ...)"), reports the failure, and leaves the file
  catalogued, grouped by exact content and exported, never `unreadable`. Other still formats keep failing as before:
  their decoders are bundled and a failure means the data cannot be read; the ground-truth corpus counts them.
- **Decode budget**: HEIC and AVIF decodes reserve their estimated working memory from the declared size (HEIC 6
  bytes per pixel, AVIF 9) in a per-run decode budget of 1 GiB (`ScanOptions::decode_memory_cap`), separate from the
  buffer budget so a decode waits only for other decodes and cannot deadlock with readers; a decode larger than the
  cap runs alone. Huge images therefore decode one at a time.
- **libheif's limit by version**: up to 1.17 the loader passes the area cap itself as the side limit (no side of an
  image within the area can reach it); from 1.18 the square root of the cap. With every version our own check
  refuses an image over the area cap from the primary image handle's size before decoding.

## Alternatives considered
- **Placeholders for every format's decode failure** - would turn corrupt JPEGs into exported files and change the
  corpus gate's unreadable counts; reconsider with evidence.
- **Count decodes in the buffer budget** - a CPU worker holding its file buffer could wait for budget held by queued
  buffers that only it can release (a deadlock).
- **Keep libheif's limit at 16,384 and accept refused panoramas** - silently loses previews of real photos.

## Consequences
- A truncated HEIC or AVIF is exported byte for byte, as in 0.2.0, with its failure listed among the run's errors.
- Scans of huge HEIC/AVIF files are slower (serialised) but bounded in memory.

## Revisit when
Other formats' failures should be catalogued too, or decode estimates prove too coarse for real files.
