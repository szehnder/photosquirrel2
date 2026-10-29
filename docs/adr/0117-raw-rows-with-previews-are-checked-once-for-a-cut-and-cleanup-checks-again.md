# 0117. RAW rows with previews are checked once for a cut, and cleanup checks a RAW again before a move

- Status: accepted
- Date: 2026-10-28
- Deciders: maintainers
- Amends: [0115](0115-a-raw-file-cut-short-is-damaged-and-damaged-files-are-counted.md) (old catalogs; the structure
  check by targeted reads; RW2 and CR3 track evidence; one rule for the damaged count),
  [0105](0105-recorded-frame-identities-keep-frames-apart.md) (`META_LOGIC_VERSION` 2), [0024](0024-reversible-quarantine-cleanup.md)
  (the pre-move check)

## Context
ADR-0115's generation bump re-reads only RAW *placeholders* of generation 0. In 0.3.0 a CR2 cut inside its sensor
strip (the likely cut: the strip is most of the file and follows the embedded preview) got real preview pixels and
perceptual hashes, joined its camera JPEG by look, could be a keeper, and cleanup could quarantine a file it covered.
Such rows have no placeholder, so nothing ever looked at them again. The analysis judges truncation over the whole
file in memory; reading every catalogued RAW in full again would cost a full read of each card.

The burst-frame-identity change (ADR-0105, ADR-0108) already has a one-time, per-row header re-read: `file.
meta_version` (migration 23), `META_LOGIC_VERSION`, the diff's `needs_frame_reread`, `ps_media::reread_frame_facts`
with bounded targeted directory reads (`tiff::load_directories`, ADR-0084), and cleanup's `RescanFirst` gate while
rows are not re-read. Migration 26 is reserved for `adobe-rgb-color`.

## Decision
- **One more metadata version, no migration.** `META_LOGIC_VERSION` is 2: "the structure of a RAW was checked for a
  cut". `ps_model::meta_reread_due(format, placeholder, meta_version)`: every row below 1 (frame identity), and a
  RAW-format row with preview pixels below 2. Version 2 changes nothing for other rows, so non-RAW rows stay current
  at 1 and are never read; a RAW placeholder was judged by this version's decode, or is re-read in full by the RAW
  generation bump. New analyses stamp 2.
- **The pass reads directories only.** A due row of version 1 has its first 64 KB read (`raw_check::STRUCTURE_HEAD`)
  plus bounded targeted reads (`raw_check::check_by_reads`): the TIFF directories and strip/tile arrays within the
  ADR-0084 bounds, CR3 top-level box headers and `moov`, the RAF header. A row of version 0 gets both checks from the
  frame re-read's one read (`ps_media::reread_header`). Only when a bound leaves a range unknown is a file of at most
  64 MB read whole once (none of 45 real RAWs of 13 makers needed it). Measured cost: 64-76 KB and 1-4 reads per
  RAW (0.1-1.2% of the file).
- **A file found cut** loses the perceptual hashes of its content (every thumbed row of that content), its thumbnail
  becomes the "could not be read" placeholder with the cut-short message, the row gets the `unreadable` stamp at the
  current RAW generation, and the dedup stamp changes, so the end-of-run regroup separates it from its camera JPEG.
  It is reported once as a run error and counted in `damaged` (the catalog aggregate). A file not cut is stamped 2,
  so the pass never repeats. Rows of offline sources wait until their source is scanned. Rows whose directories
  lie beyond the bounds in a file over 64 MB are stamped without a judgement (logged).
- **Cleanup waits for it.** `files_awaiting_meta_reread` (the `RescanFirst` gate of ADR-0108 and the report's
  "not confirmed") includes present RAW rows with preview pixels below version 2, so a plan of an upgraded catalog
  is refused until the rescan. Missing rows are not included: they cannot be re-read, and the next point covers them.
- **Cleanup checks a RAW again before every move**, whatever the catalog says (`quarantine::run`): after the source
  re-hash, a RAW source is checked by targeted reads and kept as `SkipReason::Damaged` when cut; the library copy
  that makes a file safe is checked once per run and, when it is a RAW cut short, the file is kept
  (`SkipReason::CoveringDamaged`). This also covers RAWs the analysis could not judge (over 64 MB).
- **Evidence, refined on real files** (raw.pixls.us, CC0): an IFD table's trailing 4-byte next pointer may be missing
  at the very end of the file (only its entries count); RW2/RWL with Panasonic "RAW 1" compression (tag 0x000B =
  34316) needs `RawDataOffset` + ceil(w * h / 14) * 16 bytes (it records no length); a CR3's tracks (`stco`/`co64`
  with `stsz`) are evidence too, so a CR3 cut exactly at a box boundary is found. Strip ranges keep no tolerance:
  no real file ran past its end.
- **One rule for "damaged":** present files with the `unreadable` reason, in the run summary and in the report
  (`FileFacts.damaged` excludes missing files).

## Alternatives considered
- **A new per-row column or migration 26** - the same effect as a version number the catalog already has, with a
  migration and a renumbering of `adobe-rgb-color`.
- **A one-time catalog task (ADR-0111) that reads every RAW at open** - it would read offline sources' files at open
  or never; the rescan is where files are read, and the diff already carries the per-row rule.
- **Re-read every RAW with a preview in full** - a full read of every card for a rare cut; directories suffice.
- **Clamp strips that run past the end (rawspeed does for some ARW/ORF)** - none of the 45 real files does; a
  tolerance would hide the cut it is meant to find. A camera found doing it gets a narrow exception and a fixture.
- **Only the cleanup check, no catalog pass** - grouping, keepers and export would stay wrong until the file changes.

## Consequences
- The first rescan after the update opens each RAW with a preview once and reads 64-80 KB of it; an unchanged,
  already-stamped catalog reads nothing (the `rescan-nochange-100k` budget is unchanged).
- Until that rescan, cleanup refuses plans whose groups bear such a RAW ("rescan first").
- Blind spots: MRW, X3F, CRW, the Sony DSLR-A100's ARW and RW2 with other Panasonic compressions have no check; a
  RAW over 64 MB is not labelled damaged by a scan (the analysis never holds it whole and keeps its "files over 64
  MB" placeholder, without hashes, so it joins no group by look), though cleanup's pre-move check still finds its
  cut; a cut that removes only bytes no directory records (trailing padding) is not a cut to the check.

## Revisit when
A camera writes a strip, tile or track past the end of a good file, a RAW format without a check shows up cut in
real libraries, or the analysis buffer limit changes.
