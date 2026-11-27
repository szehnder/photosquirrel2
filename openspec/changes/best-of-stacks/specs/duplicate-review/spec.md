## ADDED Requirements

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
