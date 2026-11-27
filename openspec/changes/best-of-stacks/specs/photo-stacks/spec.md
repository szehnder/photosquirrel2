## Purpose
Groups shots of one moment (bursts and near-identical frames taken seconds apart) into stacks above the duplicate groups, under thresholds the user controls, and picks the best shot of each stack without ever changing a group or a keeper.

## ADDED Requirements

### Requirement: Stacks of similar shots
Duplicate groups of photos SHALL be stacked when their keepers were taken at most the stack window apart and look alike within the stack similarity limits, with matching orientation and aspect ratios within 2%. A stack SHALL span at most 120 s from its first shot to its last. Videos, groups without a capture time and excluded groups SHALL NOT be stacked.

#### Scenario: Corpus bursts form stacks
- **WHEN** the small corpus is scanned and stacks are computed with the default thresholds
- **THEN** every `burst` must-not-merge set lies wholly within one stack, and no `minutes_apart` set lies within one stack

#### Scenario: Shots without capture time are not stacked
- **WHEN** two near-identical corpus photos without capture times are in different groups
- **THEN** neither is in a stack

#### Scenario: Stack span is limited
- **WHEN** a corpus burst has shots every 4 s for 3 minutes and the window is 5 s
- **THEN** no stack spans more than 120 s

### Requirement: Stacks never change groups
Stacks SHALL be derived from catalogued data without reading any source file, and SHALL NOT change any duplicate group, keeper or similar-shot suggestion.

#### Scenario: Stacks leave groups unchanged
- **WHEN** stacks are computed on the small corpus with the loosest thresholds
- **THEN** the groups, keepers and similar-shot suggestions are identical to those without stacks, and the corpus grouping gate scores are unchanged

#### Scenario: No source read for stacks
- **WHEN** stacks are computed after the corpus sources are taken offline
- **THEN** the same stacks are produced and no source file is opened

### Requirement: Stack thresholds are the user's
The stack window (1-60 s, default 5 s) and the stack similarity (strict, balanced or loose, default balanced) SHALL be settings, and a change SHALL recompute the stacks from catalogued data, without a rescan, and keep the user's picks and separations.

#### Scenario: Looser thresholds make fewer, larger stacks
- **WHEN** the similarity changes from strict to loose on the small corpus
- **THEN** the number of stacked photos does not decrease, the number of stacks reported changes accordingly, and no file is read

#### Scenario: Window of 1 s keeps a 5 s pair apart
- **WHEN** the window is 1 s and two near-identical corpus shots were taken 5 s apart
- **THEN** they are not in one stack, and with a window of 5 s they are

### Requirement: Best shot of a stack
Each stack SHALL have exactly one pick, chosen in this order: the shot the user picked, when it is still in the stack; the sharpest shot, counting sharpness scores within 10% of each other as equal; the shot with fewer clipped pixels; the better copy by the keeper rules; the earliest capture time. The pick SHALL show its reasons, and the choice SHALL be deterministic for the same catalog and thresholds. AI insights SHALL NOT affect the pick.

#### Scenario: The sharp frame is picked
- **WHEN** a stack holds three frames of one burst, two of them blurred
- **THEN** the unblurred frame is the pick with the reason "Sharpest"

#### Scenario: Clipping breaks a sharpness tie
- **WHEN** two equally sharp frames differ only in that one has 8% blown highlights
- **THEN** the other frame is the pick with the reason "Least clipped"

#### Scenario: Your pick is kept
- **WHEN** the user picks a blurred frame, restarts the application and the stack is recomputed
- **THEN** that frame is still the pick with the reason "Picked by you"

#### Scenario: Pick is stable
- **WHEN** stacks are computed twice on the same catalog with the same thresholds
- **THEN** every stack has the same pick

### Requirement: Take a shot out of a stack
The user SHALL be able to take a shot's group out of its stack. That group SHALL then never be stacked with the groups that stack held, under any thresholds, until the user puts it back; the decision SHALL survive restarts and rescans.

#### Scenario: Taken-out shot stays out
- **WHEN** the user takes a shot out of a stack of 4, loosens the thresholds and rescans
- **THEN** that shot is in no stack with the other 3, and the other 3 are still stacked

### Requirement: Shot quality from the thumbnail
Every photo with a thumbnail SHALL have a sharpness score and the fractions of its clipped highlights and shadows, computed from its stored thumbnail. Photos analysed before this existed SHALL get them in the background from the stored thumbnails, without reading the sources and without blocking the interface. A photo without a thumbnail SHALL have no score and rank after scored shots on that key.

#### Scenario: Scores for new photos
- **WHEN** the small corpus is scanned
- **THEN** every photo with a thumbnail has a sharpness score and clipped fractions, and a blurred copy of a photo scores lower sharpness than the photo

#### Scenario: Existing catalog filled in from thumbnails
- **WHEN** a catalog made before this change is opened with its sources offline
- **THEN** every photo with a stored thumbnail gets its scores, no source file is opened, and the gallery stays responsive while it runs
