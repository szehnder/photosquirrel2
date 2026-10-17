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
| 15 | M5 Release | `performance-budgets` | performance-budgets | all budgets measured; baseline recorded |
| 16 | M5 | `release-packaging` | release-packaging | installers for every platform on a GitHub Release; README with screenshots |
| 17 | M6 Cleanup | `quarantine-cleanup` | cleanup | move + undo round-trip proven on corpus |

## Later (not v1; propose when needed)
`similar-shots` (burst suggestions UI), `justified-layout`, `video-perceptual-dedup`, `face-clusters`,
`quick-rescan-dir-mtime`, `map-view`.
