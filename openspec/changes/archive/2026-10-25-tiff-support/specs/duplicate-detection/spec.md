# Spec Delta

## ADDED Requirements

### Requirement: TIFF copies grouped by look
TIFF copies of a photo SHALL group with its JPEG copies by look like any other resized or recompressed copy, whatever their directory placement, layout, sample format or size.

#### Scenario: TIFF copies group with their JPEG
- **WHEN** one picture is scanned as an 800 px JPEG, an 8-bit TIFF with its directories after its pixel data, a 16-bit LZW TIFF, a tiled JPEG-compressed pyramid TIFF and a TIFF larger than 64 MB
- **THEN** all five files are in one duplicate group
