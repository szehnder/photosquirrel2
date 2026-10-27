# 0107. Files that record the same frame are not "two camera originals"

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0105](0105-recorded-frame-identities-keep-frames-apart.md) (the RAW-original rule),
  [0035](0035-burst-protection-in-grouping.md) (the two-camera-originals rule)

## Context
ADR-0105 counts every RAW-format file as a camera original for the ADR-0035 rule "two camera originals of one model
with identical stored pixel dimensions but different content are two shots". The gate review found that this splits
files of one frame: a converted DNG and the same DNG after Lightroom wrote XMP into it (2 KB more, same dimensions,
pHash distance 0) became a "two different camera originals" suggestion, and so can a NEF and a DNG converted from it
when the converter keeps the sensor size. A wrong split only exports an extra copy, but the suggestion is wrong and
the owner would see the same photo twice in the library.

## Decision
The two-camera-originals rule does not apply when both files positively record the **same frame at the same capture
time**:
- the same frame: equal camera file numbers (Canon `directory * 10000 + index`); or, only when neither file records a
  file number, equal `OriginalRawFileName` stems;
- the same capture time: both present, equal to the second, and equal sub-seconds when both record one. A stem
  repeats after the camera's counter rolls over, and a file number can be reset, so the identity alone is not
  evidence.

Anything missing is not evidence: a stem against a file number, a missing capture time, or no identity at all keeps
the rule (the split). In particular a non-Canon RAW (NEF, ARW, ...) and a DNG converted from it with equal
dimensions stay a suggestion: the NEF records no `OriginalRawFileName` and the reader knows no other maker's frame
number, and a file name is never an identity (ADR-0105).

## Alternatives considered
- **Drop the RAW-original rule for DNGs** - two DNGs of two burst frames (no frame identity, no sub-second) would
  merge again: the merge ADR-0105 exists to prevent.
- **Compare a DNG's `OriginalRawFileName` with the other file's name** - names are renamed by import tools; out of
  scope since the proposal.
- **The identity without the capture time** - a counter rollover or reset would let two shots of the same number
  merge when their hashes are close.

## Consequences
- A DNG and its re-saved copy, two converted DNGs of one frame, and a camera JPEG and its CR2 in different folders
  (same file number, same time, equal dimensions) are one image again; the corpus gate's suggestion count drops by
  those pairs (the baseline is re-recorded).
- NEF + converted DNG of equal dimensions remain a suggestion until a Nikon frame number is read.

## Revisit when
Nikon `ShutterCount` or Sony `ShotNumber` is read (ADR-0105's revisit trigger), which would let those pairs join.
