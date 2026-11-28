## ADDED Requirements

### Requirement: Stacks from the CLI
`stacks list` SHALL list the stacks with their shots, pick and reasons, using the settings' thresholds unless `--window` or `--similarity` is given, and `--json` SHALL report them in the standard envelope. `stacks pick <file>` SHALL make that shot its stack's pick, and `stacks take-out <file>` SHALL take its group out of its stack. `export run --best-of only|both` SHALL export as the app's best-of choice does, and `report` SHALL count not picked files. None of these SHALL change a source file.

#### Scenario: List stacks as JSON
- **WHEN** `stacks list --json` is run on the scanned small corpus
- **THEN** every stack is listed with its shots in time order, exactly one pick, its reasons and the thresholds used

#### Scenario: Thresholds from the command line
- **WHEN** `stacks list --similarity loose --window 10` is run
- **THEN** the stacks are those of these thresholds, and the settings file is unchanged

#### Scenario: Pick from the CLI
- **WHEN** `stacks pick` is run on a shot that is not its stack's pick
- **THEN** `stacks list --json` reports it as the pick with the reason "Picked by you"

#### Scenario: Best-of export from the CLI
- **WHEN** `export run --best-of only` is run on the small corpus
- **THEN** the library holds exactly the picks and the unstacked keepers, and `report` counts the rest under not picked
