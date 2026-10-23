# Tasks

## 1. AppImage runtime pin and notice (ADR-0073)

- [ ] 1.1 Choose the pinned runtime: look up type2-runtime releases for a tagged asset of a recent build (else vendor `packaging/linux/runtime-x86_64`), record commit, URL, size and SHA-256, and confirm the pristine file's `.digest_md5`/`.upd_info`/`.sha256_sig`/`.sig_key` are zero; ADR-0073 accepted and listed in `docs/adr/README.md` (verify: `sha256sum` of the file equals the pin; `readelf -S -x .upd_info` shows zeros)
- [ ] 1.2 Write `packaging/linux/appimage-runtime.toml` and the licence texts under `packaging/linux/appimage-runtime/` (type2-runtime MIT, musl MIT, libfuse LGPL-2.1, squashfuse BSD-2-Clause, zstd BSD-3-Clause, zlib, AppImageKit AppRun MIT), with the libfuse source archive URL and SHA-256 matching the version the runtime reports (verify: `RuntimePin` parse unit test `runtime_pin_parses_and_names_every_component`)
- [ ] 1.3 `appimage_runtime`, `normalized_runtime_sha256` and `runtime_components` on ELF byte fixtures (a synthetic ELF with the four sections, followed by `hsqs`); verify `scenario_release_packaging_changed_appimage_runtime_fails_packaging`
- [ ] 1.4 Download and check the pinned runtime and AppRun, pass `LDAI_RUNTIME_FILE` to the packager, pre-place `AppRun-x86_64` in the cargo-packager cache, and extend `check_appimage` with the runtime hash, component and AppRun checks (verify: `cargo xtask package --docker` passes; with one hex digit of the pin changed it fails naming both hashes)
- [ ] 1.5 "AppImage runtime" section in `render_notices` on Linux; verify `scenario_release_packaging_appimage_runtime_notice_matches_the_pinned_runtime`
- [ ] 1.6 Release job downloads, checks and attaches the pinned `[[source]]` archives; release notes and README mention the runtime and its libfuse source; verify `scenario_release_packaging_release_attaches_the_appimage_runtime_lgpl_source` (reads `release.yml`, like `scenario_release_packaging_release_workflow_covers_every_platform`)

## 2. Per-platform LGPL lists

- [ ] 2.1 `licence_notices` writes `lgpl-sources-<os>.txt`; update `scenario_release_packaging_lgpl_components_offer_their_source` for the new name (verify: that test passes)
- [ ] 2.2 `packaging/merge-lgpl-sources.sh` and its use in `release.yml` before the LGPL download step; verify `scenario_release_packaging_lgpl_source_lists_merged_per_platform` and `scenario_release_packaging_differing_lgpl_source_lists_fail_the_release` (unix-only tests that run the script on temp dirs with fixture lists, plus a check that `release.yml` calls it)
- [ ] 2.3 Release workflow dry run (`gh workflow run release.yml` on the branch; verify: the `release-notes` artifact holds the merged list, the per-platform lists and the libfuse source)

## 3. Copy-card badge

- [ ] 3.1 `edge_chip` label "Resized copy{via}" with the full sentence as tooltip and the id `duplicates-edge-{n}`; `review_kit::chip` caps its width and ellipsizes its label; verify `scenario_duplicate_review_copy_badge_stays_within_its_card` (default and largest UI font size)
- [ ] 3.2 Recapture `docs/screenshots/duplicates-group-dark.png` and `-light.png` (`cargo xtask screenshot` on the demo library, duplicates view) and review: badge inside the card, nothing else clipped

## 4. Flaky timing tests

- [ ] 4.1 Diagnose: run `dashboard_controls_on_the_real_engine` and `scenario_scan_progress_ui_cost_independent_of_rate` 20 times each while a parallel `cargo test --workspace` runs; record the failure rate, the cancel split (click to engine stopped, engine stopped to label) and wall vs thread-CPU p95 in this task's notes (verify: notes recorded; the split confirms or refutes the sampling-tick cause)
- [ ] 4.2 `Scanner::wait_stopped`, `ScanService::wait_stopped`, `ScanBackend::wait_stopped` (default no wait; fake backend on its clock), used by `ScanMonitor::cancel`; verify `scenario_scan_progress_cancelled_shown_when_the_run_stops` (fake backend: Cancelled shown with the fake clock not advanced to the next sample) and `scenario_scan_progress_snapshot_sampled_at_most_10_hz` still passing
- [ ] 4.3 `scan_real` prints the cancel split on failure and keeps the 250 ms bound (verify: 20 loaded runs of `dashboard_controls_on_the_real_engine` pass)
- [ ] 4.4 `frame_times` measures render-thread CPU time (rustix `ThreadCPUTime` on Linux and macOS, wall time elsewhere) and alternates the two rigs frame by frame; bounds unchanged (verify: `scenario_scan_progress_ui_cost_independent_of_rate` passes 20 loaded runs; a deliberate per-file cost injected in a scratch build, e.g. re-keying every recent-photo thumbnail per 1,000 files, makes it fail)

## 5. Small fixes

- [ ] 5.1 Library toolbar: filter clusters as non-wrapping groups separated by spacing, divider elements removed; verify `scenario_gallery_toolbar_has_no_stray_divider`, then a 1280 px screenshot with the inspector open reviewed
- [ ] 5.2 ADR-0074 accepted and listed in `docs/adr/README.md`; `Ranking::runner_up` and `Group::runner_up` (serde default), chosen as the best different copy outside the keeper set; verify `scenario_best_copy_reasons_compare_against_the_best_different_copy` and `scenario_best_copy_keeper_reasons_listed`
- [ ] 5.3 `GroupDetail`/`ReviewDetail` expose the runner-up and `why_compare_line` uses it (old rows fall back); verify `scenario_duplicate_review_why_this_copy_and_next_best_name_the_same_copy` and `scenario_duplicate_review_why_this_copy_shown`
- [ ] 5.4 Keepers unchanged: `photosquirrel-cli eval-corpus` on the small corpus reports the same precision, recall and keeper accuracy as before the change (verify: identical numbers in the task notes)

## 6. Verification

- [ ] 6.1 `cargo xtask ci` is green
- [ ] 6.2 `cargo xtask package --docker` passes and the extracted AppImage's `THIRD-PARTY-LICENSES` contains the AppImage runtime section
- [ ] 6.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 6.4 `openspec validate release-hygiene --strict` passes
