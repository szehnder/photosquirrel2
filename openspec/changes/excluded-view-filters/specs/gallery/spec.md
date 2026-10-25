# Spec Delta

## MODIFIED Requirements

### Requirement: Excluded view
An Excluded view SHALL list every skipped or excluded file with its class and reasons, so that nothing is hidden silently. Each file SHALL have exactly one exclusion category, chosen in order: excluded by you, accepted AI suggestion, thumbnail cache, graphic, too small, could not be read, not a supported file. The view SHALL show each file's category.

#### Scenario: Excluded view lists reasons
- **WHEN** the user opens the Excluded view of a scanned small corpus and turns every category on
- **THEN** every truth-file junk, graphic and cache file is listed with its reasons

#### Scenario: Each excluded file has one category
- **WHEN** a corpus file is a graphic inside a thumbnail cache folder and the user also excluded it with reason "logo"
- **THEN** its category is "excluded by you", and its reasons still list the cache folder and the graphic format

## ADDED Requirements

### Requirement: Excluded view filters
The Excluded view SHALL offer one filter chip per category, each with its live count. Accepted AI suggestions SHALL have sub-chips for screenshot, document, graphic and not a photo. Every chip SHALL start on except "not a supported file". The view SHALL say how many files the current filters hide, and SHALL remember the chosen chips per catalog.

#### Scenario: Unsupported files hidden by default
- **WHEN** a catalog's Excluded view is opened for the first time and it holds 3 unsupported files and 40 other excluded files
- **THEN** the view lists the 40 files, the "Not a supported file" chip is off with count 3, and the view says 3 files are hidden by filters

#### Scenario: Filter to AI-excluded documents
- **WHEN** the user turns every chip off except the AI "document" sub-chip
- **THEN** only files excluded by accepting an AI document suggestion are listed, and the count equals the chip's count

#### Scenario: Show unsupported files on request
- **WHEN** the user turns the "Not a supported file" chip on
- **THEN** the 3 unsupported files are listed with their unsupported reasons

#### Scenario: Filter choice remembered
- **WHEN** the user turns the "Thumbnail caches" chip off, restarts the app and opens the Excluded view
- **THEN** the chip is still off and thumbnail-cache files are not listed

#### Scenario: Counts follow decisions
- **WHEN** the user excludes 2 Library photos while the Excluded view is open
- **THEN** the "Excluded by you" count goes up by 2 without reopening the view

### Requirement: Excluded view search and sort
The Excluded view SHALL filter by text that matches a file's name or folder path, case-insensitively, combined with the category chips. It SHALL sort by name, folder, size or category.

#### Scenario: Search by folder
- **WHEN** the user types "Downloads" into the Excluded view search box
- **THEN** only excluded files whose name or folder contains "downloads" in any case, and whose category chip is on, are listed

#### Scenario: Sort by size
- **WHEN** the user sorts the Excluded view by size
- **THEN** the files are listed largest first

### Requirement: Excluded view pictures
The Excluded view SHALL have a Grid mode, which shows the shown excluded files in the gallery grid with their thumbnails or placeholders, and a List mode with a thumbnail column. Selecting a file in either mode SHALL show it in the inspector. The mode SHALL be remembered.

#### Scenario: Grid mode shows excluded thumbnails
- **WHEN** the user switches the Excluded view to Grid with only the AI "screenshot" chip on
- **THEN** the grid shows exactly the AI-excluded screenshots, with their thumbnails

#### Scenario: List mode shows a thumbnail per row
- **WHEN** the Excluded view is in List mode
- **THEN** each row shows its file's thumbnail, or the placeholder when there is none, beside its name, category and reasons

### Requirement: Put back in Library
The user SHALL be able to put the selected excluded files, or every file the current filters show, back in the Library. Putting back everything shown SHALL first be confirmed with the count. A file put back SHALL get an include decision that later rescans respect, and it SHALL be possible to exclude it again.

#### Scenario: Put back AI-excluded documents
- **WHEN** the AI "document" sub-chip alone is on, 12 files are shown, and the user chooses "Put back all shown" and confirms 12 files
- **THEN** the 12 files appear in the Library with reason "included by you", leave the Excluded view, and stay in the Library after a rescan

#### Scenario: Put back needs confirmation
- **WHEN** the user chooses "Put back all shown" and cancels the confirmation
- **THEN** no decision changes

#### Scenario: Put back a classifier exclusion
- **WHEN** the user selects 2 thumbnail-cache files and puts them back
- **THEN** both are in the Library with reason "included by you" and the "Thumbnail caches" count drops by 2

### Requirement: Excluded view scales
The Excluded view SHALL read rows a page at a time, never loading every excluded row before it draws. It SHALL get every chip count from one aggregate query. Opening it SHALL stay within the `excluded-view-open` budget.

#### Scenario: Excluded view opens at scale
- **WHEN** the Excluded view opens on a 1M-file catalog with 200,000 excluded files
- **THEN** the first page and every chip count are shown within the `excluded-view-open` budget, and reading more rows happens only as the user scrolls
