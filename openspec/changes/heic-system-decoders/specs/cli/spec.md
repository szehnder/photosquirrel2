# Spec Delta

## MODIFIED Requirements

### Requirement: Doctor report
The doctor command SHALL report the application version, data directory, catalog schema version, per-format decoder availability (HEIC, AVIF, poster frames) with the decoder used or the reason and fix for the detected system, any decoder override in force, keychain availability and configured LLM provider, without sending any network request.

#### Scenario: Doctor lists decoders
- **WHEN** doctor is run with `--json` with the HEIC decoder forced unavailable
- **THEN** the output reports HEIC unavailable with a reason, fix steps for the detected system and the override in force, AVIF available, the data directory and the catalog schema version, and no network connection is opened

## ADDED Requirements

### Requirement: Scan summary names undecodable photos
When a scan ends with photos that have no preview because their decoder is missing, the scan command SHALL print, after the counts, one line per format with the count and reason, then how to enable previews; with `--json` the summary SHALL carry the same per-format counts, reasons and fixes. When the decoder is available but files still wait, it SHALL say to rescan.

#### Scenario: Summary explains HEIC without a decoder
- **WHEN** a folder with 3 HEIC photos is scanned with the HEIC decoder forced unavailable
- **THEN** the text summary says 3 HEIC photos have no preview, gives the reason and the fix steps for the detected system, and the `--json` summary has the same count, reason and steps

#### Scenario: No notice when every photo is decoded
- **WHEN** the small corpus, which holds no HEIC, is scanned
- **THEN** neither the text nor the `--json` summary mentions undecodable photos
