# Spec Delta

## ADDED Requirements

### Requirement: Why a preview is unavailable
For a photo without a preview because its decoder is missing, the inspector SHALL show the reason and how to enable previews for this system instead of a bare "preview unavailable"; when the decoder is available now, it SHALL say that a rescan of its source fills the preview in.

#### Scenario: Inspector explains a HEIC without a decoder
- **WHEN** a HEIC photo with a placeholder is selected while no HEIC decoder is available
- **THEN** the File section shows that no HEIC decoder is available on this system and the fix steps of the decoder report

#### Scenario: Inspector asks for a rescan once the decoder is available
- **WHEN** the same photo is selected after a HEIC decoder became available and before any rescan
- **THEN** the File section says a HEIC decoder is available now and a rescan of its source fills in the preview

### Requirement: A better copy not in the library is not called covered
For a group member whose disposition is superseded (better than every copy of its group in the library, ADR-0093), the inspector's Export section SHALL say that its quality is not in the library yet, name the group's library copy as the lesser copy, never call that copy "its best copy", and point to `export run --upgrade` and the export panel's option.

#### Scenario: Better copy not in the library is explained
- **WHEN** the inspector shows a HEIC keeper whose group is in the library only through a resized JPEG
- **THEN** the reason says the photo's quality is not in the library yet, names the library copy as the lesser copy and `--upgrade`, and the words "best copy" do not appear
