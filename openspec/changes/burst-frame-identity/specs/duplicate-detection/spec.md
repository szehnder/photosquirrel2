# Spec Delta

## ADDED Requirements

### Requirement: Recorded frame identities keep frames apart
Two files that both record a frame identity of the same kind, with no differing camera model, SHALL NOT be grouped when the identities differ, whatever their hashes and capture times, and SHALL be stored as a similar-shot suggestion that names both frames. A missing frame identity SHALL never count as a match.

#### Scenario: Converted burst frame without sub-second not merged
- **WHEN** frame N's converted DNG has no sub-second time and frame N+1's files have sub-second 130 ms, in the same second, pHash distance 2
- **THEN** frame N's DNG is grouped with frame N's CR2 and JPEG, frame N+1 has its own group and keeper, and the frames are a similar-shot suggestion

#### Scenario: Resized copy still joins its original
- **WHEN** a resized JPEG copy without maker notes or sub-second time is made of a camera JPEG that records file number 101-0034
- **THEN** the copy joins the camera JPEG's group

### Requirement: RAW files are camera originals
For the rule that two camera originals with the same model and pixel dimensions but different content are two shots, a RAW-format file, DNG included, SHALL count as a camera original whether or not maker notes were parsed.

#### Scenario: Two converted DNGs of two frames not merged
- **WHEN** two converted DNGs of the same model and dimensions, with different content and no frame identity, are near-identical
- **THEN** they are in different groups and recorded as a similar-shot suggestion
