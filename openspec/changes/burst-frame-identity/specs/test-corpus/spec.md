# Spec Delta

## ADDED Requirements

### Requirement: Converted RAW bursts
Each preset SHALL include burst sets of 2 or 3 frames in one second, where each frame has a RAW-like original, a camera JPEG and a converted DNG recording OriginalRawFileName and a Canon maker note in DNGPrivateData, the first frame's converted DNG has no sub-second time, each frame's files are one truth group and the frames of a set are a must-not-merge set.

#### Scenario: Converted bursts in the medium preset
- **WHEN** the medium preset is generated with seed 42
- **THEN** it holds 24 converted RAW burst sets, each frame's files are one truth group, and each set is a must-not-merge set

#### Scenario: The corpus catches a merged converted burst
- **WHEN** the converted-burst sets are scanned with frame identity reading turned off by a test hook
- **THEN** eval-corpus reports at least one must-not-merge violation
