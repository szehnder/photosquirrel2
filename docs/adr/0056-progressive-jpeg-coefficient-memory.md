# 0056. Progressive JPEG coefficient memory: serialize large decodes, refuse hostile ones

- Status: accepted
- Date: 2026-10-21
- Deciders: maintainers

## Context
A progressive JPEG cannot output a row until every scan is read, so libjpeg (ADR-0054), jpeg-decoder and
zune-jpeg all buffer the image's full-resolution DCT coefficients, 2 bytes each, whatever the output scale. A
250 MP 4:4:4 progressive file needs 1.5 GB; with 15 analysis workers (ADR-0009) several such files at once could
take the machine down. libjpeg has no cap set (`max_memory_to_use` is not reachable through the mozjpeg crate, and
without a backing store a cap would only turn into a failure), and jpeg-decoder's `max_decoding_buffer_size`
covers its output buffer, not its coefficients. A hostile header can claim 65535 x 65535 in a few bytes.

## Decision
Before any decoder runs, the frame header decides a memory plan from an upper bound of the coefficient buffer
(width and height in 8x8 blocks x 64 x 2 bytes x components, as if 4:4:4): sequential files and progressive files
up to 128 MiB (about 22 MP) decode on the calling worker as before; above that a decode waits on one process-wide
lock, so at most one large progressive buffer exists at a time; above 2 GiB (about 350 MP) the decode is refused
and the file is recorded as `analysis failed`. Worst case: 15 x 128 MiB plus one 2 GiB decode.

## Alternatives considered
- **Route large progressive files to jpeg-decoder** - it buffers the same coefficients, uncapped.
- **Set libjpeg's `max_memory_to_use`** - not exposed by the crate, and with no backing store it only fails large
  files that the machine could decode one at a time.
- **Count large decodes against the scan's read-buffer budget** - more precise but couples ps-media to the
  scanner's budget; the lock is local and enough at this scale.

## Consequences
- Libraries full of large progressive JPEGs analyse them one at a time (slower, bounded memory).
- Progressive files above ~350 MP are not analysed: they are recorded as `analysis failed` with the reason and
  listed among the scan's failures, as corrupt files are (ADR-0055), never silently grouped.
- Multi-scan sequential files are not detected from the header; for the mozjpeg path their buffer is bounded by
  the existing 256 MP output cap.

## Revisit when
Real libraries show progressive files above the refusal bound, or scans of large progressive files become a
bottleneck.
