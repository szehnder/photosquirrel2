# Spec Delta

## ADDED Requirements

### Requirement: Open a video in the system player
The inspector SHALL offer Open in player for a video copy on an online source, asking the operating system to open the file with its default application without PhotoSquirrel opening or changing it; for a copy on an offline source the action SHALL be disabled with the reason.

#### Scenario: Open a video copy in the player
- **WHEN** the user chooses Open in player for a video copy on an online source
- **THEN** the operating system is asked to open that path with its default application and the file is not modified

#### Scenario: Offline video copy
- **WHEN** a video's only copy is on an offline source
- **THEN** Open in player is disabled and its tooltip says the source is offline
