# Spec Delta

## Purpose

Lets the engine learn a file's real format, dimensions, orientation and embedded preview cheaply and safely,
so that junk can be skipped before any full read and every later stage starts from trustworthy facts.

## ADDED Requirements

### Requirement: Format detected by content
The probe SHALL identify the file format from its leading bytes (magic numbers), not from its file extension, and SHALL report files it cannot identify as unsupported without failing.

#### Scenario: JPEG with wrong extension
- **WHEN** a JPEG fixture renamed to `photo.png` is probed
- **THEN** the probe reports format JPEG and records that the extension does not match

#### Scenario: Unknown bytes are unsupported
- **WHEN** a file of 4 KB random bytes named `image.jpg` is probed
- **THEN** the probe reports format unsupported with a reason and returns no error to the caller

### Requirement: Bounded probe read
The probe MUST read at most the first 262,144 bytes (256 KB) of a file and SHALL derive format, pixel dimensions, EXIF block and embedded preview location from that prefix only.

#### Scenario: Probe reads at most 256 KB
- **WHEN** a 20 MB JPEG is probed through a reader that counts bytes
- **THEN** no more than 262,144 bytes are read and the reported dimensions equal the image's true dimensions

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
The engine SHALL report which optional decoders are available (HEIC/HEIF/AVIF stills, video poster frames) and SHALL still probe files of those formats when the decoder is unavailable.

#### Scenario: HEIC without decoder
- **WHEN** a HEIC fixture is probed in a build without the HEIC decoder
- **THEN** the probe reports format HEIC with dimensions and EXIF and marks the preview as unavailable

#### Scenario: Availability listed
- **WHEN** decoder availability is queried in a build without HEIC support and with no poster-frame tool configured
- **THEN** the report lists HEIC as unavailable and poster frames as unavailable, each with how to enable it
