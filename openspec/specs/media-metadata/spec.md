# media-metadata Specification

## Purpose
Extracts the metadata that identifies when, where and with what a photo or phone video was captured, so the
inspector can show it, the best-copy ranking can use it and the export can date files correctly.

## Requirements

### Requirement: Camera EXIF extracted
The engine SHALL extract from images, when present: camera make, model and serial, lens, DateTimeOriginal with sub-seconds and UTC offset, exposure time, f-number, ISO, focal length, flash, orientation, software and GPS.

#### Scenario: Camera EXIF extracted
- **WHEN** the fixture photo written with make "Canon", model "EOS 5D", exposure 1/250 s, f/2.8, ISO 400 and focal length 50 mm is analyzed
- **THEN** every one of those values is reported exactly, with exposure as the rational 1/250

#### Scenario: GPS converted to decimal degrees
- **WHEN** a fixture with GPS 48 deg 51' 29.6" N, 2 deg 17' 40.2" E is analyzed
- **THEN** latitude is 48.858222 and longitude is 2.294500 within 0.000001

### Requirement: Video metadata extracted
The engine SHALL extract from QuickTime and MP4 containers: creation date, duration, pixel dimensions, rotation, frame rate and device make/model when present.

#### Scenario: Phone video metadata
- **WHEN** a MOV fixture with creation date 2019-07-04T18:30:00Z, duration 12.5 s, 1920x1080 and make "Apple" is analyzed
- **THEN** all four values are reported and duration is within 0.05 s

### Requirement: Capture date resolution
Every file SHALL have one capture date with its provenance, chosen in order: EXIF DateTimeOriginal, EXIF CreateDate, container creation date, file modification time. A date from file modification time MUST be marked undated.

#### Scenario: Fallback to file time flagged
- **WHEN** a JPEG without any EXIF date and with modification time 2012-03-01 is analyzed
- **THEN** its capture date is 2012-03-01 with provenance file-mtime and it is marked undated

#### Scenario: DateTimeOriginal preferred
- **WHEN** a JPEG has DateTimeOriginal 2010-05-01 10:00:00 and CreateDate 2015-01-01 00:00:00
- **THEN** its capture date is 2010-05-01 10:00:00 with provenance DateTimeOriginal

### Requirement: JPEG quality estimated
The engine SHALL estimate a JPEG's encoder quality (1-100) from its quantization tables, within 5 of the true quality for standard IJG tables.

#### Scenario: Quality estimate within tolerance
- **WHEN** the same fixture image encoded at quality 60 and at quality 92 is analyzed
- **THEN** the estimates are within 5 of 60 and within 5 of 92 respectively

### Requirement: Camera original signal
The engine SHALL mark an image as a likely camera original when it carries maker notes or camera EXIF (make, model and DateTimeOriginal) and no editing-software signature.

#### Scenario: Maker notes mark camera original
- **WHEN** a JPEG with Canon maker notes and a copy re-saved by an image editor without maker notes are analyzed
- **THEN** only the first is marked as a likely camera original

### Requirement: Full tag list available
The engine SHALL provide every decoded metadata tag as an ordered list of group, name and display value, so that tags without a dedicated field remain visible.

#### Scenario: All tags listed
- **WHEN** a fixture with 40 EXIF tags is analyzed
- **THEN** the tag list contains 40 entries including the unrecognised tag 0xC4A5 by its numeric name

### Requirement: Corrupt metadata tolerated
A corrupt or unparseable metadata block MUST NOT prevent the file from being catalogued; the engine SHALL keep the dimensions and format it could determine and record a metadata warning.

#### Scenario: Corrupt EXIF keeps file
- **WHEN** a JPEG whose EXIF block has an invalid IFD offset is analyzed
- **THEN** its format and dimensions are reported, metadata fields are empty and one metadata warning is recorded
