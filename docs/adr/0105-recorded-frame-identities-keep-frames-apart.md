# 0105. Recorded frame identities keep frames apart

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0035](0035-burst-protection-in-grouping.md) (the metadata test of a derivative edge and the
  two-camera-originals rule), [0039](0039-contradictions-are-group-level-cannot-link-constraints.md) (the new
  contradiction is a group-level cannot-link constraint like the others), [0007](0007-incremental-rescan-by-stat-fingerprint.md)
  (a one-time metadata re-read of unchanged RAW rows)

## Context
On the owner's Canon EOS 70D SD card (CR2 originals, camera JPEGs and Adobe-converted DNGs of the same shots), five
burst frames were grouped with the frame shot just before them. Canon writes the first frame of a burst with
sub-second `00`; Adobe's DNG converter drops `SubSecTimeOriginal` when it is `00`, so frame N's DNG had no
sub-second time and the ADR-0035 sub-second rule could not separate it from frame N+1. The two-camera-originals rule
needs maker notes on both files, and Adobe moves Canon's maker notes into `DNGPrivateData`, so the DNGs did not look
like camera originals. Frames 130 ms apart have pHash distance 2: DNG N joined frame N+1's group and, as the larger
file, became its keeper. Frame N+1 had no copy of its own to export, and cleanup could have quarantined its CR2 as
covered by a copy of another photo. The synthetic corpus had no converted RAW files, so the gate could not see it.

Both files say which frame they are: the DNG records `OriginalRawFileName` (`IMG_0034.CR2`), and Canon maker notes
record the file number (`101-0034`): tag 0x0008 on older bodies, `FileIndex` and `DirectoryIndex` inside
`CameraInfo` (0x000D, at model-specific offsets) on current DSLRs such as the 70D.

## Decision
- **Frame identity is read** where the file records it, from the bytes the analysis already holds (no extra read):
  a DNG's `OriginalRawFileName` stem (upper case, extension dropped), and the Canon file number (`directory * 10000 +
  index`) from tag 0x0008 or from `CameraInfo` for the models whose offsets are known, also from maker notes kept in
  `DNGPrivateData` (`Adobe\0MakN`, rebased on the original offset it records). Maker notes in `DNGPrivateData` count
  as maker notes.
- **Different recorded frames are a contradiction** (`recorded as different frames (...): burst frames`): two files
  whose frame identities of the same kind are both present and differ, with no differing camera model, are never
  one image. Like every ADR-0039 contradiction it is a cannot-link constraint for the whole group, and the pair is
  a similar-shot suggestion. A missing identity never matches anything; a file name is never an identity.
- **RAW files are camera originals** for the ADR-0035 rule "two camera originals of one model with identical stored
  pixel dimensions but different content are two shots", whether or not their maker notes were parsed. The
  dimension clause stays: a CR2 (5472x3648) and its DNG (5568x3708) may still join; two DNGs of two frames may not.
- **Old catalogs are corrected once.** Catalog migration 23 adds `file.frame_id` and `file.meta_version` and stamps
  every non-RAW row current without reading it. On the next rescan, each unchanged analysed RAW-format row (DNG
  included) below `META_LOGIC_VERSION` 1 is re-read for its header only (the probe window, read-only, no decode,
  no hashing); its frame identity and maker-note facts are merged into the stored metadata, the row is stamped,
  and its dedup stamp changes so the run's regroup sees it. The scan summary reports the files re-read and the
  groups split.
- A regroup that splits a group leaves the separated frame pending for export. A quarantined file whose covering
  library copy belongs to another frame stops counting as accounted for (the existing ADR-0093/0096 rule) and the
  report names the reason and `cleanup undo`.

## Alternatives considered
- **Treat a missing sub-second as a mismatch for RAW files** - would split a CR2 from its own DNG for every first
  frame; frame identity says exactly which frame a file is.
- **Compare a DNG's `OriginalRawFileName` with a RAW file's own name** (the proposal's D2) - names are changed by
  import tools and are not recorded identity; the Canon file number in both files already covers the case, and a
  renamed CR2 would be split from its own DNG.
- **Re-read every catalogued file** - JPEG bursts already carry sub-seconds; reading the RAW headers alone is a few
  MB per card.
- **Re-analyse RAW rows fully** - a full read and decode of every RAW for four facts in its header.

## Consequences
- The ground-truth corpus includes converted RAW bursts (CR2-like original, camera JPEG, converted DNG per frame,
  the first frame's DNG without sub-second) as must-not-merge sets; the gate baseline is re-recorded.
- A maker note that misreports a file number can only split a pair (into a suggestion the user can merge), never
  join one.
- The `CameraInfo` offsets are per model; bodies outside the table keep the sub-second, dimension and RAW-original
  rules only.

## Revisit when
Another maker's converted files show the same merge (Nikon `ShutterCount`, Sony `ShotNumber`), or a Canon body
outside the table is reported.
