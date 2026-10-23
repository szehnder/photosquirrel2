# Design

## Context

Five independent debts found around the 0.1.0 dry run. They share a change because each is small, none
changes architecture beyond two amendments (ADR-0073, ADR-0074), and together they gate a trustworthy release:
notices that cover the whole AppImage (ADR-0058, ADR-0060), a release that keeps every platform's LGPL list,
README screenshots without visual bugs (ADR-0028, ADR-0030), and a CI that fails only on real regressions
(ADR-0026).

What was observed while writing this design (read-only; nothing was built or run):
- cargo-packager 0.11.8 (`src/package/appimage/mod.rs`, `donwload_dependencies`) downloads `AppRun-x86_64`
  (tauri `apprun-old`), `linuxdeploy-x86_64.AppImage` (tauri `linuxdeploy`) and
  `linuxdeploy-plugin-appimage-x86_64.AppImage` (`continuous`) into `<cache>/.cargo-packager/AppImage/`, only
  when the file is missing. Inside Docker the cache is `target/docker/home/.cache/.cargo-packager/AppImage/`.
- The plugin bundles appimagetool but no runtime file; appimagetool's strings show it downloads the runtime
  ("Failed to download runtime file, please download the runtime manually from
  https://github.com/AppImage/type2-runtime/releases and pass it to appimagetool with --runtime-file"). The
  plugin reads `LDAI_RUNTIME_FILE` (among `LDAI_OUTPUT`, `LDAI_COMP`, ...), and cargo-packager's script runs
  linuxdeploy with the inherited environment.
- `target/packages/PhotoSquirrel-0.1.0-linux-x86_64.AppImage`: the ELF runtime ends at byte 944,632
  (`e_shoff + e_shnum * e_shentsize`), where `hsqs` starts. Its strings name type2-runtime commit `fedfad5`,
  "FUSE library version" 3.15.0, and a help text listing "runtime, musl libc, libfuse, squashfuse, libzstd,
  zlib" with licence URLs. Its sections `.digest_md5`, `.upd_info`, `.sha256_sig`, `.sig_key` are written by
  appimagetool.
- `licence_notices` (xtask/src/package.rs) writes `target/packages/lgpl-sources.txt` on every platform from
  cargo-about reports over the four `about.toml` targets and the same `Cargo.lock`, so the lists are equal by
  construction today; `release.yml` downloads all `packages-*` artifacts with `merge-multiple: true`, so the
  three same-named files overwrite each other. `heic-<os>.txt` already uses per-platform names.
- Duplicate member card (`render_member_card`, duplicates.rs): 232-unit wide card with `overflow_hidden`, 3-unit
  padding; `edge_chip` builds `format!("Same image, resized or recompressed{via}")`, `via` being
  `ReviewDetail::matched_label` (" of the keeper" / " of #n"). `review_kit::chip` is `flex_none` with no width
  cap, so its layout box is wider than the card and the card's clip cuts it at the border
  (docs/screenshots/duplicates-group-dark.png: "Same image, resized or recompressed o").
- Library toolbar (`LibraryView::toolbar`, library.rs): two free-standing 1-px divider `div`s sit inside the
  `flex_wrap` group `library-filter-chips` between the kind chips, the camera/GPS/stacks chips and the AI/status
  menus. When the group wraps, the line can break right before or after a divider, leaving it at a line end;
  1280 px with the inspector open is one such width (reported at the M2 gate; to be confirmed by the first
  test).
- "Why this copy": `rank` (ps-dedup/src/rank.rs) explains the keeper against `idx[1]`, the second-ranked
  member, which may be a byte-identical copy or the RAW+JPEG companion; `why_compare_line` (duplicates.rs)
  skips identical copies and keeper-set members and names the first remaining member. Two different rules.
- `dashboard_controls_on_the_real_engine` (ui-workspace/tests/scan_real.rs) asserts the status label within
  `ci_slack(250 ms)` of real time. `pump` sleeps 10 ms of real time per 10 ms of the fake GPUI clock, then runs
  `run_until_parked`, so the fake clock runs at most as fast as real time and slower under load. Pause and
  resume set the run status synchronously (`Scanner::pause`/`resume` call `progress.set_status`) and
  `ScanMonitor::control` samples right after the call returns, so their label shows at once. Cancel only sets
  the cancel flag; `pipeline::execute` sets `Cancelled` after the workers drain and two catalog flushes. The
  sample taken right after `cancel()` still sees Running, so the label waits for the sampling loop's next
  `SAMPLE_INTERVAL` (100 ms of fake clock, i.e. at least 10 pump iterations of real time) after the engine
  stopped. Engine stop (40 ms read delay, in-flight decodes, two fsyncs on a shared disk) plus up to one
  inflated sampling period explains the ~1-in-5 failures under load. CI sets `CI`, so `ci_slack` gives 1 s
  there; macOS and Windows CI do not run the ui-workspace tests at all (ci.yml tests only core and I/O crates
  with `PHOTOSQUIRREL_TEST_SCALE=reduced`), so these two flakes are Linux and local only.
- `scenario_scan_progress_ui_cost_independent_of_rate` (ui-workspace/tests/scan_dashboard.rs) compares the
  wall-clock p95 of 120 frames per rate (index 114, the seventh-worst), interleaved in rounds of 15 frames. A
  Linux scheduler time slice is 1-4 ms; under load a few preempted frames land in one series and not the
  other, and the seventh-worst frame of 120 moves by about a slice. 1.2 ms is that, not a rate-dependent
  cost. The fake engine's rate-dependent work (more digits, new recent-photo keys every sample) is the
  regression the test exists to catch and remains visible in CPU time.

## Goals / Non-Goals

**Goals:**
- The AppImage's notices and source offers cover its runtime, and a runtime change cannot reach a release
  unreviewed.
- The release keeps and compares every platform's LGPL list.
- No badge, chip or divider spills or strays at any supported width; reasons and "Next best" agree.
- The two timing tests fail only on a real regression, at their current bounds.

**Non-Goals:**
- Pinning linuxdeploy and the plugin (build tools), building the runtime ourselves, AppImage signing.
- Changing the 250 ms control contract, the 8.3 ms frame budget, or which crates macOS/Windows CI tests.

## Decisions

### (a) AppImage runtime pin and notice (ADR-0073)
- **Pin file.** `packaging/linux/appimage-runtime.toml`:
  `[runtime] commit, url, sha256, size`; `[[component]] name, version, licence, text` (text = path under
  `packaging/linux/appimage-runtime/`); `[apprun] url, sha256, licence, text`; `[[source]] name, version, file, url,
  sha256` (the libfuse 3.15.0 release tarball and the `patches/libfuse/mount.c.diff` type2-runtime applies to it,
  from raw.githubusercontent.com at the pinned commit; GitHub's generated tree archives are not byte-stable, so the
  build scripts are referenced by URL at the commit instead of attached). Parsed by a `RuntimePin` struct in
  `xtask/src/package.rs` (serde + `toml`, already xtask dependencies).
- **Feed the pin to the packager.** Before `cargo packager` on Linux: download `runtime.url` and `apprun.url`
  into `target/packager-tools/` with `ureq`, check SHA-256 with `sha2` (both already xtask dependencies), set `LDAI_RUNTIME_FILE` on the packager `Command`, and copy the pinned `AppRun` to
  `<cache>/.cargo-packager/AppImage/AppRun-x86_64` (cargo-packager uses `dirs::cache_dir()`: `$XDG_CACHE_HOME`, else
  `$HOME/.cache`; xtask computes the same path; inside Docker `HOME=/work/target/docker/home`), replacing what is there. A mismatching
  download fails packaging before the build.
- **Check the result.** New pure functions, unit-tested on byte fixtures:
  `appimage_runtime(bytes) -> Result<&[u8]>` (ELF64 header: `e_shoff`, `e_shentsize`, `e_shnum`; the runtime
  is `bytes[..e_shoff + e_shentsize * e_shnum]`, and the next four bytes must be `hsqs`),
  `normalized_runtime_sha256(runtime)` (zero the four tool-filled sections found through the section header
  string table) and `runtime_components(runtime) -> Vec<String>` (the `* <name>, licensed under the terms of`
  lines of the embedded help text). `check_appimage` reads the built AppImage, fails unless the normalized hash
  equals the pin (message: pinned vs found, "update packaging/linux/appimage-runtime.toml and its notice
  texts"), unless the component names equal the pin's, or unless the extracted `AppRun` hashes to the pin.
  Task 1.1 first confirms that the pristine runtime file has these sections zeroed (so pin == normalized
  embedded runtime); if not, the pin stores the normalized hash.
- **Notice.** `render_notices` gains an optional "AppImage runtime" section on Linux (the notices file is
  generated per platform already), after the LGPL section: "The AppImage package starts with the AppImage
  runtime <commit> (https://github.com/AppImage/type2-runtime), which contains: ..." with each component,
  version and licence text, the LGPL-2.1 source offer for libfuse (the release asset name and upstream URL)
  and the AppRun launcher. The .deb and .tar.gz carry the same file; the section says it concerns the
  AppImage.
- **URL choice.** The pin uses a tagged type2-runtime release asset if one exists for the chosen build;
  otherwise the runtime file is vendored as `packaging/linux/runtime-x86_64` (about 0.9 MB) with the same
  hash check (ADR-0073 alternatives). Decided in task 1.1: release `20251108` (commit `dd6cebe`, libfuse 3.15.0,
  squashfuse 0.5.2, zstd 1.5.6, zlib 1.3.1, musl 1.2.5 from Alpine 3.21), SHA-256 `2fca8b44...`, 944,632 bytes; its
  four tool sections are zero, so the file's plain hash is the pin. The dry run's `fedfad5` came from `continuous`
  and normalizes to a different hash, which is exactly what the check now refuses.
- **Release job.** After the LGPL crate step, a step reads `[[source]]` from the pin (a small `python3 -c 'import tomllib'` read;
  the job has no Rust toolchain), downloads each archive, checks SHA-256, attaches it; failure
  stops the release. Release notes (`packaging/release-notes.md`) mention the runtime and libfuse source.

### (b) Per-platform LGPL lists
- `licence_notices` writes `lgpl-sources-<os>.txt` (`Os::name()`); the constant becomes a function
  `lgpl_sources_file(os)`.
- `packaging/merge-lgpl-sources.sh <dir> linux macos windows`: fails if any `lgpl-sources-<os>.txt` is missing;
  writes the sorted union to `<dir>/lgpl-sources.txt`; fails, printing `diff` and the platform, if any
  platform's sorted list differs from the union. Expected differences: none. Every list is computed from the
  same `Cargo.lock` over the same four targets, so a difference means a cargo-about or feature mismatch that a
  person must look at before the source offer is trusted; the union is still written so the error shows what
  would be attached.
- `release.yml`: the LGPL step runs the script first and keeps reading `packages/lgpl-sources.txt`. The
  per-platform files stay in the workflow artifacts; only the merged one feeds the downloads and notes.

### (c) Copy-card badge
- `edge_chip`: derivative label `"Resized copy{via}"`; the chip gets an id `duplicates-edge-{n}` with
  `test_support()` and a tooltip with the full sentence ("Same image, resized or recompressed, matched against
  the keeper").
- `review_kit::chip`: `max_w_full().min_w_0().overflow_hidden()`, its label in a child with `min_w_0()`,
  `whitespace_nowrap()`, `overflow_hidden()`, `text_ellipsis()` (docs/gpui-notes.md "Toolbars and tooltips").
  This protects every chip; chips already fit today are unchanged. (`chip_frame`, `chip_label` and `chip_icon`
  are the parts, so the edge chip can give its label an id.)
- Headless test in `crates/ui-workspace/tests/duplicates.rs`: open a group whose second member is a derivative
  of the keeper, assert `bounds("duplicates-edge-2")` and `bounds("duplicates-edge-2-label")` are inside
  `bounds("duplicates-member-2")` minus padding, at the default and the largest UI font size the settings allow.
  The label matters: the card's column stretches the chip's box to the content width, so the old chip's box
  always fit and only its text ran past the card (with the old wording and chip the label check fails).
- Recapture `docs/screenshots/duplicates-group-{dark,light}.png` with `cargo xtask screenshot` on the demo
  library and review them.

### (d) Flaky timing tests
- **Cancel.** Add `ScanBackend::wait_stopped(&self, limit: Duration) -> bool` (default: `false`, no wait), backed
  by `ScanService::wait_stopped` -> `Scanner::wait_stopped(limit)`: waits on the active run's thread
  (`ActiveRun`'s `thread`, polling `JoinHandle::is_finished` every 2 ms inside the engine, or a condvar set by
  `execute`) up to `limit`. `ScanMonitor::cancel` uses `control` with a closure that calls `b.cancel()` then
  `b.wait_stopped(Duration::from_secs(2))` on the background executor, so the snapshot `control` takes after it
  already shows Cancelled. No extra snapshots are read (the 10 Hz rule holds) and the UI thread never blocks.
  The fake backend implements it on its fake clock. The real-engine test keeps 250 ms; on failure it prints the
  split (click to engine stopped, engine stopped to label) so a recurrence points at the engine or the UI.
  Engine stop latency itself stays guarded by `scenario_scan_control_cancel_within_250_ms` (ps-scan, run with
  exclusive cores).
- **Frame cost.** `frame_times` measures each frame with the rendering thread's CPU clock: `rustix::time::
  clock_gettime(ClockId::ThreadCPUTime)` on Linux and macOS (rustix is a workspace dependency already; add its
  `time` feature as a ui-workspace dev-dependency), wall time elsewhere. The two rigs alternate frame by frame
  instead of in rounds of 15, so slow periods hit both series alike. Bounds stay 1 ms and 8.3 ms
  (`ci_slack` unchanged). Why this still catches a regression: a dashboard whose work grows with the file rate
  (re-rendering every recent-photo thumbnail, formatting per-file lists, per-file events) costs CPU on the
  render thread at 20,000 files/s and not at 100, and CPU time measures exactly that; only preemption, which
  is not the app's cost, is removed.
- **Diagnose before fixing.** Task 4.1 runs each test 20 times under a parallel `cargo test` load and records
  the failure rate and, for the frame test, wall vs CPU p95; if the CPU-time diff still exceeds 1 ms, the
  rate-dependent cost is real and is fixed in the dashboard instead.
- **What the diagnosis found (task 4.1 notes).** Cancel: confirmed; the label lagged the stopped engine by
  68-200 ms. Frame cost: the preemption hypothesis was refuted (wall and CPU p95 within 1% in 18 of 20 loaded
  runs); slow periods hitting one rate's 15-frame block moved the p95, which the frame-by-frame alternation
  addresses, so the comparison also flips which rig renders first each pair and takes 400 frames per rate (the
  p95 of 120 is the 7th-worst frame and moves with a handful of slow ones). CPU time stays the measure (the spec
  defines it so, and it does remove preemption where there is some).
- **Not fixed by this change.** Under a heavy parallel load (load average ~20 on 16 threads) two things remain
  outside the dashboard: the engine's own stop after a cancel (drain plus the final catalog commit on a shared
  disk) sometimes exceeds 250 ms from the click, and render-thread CPU time itself inflates past 8.3 ms (SMT
  siblings and clocks are shared). Both bounds are kept; the loaded verification of tasks 4.3 and 4.4 stays open.
  The `scan_real` bound now counts from the click (after the frame the click lands on), because the click's
  handler itself waits for the engine; counting after the handler, as before, would exclude the engine's stop.
- **Scheduling, not thresholds (ADR-0075).** Decided after the diagnosis: the flakes came from running beside the
  rest of the suite in `cargo xtask ci` and Linux CI. Every UI test that asserts a wall-clock or frame-time budget
  (`dashboard_controls_on_the_real_engine`, `scenario_scan_progress_ui_cost_independent_of_rate`,
  `scenario_app_shell_slow_query_keeps_ui_responsive`, `scenario_gallery_jump_to_any_position`,
  `gallery_frame_cpu_cost_flinging_1m`) is `#[ignore = "timing"]`; `cargo xtask timing` runs exactly those with
  `--ignored --exact --test-threads=1`, and `cargo xtask ci` runs it right after `cargo test`. A unit test keeps the
  list in `xtask/src/timing.rs` equal to the marked tests. Spec coverage is by test name, so they stay mapped.

### (e) Small fixes
- **Toolbar.** Remove the two divider `div`s; wrap the three filter clusters in `h_flex().flex_none()` groups
  with ids `library-filters-kind|meta|ai`, inner gap `space(1.0)`, outer gap `space(3.0)` in the wrapping
  container. A cluster wraps as a unit, so nothing is left alone at a line end. Test in
  `crates/ui-workspace/tests/gallery.rs` sweeps window
  widths 960-1920 px with the inspector closed and open.
- **Why this copy (ADR-0074).** `Ranking` and `Group` get `runner_up: Option<FileId>` (`#[serde(default)]`, so
  stored rows still deserialize); `rank` picks it as the ADR says (content identity from `DedupInput::exact_key`,
  keeper set from `keeper_set::companion`) and passes it to `reason`. The catalog keeps no new column: the stored
  reasons array's first element (the deciding reason, the one stated against the runner-up) carries
  `"runner_up": <file id>` (`ps_catalog::groups::reason_runner_up`), so no migration is needed and rows stored
  before have none. `GroupDetail`/`ReviewDetail` expose it; `why_compare_line` uses it when present and keeps
  today's rule only for rows without it. Keeper choice is unchanged, so `photosquirrel-cli eval-corpus` results
  must be identical.

## Risks / Trade-offs

- [The pinned runtime URL disappears] -> packaging fails loudly; fallback is the vendored file (ADR-0073).
- [appimagetool ignores `LDAI_RUNTIME_FILE` in some plugin build] -> the post-build hash check catches it;
  the plugin version in the dry-run cache reads the variable (strings), confirmed in task 1.2.
- [Help-text component parsing breaks on a reworded upstream help] -> the check fails closed and names what it
  found; updating the pin is a reviewed change anyway.
- [Thread CPU time hides a real wall-clock stall, such as a lock wait on the UI thread] -> the frame test is
  about rate-dependent cost; wall-clock frame budgets stay with the gallery and shell budget tests (ADR-0026).
- [`wait_stopped` blocks a background thread for up to 2 s] -> only after a user cancel, on the background
  executor; returns as soon as the run thread ends.
- [Group digests change for most groups] -> one-time rewrite at the first regroup; writes only rows that differ.

## Effort

About 5 developer days: (a) pinned AppImage runtime, notice and post-package check 2 days; (b) per-platform LGPL
source lists and the merge script 0.5 day; (c) badge label, chip width cap and the bounds test 0.5 day; (d) flake
diagnosis and the two fixes 1.5 days; (e) toolbar clusters and the shared runner-up 0.5-1 day. On top: one
`cargo xtask package --docker` run, one release-workflow dry run, a corpus gate run for (e) and a screenshot recapture.

## Verification

- `cargo test -p xtask package` (pin parsing, runtime extraction, normalization, components, notices section,
  LGPL list naming, merge script on fixture lists; all `scenario_release_packaging_*`).
- `cargo xtask package --docker` on Linux: passes with the pin; with a doctored pin hash it fails naming both
  hashes; the extracted AppImage's `usr/lib/photosquirrel/THIRD-PARTY-LICENSES` has the runtime section.
- `gh workflow run release.yml` dry run: `lgpl-sources-{linux,macos,windows}.txt`, merged list, attached
  libfuse source in the `release-notes` artifact.
- `cargo test -p ps-dedup rank`, `cargo test -p ps-service groups`, `cargo test -p ui-workspace --test duplicates`,
  `--test scan_dashboard`, `--test scan_real`, and the toolbar test; `photosquirrel-cli eval-corpus` unchanged.
- Flake check: `for i in $(seq 20); do cargo test -p ui-workspace --test scan_real --test scan_dashboard; done`
  while another `cargo test --workspace` runs, with `CARGO_BUILD_JOBS=4` and `TMPDIR` under `target/`.
- `cargo xtask screenshot -- --theme dark` and `--theme light` on the duplicates view, reviewed.
- `cargo xtask ci`, `openspec validate release-hygiene --strict`.
