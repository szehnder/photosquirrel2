# Spec Delta

## Purpose

Lets the engine recognise the same shot at another size, compression or with stripped metadata, and find such
candidates among a million photos in about a millisecond.

## ADDED Requirements

### Requirement: Two perceptual hashes per image
Each analyzed still image SHALL get a 64-bit DCT perceptual hash (low 8x8 frequencies of a 32x32 luminance image, compared with their median) and a 64-bit gradient difference hash, both computed after applying EXIF orientation.

#### Scenario: Known image has known hashes
- **WHEN** the reference fixture images are hashed
- **THEN** both hashes equal the golden values recorded for the current algorithm version

#### Scenario: Orientation tag matches rotated pixels
- **WHEN** a JPEG stored landscape with orientation 6 and the same image stored physically rotated with orientation 1 are hashed
- **THEN** their pHash and dHash distances are each at most 2

### Requirement: Robust to resizing and recompression
Hashes of a resized or recompressed copy SHALL stay within the derivative thresholds of the original: pHash distance <= 6 and dHash distance <= 8.

#### Scenario: Resized copy stays close
- **WHEN** a procedural fixture photo and its 384 px copy recompressed at quality 60 are hashed
- **THEN** pHash distance is at most 6 and dHash distance is at most 8

#### Scenario: Different photos far apart
- **WHEN** all pairs among 1,000 distinct procedural fixture photos are hashed
- **THEN** no pair is within both pHash distance 6 and dHash distance 8

### Requirement: Hashes are versioned
Every perceptual hash SHALL be recorded with its algorithm version, and hashes of different versions MUST NOT be compared.

#### Scenario: Version mismatch not compared
- **WHEN** a version-1 hash and a version-2 hash are compared
- **THEN** the comparison reports incompatible versions instead of a distance

### Requirement: Near-duplicate candidate lookup
A near-duplicate index SHALL return every stored image whose pHash is within distance 7 of the query and SHALL return only candidates verified on both hashes against the requested thresholds.

#### Scenario: Index recall at distance 7
- **WHEN** 100,000 random hashes plus planted neighbours at pHash distances 0 to 7 are indexed and queried
- **THEN** every planted neighbour is returned

#### Scenario: Only verified candidates returned
- **WHEN** the index is queried with thresholds pHash 6 and dHash 8
- **THEN** every returned candidate is within both thresholds of the query

### Requirement: Hashing within budget
Computing pHash and dHash from a 512 px decode SHALL take at most 0.3 ms per image on the reference machine.

#### Scenario: Hash cost within budget
- **WHEN** the hashing benchmark runs on 512 px decodes
- **THEN** the median per-image time is reported as a budget measurement and is at most 0.3 ms

### Requirement: Index lookup and build within budget
A lookup over 1,000,000 indexed hashes SHALL take at most 1 ms at p95, and building the index from 1,000,000 stored hashes SHALL take less than 1 s.

#### Scenario: Lookup within budget
- **WHEN** 10,000 random queries run against an index of 1,000,000 hashes
- **THEN** the p95 lookup time is at most 1 ms

#### Scenario: Index build under one second
- **WHEN** an index is built from 1,000,000 hashes in memory
- **THEN** the build completes in less than 1 s
