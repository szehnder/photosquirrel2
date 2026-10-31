# Proposal

## Why

The owner: "when i select a folder to scan that has a lot of images, it is as if the application ui freezes while
it starts reading that folder immediately or something."

The UI thread never blocked: a 4 ms heartbeat showed gaps of at most 17 ms while a 39,000-file folder was added and
scanned. The freeze is that nothing changes on screen.

**Cause A: adding a folder costs O(whole tree), with no feedback.**
- `Scanner::add_source` calls `source::measure`, which walks the whole tree (a listing and an `lstat` of every file)
  and fully hashes up to 64 sampled files of up to 64 MB each. The scan then walks the tree again. The export-library
  check also lists every folder within four levels with an `lstat` of every entry.
- Measured: 0.62 s for 39,000 files on a warm cache. On a cold HDD, a NAS or a USB drive, about 1.3 GB of samples
  are read: tens of seconds to minutes.
- `ScanService::unless_busy` holds the operations lock for that whole time. On the shell path the walk also runs on
  the single service thread, so other service calls queue behind it.
- Sources view: there is no pending state, the Add button stays enabled (a second click opens another picker), and
  the list doesn't change until the add returns. The shell path (Library view, Ctrl+O, onboarding, palette) shows
  no feedback at all.

**Cause B (a bug): the shell add path never shows the scan it starts.** `LibraryBackend::add_source` adds the source
and starts the scan, then `Shell::add_source` only calls `refresh()`. It never calls `start_activity_sampling` or
`ScanHub::wake`. The status bar says "Ready", and the Library gets no live updates while the scan runs. This breaks
scan-progress "Status bar progress". The existing test starts the run from the dashboard, so it misses this path.

**Smaller issues:**
- On the shell path, a source added while a scan runs is added, but its start fails and the toast wrongly says
  "Source not added".
- On the Sources path, a source added while a run is active is silently never scanned.
- The status bar says "Starting…", then shows a second phase with no label (the walk).

## What Changes

The owner chose **"Scan at once + setting"**:

- **Adding is cheap (new ADR-0121, amends 0006).** Registration does a constant amount of work: a stat, a
  canonicalise, the overlap and export-library checks (folder kinds only, no per-file `lstat`), the volume identity
  and the row insert. The first completed scan records the fingerprint and the total size from the catalog. It uses
  the identities and sizes the scan computed anyway, so the extra walk and the sample reads go away.
- **Re-linking without a fingerprint.** A source whose first scan has not finished re-links by a fingerprint derived
  from its identified files. A source with no identified file has nothing to protect, so it re-links without a
  check. Overlap and online checks are unchanged.
- **Immediate feedback on every add path.** The folder appears in the source list in the same frame the picker
  returns, as a pending "Adding…" row. The status bar says "Adding <folder>…". The Add control is unavailable while
  the picker is open or an add is pending. The row then becomes the real row, or shows its error inline.
- **The scan shows from every path (fixes B).** The new source's scan starts at once. The status bar and the
  Library's live updates follow it, whether it started from the Sources view, the Library, Ctrl+O, onboarding or
  the palette. The status bar names the walk ("Walking folders").
- **A source added during a run is queued (the owner's choice).** It is never reported as "Source not added".
  - Its row says "Queued, scans next", and the status bar adds "then 1 more source".
  - When the run completes, a run of exactly the queued sources starts on its own, and the finished source is not
    scanned again.
  - A cancelled or paused run keeps the queue visible and starts nothing until the user acts.
  - After a restart, a queued source shows "Not scanned yet".
- **Never-scanned sources get their own primary action.** Whenever a source has never finished a first scan, the
  Sources view's primary action scans only those ("Scan “Photos 2019”", "Scan 3 new sources"). Rescanning stays a
  secondary action, labelled with what it covers ("Scan 3 sources (1 new)").
- **A setting, "Scan new sources right away"** (`scanning.scan_new_sources`, on by default). When it is off, a new
  source shows "Not scanned yet", and the Sources view selects it for "Scan 1 source". After an add from
  onboarding, the shell opens Sources. The CLI's `doctor` reports the setting. `sources add` has never scanned, and
  it now says the source is not scanned yet.

## Capabilities

### Modified Capabilities
- `sources`: registration records the root and volume identity at once; the first completed scan records the total
  size and fingerprint; re-linking a source without a fingerprint.
- `scan-progress`: the pending row and status bar line on every add path; the status bar follows scans started by
  adding a source or by the queue; a source added during a run is queued; the never-scanned primary action.
- `settings`: "Scan new sources right away".
- `app-shell`: onboarding with the setting off leads to Sources.

## Impact

- `ps-scan`:
  - `fs.rs`: instrumentation counts stats and per-entry `lstat`s, and `SourceFs::subfolders` lists folder kinds
    without stats
  - `walk::find_export_library` uses `subfolders`
  - `scanner.rs` `add_source` no longer calls `source::measure`
  - `relink_source` and `fingerprint_of`, plus `source.rs` `from_catalog`
  - `pipeline.rs` `finish` records the measure
- `ps-catalog`: `Readers::source_measure_rows` (the present rows' sizes and identities) and
  `Command::SetSourceMeasure`. No migration.
- `ps-model` `Scanning::scan_new_sources`. `ps-service` settings tests. `photosquirrel-cli`: `doctor` and the
  `sources add` text.
- `ui-workspace`:
  - `views/scan.rs` `ScanMonitor`: pending adds, queued sources, the selection hint, the status line ("Adding…",
    "Walking folders")
  - `views/sources.rs`: the pending rows and the Add busy state
  - `shell.rs`: `add_source` goes through the monitor and starts activity sampling
  - `settings_window.rs`: the new toggle
  - `scan_backend.rs`: `DemoBackend` adds real rows, with an add delay hook for tests
- New **ADR-0121** (amends 0006). README "Adding sources" and the release notes.

## Out of scope

- Persisting the queue of sources waiting for a run across restarts (after a restart they show "Not scanned yet").
- Adding a source to a run that is already running (the lanes are set at the start, ADR-0009).
- A new performance budget.
