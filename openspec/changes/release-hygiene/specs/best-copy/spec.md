# Spec Delta

## MODIFIED Requirements

### Requirement: Keeper reasons
The ranking SHALL return human-readable reasons for the keeper, naming the deciding criterion against the runner-up. The runner-up SHALL be the best-ranked member outside the keeper set whose content differs from the keeper's; only when every other member is a byte-identical copy SHALL it be the best-ranked of those copies. The ranking SHALL name the runner-up it used.

#### Scenario: Keeper reasons listed
- **WHEN** the keeper is chosen by pixel count
- **THEN** the reasons include the keeper's and the runner-up's dimensions

#### Scenario: Reasons compare against the best different copy
- **WHEN** a group holds a keeper, a byte-identical copy of it in a deeper folder and a half-size resized copy
- **THEN** the named runner-up is the resized copy, the reasons say more pixels with both dimensions, and the keeper is the same as before
