# Spec Delta

## ADDED Requirements

### Requirement: Large previews of videos
With the poster-frame tool available, a video's large preview SHALL be a poster frame of up to 2048 px on the long edge, taken at the same seek as its thumbnail with one bounded tool call, at most one such call at a time, and cached like a photo's preview. Without the tool, or when it gets no frame, the large preview SHALL be unavailable with the same reason as the video's placeholder.

#### Scenario: Large preview of a MOV
- **WHEN** the large preview of a 1920x1080 MOV fixture is requested with a working poster-frame tool
- **THEN** a JPEG frame of 1920x1080 or less with a long edge above 384 px is returned, and a second request is served from the preview cache without calling the tool

#### Scenario: Large preview without the tool
- **WHEN** the large preview of a MOV fixture is requested with no poster-frame tool
- **THEN** it is unavailable with the reason that a poster-frame tool is needed, not "video"

#### Scenario: Large preview when the tool fails
- **WHEN** the large preview of a MOV fixture is requested with a poster-frame tool that exits with an error
- **THEN** it is unavailable with the reason "poster frame failed" and the tool's error
