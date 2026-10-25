# scanning Specification

## Purpose
Turns files on disk into catalogued, classified and analyzed items, reading each source strictly read-only,
fast on SSDs, spinning disks and NAS alike, with every step persisted so work is never lost.

## Requirements

### Requirement: Persisted stages
Every discovered file SHALL advance through the stages discovered, probed, analyzed and thumbed, or end as skipped or failed with a reason, and each stage SHALL be persisted together with its results.

#### Scenario: Corpus files reach final stages
- **WHEN** the small corpus is scanned to completion
- **THEN** every truth-file photo is thumbed, every expected junk file is skipped with a reason, and no file remains at discovered or probed

### Requirement: Read-only scanning
The scanner MUST open source files for reading only and MUST NOT create, modify, rename or delete anything in a source tree.

#### Scenario: Scanner never opens for writing
- **WHEN** the small corpus is scanned with file-open instrumentation enabled
- **THEN** no open in a source tree requested write, create, truncate or append access

#### Scenario: Read-only tree scans fully
- **WHEN** a corpus drive whose files and folders are all read-only is scanned
- **THEN** the scan completes with the same results as on a writable copy and the tree snapshot is unchanged

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

### Requirement: Adaptive per-source I/O lanes
Each source SHALL be read by its own lane of 1 to 16 reader threads, starting at 8 for solid-state devices and 2 for rotational or unknown devices, growing while throughput improves by more than 10% and shrinking when latency rises; files within a lane SHALL be read in directory order.

#### Scenario: Lane starts by device class
- **WHEN** a scan starts on one source detected as solid-state and one of unknown type
- **THEN** their lanes start with 8 and 2 readers respectively

#### Scenario: Lane grows while throughput improves
- **WHEN** a simulated device whose throughput rises with concurrency up to 6 readers is scanned
- **THEN** its lane settles between 5 and 7 readers

#### Scenario: Lane shrinks on latency rise
- **WHEN** a simulated rotational device whose latency doubles above 2 readers is scanned starting at 4
- **THEN** its lane shrinks to at most 2 readers

### Requirement: Bounded memory
File buffers in flight SHALL be limited to two per lane reader and to 512 MB in total.

#### Scenario: Memory cap respected
- **WHEN** 3 lanes scan 40 MB files with instrumentation of buffers in flight
- **THEN** in-flight buffer bytes never exceed 512 MB and never exceed two buffers per reader

### Requirement: Failures recorded and retried at most twice
A file that cannot be read or analyzed SHALL be recorded as failed with its error, SHALL NOT stop the scan, and SHALL be retried on at most two later runs.

#### Scenario: Unreadable file recorded
- **WHEN** a corpus file is made unreadable and the source is scanned
- **THEN** the file is failed with a permission error and all other files complete

#### Scenario: Failed retried at most twice
- **WHEN** the same unreadable file is scanned in four consecutive runs
- **THEN** it is attempted in the first three runs only and stays failed

### Requirement: Progress snapshots
While a run is active the engine SHALL expose a progress snapshot with counts per stage and per source, bytes read, throughput, the current folder of each reader, the last 64 photos found and the last 32 errors; taking a snapshot MUST NOT depend on the number of files processed.

#### Scenario: Snapshot counts match catalog
- **WHEN** a snapshot is taken right after a run completes
- **THEN** its per-stage counts equal the catalog's per-stage counts for that run

#### Scenario: Recent lists bounded
- **WHEN** a run finds 500 photos and 100 errors
- **THEN** the snapshot holds exactly the 64 most recent photos and the 32 most recent errors

### Requirement: Analysis throughput
Analysis of 12 MP JPEGs from a warm cache SHALL reach at least 150 files per second on the 16-core reference machine.

#### Scenario: Analysis throughput within budget
- **WHEN** 2,000 12 MP corpus JPEGs in the page cache are scanned on the reference machine
- **THEN** the measured analysis rate is reported as a budget measurement and is at least 150 files per second

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
