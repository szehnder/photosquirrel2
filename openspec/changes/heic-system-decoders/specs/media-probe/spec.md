# Spec Delta

## MODIFIED Requirements

### Requirement: Decoder availability reported
The engine SHALL report, per format (HEIC/HEIF stills, AVIF stills, video poster frames), whether this machine can decode it and with which decoder, and when it cannot, the reason and how to enable it for the detected system. It SHALL still probe files of those formats when no decoder is available.

#### Scenario: HEIC without decoder
- **WHEN** a HEIC fixture is probed on a machine without a HEIC decoder
- **THEN** the probe reports format HEIC with dimensions, orientation, EXIF and the location of its embedded thumbnail, exactly as with a decoder

#### Scenario: Availability listed
- **WHEN** decoder availability is queried with no HEIC decoder and with no poster-frame tool configured
- **THEN** the report lists HEIC as unavailable with its reason and fix for the detected system, AVIF as available through the bundled decoder, and poster frames as unavailable with how to enable them
