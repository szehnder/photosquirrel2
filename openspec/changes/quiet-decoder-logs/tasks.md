# Tasks

Ships in 0.3.x, first of the four (ROADMAP row 29). **No ADR and no migration** (a default log filter is not a decision
of architectural weight); ADR-0112 and migration 26 go to the next changes.

## 1. Filter

- [x] 1.1 `ps_media::LOG_DEFAULT_FILTER` (`warn,nom_exif=error,rawler=error`) with its doc comment (D1, D2), and the filter unit test. Verify: `scenario_cli_the_default_filter_keeps_errors_and_our_own_warnings`.
  Done: `crates/ps-media/tests/log_filter.rs` builds an `env_logger::Builder` with `parse_filters(LOG_DEFAULT_FILTER)` (no `env_filter` dependency); 6 rawler/nom-exif targets warn off and error on, 5 PhotoSquirrel targets warn and error on, `ps_scan` info off; 1 passed. `env_logger` and `log` are ps-media dev-dependencies only.
- [x] 1.2 Use it in `photosquirrel-cli/src/main.rs` and `photosquirrel/src/main.rs` (`default_filter_or(LOG_DEFAULT_FILTER)`), replacing both literals. Verify: `grep -rn 'nom_exif=error' crates/*/src/main.rs` finds nothing; `cargo xtask layering` green.
  Done: both binaries already depend on `ps-media` directly; the grep finds nothing; `layering OK (17 crates checked)`.

## 2. Tests

- [x] 2.1 CLI scan of a fixture that reaches rawler's lens lookup, with and without `RUST_LOG` (D3; record whether the `ps-fixtures` CR2 reaches it or the fallback is used). Verify: `scenario_cli_lens_database_warnings_are_hidden_by_default`, `scenario_cli_rust_log_shows_them_again`.
  Done: a real CLI scan, no `log::warn!` stand-in, but not with the existing `cr2_like` writer (see Deviations): new `ps_fixtures::dng::cr2_unknown_lens` reaches `rawler::lens` through ps-media's rawler fallback. `crates/photosquirrel-cli/tests/decoder_logs.rs`: default filter, no "No lens definition" and no "rawler" on stderr, exit 0, summary completed with 1 processed and 0 errors; `RUST_LOG=rawler=warn` and plain `RUST_LOG=warn` both print the `rawler::lens` line. 3 passed (with 2.2).
- [x] 2.2 Truncated CR2 fixture under the default filter. Verify: `scenario_scan_progress_damaged_raw_file_still_reported_under_the_default_filter`.
  Done: the unknown-lens CR2 cut to its first eighth (directories and maker note kept, preview JPEG cut). Default filter: no "rawler" on stderr; summary completed, 1 processed, 1 new; `previews status` has one row `raw:cr2` / `unsupported_variant` / count 1 (the human table lists it). With `RUST_LOG=rawler=warn` the same scan prints rawler warnings (lens, skipped tag) and `previews status` is identical.

## 3. Docs

- [x] 3.1 README "Command line": `RUST_LOG=rawler=warn` shows the RAW decoder's diagnostics for a bug report; the 0.3.x release-notes line. Verify: `grep -n 'RUST_LOG' README.md`.
  Done: README lines 178-180; `packaging/release-notes.md` gets a "What's new since 0.3.0" section with the line (the next 0.3.x changes add to it).

## 4. Verification

- [x] 4.1 `cargo xtask ci` green in a clean worktree; every scenario mapped.
  Done: `cargo xtask ci` in a detached worktree of 9a68f29: "CI passed" (fmt, clippy, layering, tokens, tests, `openspec validate` 35 passed, spec coverage 658 scenarios / 631 mapped, 0 unmapped in archived specs); the 4 scenarios of this change ran and passed; no `target/` appeared in the worktree. (A first run failed only because test executables were pruned mid-run; rerun clean.)
- [x] 4.2 Corpus gate unchanged (no grouping code changes): medium preset seed 42, the numbers of `preview-placeholders` (precision, recall and keeper accuracy 1.0000, 0 violations).
  Done: release CLI, medium preset seed 42, 10174 files scanned in 18.6 s with 0 errors and no `rawler` line on stderr; `eval-corpus`: precision 1.0000, recall 1.0000, keeper accuracy 1.0000, 0 must-not-merge violations, 19741/19741 pairs, 1903 keeper groups, gate PASS.
- [x] 4.3 Budgets unchanged: `cargo xtask bench --check` (`scan-analysis-12mp`, `rescan-nochange-100k`).
  Done: `cargo xtask bench --check --only scan-analysis-12mp --only rescan-nochange-100k`: 4.50 ms (budget 6.67 ms) and 0.45 s (budget 4 s), all 2 budgets met.
- [x] 4.4 Owner check (read-only, owner-run): a CLI scan of the Canon card prints no lens-database warnings; `RUST_LOG=rawler=warn` brings them back. Done 2026-10-27 on the card mounted read-only, `DCIM/100CANON` (124 files, 2.0 GB) into a scratch catalog under `target/`: default filter, 0 rawler lines (stderr only the 3 progress lines), 0 errors; `RUST_LOG=rawler=warn`, 117 `rawler::lens` warnings. Scratch catalogs removed.
- [x] 4.5 Pushed; GitHub CI green on all five jobs.
  Done: pushed daa11f6..aa66fea; run 40606558280 green on all five jobs (Linux full CI, macOS 14, Windows 2022, oldest libheif, performance budgets).
- [x] 4.6 Gate review by a fresh agent that did not write the code. Done: PASS-WITH-FIXES (MEDIUM: a truncated RAW is reported as "not supported yet", pre-existing, now ROADMAP row 33 `damaged-raw-reason`; LOW: the README and release-notes `RUST_LOG` example hid our own warnings, fixed; LOW: the damaged-file scenario asserts the reason via `previews status`, not the run summary, accepted until row 33; LOW: rawler I/O errors log a backtrace, no action).
