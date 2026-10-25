# Spec Delta

## ADDED Requirements

### Requirement: TIFF metadata wherever its directories are
Camera EXIF of a plain TIFF (make, model, DateTimeOriginal, orientation and the other tags of IFD0, Exif and GPS directories) SHALL be extracted whether its directories precede or follow the pixel data, and whether or not the file is larger than 64 MB. The metadata of a TIFF above 64 MB SHALL be gathered into at most 1 MiB, keeping one value per tag of at most 64 KB.

#### Scenario: EXIF of an end-directory TIFF extracted
- **WHEN** a TIFF whose first directory and Exif directory follow its pixel data, with make "Nikon", model "Coolscan" and DateTimeOriginal 2015-06-07 10:11:12, is analyzed
- **THEN** make and model are reported and its capture date is 2015-06-07 10:11:12 with provenance DateTimeOriginal

#### Scenario: EXIF of a TIFF larger than 64 MB extracted
- **WHEN** a TIFF larger than 64 MB whose directories follow its pixel data, with DateTimeOriginal 2015-06-07 10:11:12 and orientation 8, is scanned
- **THEN** its capture date is 2015-06-07 10:11:12 with provenance DateTimeOriginal and its orientation is 8

#### Scenario: Metadata of a large TIFF gathered in bounded memory
- **WHEN** the metadata of a TIFF whose first directory holds 1,000 entries of the same 200 KB description, or 1,000 distinct 60 KB values, is gathered
- **THEN** the 200 KB description is left out and each tag kept once, and the gathered metadata is at most 1 MiB
