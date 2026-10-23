# Tasks

## 1. AppImage runtime pin and notice (ADR-0073)

- [x] 1.1 Choose the pinned runtime: look up type2-runtime releases for a tagged asset of a recent build (else vendor `packaging/linux/runtime-x86_64`), record commit, URL, size and SHA-256, and confirm the pristine file's `.digest_md5`/`.upd_info`/`.sha256_sig`/`.sig_key` are zero; ADR-0073 accepted and listed in `docs/adr/README.md` (verify: `sha256sum` of the file equals the pin; `readelf -S -x .upd_info` shows zeros)
- [x] 1.2 Write `packaging/linux/appimage-runtime.toml` and the licence texts under `packaging/linux/appimage-runtime/` (type2-runtime MIT, musl MIT, libfuse LGPL-2.1, squashfuse BSD-2-Clause, zstd BSD-3-Clause, zlib, AppImageKit AppRun MIT), with the libfuse source archive URL and SHA-256 matching the version the runtime reports (verify: `RuntimePin` parse unit test `runtime_pin_parses_and_names_every_component`)
- [x] 1.3 `appimage_runtime`, `normalized_runtime_sha256` and `runtime_components` on ELF byte fixtures (a synthetic ELF with the four sections, followed by `hsqs`); verify `scenario_release_packaging_changed_appimage_runtime_fails_packaging`
- [x] 1.4 Download and check the pinned runtime and AppRun, pass `LDAI_RUNTIME_FILE` to the packager, pre-place `AppRun-x86_64` in the cargo-packager cache, and extend `check_appimage` with the runtime hash, component and AppRun checks (verify: `cargo xtask package --docker` passes; with one hex digit of the pin changed it fails naming both hashes)
- [x] 1.5 "AppImage runtime" section in `render_notices` on Linux; verify `scenario_release_packaging_appimage_runtime_notice_matches_the_pinned_runtime`
- [x] 1.6 Release job downloads, checks and attaches the pinned `[[source]]` archives; release notes and README mention the runtime and its libfuse source; verify `scenario_release_packaging_release_attaches_the_appimage_runtime_lgpl_source` (reads `release.yml`, like `scenario_release_packaging_release_workflow_covers_every_platform`)

## 2. Per-platform LGPL lists

- [x] 2.1 `licence_notices` writes `lgpl-sources-<os>.txt`; update `scenario_release_packaging_lgpl_components_offer_their_source` for the new name (verify: that test passes)
- [x] 2.2 `packaging/merge-lgpl-sources.sh` and its use in `release.yml` before the LGPL download step; verify `scenario_release_packaging_lgpl_source_lists_merged_per_platform` and `scenario_release_packaging_differing_lgpl_source_lists_fail_the_release` (unix-only tests that run the script on temp dirs with fixture lists, plus a check that `release.yml` calls it)
- [x] 2.3 Release workflow dry run (`gh workflow run release.yml` on the branch; verify: the `release-notes` artifact holds the merged list, the per-platform lists and the libfuse source)
  Notes: dry run 39899035479 (main, workflow_dispatch, nothing published): the `release-notes` artifact holds
  `lgpl-sources-{linux,macos,windows}.txt` (identical) and the merged `lgpl-sources.txt` (rawler 0.8.0),
  `rawler-0.8.0.crate`, and `runtime-source/fuse-3.15.0.tar.xz` and the mount.c patch with the pinned SHA-256s.

## 3. Copy-card badge

- [x] 3.1 `edge_chip` label "Resized copy{via}" with the full sentence as tooltip and the id `duplicates-edge-{n}`; `review_kit::chip` caps its width and ellipsizes its label; verify `scenario_duplicate_review_copy_badge_stays_within_its_card` (default and largest UI font size)
- [x] 3.2 Recapture `docs/screenshots/duplicates-group-dark.png` and `-light.png` (`cargo xtask screenshot` on the demo library, duplicates view) and review: badge inside the card, nothing else clipped
  Notes: recaptured on a fresh `photosquirrel-cli demo` library with `PHOTOSQUIRREL_REVIEW_GROUP=111`
  (IMG_3437-Edit.jpg: keeper, a byte-identical backup copy ranked second, a 1024x829 resized copy): the badges read
  "Exact copy of the keeper" and "Resized copy of the keeper" inside their cards, "Why this copy" says more pixels
  1944x1574 vs 1024x829 and "Next best #3" names the resized copy; nothing else clipped, both themes.

## 4. Flaky timing tests

- [x] 4.1 Diagnose: run `dashboard_controls_on_the_real_engine` and `scenario_scan_progress_ui_cost_independent_of_rate` 20 times each while a parallel `cargo test --workspace` runs; record the failure rate, the cancel split (click to engine stopped, engine stopped to label) and wall vs thread-CPU p95 in this task's notes (verify: notes recorded; the split confirms or refutes the sampling-tick cause)
  Notes (16 logical CPUs; load: `cargo test --workspace -- --test-threads=4` in a loop, load average 14-23; each test
  binary run 20 times directly, with the split and both clocks printed):
  - Cancel, before the fix: 12 of 20 runs failed. The engine had stopped 106-201 ms after the click in 19 runs (one
    had not stopped at 272 ms), and the label followed 68-200 ms (median ~150 ms) after that: the next periodic
    sample, stretched because the test's fake clock advances only 10 ms per 10 ms of real pump time. Confirms the
    sampling-tick cause.
  - Frame cost, before the fix: 5 of 20 failed (3 on the 1 ms difference, 2 over 8.3 ms). Wall and render-thread
    CPU p95 were within 1% of each other in 18 runs: preemption is *not* what moved the p95. The difference came
    from slow periods of the machine hitting one rate's block of 15 frames (CPU-time diff 1.43 ms in one run), on
    top of a constant ~0.15 ms the 100 files/s layout costs more (measured unloaded with every frame order: not
    an order effect, not rate-dependent work). The two over-budget runs had CPU p95 8.6 and 12.5 ms: under that
    load the CPU time itself inflates (shared SMT cores, lower clocks), which no clock choice removes.
  - After the fixes (wait_stopped; frame-by-frame alternation with flipped order, 400 frames per rate, CPU time):
    the label shows 4-18 ms after the engine stopped in all 20 loaded runs (was 68-200 ms). 7 of 20 still missed
    250 ms because the engine itself took 252-516 ms from the click to stop (draining, plus the final catalog
    commit on a disk shared with the load; unloaded it stops in 45-80 ms after the request and the label shows
    150-215 ms after the click, including ~40 ms of headless click dispatch). Frame test: 4 of 20 failed, 3 with
    CPU p95 10.0-10.6 ms over 8.3 ms and 1 by 1.01 ms at a CPU p95 of 11-12 ms; in the other 16 the two rates
    were 0.03-0.8 ms apart.
- [x] 4.2 `Scanner::wait_stopped`, `ScanService::wait_stopped`, `ScanBackend::wait_stopped` (default no wait; fake backend on its clock), used by `ScanMonitor::cancel`; verify `scenario_scan_progress_cancelled_shown_when_the_run_stops` (fake backend: Cancelled shown with the fake clock not advanced to the next sample) and `scenario_scan_progress_snapshot_sampled_at_most_10_hz` still passing
- [x] 4.3 `scan_real` prints the cancel split and keeps the 250 ms bound (counted from the click); the test runs in the timing pass (ADR-0075: `#[ignore = "timing"]`, `cargo xtask timing`, run alone by `cargo xtask ci` after `cargo test`) (verify: 20 runs of the timing pass on its own all pass; 20 runs beside one `cargo build -j4` reported, not required to pass)
  Notes: alone, 20 of 20 timing passes green. Beside a looping `cargo build -j4 --release` (load average ~5-7):
  17 of 20 green; the 3 failures were this test's cancel at 262, 269 and 334 ms from the click, the engine taking
  255-328 ms to stop and the label following 6-7 ms later. (Under the harsher diagnosis load of 4.1, a parallel
  `cargo test --workspace`, 7 of 20 failed the same way.)
- [x] 4.4 `frame_times` measures render-thread CPU time (rustix `ThreadCPUTime` on Linux and macOS, wall time elsewhere) and alternates the two rigs frame by frame; bounds unchanged; the test runs in the timing pass (verify: 20 runs of the timing pass on its own all pass; 20 runs beside one `cargo build -j4` reported; a deliberate per-file cost injected in a scratch build makes it fail)
  Notes: alone, 20 of 20 green; beside `cargo build -j4`, it passed in all 17 runs that reached it (the 3 failing
  runs stopped at 4.3's test first). An injected per-file cost (one element per 1,000 processed files) fails it
  (p95 4.5 ms vs 26.6 ms); one per 20,000 files adds 0.6 ms and passes. Under the 4.1 load (parallel
  `cargo test --workspace`) 4 of 20 failed, 3 of them on CPU p95 10.0-10.6 ms over 8.3 ms.

## 5. Small fixes

- [x] 5.1 Library toolbar: filter clusters as non-wrapping groups separated by spacing, divider elements removed; verify `scenario_gallery_toolbar_has_no_stray_divider`, then a 1280 px screenshot with the inspector open reviewed
  Notes: 1280x800 window, inspector open (target/screenshots/library-toolbar-1280-dark.png, not committed): the
  kind and metadata clusters share the first line with the wider gap between them, the AI and export-status pair
  wraps to the second line as a unit, no divider anywhere. Clusters also get `max_w_full().flex_wrap()`: at 960 px
  with the inspector open the filter area is ~110 px, narrower than any cluster.
- [x] 5.2 ADR-0074 accepted and listed in `docs/adr/README.md`; `Ranking::runner_up` and `Group::runner_up` (serde default), chosen as the best different copy outside the keeper set; verify `scenario_best_copy_reasons_compare_against_the_best_different_copy` and `scenario_best_copy_keeper_reasons_listed`
- [x] 5.3 `GroupDetail`/`ReviewDetail` expose the runner-up and `why_compare_line` uses it (old rows fall back); verify `scenario_duplicate_review_why_this_copy_and_next_best_name_the_same_copy` and `scenario_duplicate_review_why_this_copy_shown`
- [x] 5.4 Keepers unchanged: `photosquirrel-cli eval-corpus` on the small corpus reports the same precision, recall and keeper accuracy as before the change (verify: identical numbers in the task notes)
  Notes (medium preset, seed 42, drives a, b, c; the gate the owner runs): before the change precision 1.0000,
  recall 1.0000, keeper accuracy 1.0000, 0 must-not-merge violations (19,381 true pairs, 1,843 keeper groups);
  after, on a fresh scan with the new build, exactly the same. Regrouping the *same* pre-change catalog with the new
  build rewrote all 1,843 group rows (digests now include the runner-up) and gave the identical keeper for every
  group (path list diff empty). Two separate scans may pick different byte-identical copies as keeper (file ids
  break that tie and the parallel scan assigns them), which the gate scores as correct either way.

## 6. Verification

- [x] 6.1 `cargo xtask ci` is green
- [x] 6.2 `cargo xtask package --docker` passes and the extracted AppImage's `THIRD-PARTY-LICENSES` contains the AppImage runtime section
- [x] 6.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [x] 6.4 `openspec validate release-hygiene --strict` passes
