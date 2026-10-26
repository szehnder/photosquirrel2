# thumbnails Specification

## Purpose
Produces the downscaled images every later stage needs (analysis decodes, grid thumbnails, large previews) and
keeps them in a store that is shared by duplicates and usable while source drives are unplugged.

## Requirements

### Requirement: Downscaled analysis decode
For analysis the engine SHALL decode each image once to the smallest available scale whose long edge is at least 512 px, with EXIF orientation applied; JPEGs SHALL use DCT scaling (1/2, 1/4, 1/8) instead of a full decode, and TIFFs SHALL be decoded one strip or tile at a time, averaged down by an integer factor while decoding, from the smallest reduced-resolution level of the first image whose long edge is at least that size when the file has one.

#### Scenario: DCT scaled decode chosen
- **WHEN** a 6000x4000 JPEG is decoded for analysis
- **THEN** the decode is 750x500 (1/8 scale) and no full-resolution buffer is allocated

#### Scenario: Small image decoded at full size
- **WHEN** a 640x480 JPEG is decoded for analysis
- **THEN** the decode is 640x480

#### Scenario: TIFF decoded at a reduced scale
- **WHEN** a 6000x4000 16-bit TIFF with 16-row strips is decoded for analysis
- **THEN** the decode is 545x363 (factor 11) and the decoder held at most one strip and the downsampled image

#### Scenario: Reduced-resolution level preferred
- **WHEN** a tiled pyramid TIFF with a 4096x3072 image and 2048x1536, 1024x768 and 512x384 levels is decoded for analysis
- **THEN** the 512x384 level is decoded, at 512x384

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
Without a HEIC decoder, HEIC stills SHALL get a "preview unavailable" placeholder that names the missing decoder; without a configured poster-frame tool, videos SHALL get a styled placeholder showing duration and date. With the tool configured, a video SHALL get a poster frame. Large previews SHALL use the decoders detected now, without waiting for a rescan.

#### Scenario: HEIC placeholder names the missing decoder
- **WHEN** a HEIC fixture is thumbnailed with no HEIC decoder
- **THEN** a placeholder record is stored whose note says no HEIC decoder is available on this system, with the photo's capture date

#### Scenario: Large HEIC preview after the decoder appears
- **WHEN** a HEIC catalogued with a placeholder is opened in the large preview with a HEIC decoder now available and no rescan
- **THEN** the large preview is decoded and shown

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

### Requirement: TIFF layouts and sample formats
TIFF stills SHALL decode to the same upright picture for strips and tiles, chunky and planar samples, 8- and 16-bit integer and 32-bit floating-point samples, greyscale, RGB, CMYK and JPEG-compressed YCbCr data, uncompressed or LZW-, Deflate- or JPEG-compressed, with horizontal or floating-point predictors; alpha SHALL be composited over white and the orientation tag SHALL be applied.

#### Scenario: TIFF variants decode to the same picture
- **WHEN** one picture is written as each of those TIFF variants and decoded for analysis, together with its 8-bit RGB reference
- **THEN** every RGB and CMYK decode differs from the reference decode by at most 2 per channel value on average, JPEG-compressed ones by at most 6, and greyscale ones match the reference's luminance within 3

#### Scenario: JPEG-compressed YCbCr TIFF decodes
- **WHEN** a tiled TIFF with JPEG-compressed YCbCr tiles and no reduced-resolution level is decoded for analysis
- **THEN** the decode succeeds and differs from the reference by at most 6 per channel value on average

#### Scenario: Orientation tag applied to TIFF
- **WHEN** TIFFs stored rotated with orientation tags 3, 6 and 8 are decoded for analysis
- **THEN** each decode is upright, with the known colour in each corner

#### Scenario: Alpha composited over white
- **WHEN** an RGBA TIFF with an unassociated alpha of 0 in its left half and 255 in its right half is decoded
- **THEN** the left half is white and the right half equals the RGB reference

### Requirement: TIFF decoded in bounded memory
A TIFF decode SHALL hold at most one strip or tile of at most 32 MB (decoded and compressed) plus the downsampled image; a TIFF with a larger strip or tile SHALL be decoded whole only when the file is already held in memory, and otherwise refused as too large. When the accumulator of the requested scale would exceed 256 MB, the decode SHALL use the next larger box factor that fits instead of failing.

#### Scenario: Decode memory bounded
- **WHEN** a 6000x4000 16-bit TIFF with 16-row strips is decoded through a reader
- **THEN** the decoder reports at most one strip plus the downsampled image as its working memory, less than 4 MB

#### Scenario: Oversized strip refused when not in memory
- **WHEN** a TIFF whose single strip decodes to 75 MB is decoded through a reader
- **THEN** the decode is refused as too large and the same file in memory still decodes

#### Scenario: Large preview of a five-channel TIFF decodes
- **WHEN** a 4095x4095 CMYK TIFF with an alpha channel is decoded for a 2048 px preview
- **THEN** the decode succeeds at box factor 2, at 2047x2047

### Requirement: TIFF decode work bounded
A TIFF decode SHALL refuse as too large, before reading any strip or tile, a directory whose strips or tiles decode to more than 4 GiB or claim more than 4 GiB of compressed data, a block shared by several of them counted once for each; when the first image is refused so and no reduced-resolution level reaches the requested size, the largest level within the bound SHALL be decoded instead.

#### Scenario: TIFF decode work capped
- **WHEN** a 16.8 MB TIFF declaring 81920x81920 pixels in 400 tiles that share one 16 MB block is decoded
- **THEN** it is refused as too large, naming its dimensions, with no strip or tile decoded and less than 64 KB read

#### Scenario: Reduced-resolution level stands in for a huge first image
- **WHEN** a TIFF whose first image is above the bound and whose only reduced-resolution level is 640x640 is decoded for a 2048 px preview
- **THEN** the 640x640 level is decoded

### Requirement: TIFF decode interruptible
A TIFF decode SHALL check its caller's stop condition before every strip or tile and end as cancelled as soon as it holds, without falling back to another decoder.

#### Scenario: TIFF decode stops when cancelled
- **WHEN** the stop condition of a decode of a TIFF of 256 strips holds at the 11th strip
- **THEN** the decode ends as cancelled after decoding 10 strips, and the same file analysed from memory is not decoded again by the fallback decoder
