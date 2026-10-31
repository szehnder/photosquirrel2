# Spec Delta

## ADDED Requirements

### Requirement: Previews converted to sRGB
The probe SHALL record a still's colour space from an embedded ICC profile, a DNG preview colour space, EXIF InteropIndex `R03`, or the Canon maker note, else sRGB. Grid thumbnails and large previews of Adobe RGB, Display P3 and profiled JPEG images and RAW previews SHALL be converted to sRGB; sRGB images SHALL NOT be converted.

#### Scenario: Canon Adobe RGB JPEG thumbnail is converted
- **WHEN** a JPEG fixture with EXIF ColorSpace 0xFFFF, InteropIndex R03 and no profile, holding a saturated green patch, is thumbnailed
- **THEN** the thumbnail's patch is the patch converted from Adobe RGB (1998) to sRGB within a colour difference of 2

#### Scenario: RAW preview in Adobe RGB is converted
- **WHEN** a CR2-style fixture whose Canon maker note records Adobe RGB is thumbnailed from its embedded preview
- **THEN** the thumbnail is converted from Adobe RGB to sRGB

#### Scenario: Embedded profile is applied
- **WHEN** a JPEG fixture with an embedded Display P3 profile is shown in the large preview
- **THEN** the preview is converted from Display P3 to sRGB

#### Scenario: sRGB thumbnails are unchanged
- **WHEN** an untagged JPEG and a JPEG with an embedded sRGB profile are thumbnailed
- **THEN** both thumbnails are byte-identical to the thumbnails of the version before this change

### Requirement: Colour conversion does not change grouping
Perceptual hashes, quality estimates and grouping SHALL be computed from the unconverted analysis decode, so a colour conversion MUST NOT change any hash, group or keeper.

#### Scenario: Hashes unchanged by conversion
- **WHEN** an Adobe RGB fixture is analysed with conversion enabled and with it disabled
- **THEN** its perceptual hashes are equal

### Requirement: Older Adobe RGB thumbnails redone once
On the first rescan after this change, unchanged thumbed JPEG and RAW files whose name starts with `_` SHALL have their colour space read from the header, read-only; a file that is not sRGB SHALL get a converted thumbnail, without touching its hashes or groups. Other rows SHALL NOT be read for it, and large previews cached before SHALL NOT be shown again.

#### Scenario: Rescan recolours old Adobe RGB thumbnails only
- **WHEN** a catalog written before this change, with `_MG_` Adobe RGB JPEGs, a `_MG_` sRGB JPEG and `IMG_` JPEGs, is rescanned unchanged through a byte-counting file layer
- **THEN** only the `_MG_` files are read, the sRGB one only within the probe window, the Adobe RGB ones get converted thumbnails, and the dedup stamp is unchanged

#### Scenario: The re-check happens once
- **WHEN** the same source is rescanned again
- **THEN** no file is read for colour
