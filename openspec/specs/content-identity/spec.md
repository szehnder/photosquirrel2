# content-identity Specification

## Purpose
Gives every file a content identity so that byte-identical copies anywhere across drives are recognised as the
same file, with a cheaper sampled identity for very large videos.

## Requirements

### Requirement: Full identity for files up to 256 MB
A file of at most 268,435,456 bytes (256 MB) SHALL have a full identity: the BLAKE3 hash of its entire content, reported as 64 lowercase hexadecimal characters and marked full.

#### Scenario: Full hash matches reference
- **WHEN** a 5 MB fixture is hashed
- **THEN** its identity equals the reference BLAKE3 of the file bytes and is marked full

#### Scenario: Boundary size hashed fully
- **WHEN** a file of exactly 268,435,456 bytes is hashed
- **THEN** its identity is marked full

#### Scenario: Renamed copy has same identity
- **WHEN** a file and a renamed copy with a different modification time are hashed
- **THEN** both identities are equal

### Requirement: Sampled identity for large files
A file larger than 256 MB SHALL have a sampled identity: BLAKE3 over its size and its first, middle and last 4 MB, marked sampled, reading no more than 12 MB of the file.

#### Scenario: Large video sampled
- **WHEN** a 300 MB file is hashed through a byte-counting reader
- **THEN** its identity is marked sampled and at most 12,582,912 bytes are read

#### Scenario: Middle edit changes sampled identity
- **WHEN** one byte at the exact middle offset of a 300 MB file is changed
- **THEN** the sampled identity changes

#### Scenario: Size is part of sampled identity
- **WHEN** two large files share their first, middle and last 4 MB but differ in size
- **THEN** their sampled identities differ

### Requirement: Identity from the single analysis read
The full identity SHALL be computed from the same in-memory buffer used for decoding when the file is at most 64 MB, and from a streamed read otherwise, so that each candidate file is read once.

#### Scenario: One read per file
- **WHEN** a 12 MB JPEG is analyzed through a byte-counting reader
- **THEN** its bytes are read exactly once in total for hashing and decoding

#### Scenario: Large file streamed
- **WHEN** a 100 MB file is hashed
- **THEN** its identity equals the reference BLAKE3 and no buffer larger than 64 MB is held

### Requirement: Identity is stable
Identity computation SHALL be deterministic across runs, platforms and thread counts.

#### Scenario: Hash is stable across runs
- **WHEN** the same fixture is hashed in two processes with different thread counts
- **THEN** both identities are identical
