## ADDED Requirements

### Requirement: Review settings
The settings SHALL have a `review` section with `stack_window_s` (whole seconds, 1-60, default 5) and `stack_similarity` (`strict`, `balanced` or `loose`, default `balanced`), shown on a Review page of the Settings window. A value out of range or unknown SHALL be reported as a field error and replaced by its default, without discarding the rest of the file.

#### Scenario: Review defaults
- **WHEN** the settings are read from an empty configuration directory
- **THEN** the stack window is 5 s, the similarity is balanced, and `doctor --json` reports both

#### Scenario: Invalid stack window
- **WHEN** the settings file sets `stack_window_s = 0`
- **THEN** loading reports a field error for `review.stack_window_s`, uses 5, and keeps every other setting from the file
