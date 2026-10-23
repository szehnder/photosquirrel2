# Proposal

## Why

The 0.1.0 release dry run left five small debts that each undermine trust in a release: the AppImage ships a
runtime (with LGPL libfuse inside) that its notices never mention and that packaging fetches from a moving
"continuous" URL; the release keeps only one platform's LGPL source list; a duplicates badge overflows its card in
the README screenshot; two UI timing tests fail about one run in five on a loaded machine; and two small
inconsistencies (a stray toolbar divider, "Why this copy" vs "Next best") are visible to users.

## What Changes

- **AppImage runtime notice (a).** The AppImage type2 runtime is pinned: packaging hands appimagetool a runtime
  file whose SHA-256 is recorded in a pin file, `THIRD-PARTY-LICENSES` gains an "AppImage runtime" section
  (runtime, musl, libfuse, squashfuse, zstd, zlib, plus the AppImageKit `AppRun`), and packaging fails if the
  runtime embedded in the built AppImage, or the list of components it declares, differs from the pin. The
  release attaches the libfuse source the runtime was built from (LGPL-2.1).
- **Per-platform LGPL source lists (b).** Each platform writes `lgpl-sources-<os>.txt`; the release job merges
  their union into `lgpl-sources.txt` and fails if any platform's list differs from the union.
- **Copy-card badge (c).** The derivative badge reads "Resized copy of the keeper" (full wording in its
  tooltip), every chip truncates with an ellipsis inside its container, a headless test proves the badge stays
  inside its card, and the duplicates screenshots are recaptured.
- **Flaky timing tests (d).** Root-cause fixes, not wider thresholds: the dashboard shows "Cancelled" as soon as
  the engine has stopped instead of at the next 100 ms sample, and the frame-cost comparison measures the render
  thread's CPU time with frame-by-frame interleaving, so machine load no longer decides the result.
- **Small fixes (e).** The Library toolbar separates its filter groups by spacing instead of free-standing
  divider lines, so no divider is left alone at a line end when the chips wrap (seen at 1280 px). The keeper
  reasons and the "Next best" line compare the keeper against the same member: the best-ranked copy that is not
  in the keeper set and not byte-identical to the keeper.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `release-packaging`: the AppImage runtime is pinned, noticed and its LGPL source offered; per-platform LGPL
  source lists are merged and compared.
- `duplicate-review`: member badges fit their card; "Why this copy" and "Next best" name the same member.
- `best-copy`: the runner-up the keeper reasons name is the best different copy.
- `scan-progress`: Cancelled is shown when the run stops; the frame-cost comparison is defined on render-thread
  CPU time.
- `gallery`: the toolbar never shows a stray divider when its filters wrap.

## Impact

- ADRs relied on: 0014, 0019, 0026, 0028, 0030, 0058, 0060. New: ADR-0073 (the AppImage runtime is pinned and
  noticed; amends 0060), ADR-0074 (keeper reasons compare against the best different copy; amends 0014).
- Code: `xtask/src/package.rs` (runtime pin, ELF runtime extraction, notices section, per-platform LGPL list),
  new `packaging/linux/appimage-runtime.toml` and `packaging/linux/appimage-runtime/` licence texts,
  `packaging/merge-lgpl-sources.sh`, `.github/workflows/release.yml`, `packaging/release-notes.md`;
  `crates/ps-dedup/src/rank.rs` (runner-up), `crates/ps-dedup/src/group.rs`, `crates/ps-service/src/groups.rs`
  and `review.rs`; `crates/ui-workspace/src/views/{duplicates.rs, review_kit.rs, library.rs, scan.rs}`,
  `crates/ui-workspace/src/scan_backend.rs`, `crates/ps-scan/src/scanner.rs`, `crates/ps-service/src/scan.rs`;
  tests in `crates/ui-workspace/tests/{scan_real.rs, scan_dashboard.rs, duplicates.rs, gallery.rs}`.
- Data: stored group rows gain the runner-up; old rows deserialize without it and are rewritten by the first
  regroup in a new process (`GroupEngine::refresh` regroups everything once per process and writes only
  changed digests).
- Effort: about 5 developer days (a: 2, b: 0.5, c: 0.5, d: 1.5, e: 0.5-1), plus one Docker packaging run and a
  release-workflow dry run.

## Out of scope

- Pinning linuxdeploy and linuxdeploy-plugin-appimage themselves (build tools, not shipped); only what the
  AppImage contains (runtime, `AppRun`) is pinned.
- Building the type2 runtime ourselves, AppImage signing or update information.
- Any change to the 250 ms pause/cancel contract (scan-control) or the 8.3 ms frame budget.
- Running the UI test suite on macOS and Windows CI (today they run only the core and I/O crates).
- Other badge wording in the app beyond the duplicate member cards.
