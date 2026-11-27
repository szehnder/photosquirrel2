## ADDED Requirements

### Requirement: Exclude and include groups from the CLI
`groups exclude <group>` SHALL exclude the group as the review does, with an optional `--reason`, and `groups include <group>` SHALL include it again. `groups list --json` SHALL report, for every group, whether it is excluded. Neither command SHALL change any file.

#### Scenario: Exclude a group from the CLI
- **WHEN** `groups exclude` is run on a group of the small corpus with `--reason "not ours"`
- **THEN** `groups list --json` reports that group as excluded and reviewed, `report excluded` counts its copies under excluded by you, and no file changed

#### Scenario: Include a group from the CLI
- **WHEN** `groups include` is run on that group
- **THEN** `groups list --json` reports it as not excluded and its copies are no longer excluded
