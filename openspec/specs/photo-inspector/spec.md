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
