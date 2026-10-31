# Spec Delta

## MODIFIED Requirements

### Requirement: Placeholders when decoders are unavailable
Without a HEIC decoder, HEIC stills SHALL get a "preview unavailable" placeholder that names the missing decoder; without a configured poster-frame tool, videos SHALL get a styled placeholder showing duration and date. With the tool configured, a video SHALL get a poster frame, or, when the tool gets no frame, a placeholder whose reason is that the poster frame failed, with the tool's error. Large previews SHALL use the decoders detected now, without waiting for a rescan.

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

#### Scenario: Failing tool records a failed poster
- **WHEN** a MOV fixture is thumbnailed with a poster-frame tool that exits with an error
- **THEN** the placeholder's reason is "poster frame failed" with the tool's error, not "needs a poster-frame tool"

### Requirement: Every placeholder says why
A placeholder SHALL record why there are no pixels: a missing decoder, a missing poster-frame tool, a poster-frame tool that got no frame (with its error), an image sequence, a format variant not supported (with its detail), a file that could not be read (with the error), or a format this version reads that an older version could not. It SHALL record the decoder generation that produced it.

#### Scenario: BigTIFF placeholder names the variant
- **WHEN** a BigTIFF file is analysed
- **THEN** its placeholder reason is an unsupported variant with detail "BigTIFF", and the record carries the current TIFF decoder generation

#### Scenario: Damaged JPEG placeholder keeps the error
- **WHEN** a JPEG whose data ends after its headers is analysed
- **THEN** its placeholder reason is "could not be read" with the decoder's error message

#### Scenario: Old placeholders read as an older generation
- **WHEN** a placeholder stored before this change is read
- **THEN** it deserialises with decoder generation 0 and its stored note

## ADDED Requirements

### Requirement: Poster-frame retry at the start
When a poster-frame call with a seek past 0 s gives no frame, one more call SHALL be made at 0 s, except after a timeout or a cancel. A tool that cannot be started SHALL count as no tool.

#### Scenario: Short clip gets a frame
- **WHEN** a MOV fixture of 0.5 s is thumbnailed with a tool that fails with no output past the clip's end, as ffmpeg does
- **THEN** a poster-frame thumbnail is stored
