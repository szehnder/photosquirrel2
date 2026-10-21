# Tasks

## 1. One options builder

- [x] 1.1 `ps_service::settings::scan_options(&Settings)`; `Library::scan` (app) and `Ctx::scan_options` (CLI) use it; verify `scenario_settings_app_rescan_uses_changed_thresholds`
- [x] 1.2 Re-classify unchanged files from stored probe facts on rescan (catalog migration 13, ADR-0051); verify the rescan opens no file for re-classification
- [x] 1.3 Apply a Scanning settings change to the app's engine before the next scan start (never mid-run); verify a headless test that changes a threshold and rescans

## 2. Settings that are read nowhere today

- [x] 2.1 `scanning.include_hidden` in the walk (ADR-0052); verify `scenario_settings_hidden_files_setting_honoured`
- [x] 2.2 `scanning.preview_cache_gb` caps the preview cache; verify a cache-eviction test
- [x] 2.3 Extra cache patterns from settings in the classifier; verify a classifier test through `scan_options`
- [x] 2.4 `general.pause_on_battery` with a faked power source (ADR-0053); verify `scenario_settings_scan_pauses_on_battery`

## 3. Tests that test the real path

- [x] 3.1 Rewrite `scenario_settings_threshold_change_reclassifies` to go settings file -> service rescan -> catalog (it tests a helper with hand-built input today)
- [ ] 3.2 `cargo xtask ci` green; archive
