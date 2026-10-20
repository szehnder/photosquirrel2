# Proposal

## Why

An export library holds the copies PhotoSquirrel made. The archived `export` spec refuses a *registered* library
(and folders inside or around it) as a source, but a library written by another catalog (another computer, a
reinstalled app, a second `--data-dir`) is not registered here. Adding it, a folder inside it or a drive that
contains it as a source would scan the copies as if they were originals: they would join duplicate groups, could
become keepers and could be exported into the new library again.

## What Changes

- A folder is an export library when it holds `.photosquirrel/manifest.jsonl` (ADR-0022), whichever catalog
  wrote it.
- `sources add` (CLI and app) and re-linking refuse a root that is such a library, is inside one, or contains one
  within the first levels below it, with a message naming the library.
- The scan walk never enters a folder that is an export library, so one nested deeper than the add-time check is
  not scanned either.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `export`: libraries from other catalogs are never sources.

## Impact

- Crates: `ps-scan` (`walk::find_export_library`, the walk skip, `Scanner::add_source` / `relink_source`),
  `ps-service` tests. No file format change.

## Out of scope

- Importing another catalog's manifest to adopt its copies.
