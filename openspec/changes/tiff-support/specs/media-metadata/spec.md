# Spec Delta

## ADDED Requirements

### Requirement: TIFF metadata wherever its directories are
Camera EXIF of a plain TIFF (make, model, DateTimeOriginal, orientation and the other tags of IFD0, Exif and GPS directories) SHALL be extracted whether its directories precede or follow the pixel data, and whether or not the file is larger than 64 MB.

#### Scenario: EXIF of an end-directory TIFF extracted
- **WHEN** a TIFF whose first directory and Exif directory follow its pixel data, with make "Nikon", model "Coolscan" and DateTimeOriginal 2015-06-07 10:11:12, is analyzed
- **THEN** make and model are reported and its capture date is 2015-06-07 10:11:12 with provenance DateTimeOriginal

#### Scenario: EXIF of a TIFF larger than 64 MB extracted
- **WHEN** a TIFF larger than 64 MB whose directories follow its pixel data, with DateTimeOriginal 2015-06-07 10:11:12 and orientation 8, is scanned
- **THEN** its capture date is 2015-06-07 10:11:12 with provenance DateTimeOriginal and its orientation is 8
