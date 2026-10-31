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

### Requirement: Every placeholder says why
A placeholder SHALL record why there are no pixels: a missing decoder, a missing poster-frame tool, an image sequence, a format variant not supported (with its detail), a file that could not be read (with the error), or a format this version reads that an older version could not. It SHALL record the decoder generation that produced it.

#### Scenario: BigTIFF placeholder names the variant
- **WHEN** a BigTIFF file is analysed
- **THEN** its placeholder reason is an unsupported variant with detail "BigTIFF", and the record carries the current TIFF decoder generation

#### Scenario: Damaged JPEG placeholder keeps the error
- **WHEN** a JPEG whose data ends after its headers is analysed
- **THEN** its placeholder reason is "could not be read" with the decoder's error message

#### Scenario: Old placeholders read as an older generation
- **WHEN** a placeholder stored before this change is read
- **THEN** it deserialises with decoder generation 0 and its stored note

### Requirement: Damaged RAW files told apart from unsupported ones
A RAW file whose own structure records a directory, strip, tile or preview that ends past the end of the file SHALL get a "could not be read" placeholder that says it was cut short, even when its embedded preview is intact, and SHALL NOT get perceptual hashes. A RAW from a camera the decoder does not know, or with no embedded preview and no such evidence, SHALL stay a format variant not supported.

#### Scenario: RAW cut inside its preview is damaged
- **WHEN** a CR2 fixture cut inside its embedded preview JPEG is analysed
- **THEN** its placeholder reason is "could not be read" with a message giving the file's length and the length its data needs, and the record carries RAW decoder generation 1

#### Scenario: RAW cut after an intact preview is damaged
- **WHEN** a CR2 fixture whose embedded preview is whole but whose sensor data is cut is analysed
- **THEN** its placeholder reason is "could not be read", no thumbnail pixels and no perceptual hashes are stored, and it is not grouped with its camera JPEG

#### Scenario: Unknown camera is still not supported
- **WHEN** a complete CR2-like file without a usable embedded preview, from a camera missing from the RAW decoder's camera table, is analysed
- **THEN** its placeholder reason is a variant not supported, with a detail naming the camera's make and model

#### Scenario: Complete RAW fixtures are never damaged
- **WHEN** every complete RAW fixture that `ps-fixtures` writes, including one with a private tag whose count runs past the end of the file, is analysed
- **THEN** none of them gets a "could not be read" placeholder

### Requirement: Previews converted to sRGB
The probe SHALL record a still's colour space from an embedded ICC profile, a DNG preview colour space, EXIF InteropIndex `R03`, or the Canon maker note, else sRGB. Grid thumbnails and large previews of Adobe RGB, Display P3 and profiled JPEG images and RAW previews SHALL be converted to sRGB; sRGB images SHALL NOT be converted.

#### Scenario: Canon Adobe RGB JPEG thumbnail is converted
- **WHEN** a JPEG fixture with EXIF ColorSpace 0xFFFF, InteropIndex R03 and no profile, holding a saturated green patch, is thumbnailed
- **THEN** the thumbnail's patch is the patch converted from Adobe RGB (1998) to sRGB within a colour difference of 2

#### Scenario: RAW preview in Adobe RGB is converted
- **WHEN** a CR2-style fixture whose Canon maker note records Adobe RGB is thumbnailed from its embedded preview
- **THEN** the thumbnail is converted from Adobe RGB to sRGB

#### Scenario: Embedded profile is applied
- **WHEN** a JPEG fixture with an embedded Display P3 profile is shown in the large preview
- **THEN** the preview is converted from Display P3 to sRGB

#### Scenario: sRGB thumbnails are unchanged
- **WHEN** an untagged JPEG and a JPEG with an embedded sRGB profile are thumbnailed
- **THEN** both thumbnails are byte-identical to the thumbnails of the version before this change

### Requirement: Colour conversion does not change grouping
Perceptual hashes, quality estimates and grouping SHALL be computed from the unconverted analysis decode, so a colour conversion MUST NOT change any hash, group or keeper.

#### Scenario: Hashes unchanged by conversion
- **WHEN** an Adobe RGB fixture is analysed with conversion enabled and with it disabled
- **THEN** its perceptual hashes are equal

### Requirement: Older Adobe RGB thumbnails redone once
On the first rescan after this change, unchanged thumbed JPEG and RAW files whose name starts with `_` SHALL have their colour space read from the header, read-only; a file that is not sRGB SHALL get a converted thumbnail, without touching its hashes or groups. Other rows SHALL NOT be read for it, and large previews cached before SHALL NOT be shown again.

#### Scenario: Rescan recolours old Adobe RGB thumbnails only
- **WHEN** a catalog written before this change, with `_MG_` Adobe RGB JPEGs, a `_MG_` sRGB JPEG and `IMG_` JPEGs, is rescanned unchanged through a byte-counting file layer
- **THEN** only the `_MG_` files are read, the sRGB one only within the probe window, the Adobe RGB ones get converted thumbnails, and the dedup stamp is unchanged

#### Scenario: The re-check happens once
- **WHEN** the same source is rescanned again
- **THEN** no file is read for colour
