# Spec Delta

## ADDED Requirements

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
