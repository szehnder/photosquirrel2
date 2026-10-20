# export (delta)

## ADDED Requirements

### Requirement: Libraries from other catalogs are never sources
A folder holding `.photosquirrel/manifest.jsonl` is an export library whichever catalog wrote it. Adding or
re-linking a source MUST be refused when the root is such a library, is inside one, or contains one within the
first four levels below it, with a message naming the library; the scan MUST NOT walk into such a library at any
depth.

#### Scenario: Library from another catalog refused as a source
- **WHEN** the user adds, or re-links a source to, a folder that is an unregistered export library (it holds `.photosquirrel/manifest.jsonl`), a folder inside it or a folder containing it
- **THEN** the source is refused with a message naming the library, and a library nested deeper below an added source is not scanned
