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
