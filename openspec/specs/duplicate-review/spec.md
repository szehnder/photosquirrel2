# duplicate-review Specification

## Purpose
Lets the user inspect every duplicate group and similar-shot suggestion, understand why copies were grouped and
why a keeper was chosen, and correct those choices before anything is exported.

## Requirements

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
Each member SHALL show why it joined (exact, or derivative with pHash and dHash distances and the member it matched) and the group SHALL show "Why this copy" with the keeper's ranking reasons. The reasons and the "Next best" line SHALL name the same member: the runner-up the ranking compared the keeper against.

#### Scenario: Member join reason shown
- **WHEN** a group containing a 384 px thumbnail is opened
- **THEN** the thumbnail's card shows derivative with its two distances and the matched member

#### Scenario: Why this copy shown
- **WHEN** a group whose keeper won on pixel count is opened
- **THEN** the "Why this copy" panel names pixel count and both members' dimensions

#### Scenario: Why this copy and Next best name the same copy
- **WHEN** a group of a keeper, a byte-identical copy of it ranked second and a smaller resized copy ranked third is opened
- **THEN** the reasons compare the keeper with the resized copy's dimensions and the "Next best" line names that copy's number, not the identical copy's

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

### Requirement: Merge groups manually
The user SHALL be able to merge two groups that automatic matching kept apart. A merge SHALL change no file; it SHALL be persisted as a user decision ("always pair" between the groups' keepers, ADR-0033) that regrouping respects, the merged group's keeper SHALL be recomputed by the best-copy ranking unless the user chose one, separations SHALL win over merges, and the user SHALL be able to undo the merge.

#### Scenario: Merge two groups
- **WHEN** the user merges a group with another group
- **THEN** one group holds the members of both with a recomputed keeper, it stays merged after a rescan, and undoing the merge splits it back into the two groups

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

### Requirement: Member badges fit their card
Each member card's badge SHALL stay within the card's content area at every window width and UI font size: its label SHALL be short enough to fit a card at the default font size and SHALL be cut with an ellipsis, with the full wording in its tooltip, when it does not fit.

#### Scenario: Copy badge stays within its card
- **WHEN** a group with a resized copy matched against the keeper is opened at the default and at the largest UI font size
- **THEN** that copy's badge reads "Resized copy of the keeper" and its bounds lie inside its card's content area
