# Spec Delta

## ADDED Requirements

### Requirement: Videos in the loupe
A video in the loupe SHALL be shown from its large preview with a play mark and its duration, and SHALL offer Open in player (also the O key), which asks the operating system to open the file with its default application. When the large preview is unavailable, the loupe SHALL show the video's placeholder with its reason.

#### Scenario: Video shown large in the loupe
- **WHEN** the user opens a MOV in the loupe with a working poster-frame tool
- **THEN** the loupe shows its large poster frame with a play mark and its duration, and no "unavailable" notice

#### Scenario: Open in player from the loupe
- **WHEN** the user presses O on a video in the loupe
- **THEN** the operating system is asked to open that file with its default application, and the file is not modified

#### Scenario: Video without a poster tool in the loupe
- **WHEN** the user opens a MOV in the loupe with no poster-frame tool
- **THEN** the loupe shows the video's placeholder saying a poster-frame tool is needed, and Open in player is still offered
