# Spec Delta

## MODIFIED Requirements

### Requirement: Bounded probe read
The probe MUST read at most the first 262,144 bytes (256 KB) of a file and SHALL derive format, pixel dimensions, EXIF block and embedded preview location from that prefix, with one exception: when the image directories of a plain TIFF, or values they point to, lie beyond the prefix, the probe SHALL read them from the whole file when the file is held in memory for analysis, and otherwise with targeted reads of at most 1,048,576 bytes (1 MiB) in total and at most 64 reads, skipping large embedded blobs. Camera RAW files SHALL NOT get targeted reads.

#### Scenario: Probe reads at most 256 KB
- **WHEN** a 20 MB JPEG is probed through a reader that counts bytes
- **THEN** no more than 262,144 bytes are read and the reported dimensions equal the image's true dimensions

#### Scenario: TIFF directory after the pixel data
- **WHEN** a 2400x1600 16-bit TIFF whose first directory and Exif directory follow its pixel data, with orientation 6, camera make and model and DateTimeOriginal, is probed through a seekable reader that counts bytes
- **THEN** stored dimensions are 2400x1600, display dimensions 1600x2400, make and model are reported, no probe-window warning is recorded, and at most 262,144 + 1,048,576 bytes are read

#### Scenario: Directory reads are bounded
- **WHEN** TIFFs whose directories beyond the prefix form a loop, chain 100 directories, or claim strip arrays and values larger than 1 MiB are probed through a seekable reader that counts bytes
- **THEN** every probe returns a record without panicking, makes at most 64 reads beyond the prefix and reads at most 262,144 + 1,048,576 bytes

#### Scenario: Camera RAW probes read only the prefix
- **WHEN** the DNG fixture and a TIFF-based RAW fixture larger than 256 KB are probed through a seekable reader that counts bytes
- **THEN** at most 262,144 bytes are read and each record equals the record of probing its first 256 KB alone

#### Scenario: Whole buffer and targeted reads agree
- **WHEN** a TIFF with its directories after the pixel data is probed once from the whole file in memory and once through targeted reads
- **THEN** both records report the same format, dimensions, orientation, camera signals and capture time

## ADDED Requirements

### Requirement: First image of a multi-image TIFF
The probe SHALL report the first image directory of a TIFF as the file's image; further pages and reduced-resolution levels SHALL NOT change its dimensions or orientation, and a TIFF file SHALL be one catalogued item however many pages it has.

#### Scenario: Multipage TIFF reports its first page
- **WHEN** a TIFF with a 1200x800 first page and a 640x480 second page, each directory after its pixel data, is probed
- **THEN** its stored dimensions are 1200x800

#### Scenario: Pyramid TIFF reports its full resolution
- **WHEN** a tiled TIFF with a 2048x1536 image and reduced-resolution levels of 1024x768 and 512x384 is probed
- **THEN** its stored dimensions are 2048x1536
