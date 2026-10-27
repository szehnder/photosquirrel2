# duplicate-detection Specification

## Purpose
Groups every copy of the same shot (byte-identical, resized, recompressed or thumbnail) across all sources,
while keeping distinct shots such as bursts apart, and records why each file joined its group.

## Requirements

### Requirement: Exact copies grouped
Files with identical full content identity SHALL be in the same group regardless of name, folder, source or modification time.

#### Scenario: Exact copies grouped
- **WHEN** a photo and two renamed byte-identical copies on two other corpus drives are grouped
- **THEN** all three are in one group with edge kind exact

### Requirement: Sampled identities need matching metadata
Files with equal sampled identity SHALL be grouped as exact only when their size, duration and pixel dimensions are also equal.

#### Scenario: Sampled match needs metadata
- **WHEN** two large videos have equal sampled identity but durations of 60.0 s and 61.0 s
- **THEN** they are not grouped

### Requirement: Derivative edges
Two images SHALL be joined by a derivative edge only when pHash distance <= 6, dHash distance <= 8, display aspect ratios differ by at most 2% and their metadata does not contradict.

#### Scenario: Resized thumbnail joins group
- **WHEN** a corpus original and its 384 px recompressed copy are grouped
- **THEN** they are in one group with edge kind derivative and the recorded distances

#### Scenario: Crop not merged
- **WHEN** a corpus original and its crop with a 5% aspect-ratio change are grouped
- **THEN** they are in different groups

#### Scenario: Threshold boundary respected
- **WHEN** two images have pHash distance 7 and dHash distance 2
- **THEN** no derivative edge is created between them

### Requirement: Contradicting metadata yields suggestions
Pairs that pass the hash test SHALL NOT be grouped, and SHALL be stored as similar-shot suggestions, when any of these holds: both have a capture time and the times differ at whole-second precision; both have sub-second capture times and those differ; both have camera serials or models and they differ; or both look like camera originals (maker notes present, same camera model) with identical stored pixel dimensions but different content.

#### Scenario: Shots 5 s apart only suggested
- **WHEN** two near-identical corpus shots taken 5 s apart with the same camera are grouped
- **THEN** they are in different groups and recorded as a similar-shot suggestion

#### Scenario: Same-second burst only suggested
- **WHEN** two near-identical corpus burst frames share the capture second but have different sub-second times
- **THEN** they are in different groups and recorded as a similar-shot suggestion

#### Scenario: Two camera originals not merged
- **WHEN** two near-identical camera originals have the same model and dimensions but different content
- **THEN** they are in different groups and recorded as a similar-shot suggestion

#### Scenario: Different camera not merged
- **WHEN** two near-identical images have different camera serial numbers
- **THEN** they are in different groups and recorded as a similar-shot suggestion

### Requirement: Chain-merge guard
A connected component whose members' capture timestamps span more than 60 s SHALL be split so that no resulting group spans more than 60 s.

#### Scenario: Chain spanning 60 s split
- **WHEN** derivative edges chain five images whose capture times span 90 s
- **THEN** no resulting group spans more than 60 s

### Requirement: Membership explained
Every group member SHALL carry the reason it joined: edge kind, the member it connects to, and the pHash and dHash distances for derivative edges.

#### Scenario: Membership reason recorded
- **WHEN** a group of an original, an exact copy and a thumbnail is formed
- **THEN** the copy's reason is exact and the thumbnail's reason is derivative with its two distances

### Requirement: Incremental regrouping
Grouping new or changed files SHALL produce the same groups as regrouping the whole catalog from scratch.

#### Scenario: Incremental equals full regroup
- **WHEN** the small corpus is grouped drive by drive in three steps and, separately, all at once
- **THEN** both groupings are identical

### Requirement: User separation honoured
The user SHALL be able to separate a member from its group; the separation SHALL be persisted and MUST keep the two files apart in every later regrouping.

#### Scenario: Separated member stays apart
- **WHEN** the user separates a member from its group and the catalog is regrouped
- **THEN** the member is in a different group and the separation is listed for that pair

### Requirement: Corpus grouping gate
Grouping the ground-truth corpus SHALL reach pairwise precision >= 0.995 and recall >= 0.97 with zero must-not-merge violations.

#### Scenario: Corpus grouping gate
- **WHEN** the medium corpus with seed 42 is scanned, grouped and evaluated against its truth file
- **THEN** precision is at least 0.995, recall at least 0.97 and must-not-merge violations are 0

### Requirement: TIFF copies grouped by look
TIFF copies of a photo SHALL group with its JPEG copies by look like any other resized or recompressed copy, whatever their directory placement, layout, sample format or size.

#### Scenario: TIFF copies group with their JPEG
- **WHEN** one picture is scanned as an 800 px JPEG, an 8-bit TIFF with its directories after its pixel data, a 16-bit LZW TIFF, a tiled JPEG-compressed pyramid TIFF and a TIFF larger than 64 MB
- **THEN** all five files are in one duplicate group

### Requirement: Recorded frame identities keep frames apart
Two files that both record a frame identity of the same kind, with no differing camera model, SHALL NOT be grouped when the identities differ, whatever their hashes and capture times, and SHALL be stored as a similar-shot suggestion that names both frames. A missing frame identity SHALL never count as a match.

#### Scenario: Converted burst frame without sub-second not merged
- **WHEN** frame N's converted DNG has no sub-second time and frame N+1's files have sub-second 130 ms, in the same second, pHash distance 2
- **THEN** frame N's DNG is grouped with frame N's CR2 and JPEG, frame N+1 has its own group and keeper, and the frames are a similar-shot suggestion

#### Scenario: Resized copy still joins its original
- **WHEN** a resized JPEG copy without maker notes or sub-second time is made of a camera JPEG that records file number 101-0034
- **THEN** the copy joins the camera JPEG's group

### Requirement: RAW files are camera originals
For the rule that two camera originals with the same model and pixel dimensions but different content are two shots, a RAW-format file, DNG included, SHALL count as a camera original whether or not maker notes were parsed. The rule SHALL NOT apply to two files that both record the same frame (equal file numbers, or equal OriginalRawFileName stems when neither has a file number) and the same capture time.

#### Scenario: Two converted DNGs of two frames not merged
- **WHEN** two converted DNGs of the same model and dimensions, with different content and no frame identity, are near-identical
- **THEN** they are in different groups and recorded as a similar-shot suggestion

#### Scenario: Re-saved DNG of one frame still joins
- **WHEN** a converted DNG and the same DNG with XMP metadata written into it (same dimensions, different bytes) both record file number 101-0034 and the same capture time
- **THEN** they are in one group
