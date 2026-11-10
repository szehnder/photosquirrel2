# Spec Delta

## ADDED Requirements

### Requirement: Update channel setting
Settings SHALL store the update channel, `stable` or `beta`, shared by the application and the CLI. It SHALL be `stable` by default and for a settings file without the key. An unknown channel SHALL be rejected by validation and behave as stable. Changing the channel SHALL NOT change the update mode, and SHALL NOT cause a check when the mode is undecided or off.

#### Scenario: Channel defaults to stable
- **WHEN** settings are read from an empty configuration directory, and from a settings file written before the channel existed
- **THEN** the channel is stable in both, and every other value of the older file is unchanged

#### Scenario: Channel persists and is shared
- **WHEN** the user sets the channel to beta and restarts the application
- **THEN** the channel is beta, the update mode is what it was, and the CLI's update status reports beta

#### Scenario: Unknown channel rejected
- **WHEN** the settings file sets the update channel to `nightly`
- **THEN** validation reports the update channel key and the channel used is stable

### Requirement: Update channel choice
The Updates group of the settings window SHALL show the channel with a choice between "Stable" and "Beta". Choosing Beta SHALL first say that beta versions arrive early, may have faults, and may upgrade the catalog so that going back is not possible, and SHALL take effect only when the user confirms.

#### Scenario: Beta needs confirmation
- **WHEN** the user chooses "Beta" in the Updates group and cancels the confirmation, then chooses it again and confirms
- **THEN** the channel is stable after the cancel and beta after the confirmation, and no update request is made by either while the update mode is off
