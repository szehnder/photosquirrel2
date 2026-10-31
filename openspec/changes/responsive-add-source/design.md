# Design

## Context

- `ps-scan/src/scanner.rs` `add_source(path)` does the following in order:
  - stat
  - `normalize_root` (canonicalise)
  - `refuse_export_library`: the ancestors, plus `walk::find_export_library` down to 4 levels and 10,000 folders,
    listing through `read_dir_cached`, which `lstat`s every entry
  - overlap against existing roots and the fold checks
  - `source::measure`: a full walk, plus `choose_samples` and `hash_file` of up to 64 files of 1 B to 64 MB
  - the volume identity, then the `NewSource` row with `total_bytes` and `fingerprint`
- `relink_source` refuses a source without samples (`NoFingerprint`). `source::check` hashes the stored sample paths
  under the candidate root (it walks only when a sampled path is not in the catalog).
- `ps-service/src/scan.rs` `add_source` runs inside `unless_busy` (the operations lock).
- `ui-workspace`:
  - `views/sources.rs` `add_sources`: picker, then a background task (`add_source` per path, then
    `refresh_sources`), then `start` only when no run is active.
  - `shell.rs` `add_source`: picker, then `LibraryBackend::add_source` (add and start) per path on the service
    thread, then `refresh()`, with a "Source not added" toast for any error.
  - `views/scan.rs` `ScanMonitor`: the 10 Hz sampling loop and `status_line`. The status bar
    (`chrome::status_bar`) prefers the monitor's line, and wakes the monitor only while `shell.activity` is set.
  - The Library's live updates follow `shell.activity` (`start_activity_sampling`).
- The scan itself stores every walked file as a row with its size. Every photo and video it analyses gets a content
  identity computed by the same function as the fingerprint's (`ps_hash::identity_from_reader`, ADR-0011).

## Decisions

### D1. Constant-cost registration (ADR-0121)

`add_source` keeps every check and drops `measure`. The steps are: stat, canonicalise, export-library refusal,
overlap and fold checks, `volume_info` (mount table and by-uuid, independent of the tree), one `probe_ids`
(`statfs`), and insert the row with `total_bytes: None, fingerprint: None`.

`find_export_library`'s downward search lists through a new `SourceFs::subfolders(dir)`. That uses
`DirEntry::file_type` (`d_type`, no `lstat`; std falls back to `lstat` only where the filesystem reports no type) and
returns subfolder names only. Its bounds (4 levels, 10,000 folders) stay. The cost is now independent of the number
of files. In folders, it is capped by the bounds. Folders deeper than 4 levels cost nothing.

The instrumentation gains a `stats` counter (`metadata`, `symlink_metadata`) and an `entry_stats` counter (the
per-entry `lstat` of `read_dir_cached`). `subfolders` counts as a `read_dir`. The guard test adds a 6-file tree and
a 3,000-file tree with the same folders (plus folders below level 4 in the large one), through the counting layer.
For both: 0 opens, 0 bytes, 0 entry stats, the same number of stats and the same number of listings.

### D2. The first completed scan records the measure

`pipeline::finish`, for a scan run (not a fill job) whose status is `Completed`, records the measure for each run
source that was not offline at the start, was not lost during the run, and has `fingerprint IS NULL` in the catalog
(read at the run's start into `RunSource::measure`):
- `Readers::source_measure_rows(source)` is one query over present rows (`missing_since IS NULL`). It returns
  `(rel_path, size, identity)` for rows of 1 B to 64 MB with a `content_id`, and the `SUM(size)` of every present row.
- The samples are the [`FINGERPRINT_SAMPLES`] smallest `path_hash(rel_path)` among those rows (`source::from_rows`).
  This is the old `choose_samples` order, restricted to identified files.
- `Command::SetSourceMeasure { source, total_bytes, fingerprint }` writes `UPDATE source SET total_bytes, fingerprint`.
  It runs before the final flush, so the run's end implies the measure is stored.

No file is opened. A cancelled or interrupted run records nothing. The resumed run, or any later completed scan of
the source, records it. An empty sample set is stored as an empty blob: "measured, nothing to sample".

Rescans of measured sources do no pass (`rescan-nochange-100k` unchanged). The pass on a 1,000,000-row first scan
costs about a second, once.

### D3. Re-link, overlap and identity without a fingerprint

`Scanner::fingerprint_of(record)`: the stored samples when the column is not NULL, else `from_rows` over
`source_measure_rows` now (the identified files of an interrupted first scan). Then:
- samples not empty: the check as before (accept, or `FingerprintMismatch`);
- samples empty and no present row with a content identity: re-linked without a check. The source has no results
  to protect, and its rows are paths and stats only. `refuse_export_library` and the RunActive check still apply.
- samples empty, with identified rows (all over 64 MB): `NoFingerprint`, as before.

Overlap and folding compare roots only and are unchanged. An unscanned inner source is folded like any other. The
outer source then gets its measure from its first completed scan, over the folded rows too. The online check uses
the volume UUID recorded at registration (unchanged).

### D4. Pending adds in the monitor (every add path)

`ScanMonitor` owns the add flow, so the Sources view, the shell (Library, Ctrl+O, onboarding, palette) and the
status bar share one state:
- `picking: bool`, set while a picker is open (Add unavailable, a second Add is ignored).
- `adding: Vec<PendingAdd { key, path, name, error: Option<String> }>`.
- `add_paths(paths, scan_now, cx) -> Task<AddOutcome>`. In the same update, before any await, it pushes a
  `PendingAdd` per path and notifies, so the next frame draws the "Adding…" rows and the status bar line. One
  background task then does three things:
  - `backend.add_source` per path;
  - `backend.sources()`;
  - the start decision (D5, D6).

  The rows list and the pending entries change in the same update, so a row never flickers away between "Adding…"
  and its real row. A failed path keeps its pending row with the error and a Dismiss control. The shell path also
  shows a "Source not added" toast, only for a failed add.
- `status_line()` returns "Adding <name>…" (or "Adding N folders…") while any add without an error is pending,
  before any run line.

The Sources header's Add button is unavailable while `picking` or an add is pending. The shell's `AddSource` does
nothing in that case.

### D5. The scan starts at once and shows everywhere (fixes B)

With `scanning.scan_new_sources` on and no active run (checked in the background task from a fresh snapshot), the
task starts a run of the added sources, then takes a snapshot. The update applies it, clears the overview and calls
`ensure_sampling`. `AddOutcome` says `Started`. Both callers then call `Shell::watch_activity`, so the Library's
live updates run whichever path added the folder. The shell's own add path does it directly. The Sources view
reaches the shell with `Shell::for_window`.

`status_line` names the walk: Running in `ScanPhase::Walking` or `Starting` reads "Walking folders · N files found"
(N is the run's discovered count), never an unlabelled phase. The dashboard already says "Walking folders".

### D6. A source added during a run is queued (the owner's choice)

The owner's report: they scanned a large network folder and added a second folder from the same share while it
ran. The first scan finished, but the second never started, and the Sources view then offered "Scan 2 sources",
which would have walked the finished share again first. Their words: "if I have a running scan and I add a folder,
it should either ask me or just start that second folder scan after the first, or give me option to scan the one
that is new and not scanned at all before checking the one that just finished again".

The owner decided, and this design records it:
- **With the setting on, a source added while a run is active is queued, and it starts on its own after the run,
  with no question.**
  - The queue is per source and deduplicated (`ScanMonitor::queued`, ordered, each id once).
  - The row says "Queued, scans next". The status bar line ends in " · then 1 more source" (or "N more sources").
    The shell path's toast says "Added “X”. It scans next, after the current scan."
- **When the active run ends completed**, `run_ended` starts one run of exactly the queued sources that still exist
  and are online, through the normal Starting control, and clears them from the queue. The finished source is not
  rescanned. Queued sources that are offline stay queued.
- **When the run is cancelled, the queue stays, visible, and nothing starts** until the user acts: the user just
  said "stop". The rows keep "Queued, scans next", which now means after the next run that completes. That run can
  be a Resume of the cancelled run, or the primary "Scan N new sources" (D6b), which starts them at once and takes
  them out of the queue.
- **A pause keeps the queue.** A start that fails because a run is active (a race) puts the sources in the queue.
  "Source not added" is never shown when the add succeeded.
- **The queue lives in memory.** After a restart, or any loss of the queue, a queued source has no completed scan,
  so it shows "Not scanned yet" and is a target of the primary action. Nothing is lost.
- Adding a source to the running run was rejected: the lanes and per-source state are fixed when a run starts
  (ADR-0009, ADR-0019).

`CatalogBackend::add_source` (the shell fallback without a scan hub) no longer fails when only the start is refused
because a run is active. A fold refused because the inner source is in the active run is an add error ("wait for
the scan to finish"), shown on the pending row.

### D6b. Never-scanned sources get their own primary action

`SourceRow::first_scan_done` is true when a completed scan run (not a fill job) covered the source. Whenever an
online source has never finished a first scan and is not in an active run, the Sources header's primary action
targets only those sources, with or without the setting:
- "Scan “Photos 2019”" for one source, "Scan 3 new sources" for several. The accessible label lists the names.
- The scan picker's selection-based action becomes secondary, and its label says what it covers: "Rescan 2
  sources" when every selected source has been scanned, "Scan 3 sources (1 new)" when it mixes them. "Scan 2
  sources" never appears for a selection where only the new one is meant: the primary action is that.
- Without never-scanned sources, the header is as before (selection-based "Scan N sources" or "Rescan N sources"
  as primary).

Starting the primary action removes its sources from the queue.

### D6c. Every run start shows in the status bar and the Library

The shell observes the scan monitor. When the monitor turns active and the shell is not sampling activity, it calls
`start_activity_sampling`. So a run started by the queue, by the dashboard, by Resume or by an add all reach the
status bar and the Library's live updates through the same path as Cause B's fix.

### D7. The setting

`ps_model::settings::Scanning::scan_new_sources: bool`, default `true` (serde default, so older files load as on,
and there is no `SETTINGS_VERSION` bump). The settings window's Scanning section adds a toggle, "Scan new sources
right away": "Start scanning a folder as soon as you add it. When off, start it from Sources." The UI reads it from
`AppSettings` when the add starts.

When it is off:
- no run starts, and `AddOutcome` says `NotScanned`;
- the row shows "Not scanned yet";
- the monitor sets a selection hint (`selection_hint: Option<(generation, Vec<SourceId>)>`), which the Sources view
  applies once, so the start control reads "Scan 1 source" for the new folder.

CLI: `doctor` reports `scan_new_sources` (text and `--json`), beside `pause_on_battery`. `sources add` never started
a scan (scripts depend on that). It now prints "not scanned yet (its size and fingerprint are recorded by its first
scan)" and "next: photosquirrel-cli scan".

### D8. Onboarding with the setting off

If the shell's add outcome is `NotScanned` and the catalog summary was empty (the onboarding view was showing), the
shell shows the Sources view. That view lists the new row, selected, with "Scan 1 source" as its primary action.
From any other shell path, the toast says "Added “X”. It is not scanned yet: start it from Sources." With the
setting on, onboarding behaves as today: the Library fills in live.

### D9. Testing

- ps-scan:
  - the counting guard (D1);
  - `scenario_sources_add_a_source` (rewritten: listed at once without size or fingerprint, then measured by the
    first scan with 0 extra opens);
  - a cancelled first scan records nothing, and its resume records the measure;
  - relink of a never-scanned source, and relink after an interrupted first scan, by its identified files;
  - the existing relink, identity, remount and fold tests unchanged.
- ui-workspace (`#[gpui_kit::test]`):
  - `DemoBackend` gets real added rows (`last_scan_ms: None`) and an `add_delay` hook (a real sleep in
    `add_source`). The test answers the picker with `simulate_path_prompt_response`, renders one frame before
    parking, and finds the pending row and "Adding…";
  - the shell `AddSource` path with the Library open reads "Scanning" or "Walking folders" in `status-activity`;
  - with the setting off: no active run, "Not scanned yet" and "Scan 1 source";
  - add during a run: "Queued, scans next" and "then 1 more source"; after the run completes a run of only the
    queued source starts (the finished source is not in it); a cancelled run leaves the queue visible and starts
    nothing;
  - the primary action scans only never-scanned sources, with its exact label;
  - onboarding with the setting off opens Sources.
- photosquirrel-cli: `doctor --json` has the setting, and the `sources add` text.

## Risks / Trade-offs

- [Samples come only from identified files] -> a source of only junk files or only very large videos cannot be
  re-linked with a check. A source without identified files has no results to lose, so it re-links freely. The
  large-video case behaves as before.
- [A queued source is forgotten across a restart] -> it shows "Not scanned yet" with the Scan control. Nothing is
  lost.
- [`d_type` missing on some filesystems] -> std falls back to `lstat`, which is the old cost, bounded by the search
  limits.

## Deviations (as built)

(filled in during implementation)
