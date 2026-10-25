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
