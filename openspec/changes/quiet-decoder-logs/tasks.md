# Tasks

Ships in 0.3.x, first of the four (ROADMAP row 29). **No ADR and no migration** (a default log filter is not a decision
of architectural weight); ADR-0112 and migration 26 go to the next changes.

## 1. Filter

- [ ] 1.1 `ps_media::LOG_DEFAULT_FILTER` (`warn,nom_exif=error,rawler=error`) with its doc comment (D1, D2), and the filter unit test. Verify: `scenario_cli_the_default_filter_keeps_errors_and_our_own_warnings`.
- [ ] 1.2 Use it in `photosquirrel-cli/src/main.rs` and `photosquirrel/src/main.rs` (`default_filter_or(LOG_DEFAULT_FILTER)`), replacing both literals. Verify: `grep -rn 'nom_exif=error' crates/*/src/main.rs` finds nothing; `cargo xtask layering` green.

## 2. Tests

- [ ] 2.1 CLI scan of a fixture that reaches rawler's lens lookup, with and without `RUST_LOG` (D3; record whether the `ps-fixtures` CR2 reaches it or the fallback is used). Verify: `scenario_cli_lens_database_warnings_are_hidden_by_default`, `scenario_cli_rust_log_shows_them_again`.
- [ ] 2.2 Truncated CR2 fixture under the default filter. Verify: `scenario_scan_progress_damaged_raw_file_still_reported_under_the_default_filter`.

## 3. Docs

- [ ] 3.1 README "Command line": `RUST_LOG=rawler=warn` shows the RAW decoder's diagnostics for a bug report; the 0.3.x release-notes line. Verify: `grep -n 'RUST_LOG' README.md`.

## 4. Verification

- [ ] 4.1 `cargo xtask ci` green in a clean worktree; every scenario mapped.
- [ ] 4.2 Corpus gate unchanged (no grouping code changes): medium preset seed 42, the numbers of `preview-placeholders` (precision, recall and keeper accuracy 1.0000, 0 violations).
- [ ] 4.3 Budgets unchanged: `cargo xtask bench --check` (`scan-analysis-12mp`, `rescan-nochange-100k`).
- [ ] 4.4 Owner check (read-only, owner-run): a CLI scan of the Canon card prints no lens-database warnings; `RUST_LOG=rawler=warn` brings them back.
- [ ] 4.5 Pushed; GitHub CI green on all five jobs.
- [ ] 4.6 Gate review by a fresh agent that did not write the code.
