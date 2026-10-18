# Tasks

## 1. Theme

- [x] 1.1 Token modules, graphite dark/light theme JSON and font registration in `ui-theme`; verify `scenario_theming_dark_theme_by_default` and `scenario_theming_metadata_uses_mono_font`
- [x] 1.2 Acorn accent, neutral surround and contrast checks; verify `scenario_theming_accent_is_acorn`, `scenario_theming_photo_surround_is_neutral`, `scenario_theming_text_contrast_meets_aa`
- [x] 1.3 Live theme switching and reduced motion; verify `scenario_theming_switch_to_light_live` and `scenario_theming_reduced_motion_disables_animation`
- [x] 1.4 Token check covers the new UI crates; verify `scenario_theming_raw_colour_literal_rejected` (xtask unit test on a sample file) and `cargo xtask tokens` passes

## 2. Settings

- [x] 2.1 Settings model, TOML persistence and defaults; verify `scenario_settings_settings_persist_across_restart` and `scenario_settings_defaults_are_local`
- [x] 2.2 `ps-service::secrets` (keychain, env override, session fallback); verify `scenario_settings_no_secrets_in_settings_file`
- [x] 2.3 Validation and corrupt-file recovery; verify `scenario_settings_invalid_value_rejected` and `scenario_settings_corrupt_settings_fall_back`
- [x] 2.4 Classifier settings hash and reclassification on rescan; verify `scenario_settings_threshold_change_reclassifies`
- [x] 2.5 Settings window with sections and search; verify `scenario_settings_search_filters_settings`

## 3. Shell

- [x] 3.1 Shell layout, navigator and status bar; verify `scenario_app_shell_window_shows_main_areas`
- [x] 3.2 Onboarding and interrupted-run banner; verify `scenario_app_shell_first_launch_shows_onboarding` and `scenario_app_shell_interrupted_run_offers_resume`
- [x] 3.3 Window and layout persistence; verify `scenario_app_shell_window_layout_restored`
- [x] 3.4 Keyboard model and command palette; verify `scenario_app_shell_shortcuts_switch_views` and `scenario_app_shell_palette_finds_commands`
- [x] 3.5 Background service handle and loading states; verify `scenario_app_shell_slow_query_keeps_ui_responsive`
- [x] 3.6 Network guard test hook; verify `scenario_app_shell_no_network_at_startup`

## 4. Budgets

- [x] 4.1 `--measure-startup` and `perf-budgets.toml` entry `cold-start` (400 ms, app source); verify `scenario_app_shell_cold_start_within_400_ms`
- [x] 4.2 `--measure-idle-memory` and entry `idle-rss` (250 MB, app source, 1M synthetic catalog); verify `scenario_app_shell_idle_memory_within_250_mb`

## 5. Verification

- [ ] 5.1 `cargo xtask ci` is green
- [x] 5.2 Gate: window opens (`cargo xtask screenshot --all-surfaces` reviewed in dark and light), `cargo xtask tokens` passes, settings persist across restart
- [x] 5.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [x] 5.4 `openspec validate app-shell --strict` passes
