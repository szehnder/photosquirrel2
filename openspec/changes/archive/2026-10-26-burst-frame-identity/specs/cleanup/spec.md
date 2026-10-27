# Spec Delta

## ADDED Requirements

### Requirement: Quarantined files whose group changed
When a regroup moves a quarantined file out of the group whose library copy covered it, the file SHALL stop counting as accounted for, and the report SHALL show it as pending with its quarantine path, the reason that its library copy is of another photo, and how to restore it.

#### Scenario: Quarantined frame of a split burst is pending
- **WHEN** frame N+1's CR2 was quarantined as covered by frame N's exported DNG, and a rescan splits the frames
- **THEN** the CR2 is pending with its quarantine path, the reason names the group change and the restore command, and its drive is not 100% accounted for

### Requirement: Cleanup waits for the frame-identity re-read
While an analysed file catalogued before frame identity was read is in the cleanup's scope, or in a duplicate group with a file in it, cleanup plan and run SHALL be refused with a message to rescan first, and the completion report SHALL flag the source as not confirmed.

#### Scenario: Stale groups refused before the rescan
- **WHEN** a catalog written before frame identity holds a merged burst and a cleanup plan made then, and the upgraded catalog has not been rescanned
- **THEN** the report flags the source, a new plan and a run of the old plan are refused with "rescan first", and nothing is moved; after the rescan the report is confirmed and planning works again
