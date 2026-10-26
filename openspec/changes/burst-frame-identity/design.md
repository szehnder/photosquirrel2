# Design

## Context

Evidence from the owner's SD card (read-only scan, catalog under `target/`):

| Frame | File | Sub-second | OriginalRawFileName | Canon file number | Group |
|---|---|---|---|---|---|
| N | IMG_0034.CR2, .JPG (two copies) | 00 | - | 101-0034 | 201 |
| N | IMG_0034.DNG | absent | IMG_0034.CR2 | 101-0034 (in DNGPrivateData) | **200** |
| N+1 | IMG_0035.CR2 (two copies), .JPG (two copies), .DNG | 13 | IMG_0035.CR2 (DNG) | 101-0035 | 200 |

Group 200's keeper is IMG_0034.DNG (larger file), so frame N+1 has no keeper of its own. The same happened for
frames 0022, 0024, 0028 and 0033. In the catalog, `file.metadata` holds `ps_model::Metadata` (fields, exposure,
capture date, `likely_camera_original`), not the full tag list, so old rows cannot be corrected from stored facts.

## Decisions

### D1. Frame identity

`ps_model::Metadata.fields.frame_id: Option<FrameId>`, where a `FrameId` is a short normalised string with its
source:
- `OriginalRawFileName` (DNG tag 0xC68B, IFD0): the file stem, upper-cased (`IMG_0034`). Present in every
  Adobe-converted DNG.
- Canon maker-note file number (Canon tag 0x0008 `FileNumber`, a u32 such as 1010034, shown as `101-0034`), from
  the EXIF maker note or, in a DNG, from the maker note inside `DNGPrivateData` (tag 0xC634: `"Adobe\0MakN"`, a
  length, the original byte order and offset, then the note). Nikon `ShutterCount` and Sony `ShotNumber` only if
  they are a small, contained addition; otherwise later.

Two frame ids are compared only when they have the same source kind, except that a DNG's `OriginalRawFileName` stem
also compares with the stem of a RAW file that **is** that original (see D2). Reading stays inside the bytes the
probe already reads (the first 256 KB plus the ranges the TIFF walker follows); no new reads.

### D2. The contradiction

`metadata_contradiction` gains `Contradiction::FrameId { a, b }`: both files have a frame id of the same kind, the
camera model key matches (or either is missing), and the ids differ. It is shown as "recorded as different frames
(101-0034 and 101-0035): burst frames".

Comparing a DNG's `OriginalRawFileName` with a RAW file's own stem: a RAW file's name is the camera's name only when
the file is a camera original; renamed RAWs exist. So a stem is never a signal that two files are the same frame.
It is used only to keep files apart: both are RAW-format files of the same model, both frame ids are present, and
they differ. A resized JPEG copy never gets a frame id from its name.

A resized or recompressed copy of frame N keeps its maker notes or loses them; it never gains a different frame id,
so legitimate derivatives are unaffected.

### D3. RAW files are camera originals for the two-originals guard

`has_maker_notes` becomes true when `DNGPrivateData` carries maker notes. Separately, the two-originals guard
(ADR-0035) treats any RAW-format file as a camera original. The guard's "identical stored pixel dimensions" clause
stays: CR2 5472x3648 and its DNG 5568x3708 differ in dimensions and are still allowed to join (same frame, two
encodings), while two DNGs of two frames (same dimensions, different content) are kept apart.

### D4. Old catalogs: a metadata version

Migration 23 adds `file.frame_id TEXT` and `file.meta_version INTEGER NOT NULL DEFAULT 0`. `META_LOGIC_VERSION = 1`
in `ps-media`. On a rescan, unchanged analysed RAW-format rows (DNG included) with `meta_version < 1` are re-read
for metadata only: the probe's header bytes, read-only, no decode, no hashing. Their metadata, `frame_id` and
`meta_version` are written, and the dedup stamp is bumped so the regroup at the end of the run sees them. Non-RAW
rows are stamped to version 1 by the migration without reading (a JPEG's maker-note file number is a nice-to-have
for JPEG-to-JPEG bursts, which already carry sub-seconds; it is filled in when the file is next analysed).

The scan summary reports "N RAW files re-read for frame numbers, M groups split".

### D5. Effects of a split on export and cleanup

- Export: the separated frame is a new group whose keeper is not in the library: it is pending, counted in the
  report, and the next export copies it. ADR-0092 (no copying twice) is unaffected: the old group's library copy
  still covers the old frame.
- Cleanup: ADR-0096/0099 already re-derive coverage from the current groups before each move, so no new file is
  moved on the strength of the wrong frame. A file already in quarantine whose covering library copy belongs to
  another frame is, after the regroup, a member of a group with no library copy: the existing rule "accounted for
  only while the library copy that makes it safe exists" reports it as pending with its quarantine path. The report
  adds the reason "its group changed: the library copy is of another frame" and the hint `cleanup undo` /
  "Restore" for that run.

### D6. Corpus

`ps-fixtures` adds a `converted_raw_bursts` count to each preset (small 2, medium 24, large 140). Each set is 2 or 3
frames in one second: a RAW-like original per frame (the existing DNG writer with a CR2-style stem), a camera JPEG
per frame, and a converted DNG per frame carrying `OriginalRawFileName` and a Canon maker note in `DNGPrivateData`;
the first frame's sub-second is `00` in the original and JPEG and **absent** in the converted DNG. Every frame's
files form one truth group; the frames of a set form a must-not-merge set. The medium-preset baseline (pairs,
groups, files, reclaimable bytes, suggestions) is re-recorded in the gate docs; precision, recall and keeper
accuracy stay 1.0000 with 0 violations. Before the fix, the new sets must produce violations (the test proves the
corpus would have caught the bug).

## Risks

- A maker note that misreports a file number would split a real derivative pair: it only ever splits, never merges,
  and the pair becomes a suggestion the user can merge.
- Re-reading RAW headers on the first rescan costs about one small read per RAW file (the card: 1,458 RAW and DNG
  files, a few MB in total). Offline sources are re-read when they come back.

## Deviations (as built)

- **A frame identity holds both kinds.** `FrameId` has two optional parts, the `OriginalRawFileName` stem and the
  Canon file number, because a converted DNG records both; each kind compares only with the same kind.
- **Canon file numbers also come from `CameraInfo`.** The owner's EOS 70D (like every current Canon DSLR) writes no
  tag 0x0008; its file number is `FileIndex` + 1 and `DirectoryIndex` - 1 at model-specific offsets of `CameraInfo`
  (0x000D). A table of the models with known offsets is read; out-of-range values are ignored.
- **No stem comparison (D2's second paragraph).** A RAW file's own name is never treated as a frame identity: names
  are not recorded identity (the proposal puts names out of scope), a renamed CR2 would be split from its own DNG,
  and the Canon file number in both files covers the CR2-vs-DNG case.
- **The re-read covers more than RAW rows (ADR-0106).** Re-reading RAW rows only left old catalogs wrong: the
  JPEGs of a burst have no stored frame number, so frame 34's converted DNG could still join frame 35's JPEGs and
  CR2 and push frame 35's own DNG out (found by the cleanup test, `ps-service/tests/frame_identity.rs`). Migration 23
  therefore also leaves Canon camera files of the same capture second as a RAW-format row for the re-read, and a
  row in a quarantine folder is re-read there (read-only). The incremental-rescan delta says so.
- **The re-read merges, it does not replace.** A re-read reads the probe window only, so it writes the frame
  identity, `has_maker_notes` (or-ed) and the camera-original signal (or-ed) into the stored metadata and keeps the
  rest of the metadata the full analysis read.
- **`META_LOGIC_VERSION` lives in `ps-model`** next to `DATE_LOGIC_VERSION` (the catalog and the scanner both read
  it); `ps-media` re-exports it.
- **One test hook.** `ps_model::frame::set_frame_identity_enabled(false)` turns off reading frame identities and
  `DNGPrivateData` maker notes (ps-media) and the frame and RAW-original rules (ps-dedup), so a test can group a
  corpus the way the build before this change did. It is only called by tests.

## Evidence: the corpus catches the bug (task 2.2)

The medium preset (seed 42) with the 24 converted-burst sets (60 frames, 240 files), scanned from drive-a/b/c by
the release CLI built from the code **before** this change's fix (fixtures only, commit after ADR-0105):
10,173 files in 17.6 s, 0 errors; `eval-corpus`: precision 0.9988, recall 0.9991, keeper accuracy 0.9968
(1,897 of 1,903), **6 must-not-merge violations**, all converted bursts (`m0506`, `m0508`, `m0511`, `m0516`,
`m0525`, `m0526`: each a result group holding two frames of one set); 19,741 true pairs, 19,747 result pairs,
19,723 correct. `groups list --json`: 1,903 groups, 8,834 grouped files, 4,162,408,110 reclaimable bytes, 6,254
suggestions. Gate: FAIL. The same merge as on the SD card: the first frame's DNG (no sub-second, maker notes only in
`DNGPrivateData`) joined the next frame's group.

## Gate after the fix (task 7.1)

Medium preset, seed 42, drive-a/b/c, release CLI: precision, recall and keeper accuracy 1.0000, 0 must-not-merge
violations. New baseline: 19,741 pairs, 1,903 groups, 8,834 grouped files, 4,166,018,502 reclaimable bytes, 6,398
suggestions (old: 19,381 / 1,843 / 8,594 / 4,048,886,433 / 5,570). The differences are exactly the converted-burst
sets: +60 groups, +240 files, +360 pairs, +117,132,069 bytes (each frame's CR2 and camera JPEG), +828 suggestions
(768 across frames of a set, 60 CR2-to-JPEG-copy pairs inside a frame's group; see tasks.md 7.1).

## The owner's SD card (task 7.2)

Read-only mount (`ro` checked), a copy of the scratch catalog written by the pre-fix CLI (schema 22), rescanned by
the release CLI of this change; nothing exported, cleaned up or sent to AI, nothing written to the card.

- Before: 2,632 files, 752 groups with 2,575 files, 2,145 suggestions. **18** groups held two frames, not 5: the
  five `IMG_` frames named in the proposal (0022, 0024, 0028, 0033, 0035, each with the previous frame's DNG as its
  keeper) and 13 more `_MG_` frames (the camera's Adobe RGB file names) with the same merge.
- Rescan: 2,591 files re-read for frame numbers, 679 MB read in 16.5 s (header only), all opened read-only (strace:
  0 write-capable opens): 1,071 CR2, 387 DNG and 1,133 JPG (every camera JPEG on the card has a RAW file in its
  second, ADR-0106); no MOV, PSD or other file was read. The summary reported "18 groups split".
- After: 752 groups with 2,575 files (unchanged), 2,291 suggestions (+146, all between the separated frames); 0 groups
  mix two Canon file numbers or two `OriginalRawFileName` stems, 0 groups hold two file stems. The 18 DNGs moved back
  to their own frame's group (CR2+JPEG groups 390 -> 372, CR2+JPEG+DNG groups 350 -> 368); each of frames 0022,
  0024, 0028, 0033 and 0035 now has its own DNG as its keeper. All 734 single-frame groups of before are intact (no
  legitimate CR2+JPG+DNG group was split), and no file became ungrouped.
- Found on the way, not changed here: on this exFAT card, a first rescan of the untouched copy saw 1,355 of 2,632
  files as "changed" because their inode numbers (the fingerprint's `file_index`) differ after a remount, and
  re-analysed them in full (31 GB, 6 minutes). The check above therefore ran on a fresh copy of the catalog with
  `file_index` cleared (as on a filesystem with stable file ids). Worth its own change: exFAT/FAT inode numbers are
  not stable file identities (ADR-0007).
