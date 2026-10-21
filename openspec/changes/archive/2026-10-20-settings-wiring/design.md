# Design

## One options builder
`ps_service::settings::scan_options(&Settings) -> ScanOptions` (and `apply_scan_settings`, which writes the same
fields into existing options) is the only place that maps settings to scanner options: classifier thresholds and
extra cache patterns, `include_hidden`, and the poster-frame tool (the configured one, else `ffmpeg` on `PATH`).
The CLI's `Ctx::scan_options` and the app's `Library` both call it. `preview_cache_bytes(&Settings)` gives the
preview cache cap (binary GB, as ADR-0016's 4 GB).

## Re-classification that actually happens (ADR-0051)
The gate finding hid a deeper gap: neither scanner re-classified unchanged files at all, and the catalog did not
keep what the classifier reads. The probe stage now stores `ClassFacts` (compact JSON) and the classifier settings
hash with every class (catalog migration 13). The rescan diff re-classifies unchanged, finished files whose hash
differs from the run's settings from those facts alone (`ps_media::classify_facts`, the classifier's only
implementation) and writes `Command::Reclassify`. A skipped file that becomes a candidate goes back to `probed`
and is analysed like a new photo; nothing else is read. `DiffCounts::reclassified` reports it.

## Between runs, never mid-run
`Scanner` keeps its options behind a mutex and clones them when a run starts (`set_options`/`update_options`).
`Library::apply_settings(&Settings)` stores the settings (with a revision, so a change that races the lazy engine
open is applied on the next `Library::scan()`), updates the open engine's options and, when the preview cap
changed, drops the open preview cache so the next preview reopens it at the new cap (reopening evicts down to
it). The app installs a listener on its settings global: start-up and every Scanning or General change call
`LibraryBackend::apply_settings` on a background thread (it may wait for a preview being made).

## Hidden folders (ADR-0052)
`WalkOptions { include_hidden }` in the walk; dot-folders are skipped when off and counted per source in the run
summary (`hidden_dirs_skipped`). The setting now defaults to on (settings version 3 migrates version-2 files,
whose `false` was never honoured, to `true`); source fingerprints always include hidden folders.

## Pause on battery (ADR-0053)
`ps_service::power::PowerSource` (`on_battery()`), with a platform probe (sysfs on Linux, IOKit's
`IOPSGetTimeRemainingEstimate` on macOS, `GetSystemPowerStatus` on Windows; unknown means mains) and a fake for
tests. The `Library` runs a watcher thread once the engine is open and the setting is on: every few seconds, a
running scan is paused when on battery (unless the user resumed that run on battery), and the reason is kept
(`Library::power_pause`): "paused on battery power", then "mains power is back" once it returns. The scan
dashboard shows the note next to its Resume button. The CLI does not pause (it is an explicit, attended command).

## Tests
- `scenario_settings_threshold_change_reclassifies`: settings file -> `ScanService` opened with `scan_options` ->
  rescan -> catalog classes; instrumented file layer proves no re-classification read.
- `scenario_settings_app_rescan_uses_changed_thresholds`: the same through `Library::apply_settings` on an open
  engine, compared with the CLI path.
- Headless UI test: a settings-window change reaches the backend and the next rescan re-classifies.
