# photo-classification Specification

## Purpose
Decides from cheap probe data whether a file is a photo or phone/camera video the user cares about, rather than
an icon, UI graphic or cache thumbnail, and always explains the decision so nothing disappears silently.

## Requirements

### Requirement: Every file classified with reasons
Every probed file SHALL be assigned exactly one class of `photo`, `likely_photo`, `graphic`, `thumbnail_cache` or `junk`, together with at least one human-readable reason. Classification MUST depend only on probe data, the file's path and settings.

#### Scenario: Class and reason assigned
- **WHEN** every file of the media fixture set is classified
- **THEN** each file has exactly one class and a non-empty reason list

#### Scenario: Classification is deterministic
- **WHEN** the same probe record, path and settings are classified twice
- **THEN** both results are identical

### Requirement: Camera EXIF means photo
A still image with camera EXIF (make, model, DateTimeOriginal or exposure data) SHALL be classified `photo` regardless of its size, unless its path is a known cache folder.

#### Scenario: Camera EXIF wins over size
- **WHEN** a 150x150 JPEG with make "NIKON" and DateTimeOriginal is classified
- **THEN** it is `photo` with reason "camera EXIF"

### Requirement: Small images are junk
A still image without camera EXIF SHALL be classified `junk` when its shorter side is below 200 px or its pixel count is below 120,000; these thresholds MUST come from settings.

#### Scenario: Tiny image is junk
- **WHEN** a 180x500 PNG without EXIF is classified with default settings
- **THEN** it is `junk` with a reason naming the 200 px minimum side

#### Scenario: Boundary size is kept
- **WHEN** a 200x600 JPEG without EXIF (120,000 pixels) is classified with default settings
- **THEN** it is `likely_photo`

#### Scenario: Custom threshold applied
- **WHEN** the minimum side setting is changed to 100 px and a 180x800 JPEG without EXIF is classified
- **THEN** it is `likely_photo`

### Requirement: Cache folders detected
Files under known cache locations (`.thumbnails`, folders containing `Thumbs.db`, `AppData`, `node_modules`, `.cache`, Apple Photos/iPhoto `resources/proxies`, Lightroom previews) SHALL be classified `thumbnail_cache`.

#### Scenario: Lightroom previews are cache
- **WHEN** a JPEG with camera EXIF at `Lightroom/Catalog Previews.lrdata/0/A1/abc.jpg` is classified
- **THEN** it is `thumbnail_cache` with a reason naming the Lightroom previews folder

#### Scenario: Thumbs.db neighbour is cache
- **WHEN** a JPEG sits in a folder that also contains `Thumbs.db`
- **THEN** it is `thumbnail_cache`

### Requirement: Graphics detected
PNG, GIF, SVG and ICO files without camera EXIF that use a palette or alpha channel SHALL be classified `graphic`.

#### Scenario: Palette PNG is graphic
- **WHEN** a 1024x768 palette PNG with transparency and no EXIF is classified
- **THEN** it is `graphic`

#### Scenario: ICO is graphic
- **WHEN** a 256x256 ICO file is classified
- **THEN** it is `graphic`

### Requirement: Phone and camera videos
A QuickTime or MP4 video with device make/model or a creation date SHALL be classified `photo` with media kind video; a video shorter than 2 s or with a shorter side below 320 px SHALL be classified `junk`.

#### Scenario: Phone video is photo
- **WHEN** a 1920x1080, 12 s MOV with make "Apple" is classified
- **THEN** it is `photo` with media kind video

#### Scenario: Short clip is junk
- **WHEN** a 1.5 s MP4 at 1280x720 is classified
- **THEN** it is `junk` with a reason naming the 2 s minimum duration

### Requirement: Other images are likely photos
An image matching none of the rules above SHALL be classified `likely_photo`.

#### Scenario: Plain large JPEG is likely photo
- **WHEN** a 3000x2000 JPEG without EXIF outside any cache folder is classified
- **THEN** it is `likely_photo` with reason "no camera metadata"
