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
Every file SHALL have one capture date with its provenance and precision, chosen in order: EXIF DateTimeOriginal, EXIF CreateDate, container creation date, a date in the name of a folder containing the file, file modification time. A date from file modification time MUST be marked undated. A folder date MUST NOT be marked undated and MUST NOT count as an embedded date for duplicate grouping or best-copy ranking.

#### Scenario: Fallback to file time flagged
- **WHEN** a JPEG without any EXIF date and with modification time 2012-03-01 is analyzed
- **THEN** its capture date is 2012-03-01 with provenance file-mtime and it is marked undated

#### Scenario: DateTimeOriginal preferred
- **WHEN** a JPEG has DateTimeOriginal 2010-05-01 10:00:00 and CreateDate 2015-01-01 00:00:00
- **THEN** its capture date is 2010-05-01 10:00:00 with provenance DateTimeOriginal

#### Scenario: Folder date below EXIF and above file time
- **WHEN** a JPEG without EXIF dates and modified 2024-03-01 is in folder `2019-07-04 Beach`, and a JPEG with DateTimeOriginal 2018-01-02 is in the same folder
- **THEN** the first has capture date 2019-07-04 with provenance folder-name, precision day and is not undated, and the second keeps 2018-01-02 from DateTimeOriginal

#### Scenario: Folder date does not change duplicate groups
- **WHEN** the ground-truth corpus is scanned with every stripped copy placed in a dated folder
- **THEN** the duplicate groups and keepers are identical to a scan without dated folders

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

### Requirement: Folder-name dates
A folder name SHALL give a date only when it starts with `YYYY-MM-DD`, `YYYY_MM_DD`, `YYYY.MM.DD` or `YYYYMMDD` followed by the end of the name or a non-digit (day precision), or with `YYYY-MM`, `YYYY_MM` or `YYYY.MM` (month precision), or when consecutive folders read `YYYY/MM/DD` (day) or `YYYY/MM` (month). Only folders inside the source count, the folder nearest the file wins, and a year alone is never a date.

#### Scenario: Folder date patterns
- **WHEN** EXIF-less files modified on 2024-03-01 are analyzed in folders `2019-07-04`, `2019_07_04 Beach`, `20190704`, `2019/07/04`, `2019-07 Italy`, `2019/07`, `2019`, `Backup 2019-07-04` and `DSC_19871234`
- **THEN** they are dated 2019-07-04 (day) for the first four, July 2019 (month) for the next two, and by their file time for the last three

#### Scenario: Nearest folder wins
- **WHEN** an EXIF-less file is at `2018/2019-07-04 Beach/raw/IMG_0001.JPG` inside the source
- **THEN** its capture date is 2019-07-04 from the folder `2019-07-04 Beach`

#### Scenario: Source root name not used
- **WHEN** a folder named `2019-07-04` is added as a source and an EXIF-less file directly inside it is analyzed
- **THEN** the file is dated by its file time

### Requirement: Two-year ranges are not folder dates
A folder name that starts with a range of two years SHALL NOT give a folder date: `YYYY-YYYY`, and `YYYY-YY`, `YYYY_YY` or `YYYY.YY` with `YY` the last two digits of the following year when more text follows it and its parent folder is not a year folder of `YYYY` (`YYYY`, optionally followed by a space and text). Any other such name MUST be read as a month.

#### Scenario: School-year folder is not a month
- **WHEN** EXIF-less files modified on 2024-03-01 are analyzed in folders `2011-12 School Year`, `2008_09 Season`, `2011-2012` and `2011-11 Thanksgiving`
- **THEN** the first three are dated by their file time and the last is dated November 2011 (month)

#### Scenario: Bare month folder next to the school-year pattern is a month
- **WHEN** EXIF-less files modified on 2024-03-01 are analyzed in folders `2010-11`, `2005_06` and `2011.12`
- **THEN** they are dated November 2010, June 2005 and December 2011 (month)

#### Scenario: Month folder inside its year folder is a month
- **WHEN** EXIF-less files modified on 2024-03-01 are analyzed in folders `2010/2010-11`, `2010/2010-11 Thanksgiving`, `2011 Family/2011-12 Christmas` and `Kids/2011-12 School Year`
- **THEN** the first three are dated November 2010, November 2010 and December 2011 (month), and the last is dated by its file time

### Requirement: Folder date plausibility
A folder date SHALL NOT be used when the file's modification time is on or after 1990-01-01 and lies more than one day before the folder date (for a month folder, before the start of that month minus one day); the file then falls back to its modification time.

#### Scenario: Backup folder date ignored
- **WHEN** an EXIF-less file modified on 2012-03-01 is in a folder `2019-07-04`
- **THEN** its capture date is 2012-03-01 from file-mtime and it is undated

#### Scenario: Unset camera clock does not block folder date
- **WHEN** an EXIF-less file modified on 1980-01-01 is in a folder `2019-07-04`
- **THEN** its capture date is 2019-07-04 from folder-name

### Requirement: Month precision is never a day
A capture date with month precision SHALL be stored as the first day of that month, sorted there, and SHALL be shown and filed as a month only, never as a day.

#### Scenario: Month folder stays a month
- **WHEN** an EXIF-less file in folder `2019-07 Italy` is analyzed and its capture date is shown in the CLI JSON
- **THEN** the date is 2019-07 with precision month and no day or time is reported

### Requirement: Local day of a time instant
A file modification time, and a container creation date that is only the movie header's UTC time (`mvhd`), SHALL become a capture date by applying the UTC offset the file records for that time, else the machine's local time-zone rules at that instant, and the offset used SHALL be stored with the date. An EXIF date, and a keyed QuickTime creation date (`com.apple.quicktime.creationdate`, which records the device's offset, +00:00 included), SHALL keep the recorded wall-clock time and offset.

#### Scenario: File time in the local zone
- **WHEN** an EXIF-less JPEG modified at 2019-07-05 03:30 UTC is analyzed with the local zone America/Los_Angeles
- **THEN** its capture date is 2019-07-04 20:30 with offset -07:00, provenance file-mtime, undated

#### Scenario: Daylight saving of the file's date applies
- **WHEN** an EXIF-less JPEG modified at 2019-01-15 07:30 UTC is analyzed with the local zone America/Los_Angeles in July
- **THEN** its capture date is 2019-01-14 23:30 with offset -08:00

#### Scenario: UTC video creation time made local
- **WHEN** a MOV whose only creation date is 2019-07-05T02:00:00Z is analyzed with the local zone America/New_York
- **THEN** its capture date is 2019-07-04 22:00 with offset -04:00 and provenance container

#### Scenario: Keyed QuickTime date never shifted
- **WHEN** a MOV whose keyed creation date is 2019-07-04T00:30:00+00:00 is analyzed with the local zone America/Los_Angeles
- **THEN** its capture date is 2019-07-04 00:30 with offset +00:00 and provenance container

#### Scenario: EXIF wall clock never shifted
- **WHEN** JPEGs with DateTimeOriginal 2019-07-04 23:30:00, one without an offset and one with OffsetTimeOriginal +09:00, are analyzed with the local zone Europe/Berlin
- **THEN** both capture dates are 2019-07-04 23:30:00, the second with offset +09:00
