# Spec Delta

## Purpose

Lets the user inspect every duplicate group and similar-shot suggestion, understand why copies were grouped and
why a keeper was chosen, and correct those choices before anything is exported.

## ADDED Requirements

### Requirement: Group list
The Duplicates view SHALL list all groups with keeper thumbnail, member count, reclaimable bytes and review state, sortable by reclaimable bytes (default), member count or capture date, and filterable by exact only, has derivatives, unreviewed, overridden and has suggestions.

#### Scenario: Groups listed with counts
- **WHEN** the Duplicates view opens on the scanned small corpus
- **THEN** every group is listed once with its member count and reclaimable bytes, largest reclaimable first

#### Scenario: Filter unreviewed groups
- **WHEN** the user reviews 3 groups and filters by unreviewed
- **THEN** those 3 groups are not listed and the review progress shows 3 reviewed

### Requirement: Side-by-side compare
Opening a group SHALL show its members side by side with thumbnail, dimensions, file size, format, quality estimate, capture date, source and path, and SHALL allow a synchronised zoomed comparison of two members.

#### Scenario: Members compared side by side
- **WHEN** the user opens a group of an original, a resized copy and a recompressed copy
- **THEN** three member cards show their dimensions, sizes, formats and quality estimates

### Requirement: Explanations
Each member SHALL show why it joined (exact, or derivative with pHash and dHash distances and the member it matched) and the group SHALL show "Why this copy" with the keeper's ranking reasons.

#### Scenario: Member join reason shown
- **WHEN** a group containing a 384 px thumbnail is opened
- **THEN** the thumbnail's card shows derivative with its two distances and the matched member

#### Scenario: Why this copy shown
- **WHEN** a group whose keeper won on pixel count is opened
- **THEN** the "Why this copy" panel names pixel count and both members' dimensions

### Requirement: Override keeper
The user SHALL be able to choose another member as keeper by clicking it or pressing its number key; the override SHALL be persisted immediately and shown as "chosen by you".

#### Scenario: Override keeper
- **WHEN** the user presses 2 in a group whose keeper is member 1
- **THEN** member 2 becomes the keeper with reason "chosen by you"

#### Scenario: Override survives restart
- **WHEN** the application is restarted after an override
- **THEN** the group still shows the user's chosen keeper

### Requirement: Separate a falsely merged member
The user SHALL be able to mark a member as not a duplicate; it SHALL leave the group at once, become its own keeper and stay separate after regrouping.

#### Scenario: Separate a member
- **WHEN** the user marks a member as not a duplicate
- **THEN** it disappears from the group, appears in the Library as its own item and remains separate after a rescan

### Requirement: Keeper sets shown as pairs
A RAW+JPEG keeper set SHALL be shown as one paired keeper card, and choosing either file of the pair SHALL keep the pair together.

#### Scenario: Keeper set shown as pair
- **WHEN** a group whose keeper set is a DNG and its camera JPEG is opened
- **THEN** one paired keeper card shows both files

### Requirement: Review flow
The user SHALL be able to mark a group reviewed and move to the next or previous group with J and K; marking reviewed SHALL advance to the next unreviewed group.

#### Scenario: Keyboard review flow
- **WHEN** the user presses R (mark reviewed) on a group
- **THEN** the group is marked reviewed and the next unreviewed group opens

### Requirement: Similar-shot suggestions
Similar-shot suggestions SHALL be shown in a separate panel as side-by-side pairs with the reason they were not merged; the user SHALL be able to dismiss a suggestion, and suggestions MUST NOT be merged into groups by any action in this view.

#### Scenario: Suggestions not collapsed
- **WHEN** the suggestions panel shows a burst pair taken 5 s apart
- **THEN** both shots remain separate Library items and the pair shows the timestamp difference as the reason

#### Scenario: Dismiss a suggestion
- **WHEN** the user dismisses a suggestion
- **THEN** it no longer appears in the panel after restart and both files are unchanged
