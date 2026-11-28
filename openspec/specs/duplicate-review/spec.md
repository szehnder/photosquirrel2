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

### Requirement: Group composition fits its panel
Every line of the "Why this copy" panel, both the keeper reasons and the "In this group" composition lines, SHALL stay within the panel's content area at every window width and UI font size. A line too long for one row SHALL wrap under its own text, and the count column SHALL stay aligned.

#### Scenario: Pair keeper composition wraps inside the panel
- **WHEN** a group whose keeper set is a RAW+JPEG pair is opened at the narrowest window width and at the largest UI font size
- **THEN** the line "keepers (RAW+JPEG pair): export takes both together" lies inside the panel's content area, on more than one row, with its count still in the count column

#### Scenario: Composition lines fit at the default size
- **WHEN** a group with an exact copy and two resized copies is opened at the default window width and UI font size
- **THEN** every composition line lies inside the panel's content area

### Requirement: Review progress card
The Duplicates list header SHALL show review progress as a card on a tinted surface. Its main figure SHALL be the number of groups left to review, with "N of M reviewed" and a progress bar below it. When no group is left, the card SHALL show a success state naming the total. The figures SHALL cover every group, whatever filter is set.

#### Scenario: Card shows groups left
- **WHEN** the user marks 3 of the small corpus's groups reviewed
- **THEN** the card's main figure is the group total minus 3 followed by "left", and it reads "3 of" the total "reviewed"

#### Scenario: Card shows all reviewed
- **WHEN** the user marks every group reviewed
- **THEN** the card shows its success state, reading "All" with the group total and "groups reviewed", and its progress bar is full

#### Scenario: Card ignores the list filter
- **WHEN** the user marks 2 groups reviewed and then filters by unreviewed
- **THEN** the card still counts every group and shows 2 reviewed

### Requirement: AI insight filters on groups
The Duplicates list SHALL filter groups by category, contains people, contains children, contains pets and "Not described yet" (the AI filters the gallery offers), combined with its other filters. A group SHALL match when any of its files' effective insights matches. "Not described yet" SHALL match groups in which no file has an insight. The filter SHALL be kept across restarts.

#### Scenario: Filter groups with children
- **WHEN** insights are stored for the scanned small corpus's keepers and the user filters the Duplicates list by "contains children"
- **THEN** exactly the groups having a file whose effective insight has at least one child are listed

#### Scenario: A user correction decides the match
- **WHEN** the user corrects a keeper's category from "nature" to "pets" and filters the Duplicates list by category "pets"
- **THEN** that keeper's group is listed

#### Scenario: Groups not described yet
- **WHEN** insights are stored for half of the corpus's keepers and the user filters by "Not described yet"
- **THEN** exactly the groups with no described file are listed

#### Scenario: AI filter combines with unreviewed
- **WHEN** the user reviews 2 of the groups that contain pets and then filters by "contains pets" and unreviewed
- **THEN** the groups that contain pets are listed except those 2

#### Scenario: Filter kept across restarts
- **WHEN** the user filters the Duplicates list by "contains people" and restarts the application
- **THEN** the Duplicates list opens filtered by "contains people"

#### Scenario: Filtering never changes groups
- **WHEN** the user applies and clears every AI filter in turn
- **THEN** every group's members and keeper are identical to before

### Requirement: Filtered review progress
With an AI filter set, the review progress card SHALL also show how many matching groups are reviewed ("Matching: N of M reviewed"), and marking a group reviewed SHALL open the next unreviewed matching group. J and K SHALL move only between matching groups.

#### Scenario: Matching progress shown
- **WHEN** 5 groups contain pets, the user filters by "contains pets" and marks 2 of them reviewed
- **THEN** the card reads "Matching: 2 of 5 reviewed" and its main figure still counts every group

#### Scenario: Review advances within the filter
- **WHEN** the list is filtered by "contains pets" and the user presses R on a group
- **THEN** the next unreviewed group that contains pets opens, skipping groups that do not match

### Requirement: Filtered list follows new insights
While an AI filter is set, a group SHALL join or leave the list within 1 s of an insight or correction being stored that changes its match, keeping the selected group and the scroll position when the selected group still matches.

#### Scenario: Newly described group joins the list
- **WHEN** the list is filtered by "contains children" and a recording provider stores an insight with 1 child for an undescribed keeper
- **THEN** that keeper's group appears in the list within 1 s and the selected group stays selected

### Requirement: How similar shots are found
The first time the user opens the similar shots, the view SHALL show an explainer stating the look-alike thresholds the grouping uses, that a pair is listed only when its metadata shows two shots, that similar shots are never merged, and that shots of one moment which look different are not listed. Dismissing it SHALL be kept across restarts, and an info button SHALL reopen it.

#### Scenario: Explainer shown the first time
- **WHEN** the user opens the Similar shots tab for the first time on a catalog with suggestions
- **THEN** the explainer is shown and names the perceptual hash limits 6 and 8 and the 2% aspect-ratio limit

#### Scenario: Explainer stays dismissed
- **WHEN** the user dismisses the explainer, restarts the application and opens the Similar shots tab
- **THEN** the explainer is not shown, and pressing the tab's info button shows it

#### Scenario: Explainer from a group
- **WHEN** the user has never seen the explainer and follows "Show similar shots" from a group
- **THEN** the explainer is shown

#### Scenario: Explainer thresholds follow the grouping
- **WHEN** the explainer is shown
- **THEN** the limits it names equal the limits the grouping applies to derivative edges

### Requirement: Suggestion reasons in plain words
Every similar-shot suggestion SHALL show a one-line reason in plain words naming what shows the two files are different shots (capture time difference, different camera, different frame, or two camera originals), and its tooltip SHALL add both hash distances against their limits and why such pairs are kept separate.

#### Scenario: Burst pair reason
- **WHEN** the suggestions panel shows a corpus burst pair taken N seconds apart
- **THEN** its reason reads "Look the same, but taken N s apart" and its tooltip names the pair's pHash and dHash distances with the limits 6 and 8

#### Scenario: Different frames reason
- **WHEN** the suggestions panel shows two converted burst frames that record different frame numbers
- **THEN** its reason names both frame numbers

### Requirement: Exclude a group from the review
The user SHALL be able to exclude a whole duplicate group from the Duplicates review, with an optional reason, by a key and by a button in the group's header. Excluding SHALL record the user's exclusion of every copy in the group, so that the whole group leaves the library as "excluded by you" and stays out when a rescan changes its keeper, SHALL mark the group reviewed and SHALL open the next unreviewed group. No file SHALL be changed, moved or removed.

#### Scenario: Exclude a group with E
- **WHEN** the user opens a group of 3 copies in the small corpus and presses E
- **THEN** every copy of the group is excluded by the user, the group is marked reviewed, the next unreviewed group opens, and the 3 files are byte-identical with unchanged modification times

#### Scenario: Excluded group with a reason
- **WHEN** the user excludes a group from its header with the reason "not ours"
- **THEN** every copy appears in the Excluded view with the reason "excluded by you: not ours"

#### Scenario: Excluded group stays out after its keeper changes
- **WHEN** the user excludes a group and a rescan adds a larger copy of the same photo to it
- **THEN** the new copy joins the group as its keeper, and every copy that was in the group stays excluded

#### Scenario: Excluded group is not exported
- **WHEN** the user excludes a group and then exports the small corpus
- **THEN** no file of that group is in the library, and the completion report lists all of its copies as excluded and none as pending

### Requirement: Excluded groups stay in the review
An excluded group SHALL stay in the Duplicates list, marked "Excluded, none kept", and SHALL count as reviewed in the review progress card. Its header SHALL offer to include it again, which SHALL remove the exclusion this action recorded from every copy, so the group returns to the library with its keeper. Right after an exclusion, a notice SHALL offer Undo, which restores the group's previous state, including whether it was reviewed.

#### Scenario: Excluded group listed and counted
- **WHEN** the user excludes 1 group and marks 2 others reviewed
- **THEN** the list shows the excluded group with "Excluded, none kept" and the progress card reads "3 of" the total "reviewed"

#### Scenario: Include the group again
- **WHEN** the user opens an excluded group and chooses Include again
- **THEN** its keeper and copies are no longer excluded, the group is shown without the excluded mark, and its keeper is the one it had before

#### Scenario: Undo an exclusion
- **WHEN** the user excludes an unreviewed group and chooses Undo in the notice
- **THEN** no copy of the group is excluded and the group is unreviewed again

### Requirement: Stacks tab
The Duplicates view SHALL have a Stacks tab, opened by T, that lists the stacks with their shot count and time span, and opens a stack as its shots in time order. The pick SHALL be marked "Best of" with its reasons, and a progress card SHALL count stacks left to review.

#### Scenario: Open a stack
- **WHEN** the user presses T on the small corpus and opens the first stack
- **THEN** its shots are shown in capture-time order, one is marked "Best of" with its reasons, and the card shows the number of stacks left

### Requirement: Stack review keys
In an open stack, the number keys SHALL pick a shot, Space SHALL compare the pick with the focused shot, S SHALL take the focused shot out of the stack, R SHALL mark the stack reviewed and open the next unreviewed one, and J and K SHALL move between stacks. No action in the Stacks tab SHALL change a file or a duplicate group.

#### Scenario: Pick with a number key
- **WHEN** the user presses 2 in an open stack
- **THEN** the second shot becomes the pick with "Picked by you", and every file and group is unchanged

#### Scenario: Review stacks with R
- **WHEN** the user presses R on a stack
- **THEN** it is marked reviewed, the next unreviewed stack opens, and the card counts one stack fewer left

#### Scenario: Take out with S
- **WHEN** the user focuses a shot in a stack of 3 and presses S
- **THEN** the stack shows 2 shots and the taken-out shot is in no stack with them

### Requirement: Stack thresholds in the review
The Stacks tab header SHALL show the stack window and similarity as controls, with the resulting count of stacks and stacked photos. Changing a control SHALL update the list and the count without a rescan, and SHALL save the setting.

#### Scenario: Retune from the header
- **WHEN** the user changes Similarity from Balanced to Strict in the Stacks header
- **THEN** the count of stacks and stacked photos updates within a second, the list shows the new stacks, and the settings file holds `strict`

### Requirement: Exclude a stack
In the Stacks tab, E and an "Exclude stack" button SHALL exclude every copy of every shot of the open stack ("keep none"), so the stack leaves the list; a notice SHALL offer Undo, which restores every decision it replaced. No file SHALL be changed.

#### Scenario: Exclude a stack
- **WHEN** a stack of the small corpus is excluded
- **THEN** every copy of every shot it held is excluded by the user, the stack is no longer listed, and Undo brings it back as it was

#### Scenario: Exclude a stack with E
- **WHEN** the user presses E on an open stack
- **THEN** the stack leaves the list, its files are excluded, no file changed, and the notice offers Undo

### Requirement: Duplicates inside a stack
A shot of a stack that is a duplicate group SHALL say how many copies it holds and offer to open that group in the Groups tab, and stacks and shots with copies SHALL be drawn as a small pile.

#### Scenario: A duplicate group in a stack
- **WHEN** the user opens a stack holding a duplicate group and chooses Open group on that shot
- **THEN** the Groups tab opens with that group, whose keeper is the shot
