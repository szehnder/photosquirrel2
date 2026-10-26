# Spec Delta

## ADDED Requirements

### Requirement: Exclusion category stored
The catalog SHALL store each file's exclusion category, and keep it current when a file is classified, re-classified or given a decision. It SHALL store whether each user decision came from an AI suggestion, and of which kind. A migration SHALL fill both in for existing catalogs. A file of unrecognised content SHALL be "could not be read" when it has a photo or video extension or could not be read at all.

#### Scenario: Old catalog gets categories
- **WHEN** a catalog from before this change, with 4 decisions whose reason ends in "(AI suggestion)" and 3 classifier-excluded unsupported files, is opened
- **THEN** after the migration the 4 decisions are AI decisions of their kinds, the 3 files have the "not a supported file" category, and no file bytes were read

#### Scenario: Category follows re-classification
- **WHEN** a file classified as too small is re-classified as a likely photo after the minimum side setting is lowered
- **THEN** its stored exclusion category is cleared and it is no longer listed in the Excluded view

#### Scenario: Recognised format that cannot be read is not unsupported
- **WHEN** a file with a TIFF signature fails to probe
- **THEN** its category is "could not be read", not "not a supported file"

#### Scenario: Damaged photo is not unsupported
- **WHEN** a file named "IMG_9999.JPG" whose first bytes are damaged is scanned
- **THEN** its category is "could not be read", not "not a supported file", and the Excluded view lists it by default
