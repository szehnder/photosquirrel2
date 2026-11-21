# Spec Delta

## ADDED Requirements

### Requirement: Suggestions explained
The suggestions command's JSON output SHALL give each suggestion a reason kind (capture time, sub-second, camera serial, camera model, two camera originals or frame) and the same plain-words explanation the app shows, and the explain command SHALL print that explanation for a pair that is a suggestion.

#### Scenario: Suggestion JSON carries the explanation
- **WHEN** the suggestions command runs with JSON output on the scanned small corpus
- **THEN** each burst pair has reason kind capture time and an explanation that names its time difference

#### Scenario: Explain prints the reason
- **WHEN** the explain command runs on the two files of a burst pair taken 5 s apart
- **THEN** it prints "Look the same, but taken 5 s apart" with both distances and their limits
