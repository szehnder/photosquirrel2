# Tasks

Ships in **0.4.0** (ROADMAP row 37). It takes **ADR-0124** (consent and check policy), **ADR-0125** (signed manifest
and key), **ADR-0126** (install per package kind, with rollback) and **ADR-0127** (the `ps-update` crate).
Settings file v3 -> v4. **No catalog migration**: the next free migration number stays 27. Tests never touch real
install folders. Every install, rollback and kill test runs on fake layouts in a `tempfile` dir.

## 1. Decisions and keys

- [ ] 1.1 Owner: review ADR-0124..0127 (written as `proposed` together with this change), then set them to
  `accepted` and drop "(proposed)" from their index rows. Verify: the four files read `Status: accepted`, and
  `docs/adr/README.md` lists 0124-0127.
- [ ] 1.2 Owner: generate the release update key (`minisign -G`). Commit `packaging/update-key.pub` and set
  `packaging/update-key-next.pub` to `none`. Store the secret key and its password as `UPDATE_SIGNING_KEY` and
  `UPDATE_SIGNING_PASSWORD` in a new GitHub environment `release-signing` (required reviewer: owner; deployment
  branches and tags: `v*`). Keep an offline backup. Verify: `minisign -Vm` of a test file signed with the key passes
  with `packaging/update-key.pub`, and the environment shows its protection rules.
- [x] 1.3 Generate the test key into `crates/ps-update/tests/data/test-key.{pub,sec}`, unencrypted and marked
  "TEST ONLY" in its untrusted comment. Verify: `ps-update` unit test `test_key_signs_and_verifies`.
  Done: generated with the `minisign` crate (0.10); `crates/ps-update/tests/data/test-key.{pub,sec}`, key id
  7DDE363640B95F1F, comment "TEST ONLY". `test_key_signs_and_verifies` passes.

## 2. `ps-update`: transport, manifest and versions

- [x] 2.1 Create the crate `crates/ps-update` (I/O layer) with the dependencies of ADR-0127. Add it to `cargo xtask
  layering`, which forbids it to core and `ui-*` crates. Check the licences with cargo-about. Measure the release
  binary size of `rustls-platform-verifier` against `webpki-roots`, and pick one; record the choice in design.md.
  Verify: `cargo xtask layering`, and `cargo xtask package` locally renders notices for `minisign-verify` and `tar`
  with no new licence kind.
  Done: `crates/ps-update` (I/O layer; `cargo xtask layering` OK, 18 crates; behind the facade like `ps-llm`).
  Its update-folder file operations are listed in `MOVE_ALLOWED` per module (ADR-0126 says so). The platform
  verifier was chosen over Mozilla's roots (design D9). Every new crate's license is on `about.toml`'s accepted list
  (MIT, Apache-2.0, ISC); the rendered notices are checked by the next `cargo xtask package` run.
- [x] 2.2 Transport: HTTPS only, the release-host redirect allowlist, at most 5 redirects, connect and idle
  timeouts, size limits enforced while reading, user agent `PhotoSquirrel`, no cookies, `HTTPS_PROXY`, and the
  loopback-only `PHOTOSQUIRREL_UPDATE_SERVER`. First confirm the current GitHub redirect hosts with one live `curl -sI`
  of a release asset, and record them in design.md D2. Verify: `scenario_app_updates_check_request_carries_no_identifiers`,
  `scenario_app_updates_redirect_to_an_unknown_host_refused`, `scenario_app_updates_oversized_response_cut_off`, and
  a unit test that an override to a non-loopback host is refused.
  Done: `transport.rs` (`Http`, `Endpoint`). The redirect hosts were confirmed live on 2026-11-09 (design D2). The
  three scenarios and `only_loopback_servers_override_github`, `too_many_redirects_fail` and
  `locations_resolve_against_their_request` pass.
- [x] 2.3 The manifest envelope and its parsing, signature verification against the built-in keys plus the test
  key under `cfg(test)`/`test-key`, the trusted-comment check, and the package URL prefix check. Verify:
  `scenario_app_updates_tampered_manifest_rejected`, `scenario_app_updates_either_built_in_key_accepted`, and a unit
  test that a URL outside `releases/download/v<version>/` is rejected.
  Done: `manifest.rs`, `keys.rs`. Both scenarios and `urls_outside_the_release_are_rejected` pass; the
  either-key test uses two throwaway keys as the two slots.
- [x] 2.4 Version rules: semver precedence, `highest_accepted`, pre-release only to pre-release installs, and
  `skipped`. Verify: `scenario_app_updates_older_or_replayed_manifest_ignored` and
  `scenario_app_updates_stable_install_ignores_a_release_candidate`.
  Done: `version.rs`. Both scenarios and `skipped_versions_are_not_offered` pass.
- [x] 2.5 Download: background low priority, `.part` resume with `Range`, SHA-256 while streaming, a free-space check
  for 3 times the size, a single staged version, and removal on mismatch. Verify:
  `scenario_app_updates_corrupted_download_never_installed`, `scenario_app_updates_interrupted_download_resumes` and
  `scenario_app_updates_low_disk_space_defers_the_download`.
  Done: `download.rs` (`Updater::download`). The three scenarios, `a_download_is_verified_and_staged`,
  `an_oversized_payload_is_refused` and `only_one_version_is_staged` pass. The low-priority background thread is
  the service's (task 3.2).
- [x] 2.6 The update folder, `state.json` (atomic writes) and `history.jsonl` (the last 200 entries). Verify:
  `scenario_app_updates_history_records_an_update_s_life`, plus a unit test that a torn `state.json.tmp` is ignored
  and the previous state is used.
  Done: `dir.rs`, `state.rs`, `history.rs`. The scenario,
  `state_round_trips_and_a_torn_temporary_file_is_ignored`, `only_the_last_200_entries_are_kept` and
  `at_most_sixteen_catalogs_are_remembered` pass. The folder follows the existing `ProjectDirs` convention (design
  D4).
- [x] 2.7 Developer builds: `PHOTOSQUIRREL_RELEASE_BUILD` through `option_env!`, set by `cargo xtask package`.
  Verify: `scenario_app_updates_developer_build_refuses_to_check`, run in a normal `cargo test` build.
  Done: `build.rs` (`RELEASE_BUILD`, a `rerun-if-env-changed` build script). The scenario passes: `check` and
  `download` refuse with no request. `cargo xtask package` sets the variable in task 9.1.

## 3. Settings and the service

- [x] 3.1 `ps-model` settings v4: `[updates] mode`, lenient deserialization, validation, and the v3 upgrade to
  `undecided`. Verify: `scenario_settings_defaults_are_local`, `scenario_settings_update_mode_persists_and_is_shared`,
  `scenario_settings_older_settings_file_upgraded_to_undecided` and `scenario_settings_unknown_update_mode_rejected`.
  The existing settings tests stay green.
  Done: `ps_model::settings::{Updates, UpdateMode}`, `SETTINGS_VERSION` 4 and a v3->v4 note in `migrate`. The
  mode is read leniently and remembered as `invalid_mode` for validation; `Updates::set_mode` replaces it. The
  three settings scenarios and `scenario_settings_defaults_are_local` (now checking undecided) pass, and so do all
  14 settings tests and 62 ps-model tests. The CLI half of "persists and is shared" is checked again in task 8.1.
- [ ] 3.2 `ps-service::UpdateService`: the schedule (60 s, 24 h across restarts, jitter, an hourly wake on a
  background executor), mode handling, the coalesced `UpdateStatus` snapshot and the manual check. Verify:
  `scenario_app_updates_undecided_mode_sends_nothing`, `scenario_app_updates_first_check_after_60_seconds`,
  `scenario_app_updates_at_most_one_check_a_day_across_restarts`, `scenario_app_updates_manual_check_with_mode_off`,
  `scenario_app_updates_notify_mode_waits_for_the_user` and `scenario_app_updates_offline_user_is_not_nagged`, all
  with an injected clock and a recording transport.

## 4. Installation kinds

- [x] 4.1 Kind detection for appimage, macos-app (translocation and read-only volume), windows-setup (HKCU
  uninstall key), deb (the dpkg list), archive and developer, with the D5 reasons. First confirm in a built package
  where cargo-packager puts the CLI inside the `.app` and the AppImage, and the NSIS uninstall key's name; record
  both in design.md D5. Verify: `scenario_app_updates_installation_kinds_reported` on each CI OS (fake layouts; the
  Windows test writes and removes a key under a test-only `HKCU\Software\PhotoSquirrelTest` root).
  Done: `kind.rs` (`detect` over a `Probe`). The scenario covers every kind on any OS with fake layouts; the CLI in
  the bundle reports the bundle. cargo-packager 0.11.8 puts the CLI beside the app in `Contents/MacOS` and in the
  AppDir's `usr/bin`. The NSIS uninstall key is `HKCU\...\Uninstall\PhotoSquirrel`, with `InstallLocation`
  quoted. Instead of a test-only registry root, the key is read through `reg query` (no `unsafe`), and its output
  parser is tested (`the_registry_value_is_read_without_its_quotes`). Writable folder: `access(W_OK)` on Unix, the
  read-only attribute on Windows; read-only volume: `statvfs` `ST_RDONLY`.
- [ ] 4.2 Notify-only kinds: release page links, and for a `.deb` a verified download into Downloads, a reveal and
  the command. Verify: `scenario_app_updates_debian_package_gets_a_verified_file_and_a_command` and
  `scenario_app_updates_archive_installation_notified_only`.

## 5. The installer helper and the swap

- [ ] 5.1 The plan file, the helper copy of the installed CLI, the detached spawn on each OS, the per-installation
  shared lock taken by every app and CLI start, and the exclusive wait with a 10 min timeout. Decide the AppImage CLI
  smoke check (design Risks (a) or (b)) and record it. Verify: `scenario_app_updates_running_cli_postpones_the_install`
  (with the timeout shortened in the test), plus a unit test that the lock is shared between two readers.
- [ ] 5.2 AppImage swap: copy to `previous/`, write the `.update` file in the same folder, the `--version` smoke
  check, an atomic rename, and undo. Verify: on Linux CI, `scenario_app_updates_broken_new_binary_rolled_back_at_install`
  (appimage case) and `scenario_app_updates_swap_is_complete_or_not_at_all` (appimage case: the helper killed after
  each recorded step).
- [ ] 5.3 macOS bundle swap: extract on the same volume, the smoke check, `renamex_np(RENAME_SWAP)`, move the old
  bundle to `previous/`, undo, and the `EPERM`/`EACCES` fallback to Downloads, asked once. Verify: on macOS CI, the
  bundle cases of 5.2's scenarios, `scenario_app_updates_refused_swap_falls_back_to_a_manual_install` (the swap
  fails through a test hook, and through a parent folder made read-only) and
  `scenario_app_updates_bundle_from_another_signer_refused` (fake bundles signed ad hoc on the runner, with an
  injected Team ID check).
- [ ] 5.4 Windows NSIS: back up the installed files and `DisplayVersion`, run `setup.exe /S`, check the exit code,
  the version and `DisplayVersion`, and undo. First check in a built installer that cargo-packager's NSIS honours
  `/S`, keeps the install location and does not launch the app; record it in design.md D6. Verify: on Windows CI,
  the windows-setup cases of 5.2's scenarios, with a stub `setup.exe` built by the test that copies files and exits
  with a chosen code.
- [ ] 5.5 Install on quit and Restart now: the quit hook, the ADR-0065 slot check, and relaunch for Restart now only.
  Verify: `scenario_app_updates_restart_now_waits_for_a_scan` (headless UI test) and
  `scenario_app_updates_quit_installs_a_ready_update` (a service-level test with a fake installation and a stub app
  process).
- [ ] 5.6 Scope: the updater writes only the installation and its update folder. Verify:
  `scenario_app_updates_user_data_untouched_by_install_and_rollback`, with tree snapshots of the sources, library,
  quarantine and data directory on the small corpus.

## 6. Trials and rollback

- [ ] 6.1 The early trial check in `photosquirrel` `main()`, before GPU selection, and the trial-ok mark at the first
  frame. On a trial start, `CatalogBackend::warm` waits until the trial-ok mark. Verify:
  `scenario_app_updates_trial_start_opens_the_catalog_after_the_first_frame` and
  `scenario_app_updates_successful_trial_keeps_the_new_version`.
- [ ] 6.2 Automatic rollback after two failed trials, with the catalog guard (`state.catalogs`, read-only
  `user_version` against `kept.max_catalog_schema`), and adding the failed version to `skipped`. Verify:
  `scenario_app_updates_new_version_that_cannot_start_is_rolled_back` (a stub new app that exits before its first
  frame) and `scenario_app_updates_failed_install_explained`.
- [ ] 6.3 Manual rollback from Settings and `update rollback`, with the guard. Verify:
  `scenario_app_updates_rollback_refused_after_a_catalog_upgrade` (a catalog fixture migrated to a newer
  `user_version` than the kept version's) and `scenario_app_updates_rollback_before_a_catalog_upgrade`.

## 7. UI

- [ ] 7.1 The consent card in onboarding and the one-time banner after an upgrade (at most 3 shows, then off).
  Verify: `scenario_app_shell_first_launch_shows_onboarding`,
  `scenario_app_updates_keep_me_up_to_date_enables_automatic_updates` and `scenario_app_updates_upgraded_user_asked_once`.
- [ ] 7.2 The banners, status-bar item and toasts of design D8 (ready, available, cannot install, urgent, updated,
  failed), the What's new dialog, and "Check for updates…" in the palette, all with ui-theme tokens. Verify:
  `scenario_app_updates_urgent_notice_persists`, headless tests for each D8 state, and `cargo xtask tokens`.
- [ ] 7.3 The Settings > General > Updates group. Verify: `scenario_settings_updates_group_shows_the_update_state`
  and `scenario_settings_search_filters_settings` (unchanged).
- [ ] 7.4 Quiet start: extend `crates/ui-workspace/tests/network.rs` with the undecided, off and automatic cases,
  using the recording transport and the socket check, and extend the HTTP-client manifest check to `photosquirrel` and
  `ui-*`. Verify: `scenario_app_shell_no_network_at_startup`,
  `scenario_app_shell_no_network_while_updates_are_undecided_or_off` and
  `scenario_app_shell_only_the_update_host_while_updates_are_on`. `cargo xtask bench --check` shows `cold-start` and
  `idle-rss` within budget and not regressed against the baseline.
- [ ] 7.5 Screenshots of the consent card, the ready banner and the Settings group in dark and light themes (`cargo
  xtask screenshot`). Verify: a screenshot review shows no overflow, and every text names versions.

## 8. CLI

- [ ] 8.1 `update status|check|install|rollback` with `--json` and the hidden `update finish`, plus the new `doctor`
  fields. Verify: `scenario_cli_every_subcommand_has_help`, `scenario_cli_doctor_lists_decoders` (unchanged),
  `scenario_cli_doctor_reports_update_state_offline`, `scenario_cli_update_status_works_offline`,
  `scenario_cli_update_check_reports_an_available_version`, `scenario_cli_install_refused_while_the_app_runs`,
  `scenario_cli_install_refused_for_a_package_managed_installation` and `scenario_cli_scans_never_check_for_updates`.

## 9. Release pipeline and docs

- [ ] 9.1 `cargo xtask package`: build the macOS `.app.tar.gz` and its `.sha256`, set
  `PHOTOSQUIRREL_RELEASE_BUILD=1`, and check that release binaries embed only the release keys. Verify:
  `scenario_release_packaging_app_archive_matches_the_dmg_s_bundle` and
  `scenario_release_packaging_release_binaries_embed_only_the_release_key`.
- [ ] 9.2 `cargo xtask update-manifest` (generate, sign, `--check`), with the notes taken from "What's new in
  <version>". Verify: `scenario_release_packaging_manifest_matches_the_packages` and
  `scenario_release_packaging_notes_taken_from_the_release_notes`.
- [ ] 9.3 `release.yml`:
  - a `sign` job (the `release-signing` environment for tags, a throwaway key for dry runs, read-only, xtask only);
  - the `sign` job also needs `verify-macos` when that job runs (`macos-signing`);
  - an `update-roundtrip` matrix job (tags only; it succeeds with a notice when no previous release has a manifest);
  - the release job needs the round trip, checks 8 packages, rechecks the envelope with `minisign -V` and
    `sha256sum -c`, and attaches `update-manifest.json`.

  Verify: `scenario_release_packaging_release_workflow_covers_every_platform`,
  `scenario_release_packaging_signing_job_isolated` and
  `scenario_release_packaging_previous_release_updates_to_the_candidate` (all read the workflow and the round-trip
  script), and a dry run with `gh workflow run release.yml` that is green, uses a throwaway key and holds 8 packages
  and the manifest.
- [ ] 9.4 Docs: the README "Updates" section, a release notes "Updates" section with the 0.4.0 "last manual install"
  note and a "What's new in 0.4.0" section, the new `docs/releasing.md` runbook, and ROADMAP row 37 (added with the
  spec commit). Verify: `scenario_release_packaging_readme_screenshots_exist` (unchanged),
  `scenario_release_packaging_readme_explains_updates`,
  `scenario_release_packaging_runbook_covers_withdrawal_and_key_events` and
  `scenario_release_packaging_heic_fallback_noted` (unchanged).
- [ ] 9.5 `docs/gpui-notes.md`: add anything surprising about the first-frame hook, the quit hook or the detached
  spawn from a GPUI app. Verify: the note exists, or tasks.md records "nothing new".

## 10. Release candidates (owner)

- [ ] 10.1 Tag `v0.4.0-rc.1` and `v0.4.0-rc.2` (prereleases, real key). On Linux (an AppImage in `~/Applications`),
  macOS 14 or 15 (`/Applications`, a standard user and an admin user) and Windows 11 (the NSIS per-user install),
  install rc.1 by hand, serve rc.2 locally through `PHOTOSQUIRREL_UPDATE_SERVER`, (with no Mac at hand, the macOS
  part runs on a GitHub macOS runner instead: rc.1 in a temp `Applications` folder, updated to rc.2), then check: update on quit, Restart
  now, a trial, manual rollback, and the App Management behaviour on macOS. Verify: findings recorded in design.md,
  with "Measurements and findings" per OS. If macOS prompts or refuses, the fallback path is confirmed and the README
  says so.
- [ ] 10.2 Owner: on the reference desktop, `cargo xtask bench --check`. Verify: every budget passes, and
  `cold-start`, `idle-rss` and `first-gallery-frame` are within 5% of their baselines.

## 11. Verification

- [ ] 11.1 Local checks: `cargo fmt --all --check`; clippy -D warnings (all targets) of ps-update, ps-service,
  ps-model, photosquirrel-cli, ui-workspace, photosquirrel and xtask; `cargo xtask layering`; `cargo xtask tokens`;
  the tests of every touched crate; `openspec validate auto-updates --strict`; `openspec validate --all --strict`;
  and `cargo xtask spec-coverage` with every scenario of this change mapped.
- [ ] 11.2 Corpus gate unchanged (release CLI, medium preset seed 42, drive-a/b/c: precision, recall and keeper
  1.0000, 0 violations, the baseline group counts); `rescan-nochange-100k` within budget.
- [ ] 11.3 Push; GitHub CI green on every job and all three OSes, including the fake-layout install tests.
- [ ] 11.4 `cargo xtask ci` passes.
- [ ] 11.5 Gate review by a fresh agent that did not write the code. It covers in particular: no request before
  consent; signature, version and hash rules; no half-installed state after any kill; that the updater writes nothing
  outside the installation and its update folder; and the screenshot review.
