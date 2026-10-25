# Spec Delta

## MODIFIED Requirements

### Requirement: Placeholders when decoders are unavailable
Without a HEIC decoder, HEIC stills SHALL get a "preview unavailable" placeholder that names the missing decoder; without a configured poster-frame tool, videos SHALL get a styled placeholder showing duration and date. With the tool configured, a video SHALL get a poster frame. Large previews SHALL use the decoders detected now, without waiting for a rescan.

#### Scenario: HEIC placeholder names the missing decoder
- **WHEN** a HEIC fixture is thumbnailed with no HEIC decoder
- **THEN** a placeholder record is stored whose note says no HEIC decoder is available on this system, with the photo's capture date

#### Scenario: Large HEIC preview after the decoder appears
- **WHEN** a HEIC catalogued with a placeholder is opened in the large preview with a HEIC decoder now available and no rescan
- **THEN** the large preview is decoded and shown

#### Scenario: Video placeholder without tool
- **WHEN** a MOV fixture is thumbnailed with no poster-frame tool configured
- **THEN** a placeholder record with the video's duration and capture date is stored instead of a frame

#### Scenario: Poster frame from configured tool
- **WHEN** a MOV fixture is thumbnailed with the poster-frame tool path set to a working executable
- **THEN** a 384 px poster-frame thumbnail is stored
