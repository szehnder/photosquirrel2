# Spec Delta

## Purpose

Lets the user browse, filter and open any photo in a collection of up to a million items with smooth scrolling,
instant jumps and lazily loaded thumbnails.

## ADDED Requirements

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
An Excluded view SHALL list every skipped or excluded file with its class and reasons, so that nothing is hidden silently.

#### Scenario: Excluded view lists reasons
- **WHEN** the user opens the Excluded view of a scanned small corpus
- **THEN** every truth-file junk, graphic and cache file is listed with its reasons

### Requirement: Live updates keep position
Items added while a scan runs SHALL appear without moving the item at the top of the viewport.

#### Scenario: New items keep scroll anchor
- **WHEN** 1,000 items with earlier dates are added while the user views the middle of the gallery
- **THEN** the item at the top of the viewport stays at the top
