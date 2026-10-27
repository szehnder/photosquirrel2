# Spec Delta

## ADDED Requirements

### Requirement: Every placeholder says why
A placeholder SHALL record why there are no pixels: a missing decoder, a missing poster-frame tool, an image sequence, a format variant not supported (with its detail), a file that could not be read (with the error), or a format this version reads that an older version could not. It SHALL record the decoder generation that produced it.

#### Scenario: BigTIFF placeholder names the variant
- **WHEN** a BigTIFF file is analysed
- **THEN** its placeholder reason is an unsupported variant with detail "BigTIFF", and the record carries the current TIFF decoder generation

#### Scenario: Damaged JPEG placeholder keeps the error
- **WHEN** a JPEG whose data ends after its headers is analysed
- **THEN** its placeholder reason is "could not be read" with the decoder's error message

#### Scenario: Old placeholders read as an older generation
- **WHEN** a placeholder stored before this change is read
- **THEN** it deserialises with decoder generation 0 and its stored note
