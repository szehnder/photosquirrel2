## ADDED Requirements

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
