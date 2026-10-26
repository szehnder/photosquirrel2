# Spec Delta

## ADDED Requirements

### Requirement: Quarantined files whose group changed
When a regroup moves a quarantined file out of the group whose library copy covered it, the file SHALL stop counting as accounted for, and the report SHALL show it as pending with its quarantine path, the reason that its library copy is of another photo, and how to restore it.

#### Scenario: Quarantined frame of a split burst is pending
- **WHEN** frame N+1's CR2 was quarantined as covered by frame N's exported DNG, and a rescan splits the frames
- **THEN** the CR2 is pending with its quarantine path, the reason names the group change and the restore command, and its drive is not 100% accounted for
