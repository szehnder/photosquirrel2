# gallery Specification

## Purpose
Lets the user browse, filter and open any photo in a collection of up to a million items with smooth scrolling,
instant jumps and lazily loaded thumbnails.

## Requirements

### Requirement: Whole-collection grid
The gallery SHALL lay out the complete ordered result of the current filter and sort, with a scrollbar that reflects the exact item count and the ability to jump to any position without loading the items before it.

#### Scenario: Scrollbar reflects total
- **WHEN** the gallery shows a 1,000,000-item synthetic catalog
- **THEN** the scroll range corresponds to exactly 1,000,000 items at the current cell size

#### Scenario: Jump to any position
- **WHEN** the user drags the scrollbar to 50% of a 1,000,000-item gallery
- **THEN** the item at index 500,000 is visible and its cell is drawn within 100 ms

### Requirement: Zoomable cells
Cell size SHALL be adjustable between 96 and 512 px, and zooming SHALL keep the item under the pointer (or the selection when using the keyboard) in view.

#### Scenario: Zoom clamps to range
- **WHEN** the user zooms out past the minimum and in past the maximum
- **THEN** cell size stops at 96 px and at 512 px

#### Scenario: Zoom keeps anchor item
- **WHEN** the user zooms with the pointer over item 12,345
- **THEN** item 12,345 is still under the pointer after the zoom

### Requirement: Prioritised lazy thumbnails
Thumbnails SHALL load visible cells first, then up to 2 screens ahead in the scroll direction; requests for cells that leave this window SHALL be cancelled, and cells without a thumbnail SHALL show their dominant colour, then fade the image in.

#### Scenario: Visible thumbnails load first
- **WHEN** the gallery opens at a position with 60 visible cells and a cold texture cache
- **THEN** the first 60 completed decodes are exactly the visible cells

#### Scenario: Offscreen requests cancelled
- **WHEN** the user flings 50,000 items in 1 s
- **THEN** pending decodes for cells more than 2 screens from the final viewport are cancelled and never uploaded

#### Scenario: Dominant colour placeholder
- **WHEN** a visible cell's thumbnail is not decoded yet
- **THEN** the cell is filled with the item's stored dominant colour

### Requirement: Bounded texture cache
Thumbnail textures SHALL be held in a least-recently-used cache of at most 3,000 textures and 512 MB of GPU memory.

#### Scenario: Texture cache bounded
- **WHEN** the user scrolls through 100,000 items
- **THEN** the cache never holds more than 3,000 textures or 512 MB

### Requirement: Scroll performance
While flinging through 1,000,000 items with warm textures, the 95th percentile frame time SHALL be at most 8.3 ms; the first gallery frame after start-up with a 1,000,000-item catalog SHALL appear within 600 ms.

#### Scenario: 1M scroll p95 within 8.3 ms
- **WHEN** the release application runs its render benchmark on a 1,000,000-item catalog
- **THEN** the frame-interval p95, recorded as a budget measurement, is at most 8.3 ms

#### Scenario: First gallery frame within 600 ms
- **WHEN** the release application starts on a 1,000,000-item catalog with its startup measurement flag
- **THEN** the time to the first gallery frame with cells, recorded as a budget measurement, is at most 600 ms

### Requirement: Duplicate stacks
By default each duplicate group SHALL appear as one tile showing its keeper with a badge of the member count; the user SHALL be able to expand a stack in place or show all copies.

#### Scenario: Group shows as one stack
- **WHEN** the Library view shows a group of 4 copies
- **THEN** one tile with the keeper's thumbnail and a badge "4" is shown

#### Scenario: Expand a stack
- **WHEN** the user expands that stack
- **THEN** the 4 members appear in place with the keeper marked

### Requirement: Sort and filter
The gallery SHALL sort by capture date (default), file name or file size, and filter by class, media kind, source, folder, capture date range, camera model and presence of GPS; filters SHALL combine with AND.

#### Scenario: Filter by camera model
- **WHEN** the user filters a scanned small corpus by one camera model
- **THEN** exactly the items with that camera model are shown and the count is displayed

### Requirement: Timeline scrubber
A timeline scrubber SHALL show the capture-date range of the current result and jump to the first item of the chosen year or month.

#### Scenario: Scrubber jumps to year
- **WHEN** the user clicks 2012 on the scrubber of a date-sorted gallery
- **THEN** the first visible item is the earliest item captured in 2012

### Requirement: Selection
The gallery SHALL support click, Shift-click range, Ctrl/Cmd-click toggle, select all and arrow-key navigation, keeping the focused item in view.

#### Scenario: Shift-click selects range
- **WHEN** the user clicks item 10 and Shift-clicks item 20
- **THEN** items 10 through 20 are selected

### Requirement: Loupe
Pressing Enter or E on an item SHALL open it in a loupe fitted to the window using its large preview, with arrow keys moving to neighbours and Esc returning to the grid at the same position.

#### Scenario: Loupe opens selected photo
- **WHEN** the user selects a photo and presses E
- **THEN** the loupe shows that photo fitted to the window from its 2048 px preview

#### Scenario: Loupe returns to same position
- **WHEN** the user moves 5 photos forward in the loupe and presses Esc
- **THEN** the grid shows and selects the photo last shown in the loupe

### Requirement: Offline photos browsable
Photos on offline sources SHALL remain in the gallery from their stored thumbnails; the loupe SHALL show the thumbnail with a "source offline" notice when no large preview can be made.

#### Scenario: Offline photo shows thumbnail
- **WHEN** the loupe opens a photo whose source is offline and which has no cached large preview
- **THEN** the stored thumbnail is shown with a "source offline" notice

### Requirement: Excluded view
An Excluded view SHALL list every skipped or excluded file with its class and reasons, so that nothing is hidden silently. Each file SHALL have exactly one exclusion category, chosen in order: excluded by you, accepted AI suggestion, thumbnail cache, graphic, too small, could not be read, not a supported file. The view SHALL show each file's category.

#### Scenario: Excluded view lists reasons
- **WHEN** the user opens the Excluded view of a scanned small corpus and turns every category on
- **THEN** every truth-file junk, graphic and cache file is listed with its reasons

#### Scenario: Each excluded file has one category
- **WHEN** a corpus file is a graphic inside a thumbnail cache folder and the user also excluded it with reason "logo"
- **THEN** its category is "excluded by you", and its reasons still list the cache folder and the graphic format

### Requirement: Live updates keep position
Items added while a scan runs SHALL appear without moving the item at the top of the viewport.

#### Scenario: New items keep scroll anchor
- **WHEN** 1,000 items with earlier dates are added while the user views the middle of the gallery
- **THEN** the item at the top of the viewport stays at the top

### Requirement: Toolbar wraps without stray dividers
When the Library toolbar's filters wrap onto more lines, no separator SHALL appear at the start or end of a line or on a line of its own; filter groups SHALL stay visually separated and every control SHALL stay inside the toolbar.

#### Scenario: Toolbar has no stray divider
- **WHEN** the Library is laid out at window widths from 960 px to 1920 px in 20 px steps, with the inspector closed and open
- **THEN** the toolbar contains no free-standing separator element, the gap between filter groups is wider than the gap within a group, and every filter control lies inside the toolbar

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
