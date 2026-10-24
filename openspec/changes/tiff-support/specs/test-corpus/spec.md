# Spec Delta

## ADDED Requirements

### Requirement: TIFF fixtures
The fixture generator SHALL write TIFF test images with the directories before or after the pixel data, strips or tiles, chunky or planar samples, 8- or 16-bit integer or 32-bit float samples, greyscale, RGB, RGBA, CMYK or JPEG-compressed YCbCr, uncompressed or LZW or Deflate with predictors, several pages or reduced-resolution levels, an orientation tag and camera EXIF; the ground-truth corpus presets SHALL NOT contain them.

#### Scenario: End-directory TIFF generated
- **WHEN** a TIFF with its directories after the pixel data is generated for each layout and sample format
- **THEN** its first directory offset lies after all of its strip or tile data and an independent TIFF decoder reads the expected dimensions and samples

#### Scenario: Corpus presets contain no TIFF
- **WHEN** the small corpus is generated
- **THEN** no generated file is a TIFF
