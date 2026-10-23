# PhotoSquirrel: guide for agents

PhotoSquirrel is a native desktop app (Rust, GPUI via `gpui-kit`) that scans drives, USB backups and NAS shares
full of photos and videos, finds the **best copy of every photo** across exact duplicates, thumbnails, resized and
recompressed versions, shows EXIF and optional LLM insights, and **copies** the deduplicated best copies into a
library folder ready for Apple Photos, Google Photos or Lightroom.

## The owner's priorities (do not reorder)
1. **Never lose a photo. Non-destructive copy-out first** (ADR-0004). Sources are read-only; export copies,
   verifies the hash, then renames. Nothing ever deletes a user file.
2. **Efficient rescans and resume**: remember what was seen, diff by stat fingerprint, resume interrupted scans,
   pause/cancel quickly (ADR-0007, ADR-0008).
3. **Optional reversible cleanup** of already-accounted-for files via quarantine + undo journal (ADR-0024).
Also: photos (people, our son, pets, travel) matter; icons/graphics/caches do not (ADR-0015). Dedup = "same image at
any size"; bursts are suggestions only (ADR-0013). Performance and polish are features ("Lightroom on steroids").
Scale: 100k-1M+ files on mixed SSD/HDD/NAS.

## Where things are
- `docs/adr/`: architecture decisions, one per file. **Read the index before changing architecture.** Never edit an
  accepted ADR; write a new one that supersedes it.
- `openspec/`: spec-driven development. `specs/` is the current truth (filled as changes are archived).
  `changes/<name>/` holds in-flight work (proposal, design, tasks, delta specs). `ROADMAP.md` gives the order.
- `crates/` (layering in ADR-0003, enforced by `cargo xtask layering`):
  - core (no tokio, no gpui-kit): `ps-model`, `ps-media`, `ps-hash`, `ps-dedup`, `ps-fixtures`
  - I/O (no gpui-kit): `ps-catalog`, `ps-scan`, `ps-llm`, `ps-export`, `ps-service` (the facade), `photosquirrel-cli`
  - UI: `ui-theme` (tokens), `ui-gallery` (virtualized grid), `ui-workspace` (shell, panels), `photosquirrel` (binary)
- `xtask/`: `cargo xtask <ci|doctor|bench|timing|screenshot|package|spec-coverage|layering|tokens>`.
- `packaging/`: Linux packaging Docker image, desktop file, release notes template (ADR-0030).
- `docs/gpui-notes.md`: hard-won GPUI API knowledge. Read it before UI work; add to it when an API surprises you.

## Workflow
1. Pick the next unarchived change from `openspec/ROADMAP.md`.
2. `/opsx:apply <change>`: work through `tasks.md` top to bottom, ticking each box only after its verification passes.
3. Run `cargo xtask ci` (fmt, clippy -D warnings, layering, design tokens, tests, `openspec validate --all --strict`,
   spec coverage).
4. `/opsx:archive <change>` once CI is green and every scenario maps to a test.
5. Commit in meaningful chunks: one commit per coherent step (spec, then implementation slices, then archive),
   never a giant mixed commit. Messages say what and why.

## Milestone gates
Each milestone in the roadmap ends with a gate checked by a fresh reviewer agent that did not write the code:
`cargo xtask ci` green; every scenario mapped to a passing test; budgets in `perf-budgets.toml` for the milestone
measured; for M1, the ground-truth corpus gate (ADR-0025: precision >= 0.995, recall >= 0.97, keeper accuracy
>= 0.99, zero must-not-merge violations) via `photosquirrel-cli eval-corpus`; for UI milestones, a screenshot review.

## Rules
- **Safety:** only `ps-export` may create, rename or move files under user paths, and only by copy-verify-rename or
  journaled quarantine moves. The scanner opens files read-only. There is no delete code path. Keep it that way.
- **LLM output is advisory** (ADR-0021): it never decides grouping, keepers, export or exclusion by itself.
- **Layering:** core crates must not depend on tokio or gpui-kit; I/O crates must not depend on gpui-kit.
- **Never block the UI thread.** Catalog queries, decoding and network go to background executors/threads.
- **No per-file events to the UI**; progress is sampled snapshots (ADR-0019).
- **No raw colours or pixel sizes in UI code;** use `ui-theme` tokens (ADR-0028, `cargo xtask tokens`).
- **Scenario traceability:** every `#### Scenario:` in a spec has a test named `scenario_<capability>_<snake_slug>`.
- **New dependency of consequence, file format, crate boundary or perf budget -> new ADR.**
- Tests never touch real user folders; use `ps-fixtures` corpora in temp dirs. LLM tests use recorded fixtures
  (`PHOTOSQUIRREL_LIVE_LLM=1` for live LM Studio tests).
- Prefer verifying through `photosquirrel-cli` (ADR-0027) and headless `#[gpui_kit::test]` UI tests. GUI launches
  pop windows on the developer's desktop; use `cargo xtask screenshot` for visual checks.

## Machine resources
The dev machine has 30 GB RAM, a 16 GB RAM-backed `/tmp` and a shared disk. GPUI builds are large (20-70 GB per
target dir). At most two agents compile at once, each with `CARGO_BUILD_JOBS=4` and `--test-threads=4`. Shared
test corpora live under `CARGO_TARGET_TMPDIR`; other temp data goes in a `tempfile` dir that is removed on drop.
`cargo xtask ci` and `cargo xtask bench` create `<target dir>/tmp` and point TMPDIR/TMP/TEMP at it for the cargo
they run, so even leaks land on disk; when running `cargo test` by hand, set `TMPDIR` to an existing folder under
`target/`. Never leave directories in `/tmp`. Delete per-agent target dirs once their work is committed.

## Useful commands
```
cargo xtask doctor                         # prerequisites (libheif, ffmpeg, LM Studio are optional)
cargo xtask ci                             # everything CI runs (the timing pass last)
cargo xtask timing --runs 20               # only the wall-clock budget tests, alone (ADR-0075)
cargo run --release -p photosquirrel       # the app
cargo run --release -p photosquirrel-cli -- fixtures generate --out corpus --seed 7
cargo run --release -p photosquirrel-cli -- scan corpus && cargo run -p photosquirrel-cli -- report
cargo xtask bench --check                  # performance budgets (ADR-0026)
cargo xtask screenshot -- --theme dark     # capture the app window to a PNG
cargo xtask package --docker               # Linux installers into target/packages
openspec list && openspec validate --all --strict
```
