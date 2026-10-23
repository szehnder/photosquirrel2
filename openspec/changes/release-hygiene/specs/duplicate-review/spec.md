# Spec Delta

## ADDED Requirements

### Requirement: Member badges fit their card
Each member card's badge SHALL stay within the card's content area at every window width and UI font size: its label SHALL be short enough to fit a card at the default font size and SHALL be cut with an ellipsis, with the full wording in its tooltip, when it does not fit.

#### Scenario: Copy badge stays within its card
- **WHEN** a group with a resized copy matched against the keeper is opened at the default and at the largest UI font size
- **THEN** that copy's badge reads "Resized copy of the keeper" and its bounds lie inside its card's content area

## MODIFIED Requirements

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
