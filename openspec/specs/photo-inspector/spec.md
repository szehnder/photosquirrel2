# photo-inspector Specification

## Purpose
Shows everything PhotoSquirrel knows about the selected photo (file, capture, camera, location, quality,
classification and every copy across drives) so the user can trust and act on its decisions.

## Requirements

### Requirement: Inspector sections
For a selected item the inspector SHALL show: File (name, path, source, size, format, dimensions), Capture (date and its provenance), Camera (make, model, lens, exposure, f-number, ISO, focal length), Location (GPS coordinates), Quality (estimate), Identity (content hash) and Classification (class and reasons).

#### Scenario: Inspector shows camera EXIF
- **WHEN** a corpus photo with full camera EXIF is selected
- **THEN** the Camera section shows its make, model, exposure, f-number, ISO and focal length exactly as extracted

#### Scenario: Missing fields shown as absent
- **WHEN** an EXIF-stripped copy is selected
- **THEN** the Camera section shows each field as absent rather than hiding the section, and the capture date shows its provenance

### Requirement: All copies listed
The inspector SHALL list every member of the item's duplicate group with source, path, dimensions, size, join reason and keeper status, and offer to open the group in duplicate review.

#### Scenario: Inspector lists all copies
- **WHEN** a keeper of a 4-member group is selected
- **THEN** all 4 members are listed with their sources and the keeper is marked

### Requirement: Full tag list
The inspector SHALL offer an expandable list of every metadata tag with a search field that filters by tag name or value.

#### Scenario: Search raw tags
- **WHEN** the user types "Lens" in the tag search
- **THEN** only tags whose name or value contains "Lens" are listed

### Requirement: Copy values
Every field value SHALL be copyable to the clipboard.

#### Scenario: Copy hash to clipboard
- **WHEN** the user copies the content hash field
- **THEN** the clipboard contains the 64-character hash

### Requirement: Reveal in file manager
The inspector SHALL offer to reveal a copy in the operating system's file manager without opening or changing the file.

#### Scenario: Reveal in file manager
- **WHEN** the user chooses Reveal for a copy on an online source
- **THEN** the platform file manager is asked to reveal that path and the file is not modified

### Requirement: Multi-selection summary
With several items selected the inspector SHALL show the count, total size, capture-date range and the number of groups they belong to.

#### Scenario: Multi-selection summary
- **WHEN** 25 items spanning 2010 to 2014 are selected
- **THEN** the inspector shows 25 items, their total size and the range 2010-2014

### Requirement: User include and exclude
The user SHALL be able to exclude an item (with an optional reason) or include an item the classifier excluded; the choice SHALL be persisted, shown as a reason and respected by later rescans.

#### Scenario: Include an excluded file
- **WHEN** the user includes a file classified as graphic
- **THEN** it leaves the Excluded view, appears in the Library with reason "included by you" and stays included after a rescan

#### Scenario: Exclude a photo
- **WHEN** the user excludes a photo with reason "screenshot"
- **THEN** it moves to the Excluded view with reason "excluded by you: screenshot"

### Requirement: Capture precision shown
The inspector SHALL show a capture date with month precision as a month and year only and a date with day precision without a time, each with its provenance.

#### Scenario: Month-precision date shown as a month
- **WHEN** an EXIF-less photo dated by the folder `2019-07 Italy` is selected
- **THEN** the Capture section shows "July 2019" with provenance folder name and no day or time

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

### Requirement: A copy better than the keeper you chose is kept
For a group member covered by a keeper the user chose while its automatic rank is above that keeper (ADR-0096), the inspector's Export section SHALL name the chosen keeper's library copy as the user's choice, say that this copy is better quality and is kept, and never call the keeper "its best copy".

#### Scenario: A copy better than the keeper you chose is explained
- **WHEN** the inspector shows a HEIC original whose group's keeper the user set to its resized JPEG, which is in the library
- **THEN** the disposition is covered, the reason names the JPEG as the keeper the user chose and says the HEIC is better quality and kept, and "best copy" does not appear

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

### Requirement: Colour space shown
The inspector's File section SHALL show a photo's colour space when it is not sRGB (Adobe RGB (1998), Display P3, or the embedded profile's description), and the loupe SHALL show the preview converted to sRGB.

#### Scenario: Adobe RGB photo names its colour space
- **WHEN** the Canon Adobe RGB JPEG fixture is selected
- **THEN** the File section shows "Colour: Adobe RGB (1998)" and the loupe shows the converted preview

#### Scenario: sRGB photo shows no colour row
- **WHEN** an untagged JPEG is selected
- **THEN** the File section shows no colour row
