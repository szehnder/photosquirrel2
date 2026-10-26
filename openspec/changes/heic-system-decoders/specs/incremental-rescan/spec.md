# Spec Delta

## ADDED Requirements

### Requirement: Previews filled in once a decoder is available
A file analysed without the decoder its format needs SHALL record which decoder was missing. When that decoder is available, the next rescan SHALL read only those files, once each, and give them previews, perceptual hashes and look-matching, updating groups and keepers; while it is missing they SHALL be counted and not read. Catalogs from earlier versions SHALL have such files marked.

#### Scenario: Rescan decodes only files that waited for a decoder
- **WHEN** a source of 20 JPEGs and 5 HEIC photos scanned without a HEIC decoder is rescanned unchanged with one, through a byte-counting file layer
- **THEN** each HEIC is read once in full, no JPEG byte is read, the 5 get image thumbnails and perceptual hashes, and the summary counts 5 previews filled in

#### Scenario: HEIC grouped with its resized JPEG after the decoder arrives
- **WHEN** a HEIC photo and a resized JPEG copy of the same image, separate after a scan without a HEIC decoder, are rescanned with one
- **THEN** they form one duplicate group whose keeper is the HEIC, with a look-based join reason

#### Scenario: Files keep waiting while the decoder is missing
- **WHEN** the same source is rescanned unchanged and still without a HEIC decoder
- **THEN** zero bytes of file content are read and the summary reports 5 HEIC photos awaiting a decoder

#### Scenario: Old catalog marks undecoded HEIC and AVIF
- **WHEN** a catalog written before this change, holding HEIC and AVIF rows with placeholders and a HEIC row with hashes, is opened
- **THEN** the placeholder rows are marked as waiting for their decoder, the row with hashes is not, and the next rescan fills in the AVIF previews with the bundled decoder

### Requirement: A re-decode never fails an analysed file
A file that cannot be read to fill in its preview SHALL keep its catalogued analysis, stage, group and waiting mark, and the error SHALL be reported. A file whose data cannot be decoded SHALL keep its exact-copy analysis and placeholder, SHALL no longer be waiting, and the error SHALL be reported.

#### Scenario: Unreadable file keeps its analysis during a re-decode
- **WHEN** a waiting HEIC that is the keeper of an exported exact-copy group cannot be opened during the rescan that fills in previews
- **THEN** it keeps its stage, analysis and group, it is still waiting, the error is reported, and the export plan copies nothing new

#### Scenario: Corrupt HEIC keeps its exact-copy analysis
- **WHEN** a waiting HEIC whose image data is corrupt is re-decoded with a decoder that rejects it
- **THEN** it keeps its stage, identity, group and placeholder, it is no longer waiting, the error names the decode failure, and a further rescan reads none of its bytes

#### Scenario: Content changed under an unchanged fingerprint is analysed again
- **WHEN** a HEIC waiting for a decoder is rewritten with different bytes of the same length and its modification time restored, and the next rescan has a HEIC decoder
- **THEN** the re-decode's full read finds another identity, and the file is analysed again as a changed file: the identity of the new bytes, hashes, no waiting mark, and a thumbnail under the new identity

### Requirement: Failed decodes are retried when transient or after a decoder change
A HEIC or AVIF whose decode failed with its decoder present SHALL record the failure, the decoder backend and an attempt count (ADR-0098). An unchanged rescan SHALL decode it again while the failure is transient (a timeout, AV1 decoding paused, no thread) with fewer than 3 attempts, or when the backend changed, and SHALL NOT read it otherwise.

#### Scenario: Timed-out AVIF is retried on rescan
- **WHEN** an AVIF's decode is abandoned by the watchdog during a scan, and the folder is rescanned unchanged, first while decoding still hangs and then after it completes
- **THEN** the AVIF keeps a placeholder with a transient failure, the first rescan reads it again (attempt 2), and the second fills in its preview and hashes and clears the failure

#### Scenario: Corrupt HEIC is not re-read on an unchanged rescan
- **WHEN** a HEIC the decoder rejects is scanned and the folder is rescanned unchanged twice, then once with a newer decoder backend
- **THEN** the unchanged rescans read none of its bytes, and the rescan with the newer backend reads it once and records the new backend
