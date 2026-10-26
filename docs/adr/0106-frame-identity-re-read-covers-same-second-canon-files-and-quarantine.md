# 0106. The frame-identity re-read also covers same-second Canon files and files in quarantine

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0105](0105-recorded-frame-identities-keep-frames-apart.md) (which rows migration 23 leaves for the
  one-time re-read, and where a quarantined row is read)

## Context
ADR-0105 re-reads the header of unchanged RAW rows only and marks every other row current. Building it showed that
this leaves old catalogs wrong. Frame identity separates two files only when both record it, and the JPEGs of a
burst were analysed before it existed: after the RAW-only re-read, frame 34's converted DNG (a frame number, no
sub-second) still had no contradiction with frame 35's camera JPEGs (no frame number stored). With hashes that put
the DNG nearer frame 35, the regroup kept the DNG with frame 35's JPEGs and CR2, and pushed frame 35's own DNG out
of the group: frame 35 lost its keeper again. A file a cleanup run moved into the quarantine folder is still a
grouping input (ADR-0034) but is never walked, so it was not re-read either.

## Decision
- Migration 23 leaves below `META_LOGIC_VERSION` every RAW-format row and every row of a Canon camera (catalogued
  model `Canon...`) whose capture second (`capture_ts`) is the capture second of a RAW-format row; it marks every
  other row current without reading it. The rescan re-reads exactly the rows below the version, with the same
  header-only, read-only read.
- A row below the version that a cleanup run moved into its source's quarantine folder is re-read there (its
  quarantine path, read-only); its catalog row stays at its original path.

## Alternatives considered
- **RAW rows only (ADR-0105)** - leaves the merge on old catalogs, as above.
- **Every Canon JPEG** - a burst needs files of one second only; a phone-heavy or JPEG-only library would read
  thousands of headers for nothing.
- **Give a JPEG the frame of its RAW twin (same folder, stem and time) while grouping** - covers only twins in one
  folder, not copies elsewhere, and makes grouping depend on names.

## Consequences
- The first rescan of an upgraded catalog also reads the headers of the Canon JPEGs shot in the same second as a
  RAW file (on the owner's card, the camera JPEGs of its RAW shots).
- A fresh catalog is unaffected: analysis records frame identity for every file.

## Revisit when
Another maker gains a frame identity (its same-second files then need the same treatment).
