# 0052. Hidden folders are scanned by default

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
`scanning.include_hidden` ("Include hidden folders") was shown in the settings window, defaulted to off, and was
read nowhere: every scan walked dot-folders. Honouring an "off" default would silently drop whatever a backup drive
keeps in `.photos`, `.Trash` or `.dropbox.cache` folders, against the first priority (never lose a photo) and the
gallery spec ("nothing is hidden silently"). Dot-folders also hold caches (`.thumbnails`, `.cache`) that the
classifier recognises and that join duplicate groups as cache copies, so the completion report can account for
them; the ground-truth corpus has such folders.

## Decision
- Hidden folders (names starting with `.`) are walked by default: `scanning.include_hidden` defaults to **on**.
  Settings files from version 2, whose `false` was never honoured, are migrated to `true` (settings version 3), so
  nobody loses what they were scanning.
- Turning it off skips those folders in the walk (files directly in a source are never skipped). The run summary
  counts the skipped folders per source (`hidden_dirs_skipped`) and the log names the count. Files already
  catalogued under a now-skipped folder are marked missing like any file no longer found (kept, never deleted);
  turning the setting back on brings them back unchanged.
- The source fingerprint and size measured when a source is added always include hidden folders, so the setting
  never changes a source's identity.

## Alternatives considered
- **Keep the off default** - silently skips photos on first scan; the user would have to know to turn it on.
- **Skip only non-cache hidden folders** - two rules for one checkbox; the cache folders are what make "accounted
  for" complete.
- **Windows hidden attribute** - not portable to sources copied between systems; the dot rule is what users see on
  every platform's backups.

## Consequences
- Users who want dot-folders ignored turn the setting off; the dashboard summary says how many were skipped.
- `.photosquirrel-quarantine` and export libraries stay excluded whatever the setting.

## Revisit when
Users report noise from hidden folders that the classifier does not already set aside.
