# Spec Delta

## ADDED Requirements

### Requirement: Why no preview dialog
The "Why no preview?" dialog SHALL name the file's format and its placeholder reason in plain words, and state that the file is still catalogued, matched by exact content and exported. It SHALL give the action for the reason: the decoder report's install steps, "update PhotoSquirrel", "may be damaged", or a Fill in previews button when a preview is possible now. It SHALL offer to show every file with the same reason.

#### Scenario: Missing decoder shows install steps
- **WHEN** the dialog opens for a HEIC with no decoder on a Windows system missing the HEVC Video Extensions
- **THEN** it shows the reason, the decoder report's steps with the Store link shown as a URL and opened only on click, and "then restart PhotoSquirrel and fill in previews"

#### Scenario: Decoder available offers Fill in previews
- **WHEN** the dialog opens for a HEIC placeholder after a HEIC decoder became available
- **THEN** it says a preview can be made now and its Fill in previews button starts the job for HEIC files

#### Scenario: Unsupported variant explains the limit
- **WHEN** the dialog opens for a BigTIFF placeholder
- **THEN** it says this version cannot read BigTIFF files, that the file is still catalogued, matched by exact content and exported, and that a later version fills the preview in after an update

#### Scenario: Damaged file shows the error
- **WHEN** the dialog opens for a JPEG whose placeholder reason is "could not be read"
- **THEN** it shows the decoder's error and says the file may be damaged and can be checked in another app

#### Scenario: Show files with the same reason
- **WHEN** the user clicks "Show them" in a dialog that says 12 other files have no preview for this reason
- **THEN** the dialog closes and the Library shows the 13 files through the No preview filter
