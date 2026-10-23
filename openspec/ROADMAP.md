# Roadmap: change order

Implement changes in this order. Each change lives in `openspec/changes/<name>/` until archived.
A milestone gate must pass before the next milestone starts. Gates are checked by a reviewer agent
(see AGENTS.md "Milestone gates").

| # | Milestone | Change | Capabilities (delta specs) | Gate |
|---|---|---|---|---|
| 1 | M0 Foundations | `bootstrap-workspace` | — (tooling) | `cargo xtask ci` green on skeleton crates |
| 2 | M1 Engine | `media-analysis` | media-probe, media-metadata, photo-classification, thumbnails | decoders + classifier unit-tested on fixtures |
| 3 | M1 | `content-hashing` | content-identity, perceptual-hashing | pHash/dHash + MIH index benches within budget |
| 4 | M1 | `ground-truth-corpus` | test-corpus | `photosquirrel-cli fixtures generate` writes corpus + truth.json |
| 5 | M1 | `catalog` | catalog | 50k rows/s insert bench; migrations tested |
| 6 | M1 | `scanning` | sources, scanning, incremental-rescan, scan-control | scan/cancel/resume/rescan integration tests on corpus |
| 7 | M1 | `duplicate-groups` | duplicate-detection, best-copy | **corpus gate: precision >= 0.995, recall >= 0.97, keeper >= 0.99** |
| 8 | M1 | `headless-cli` | cli | every engine feature scriptable with `--json` |
| 9 | M2 UI | `app-shell` | app-shell, theming, settings | window opens, theme tokens enforced, settings persist |
| 10 | M2 | `gallery` | gallery, photo-inspector | **1M-item scroll p95 <= 8.3 ms** |
| 11 | M2 | `scan-dashboard` | scan-progress | live progress, pause/resume/cancel from UI |
| 12 | M2 | `duplicate-review` | duplicate-review | review groups, override keeper |
| 13 | M3 AI | `ai-insights` | llm-providers, ai-queue, ai-review | recorded-fixture tests for each provider; live LM Studio smoke |
| 14 | M4 Export | `export` | export, completion-report | export to temp library, idempotent re-export, hash verify |
| 15 | M4 | `export-hardening` | export | another catalog's library refused as a source |
| 16 | M5 Release | `settings-wiring` | settings | app and CLI scanners built from the same settings; pause on battery |
| 17 | M5 | `performance-budgets` | performance-budgets | all budgets measured; baseline recorded |
| 18 | M5 | `release-packaging` | release-packaging | installers for every platform on a GitHub Release; README with screenshots |
| 19 | M6 Cleanup | `quarantine-cleanup` | cleanup | move + undo round-trip proven on corpus |
| 20 | M7 Polish | `release-hygiene` | release-packaging, duplicate-review, best-copy, scan-progress, gallery | packaging fails on an unpinned or changed AppImage runtime and the notice lists it; release merges per-platform LGPL lists; both timing tests pass 20/20 under load with unchanged bounds; screenshot review shows no badge overflow or stray divider; corpus gate numbers unchanged |
| 21 | M7 | `cli-power-pause` | settings, cli | CLI scan on a fake battery pauses, reports it on stderr and in `--json`, resumes on mains; `--ignore-battery` keeps it running; Ctrl-C while paused exits 130 with a resumable run |
| 22 | M7 | `cleanup-selection` | cleanup | narrowed plan moves only the chosen folders/files and a folder undo restores only that folder (tree snapshots on corpus); discard touches no file and is refused once anything moved; headless UI test of select, undo by folder and discard |
| 23 | M7 | `capture-dates` | media-metadata, export, incremental-rescan, photo-inspector | **corpus gate unchanged**; folder-dated and month-only files filed correctly; local-zone `Undated/` days; old catalog upgraded, rescanned (0 file bytes read for re-dating) and re-exported copies 0 files with an identical library tree |

## Later (not v1; propose when needed)
`similar-shots` (burst suggestions UI), `justified-layout`, `video-perceptual-dedup`, `face-clusters`,
`quick-rescan-dir-mtime`, `map-view`.
