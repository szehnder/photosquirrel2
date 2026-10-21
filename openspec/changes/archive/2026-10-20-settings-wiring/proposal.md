# Proposal

## Why

The M3 gate review (finding P5) found that the app ignores most Scanning settings. The GUI opens its scanner with
`ScanOptions::default()` (`ps-service/src/library.rs`, `Library::scan`), so classifier thresholds, extra cache
patterns, `include_hidden`, `preview_cache_gb` and the poster-frame tool take effect only in `photosquirrel-cli`
(which passes the classifier settings and the poster tool). `general.pause_on_battery` is shown in the settings
window but read nowhere. `scenario_settings_threshold_change_reclassifies` tests the reclassification helper with
hand-built records, not a rescan through the app's or the CLI's scanner, so it did not catch this.

## What Changes

- One function in `ps-service` builds `ScanOptions` from `Settings` (classifier, cache patterns, hidden files,
  poster tool with the ffmpeg-on-PATH fallback); the app and the CLI both use it.
- A settings change that affects scanning reaches the app's open engine (reopen or update its options between
  runs; never mid-run).
- `scanning.include_hidden` and `scanning.preview_cache_gb` are honoured by the walk and the preview cache.
- `general.pause_on_battery` pauses a running scan when the machine switches to battery and offers to resume on
  mains (platform power-source probe behind a small trait; tests fake it).
- The threshold scenario is re-tested end to end: settings file -> rescan through the service -> reclassified
  files, with no content reads.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `settings`: every Scanning and General setting takes effect in the app and the CLI alike.

## Impact

- Crates: `ps-service` (`library.rs`, a `scan_options(&Settings)` builder), `ps-scan` (hidden files, preview cache
  cap), `photosquirrel-cli` (use the shared builder), `ui-workspace` (apply changes between runs).
- Possibly a new ADR if the power-source probe needs a platform dependency.

## Out of scope

- Live reload of `settings.toml` edited outside the app (ADR-0049 covers consent only).
