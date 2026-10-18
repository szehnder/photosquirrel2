# Spec Delta

## Purpose

Produces the downscaled images every later stage needs (analysis decodes, grid thumbnails, large previews) and
keeps them in a store that is shared by duplicates and usable while source drives are unplugged.

## ADDED Requirements

### Requirement: Downscaled analysis decode
For analysis the engine SHALL decode each image once to the smallest available scale whose long edge is at least 512 px, with EXIF orientation applied; JPEGs SHALL use DCT scaling (1/2, 1/4, 1/8) instead of a full decode.

#### Scenario: DCT scaled decode chosen
- **WHEN** a 6000x4000 JPEG is decoded for analysis
- **THEN** the decode is 750x500 (1/8 scale) and no full-resolution buffer is allocated

#### Scenario: Small image decoded at full size
- **WHEN** a 640x480 JPEG is decoded for analysis
- **THEN** the decode is 640x480

### Requirement: Grid thumbnail format
Each image SHALL get one grid thumbnail: a JPEG of 384 px on the long edge (never upscaled) at quality 82 with orientation applied, plus its dominant colour.

#### Scenario: Thumbnail long edge is 384
- **WHEN** a 4000x3000 photo is thumbnailed
- **THEN** the thumbnail is a 384x288 JPEG and a dominant colour is stored

#### Scenario: Orientation applied to thumbnail
- **WHEN** a 4000x3000 JPEG with EXIF orientation 6 is thumbnailed
- **THEN** the thumbnail is 288x384 and visually upright

### Requirement: Embedded preview preferred
When an embedded EXIF, RAW or HEIC preview with a long edge of at least 320 px exists, thumbnails SHALL be made from it instead of decoding the full image; RAW sensor data MUST never be demosaiced.

#### Scenario: Embedded preview preferred
- **WHEN** a RAW fixture with a 1616x1080 embedded preview is thumbnailed
- **THEN** the thumbnail is made from the embedded preview and no sensor data is decoded

#### Scenario: Small preview not used
- **WHEN** a JPEG with a 160x120 EXIF thumbnail is thumbnailed
- **THEN** the thumbnail is made from the main image

### Requirement: Content-addressed thumbnail store
Grid thumbnails SHALL be stored in a dedicated store, separate from the catalog, keyed by content identity, so byte-identical files share one thumbnail and thumbnails remain readable when the source is unavailable.

#### Scenario: Duplicates share one thumbnail
- **WHEN** three byte-identical copies of a photo are thumbnailed
- **THEN** the store contains one thumbnail for their content identity

#### Scenario: Thumbnail available offline
- **WHEN** a photo is thumbnailed and its source folder is then renamed away
- **THEN** its thumbnail and dominant colour are still returned by content identity

### Requirement: Large previews on demand
2048 px (long edge) previews SHALL be generated on demand and cached on disk in a least-recently-used cache whose size MUST NOT exceed its cap (default 4 GB, from settings).

#### Scenario: Preview cache stays under cap
- **WHEN** the cap is set to 10 MB and previews totalling 30 MB are requested
- **THEN** the cache stays at or below 10 MB and the least recently used previews are evicted first

### Requirement: Placeholders when decoders are unavailable
Without a HEIC decoder, HEIC stills SHALL be marked "preview unavailable"; without a configured poster-frame tool, videos SHALL get a styled placeholder showing duration and date. With the tool configured, a video SHALL get a poster frame.

#### Scenario: Video placeholder without tool
- **WHEN** a MOV fixture is thumbnailed with no poster-frame tool configured
- **THEN** a placeholder record with the video's duration and capture date is stored instead of a frame

#### Scenario: Poster frame from configured tool
- **WHEN** a MOV fixture is thumbnailed with the poster-frame tool path set to a working executable
- **THEN** a 384 px poster-frame thumbnail is stored

### Requirement: Thumbnail stage throughput
Downscaled decode plus grid thumbnail of a 12 MP JPEG SHALL cost at most 100 ms on one core of the reference machine, the per-file share of the 150 files/s on 16 cores analysis budget (15 worker cores).

#### Scenario: Decode within per-file budget
- **WHEN** the decode-and-thumbnail benchmark runs single-threaded on 12 MP JPEG fixtures
- **THEN** the median per-file time is reported as a budget measurement and is at most 100 ms
