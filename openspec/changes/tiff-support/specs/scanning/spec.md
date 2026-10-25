# Spec Delta

## MODIFIED Requirements

### Requirement: Junk costs only a probe
Files classified as junk or graphic from probe data SHALL NOT be read beyond the 256 KB probe, except plain TIFFs whose directories lie beyond the probe: those of at most 64 MB are read in full before they are classified, and larger ones at most 1 MiB beyond the probe.

#### Scenario: Junk costs only a probe
- **WHEN** the small corpus is scanned with a byte-counting file layer
- **THEN** no skipped file had more than 262,144 bytes read

### Requirement: One full read per candidate
Each analyzed candidate file SHALL be read in full at most once per run, plus at most its 256 KB probe, plus at most 1 MiB of directory reads for a plain TIFF larger than 64 MB.

#### Scenario: One full read per candidate
- **WHEN** the small corpus is scanned with a byte-counting file layer
- **THEN** bytes read for every analyzed file are at most its size plus 262,144

#### Scenario: End-directory TIFF read once
- **WHEN** a 20 MB TIFF with its directories after its pixel data is scanned with a byte-counting file layer
- **THEN** exactly its size is read and it is thumbed with its true dimensions and perceptual hashes

## ADDED Requirements

### Requirement: Large TIFFs analysed in one pass
A plain TIFF larger than 64 MB SHALL be hashed and decoded for analysis from one sequential read in its I/O lane, counted against the in-flight memory cap as one 64 MB buffer, so that it gets perceptual hashes and an image thumbnail like a smaller photo; a TIFF that cannot be decoded that way SHALL keep its content identity and get a placeholder giving the reason.

#### Scenario: Large TIFF analysed in one pass
- **WHEN** a TIFF larger than 64 MB with its directories after its pixel data is scanned with a byte-counting file layer
- **THEN** its identity equals the BLAKE3 of the file, it has perceptual hashes and an image thumbnail of its true aspect ratio, and at most its size plus 1 MiB was read

#### Scenario: Undecodable large TIFF keeps its identity
- **WHEN** a TIFF larger than 64 MB whose only strip decodes to more than 32 MB is scanned
- **THEN** it is thumbed with a "preview unavailable" placeholder naming the reason, and its content identity equals the BLAKE3 of the file

### Requirement: TIFFs catalogued without dimensions analysed again
An unchanged plain TIFF that an earlier version catalogued as thumbed without dimensions (its directories were beyond the probe) SHALL be analysed again on the next rescan, once. A TIFF whose dimensions the probe cannot read SHALL get the decoder's, and a TIFF whose dimensions neither can read SHALL NOT be thumbed, so no rescan reads such a file again unless it changes.

#### Scenario: TIFF catalogued without dimensions analysed again
- **WHEN** a catalog row of an unchanged TIFF that is thumbed without dimensions is rescanned
- **THEN** the file is analysed again, counted as re-analysed, gets its dimensions, and a second rescan reads nothing

#### Scenario: TIFF with unusual dimension types analysed once
- **WHEN** a TIFF whose ImageWidth and ImageLength are stored as 64-bit LONG8 values is scanned and then rescanned
- **THEN** it is thumbed with its true dimensions, and the rescan re-analyses nothing and reads none of its bytes
