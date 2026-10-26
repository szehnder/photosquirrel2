# media-probe Specification

## Purpose
Lets the engine learn a file's real format, dimensions, orientation and embedded preview cheaply and safely,
so that junk can be skipped before any full read and every later stage starts from trustworthy facts.

## Requirements

### Requirement: Format detected by content
The probe SHALL identify the file format from its leading bytes (magic numbers), not from its file extension, and SHALL report files it cannot identify as unsupported without failing.

#### Scenario: JPEG with wrong extension
- **WHEN** a JPEG fixture renamed to `photo.png` is probed
- **THEN** the probe reports format JPEG and records that the extension does not match

#### Scenario: Unknown bytes are unsupported
- **WHEN** a file of 4 KB random bytes named `image.jpg` is probed
- **THEN** the probe reports format unsupported with a reason and returns no error to the caller

### Requirement: Bounded probe read
The probe MUST read at most the first 262,144 bytes (256 KB) of a file and SHALL derive format, pixel dimensions, EXIF block and embedded preview location from it. When a plain TIFF's image directories, or values they point to, lie beyond that prefix, the probe SHALL read them from the whole file if it is held in memory for analysis, else with at most 64 targeted reads totalling at most 1 MiB, skipping large embedded blobs. Camera RAW files SHALL NOT get targeted reads.

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

### Requirement: Orientation-aware dimensions
The probe SHALL report both stored pixel dimensions and display dimensions after applying the EXIF orientation tag (values 1-8).

#### Scenario: Rotated orientation swaps dimensions
- **WHEN** a 4000x3000 JPEG with EXIF orientation 6 is probed
- **THEN** stored dimensions are 4000x3000 and display dimensions are 3000x4000

### Requirement: Embedded preview located
The probe SHALL report the presence, byte range and pixel dimensions of an embedded EXIF, RAW or HEIC preview image when one exists within the probed prefix or is referenced from it.

#### Scenario: Embedded preview found in RAW
- **WHEN** a DNG fixture containing a 1024x683 embedded JPEG preview is probed
- **THEN** the probe reports an embedded preview of 1024x683 and its byte range

#### Scenario: No preview reported as absent
- **WHEN** a PNG without any embedded preview is probed
- **THEN** the probe reports no embedded preview

### Requirement: Source files opened read-only
The probe MUST open files for reading only and MUST NOT change file contents, size or modification time.

#### Scenario: Probe never opens for writing
- **WHEN** a directory of fixtures whose files and folder are marked read-only is probed
- **THEN** every probe succeeds and every file's bytes, size and modification time are unchanged

### Requirement: Malformed input contained
Truncated, corrupt or adversarial files SHALL produce a probe error record with a reason and MUST NOT crash or hang the process; a probe SHALL finish within 1 second per file on the fixture set.

#### Scenario: Truncated JPEG reports error
- **WHEN** a JPEG truncated to its first 1,000 bytes is probed
- **THEN** the probe returns an error record naming the truncation and the process continues

#### Scenario: Fuzzed headers never panic
- **WHEN** 10,000 randomly mutated copies of the fixture headers are probed
- **THEN** no probe panics and each completes within 1 second

### Requirement: Decoder availability reported
The engine SHALL report, per format (HEIC/HEIF stills, AVIF stills, video poster frames), whether this machine can decode it and with which decoder, and when it cannot, the reason and how to enable it for the detected system. It SHALL still probe files of those formats when no decoder is available.

#### Scenario: HEIC without decoder
- **WHEN** a HEIC fixture is probed on a machine without a HEIC decoder
- **THEN** the probe reports format HEIC with dimensions, orientation, EXIF and the location of its embedded thumbnail, exactly as with a decoder

#### Scenario: Availability listed
- **WHEN** decoder availability is queried with no HEIC decoder and with no poster-frame tool configured
- **THEN** the report lists HEIC as unavailable with its reason and fix for the detected system, AVIF as available through the bundled decoder, and poster frames as unavailable with how to enable them

### Requirement: First image of a multi-image TIFF
The probe SHALL report the first image directory of a TIFF as the file's image; further pages and reduced-resolution levels SHALL NOT change its dimensions or orientation, and a TIFF file SHALL be one catalogued item however many pages it has.

#### Scenario: Multipage TIFF reports its first page
- **WHEN** a TIFF with a 1200x800 first page and a 640x480 second page, each directory after its pixel data, is probed
- **THEN** its stored dimensions are 1200x800

#### Scenario: Pyramid TIFF reports its full resolution
- **WHEN** a tiled TIFF with a 2048x1536 image and reduced-resolution levels of 1024x768 and 512x384 is probed
- **THEN** its stored dimensions are 2048x1536
