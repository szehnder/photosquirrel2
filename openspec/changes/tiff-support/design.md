# Design

## Context

See proposal.md for the why. Current state, read from the code:

- **Probe.** `ps_media::probe::probe_bytes(prefix, size, path)` truncates to `PROBE_LIMIT` (256 KB, ADR-0010) and
  walks TIFF IFDs with the bounded walker in `ps_media::tiff` (64 IFDs, 1,024 entries each, depth 4, cycle-safe,
  chain IFD0..IFD8). `probe_tiff` takes IFD0's dimensions for plain TIFFs and warns "first IFD lies beyond the
  probe window" when IFD0 is outside the prefix; nothing else is then known. TIFF-based RAW (CR2, NEF, ARW, DNG,
  PEF, SRW, 3FR, IIQ) shares the walker and `probe_tiff`.
- **Scanner.** `ps_scan::pipeline::process_file` reads the prefix, probes, classifies, submits `Probed`, then does
  the single full read (ADR-0010): files <= `ANALYSIS_BUFFER_MAX_BYTES` (64 MB) into one buffer (the prefix is its
  head; the in-flight memory budget is acquired for the file size), larger files streamed through
  `ps_hash::identity_from_reader` (full BLAKE3 <= 256 MB, sampled above, ADR-0011) keeping only the prefix. The
  CPU worker (`analyze`) gives streamed stills the placeholder "preview unavailable (file too large)" and no
  perceptual hash. Grouping then refuses look matches: no hash, and for end-IFD TIFFs no display dimensions
  (`NoMatch::UnknownDimensions`, `ps_dedup::edges`).
- **Decoding.** `decode::decode_scaled` decodes TIFF with `image` (`decode_generic`): a full-resolution decode
  converted to RGBA8, then downscaled by the caller. `image` 0.25.10 maps TIFF colour types itself and refuses
  YCbCr; it uses `tiff` 0.11.3 underneath, whose `Decoder::read_chunk` decodes one strip or tile (any
  compression it supports, predictors, byte order, WhiteIsZero) and returns raw samples, YCbCr included (its JPEG
  path keeps the input colour space).
- **Metadata.** `extract_metadata(bytes, ..)` parses EXIF with nom-exif over the bytes it is given: the whole
  buffer for files <= 64 MB, only the prefix for streamed files.
- **Byte accounting.** Scanning specs require junk to cost at most 262,144 bytes and an analysed file at most its
  size plus 262,144; `ps_scan::fs` has a byte-counting layer used by those tests. The corpus has no TIFF files.
- **Previews.** `ps_service::library` (2048 px preview) and `ps_service::ai` read the whole file and call
  `probe_bytes`, so end-IFD TIFFs lack orientation there too, and a 1 GB TIFF is read whole into memory.

## Goals / Non-Goals

**Goals:**
- Every 8-bit-displayable TIFF of the test folder (strips/tiles, chunky/planar, 8/16-bit, float, CMYK, alpha,
  JPEG YCbCr, multipage, pyramid, end-IFD, up to 1 GB) gets true dimensions, orientation, metadata, a thumbnail
  and perceptual hashes, and groups with the JPEG/HEIC copies of the same picture.
- No extra full read: still one read per file (plus the probe prefix), and junk TIFFs still cheap.
- Bounded memory for any TIFF size: one strip or tile plus a small accumulator.
- RAW probing, RAW decoding and the corpus gate unchanged.

**Non-Goals:**
- Colour management, HDR tone mapping, BigTIFF, palette/Lab decoding beyond the previous decoder, pages after the
  first as photos, re-analysis of already catalogued TIFFs whose content did not change (see Decision 8).

## Decisions

1. **Directory reads beyond the window (ADR-0084).** `ps_media::tiff::View` = prefix + `Segment`s at absolute
   offsets; the walker reads through it (`Tiff::parse_view`), so every existing bound applies unchanged and
   `Tiff::parse(&[u8])` behaves exactly as before. `Tiff::walk_noting` records what it could not read: IFD tables
   (read-ahead 4 KB for a table of unknown size) and out-of-line values the probe or decoder needs (`wanted_value`:
   at most 64 KB each, strip/tile offset and byte-count arrays up to 512 KB; XMP, ICC, IPTC, Photoshop, maker notes
   and DNG private tags never). `load_directories(prefix, size, fetch)` repeats walk -> merge misses (touching or
   within 4 KB) -> fetch until nothing new is missing, never asking for a range twice, at most 1 MiB in 64 reads.
   Entry points:
   - `probe_bytes`: unchanged (prefix only, still warns).
   - `probe_buffer(bytes, path)`: the whole file is in memory; a plain TIFF is walked over all of it.
   - `needs_directory_reads(prefix, size)`: plain TIFF (the prefix sniffs as `Format::Tiff`, not RAW) with misses.
   - `probe_with_directory_reads(prefix, size, path, fetch)`: prefix probe, or the loader plus the TIFF probe over
     the view; returns the segments for reuse.
   - `probe_seekable(reader, size, path)` / `probe_path`: the above with seek-and-read fetches.
   Only plain TIFFs ever take the new path; `sniff` and the RAW branches still see the prefix only.
2. **Scanner read order for TIFFs (ADR-0084).** After the prefix probe, a plain TIFF that `needs_directory_reads`:
   - <= 64 MB: acquire the in-flight budget for its size, finish the full read now, `probe_buffer` the buffer,
     then classify and continue with the buffer already read. Bytes read: exactly the file size, as for any
     candidate; a junk result costs the whole file (at most 64 MB) instead of 256 KB, accepted because the
     classifier cannot judge a TIFF without its directory and such files are almost always photos or scans.
   - > 64 MB: targeted reads through the lane's `Controlled` reader (cancellation checkpoints and byte counts
     apply), then seek back to the end of the prefix. At most 1 MiB more than size + 256 KB.
3. **Strip-wise TIFF decoder (ADR-0085).** `ps_media::tiff_decode::decode(reader: Read + Seek, min_long)` over the
   `tiff` crate (direct dependency, same 0.11 as `image`):
   - Level: the first image, or the smallest reduced-resolution directory of the main chain (NewSubfileType bit 0,
     aspect within 1% of IFD0, long edge >= `min_long`), looking at most at the walker's 9 chain directories.
     Pages (bit 1) and thumbnails smaller than `min_long` are never used; the first page is the image.
   - Scale: integer box factor `k = max(1, long_edge / min_long)`, output `floor(w/k) x floor(h/k)`: the long edge
     stays >= `min_long` (512 for analysis, 384 for thumbnails, 2048 for previews), like the JPEG DCT scales.
   - Chunks are visited in file-offset order (a one-pass reader only moves forward); each is checked against
     `TIFF_CHUNK_MAX_BYTES` (32 MB, decoded and compressed) before it is read, then every sample is converted to 8
     bits (integers by their bit depth, signed by offset, floats 0..1 clipped, 1/2/4-bit unpacked) and summed into
     a `u32` accumulator per channel (at most 5: CMYK + alpha). Planar chunks fill their own channel.
   - At the end, per output pixel: grey, RGB, CMYK (`(255-c)(255-k)/255`), full-range YCbCr (JFIF), then alpha
     over white (associated or unassociated per ExtraSamples), like `decode_generic`. Orientation is applied by
     the caller from the probe (tag 0x0112 in IFD0, ADR-0010's single orientation step).
   - Errors: `Unsupported` (palette, Lab, non-JPEG subsampled YCbCr), `TooLarge` (new `DecodeError` variant:
     chunk above 32 MB or accumulator above 256 MB), `Failed` (corrupt). For a file in memory
     (`decode_scaled`), any error falls back to `decode_generic`, so nothing that decoded before
     stops decoding.
   - Stats (`TiffDecodeStats`: level, factor, chunks, peak chunk and accumulator bytes) for tests and budgets.
4. **One pass for TIFFs above 64 MB (ADR-0086).** `ps_hash::IdentityStream` computes the identity from bytes fed in
   file order (full: all bytes; sampled: size + the three windows), equal to `identity_from_reader`.
   `ps_scan::tiff_pass::OnePass` is a `Read + Seek` over the lane's reader: reads inside the prefix or the fetched
   directory segments are served from memory; any other read must be at or after the physical position, and the
   gap is read through and hashed when the identity needs it, else skipped with a seek; a backward read is an
   error (the decoder then fails and the file gets a placeholder). After the decode, `finish` reads the rest the
   identity needs, checks the end of the file (full identities) and returns the identity; an I/O error or a
   cancel seen during the decode is reported as a read failure, exactly like the streamed identity today. The
   in-flight budget is acquired for 64 MB (one analysis buffer). The decoded image travels to the CPU worker in
   the job (`Content::Streamed { decoded, metadata_block }`), which hashes it and makes the thumbnail; a decode
   error gives the placeholder "preview unavailable (<reason>)" and the identity is kept.
5. **Metadata of streamed TIFFs (ADR-0084).** `Tiff::metadata_block` writes a compact TIFF (same byte order) with
   IFD0, Exif, GPS and Interop entries whose values are known, minus strip/tile arrays, sub-IFDs and blobs, offsets
   rewritten; `extract_metadata` gets that block instead of the prefix, so nom-exif reads it like any TIFF. Files
   <= 64 MB keep nom-exif over the whole buffer.
6. **Previews.** `ps_service` probes previews with `probe_buffer`. The 2048 px preview of a plain TIFF above 64 MB
   probes with `probe_path` and decodes from the open file with `decode_tiff_reader` (bounded memory) instead of
   `read_all`.
7. **Fixtures.** `ps_fixtures::tiff_image`: a `TiffSpec` (dimensions, layout strips/tiles, samples
   grey/RGB/RGBA/CMYK/YCbCr-JPEG, bits 8/16/float32, planar, compression none/LZW/Deflate/JPEG with predictors,
   directory placement before/after the data, orientation, ExifSpec, extra pages, reduced-resolution levels),
   written by the fixture TIFF writer (extended with a data-first layout and multiple strips) and, for LZW/Deflate,
   by the `tiff` crate's encoder (which itself writes directories after the data). Pictures come from
   `photo_rgb`, so tests compare against the same 8-bit RGB. No corpus preset uses them.
8. **Existing catalogs.** An unchanged, thumbed plain TIFF row without dimensions (the end-IFD symptom of the
   previous probe) is analysed again on the next rescan (`DiffCounts::reanalyzed`, processed from `discovered`
   like a changed file, no migration). After this change such a row cannot recur (a TIFF whose IFD0 cannot be read
   fails), so it is re-read once. Thumbed TIFFs above 64 MB with a placeholder but known dimensions are not
   re-queued (no marker distinguishes them without a migration); they get the new analysis when their content
   changes. Deferred to a later change if needed.
9. **Budgets (ADR-0087).** `tiff-analysis-decode`: median analysis decode + thumbnail of a 12 MP (4000x3000) 16-bit
   LZW TIFF with horizontal predictor and end directory, from memory, single-threaded. `tiff-stream-memory`:
   working memory reported by the decoder for a 6000x4000 16-bit strip TIFF decoded through a reader. Both are
   `measure` tests in `ps-media` (`tests/tiff.rs`), run by `cargo xtask bench`.

## Risks / Trade-offs

- [The `tiff` crate's LZW decoder is about 100 MB/s on the dev machine] -> same cost as the previous `image`
  path (measured 365 ms vs 383 ms for a 6 MP 16-bit LZW file); a 1 GB LZW scan takes about 8 s of lane CPU,
  about as long as reading it from a spinning disk.
- [Decoding on the I/O lane blocks that lane's reader for the decode time] -> only for TIFFs above 64 MB, and
  cancellation still reaches every 1 MB read; lanes have 2-16 readers.
- [Box downsampling differs from Lanczos] -> perceptual hashes downscale to 32x32 anyway; measured MAD 0.09 vs
  a box-resized reference, and grouping thresholds (ADR-0013) are far wider.
- [A reduced-resolution level could be stale] -> same aspect ratio required; JPEG/RAW previews carry the same risk.
- [Junk end-IFD TIFFs <= 64 MB are read whole] -> bounded by 64 MB; documented in the scanning spec.

## Migration Plan

No schema change. Rows of TIFFs whose first IFD was beyond the window are re-analysed once by the next rescan
(Decision 8). Rollback: revert; rows written by the new code stay valid for the old code.

## Verification

- `cargo test -p ps-media -p ps-hash -p ps-scan -p ps-fixtures -p ps-service` (TIFF scenarios, RAW probe tests).
- `cargo xtask ci` in a clean worktree.
- Corpus gate: `photosquirrel-cli fixtures generate --out D/corpus --seed 42 --preset medium`, `sources add`, `scan`,
  `eval-corpus --data-dir D/data D/corpus`: precision/recall/keeper accuracy 1.0000, 0 must-not-merge, 1,843 keeper
  groups, as before.
- Format folder: fresh data dir, scan `target/agent-review/tmp/fmt/src` with the release CLI, per-file stage,
  dimensions, perceptual hash and group before and after.
- `cargo xtask bench --check` for the two new budgets.
- `openspec validate tiff-support --strict`, `cargo xtask spec-coverage`.
