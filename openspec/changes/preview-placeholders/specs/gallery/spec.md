# Spec Delta

## ADDED Requirements

### Requirement: Placeholder tiles
An item with a placeholder SHALL be drawn as a placeholder tile, never a blank square. The tile SHALL show the media glyph, the format badge, "No preview", and a status mark for its reason, using theme tokens only. At small tile sizes it SHALL show only the glyph and badge. The loupe, the Excluded grid and the list thumbnails SHALL use the same placeholder.

#### Scenario: TIFF without a preview shows a placeholder tile
- **WHEN** the grid shows a TIFF whose placeholder reason is an unsupported variant
- **THEN** its cell paints a placeholder tile with the "TIFF" badge, "No preview" and the not-supported mark, and no flat fill

#### Scenario: Small tiles keep glyph and badge
- **WHEN** the grid's tile size is the smallest setting
- **THEN** placeholder tiles show the glyph and the format badge, and no text is clipped

#### Scenario: Loupe shows the large placeholder
- **WHEN** the loupe opens a HEIC that has no preview and no decoder
- **THEN** the loupe shows the large placeholder with the reason and an info button

### Requirement: Info button on placeholders
A placeholder tile SHALL show an info button on hover, keyboard focus and selection. Clicking it, or pressing the info shortcut on a selected placeholder, SHALL open the "Why no preview?" dialog for that file without changing the selection.

#### Scenario: Info button opens the dialog
- **WHEN** the user hovers a placeholder tile and clicks its info button
- **THEN** the "Why no preview?" dialog opens for that file and the selection is unchanged

### Requirement: No preview filter
The Library SHALL offer a "No preview" filter, by placeholder reason and format, that shows only items without a preview.

#### Scenario: Filter to HEIC files waiting for a decoder
- **WHEN** the user picks "No preview: needs a library" with format HEIC
- **THEN** the Library shows exactly the HEIC items whose placeholder reason is a missing decoder

### Requirement: Fill in previews offered
When some items have placeholders that could become previews now, because a decoder became available or this version decodes their format at a newer generation, the Library SHALL show a one-line notice with the count and a Fill in previews action, until the job has run or the notice is dismissed for this session.

#### Scenario: Notice after an update that adds TIFF variants
- **WHEN** a catalog with 37 TIFF placeholders from decoder generation 0 is opened by a version whose TIFF generation is 1
- **THEN** the Library notice says 37 TIFF files can get previews now, and offers Fill in previews
