# Spec Delta

## ADDED Requirements

### Requirement: RAW placeholders re-classified once after the update
The RAW decoder generation SHALL be 1, so the first rescan of a source or Fill in previews after the update SHALL re-read each RAW placeholder of generation 0 once and store its corrected reason at generation 1, opening no other file. A re-read RAW that is still a variant not supported SHALL NOT be reported as a run error.

#### Scenario: Rescan re-classifies an old damaged RAW
- **WHEN** a source holding a cut CR2 with an "unsupported variant" placeholder of generation 0, 20 thumbed JPEGs and 5 thumbed CR2s is rescanned unchanged
- **THEN** only the cut CR2 is opened, its placeholder reason becomes "could not be read" at generation 1, and a second rescan opens no file

#### Scenario: Re-read unknown camera is not an error
- **WHEN** a source holding an unknown-camera CR2 placeholder of generation 0 is rescanned unchanged
- **THEN** the file is read once, its reason stays a variant not supported at generation 1, and the run reports 0 errors

### Requirement: RAW files with previews checked once for a cut
The first rescan of a source after the update SHALL check each unchanged RAW row that an earlier version stored with preview pixels for a cut, once, reading only its first 64 KB and its directories. A file found cut SHALL lose its hashes, get the "could not be read" placeholder, be regrouped and counted as damaged. Missing sources SHALL wait until they are scanned.

#### Scenario: Rescan finds a RAW cut after its intact preview
- **WHEN** a 0.3.0 catalog holds a CR2 cut after its intact preview, with a thumbnail and hashes, grouped with its camera JPEG, and the source is rescanned
- **THEN** the CR2 is damaged, out of the JPEG's group, the JPEG is a keeper, damaged is 1, only directory bytes of the CR2s are read, and a second rescan reads nothing
