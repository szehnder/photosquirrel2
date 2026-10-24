# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

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
A TIFF decode SHALL hold at most one strip or tile of at most 32 MB (decoded and compressed) plus the downsampled image; a TIFF with a larger strip or tile SHALL be decoded whole only when the file is already held in memory, and otherwise refused as too large.

#### Scenario: Decode memory bounded
- **WHEN** a 6000x4000 16-bit TIFF with 16-row strips is decoded through a reader
- **THEN** the decoder reports at most one strip plus the downsampled image as its working memory, less than 4 MB

#### Scenario: Oversized strip refused when not in memory
- **WHEN** a TIFF whose single strip decodes to 75 MB is decoded through a reader
- **THEN** the decode is refused as too large and the same file in memory still decodes
