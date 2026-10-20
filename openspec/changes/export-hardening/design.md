# Design

## Detection
`ps_scan::walk::is_export_library(dir)` stats `dir/.photosquirrel/manifest.jsonl` (no symlink following). The
names mirror `ps_export::LIBRARY_META_DIR` and `ps_export::manifest::MANIFEST_FILE`; ps-scan does not depend on
ps-export, so a ps-service test asserts they are equal.

## When a source is added or re-linked
`find_export_library(root)` checks the root and each ancestor, then the folders below the root breadth-first up to
4 levels and 10,000 folders (symlinks not followed). This keeps adding a large NAS share fast. The existing
registered-library check (catalog registry) still runs afterwards.

## During the walk
A folder (other than the source root) whose listing contains `.photosquirrel` and that holds the manifest is
skipped with a warning and listed in `WalkOutput::libraries_skipped`, so a library nested deeper than the add-time
search is never scanned. Files previously catalogued there show as missing on the next scan, like any file that
left the walk; nothing on disk is touched.
