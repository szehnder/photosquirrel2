# 0084. TIFF directories beyond the probe window: from the analysis buffer, else bounded targeted reads (amends 0010)

- Status: accepted
- Date: 2026-10-24
- Deciders: maintainers
- Amends: [0010](0010-single-read-analysis.md)

## Context
ADR-0010 limits the probe to the first 256 KB. libtiff, ImageMagick, vips and most scanner software write a TIFF's
first image directory (IFD0) after the pixel data, at the end of the file: in a test folder of 16 TIFFs of one
picture, 13 had IFD0 beyond 256 KB (one at offset 36,000,008 of a 36 MB file). The probe then knows no dimensions,
orientation, EXIF or capture date, the classifier judges the file blind, and grouping refuses every look match
("dimensions unknown"), so an archive scan never meets its JPEG twin. The directories are small (a table of a few
hundred bytes plus strip arrays), but they can be anywhere: multipage files chain IFDs after every page.

## Decision
For **plain TIFFs only** (the prefix sniffs as TIFF, not as a TIFF-based RAW), when an IFD table, or a value the
probe or the decoder needs, lies beyond the prefix:

- **Files the scanner reads in full (<= 64 MB):** the scanner does the single full read before classifying and
  probes from the whole buffer. Bytes read stay exactly the file size; a TIFF classified junk this way costs its
  whole size (at most 64 MB) instead of 256 KB, accepted because the classifier cannot judge it without its
  directory and nearly all such files are photos or scans.
- **Larger files, and probes outside a scan (`probe_path`):** targeted reads. The walker runs over a view of the
  prefix plus fetched segments and records what it lacks; misses within 4 KB of each other are merged and fetched
  (4 KB read-ahead for a table of unknown size), and the walk repeats until nothing new is missing. Bounds: 1 MiB
  in total, 64 reads, no range requested twice, and the walker's existing bounds (64 IFDs, 1,024 entries, depth 4,
  chain IFD0-IFD8, cycle guard). Values never fetched: XMP, ICC, IPTC, Photoshop resources and layers, maker notes
  and DNG private tags; other values up to 64 KB each, strip/tile arrays up to 512 KB.
- Metadata of a file not held in memory comes from a compact TIFF written from the fetched directories (IFD0,
  Exif, GPS, Interop; strip arrays, sub-images and blobs left out), so the EXIF parser never needs the pixel data.

RAW files keep the prefix-only probe, so RAW dimensions, previews and the corpus gate cannot change.

## Alternatives considered
- **Always read the last 64 KB too** - misses multipage chains and IFDs in the middle, and costs a seek for every
  TIFF, including those whose IFD0 is at offset 8.
- **Probe after the full read for every TIFF** - files above 64 MB are not held in memory, and the probe facts
  are needed before the read to classify.
- **Raise the probe limit for TIFFs** - any limit below the file size misses end-of-file directories.

## Consequences
- End-directory TIFFs get dimensions, orientation, EXIF and dates, so classification and grouping work.
- The scanning budget becomes: junk at most 256 KB, except plain end-directory TIFFs (whole file up to 64 MB, or
  256 KB + 1 MiB); candidates at most size + 256 KB, plus 1 MiB of directory reads for TIFFs above 64 MB.
- A targeted read is a seek on a spinning disk or a round trip on a NAS: at most a few per large TIFF.

## Revisit when
BigTIFF (64-bit offsets) files appear in collections, or a RAW format turns out to keep its directories at the end.
