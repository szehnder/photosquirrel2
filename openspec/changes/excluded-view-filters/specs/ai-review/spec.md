# Spec Delta

## MODIFIED Requirements

### Requirement: Accept or reject suggestions
The user SHALL be able to accept or reject AI suggestions one by one or in bulk. Accepting an exclusion suggestion SHALL record a user exclusion with the AI's reason, and SHALL store that the decision came from an AI suggestion and of which kind (screenshot, document, graphic, not a photo).

#### Scenario: Accept exclusion suggestion
- **WHEN** the user accepts 5 screenshot suggestions in bulk
- **THEN** the 5 files are excluded with reason "excluded by you: screenshot (AI suggestion)", and each decision is stored as coming from an AI suggestion of kind screenshot

## ADDED Requirements

### Requirement: Putting back rejects the suggestion
Putting a file excluded by an accepted AI suggestion back in the Library SHALL mark that suggestion rejected, so that a later AI run on the unchanged file does not suggest it again.

#### Scenario: Put back stays put after another AI run
- **WHEN** the user puts back a file excluded by an accepted document suggestion and the AI queue then re-describes it with the same result
- **THEN** no exclusion suggestion is open for the file and it stays in the Library
