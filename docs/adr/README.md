# Architecture decision records

One decision per file, never edited once accepted (supersede instead). Template: [template.md](template.md).

| # | Decision |
|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions |
| [0002](0002-rust-and-native-gpui-ui.md) | Rust everywhere, native GPU UI with GPUI (gpui-kit) |
| [0003](0003-layered-cargo-workspace.md) | Layered Cargo workspace |
| [0004](0004-non-destructive-by-construction.md) | Non-destructive by construction |
| [0005](0005-sqlite-catalog-single-writer.md) | SQLite catalog with a single batching writer |
| [0006](0006-source-and-volume-identity.md) | Sources and volume identity |
| [0007](0007-incremental-rescan-by-stat-fingerprint.md) | Incremental rescans by stat fingerprint |
| [0008](0008-resumable-staged-pipeline.md) | Resumable staged pipeline with cooperative cancellation |
| [0009](0009-threaded-pipeline-with-io-lanes.md) | Threaded pipeline with per-volume I/O lanes |
| [0010](0010-single-read-analysis.md) | Probe first, then a single full read per file |
| [0011](0011-content-identity.md) | Content identity: BLAKE3, sampled hashing for large videos |
| [0012](0012-perceptual-hashing-and-index.md) | Perceptual hashes and a near-duplicate index |
| [0013](0013-duplicate-grouping-rules.md) | Duplicate grouping: same image at any size, never bursts |
| [0014](0014-best-copy-ranking.md) | Explainable best-copy ranking |
| [0015](0015-photo-classifier.md) | What counts as a photo: probe-time classifier |
| [0016](0016-thumbnail-store.md) | Thumbnails in a content-addressed SQLite store; embedded previews first |
| [0017](0017-format-support.md) | Formats: JPEG/PNG/TIFF/WebP/GIF native, RAW via rawler, HEIC via libheif, video metadata native, poster frames via ffmpeg |
| [0018](0018-virtualized-gallery-grid.md) | Virtualized gallery grid with a full ID index and a texture LRU |
| [0019](0019-progress-as-coalesced-snapshots.md) | Progress as coalesced snapshots, not per-file events |
| [0020](0020-llm-provider-layer.md) | Own LLM vision provider layer; LM Studio default |
| [0021](0021-llm-advisory-and-private.md) | LLM output is advisory; local by default |
| [0022](0022-export-layout-and-sidecars.md) | Copy-only export: layout, verification, sidecars, manifest |
| [0023](0023-disposition-ledger.md) | Disposition ledger and completion reports |
| [0024](0024-reversible-quarantine-cleanup.md) | Cleanup by reversible quarantine (later phase) |
| [0025](0025-testing-strategy-and-ground-truth-corpus.md) | Testing strategy and a synthetic ground-truth corpus |
| [0026](0026-performance-budgets.md) | Performance budgets as acceptance criteria |
| [0027](0027-service-facade-and-headless-cli.md) | Service facade and a headless CLI as verification surface |
| [0028](0028-design-tokens-and-theme.md) | Design tokens and a photo-first dark theme |
| [0029](0029-no-telemetry.md) | No telemetry |
| [0030](0030-release-packaging.md) | Installers built by GitHub Actions with cargo-packager |
| [0031](0031-secrets-in-os-keychain.md) | API keys in the OS keychain |
| [0032](0032-x86-64-v2-baseline.md) | x86-64-v2 CPU baseline |
