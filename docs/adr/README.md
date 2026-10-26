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
| [0033](0033-user-separation-of-group-members.md) | Users can split members out of a duplicate group |
| [0034](0034-quarantined-disposition.md) | A `quarantined` disposition |
| [0035](0035-burst-protection-in-grouping.md) | Stricter capture-time rule for derivative edges (burst protection) |
| [0036](0036-previews-for-llm-and-component-budgets.md) | LLM previews and component budgets |
| [0037](0037-raw-jpeg-pairs-exempt-from-two-originals-rule.md) | RAW+JPEG pairs are exempt from the two-camera-originals rule |
| [0038](0038-no-overwrite-rename.md) | No-overwrite atomic rename for export (rustix) |
| [0039](0039-contradictions-are-group-level-cannot-link-constraints.md) | Contradictions are group-level cannot-link constraints |
| [0040](0040-post-rename-verification-and-suspect-ledger-entries.md) | Post-rename verification and `suspect` ledger entries |
| [0041](0041-exclusive-catalog-owner-lock.md) | One owner process per catalog: an exclusive OS lock |
| [0042](0042-interim-json-review-stores.md) | Interim JSON stores for review state and AI corrections |
| [0043](0043-group-capture-date-for-export-filing.md) | Export files a keeper by the best capture date its duplicate group knows |
| [0044](0044-export-replan-trusts-the-ledger.md) | An export re-plan trusts the ledger: size and modification time, no re-hash (budget amended by 0050) |
| [0045](0045-sidecars-in-the-export-ledger.md) | XMP sidecars have export ledger rows |
| [0046](0046-library-copies-changed-by-other-apps.md) | A library copy changed by another app stays the export of its photo |
| [0047](0047-hosted-consent-bound-to-provider-and-host.md) | Hosted AI consent is bound to a provider and a host; no redirects |
| [0048](0048-consent-binds-the-scheme-hosted-needs-https.md) | Hosted consent binds the scheme; hosted providers need HTTPS (extends 0047) |
| [0049](0049-settings-file-ownership-and-external-edits.md) | The running app owns `settings.toml`; external consent withdrawals win (extends 0047) |
| [0050](0050-export-replan-budget-with-sidecar-rows.md) | The export re-plan budget with sidecar ledger rows: 800 ms (amends 0044) |
| [0051](0051-reclassification-from-stored-probe-facts.md) | Re-classification from stored probe facts (catalog migration 13) |
| [0052](0052-hidden-folders-scanned-by-default.md) | Hidden folders are scanned by default |
| [0053](0053-power-source-probe-without-dependencies.md) | Power source probe: platform calls, no dependency |
| [0054](0054-dct-scaled-jpeg-decodes-via-mozjpeg.md) | DCT-scaled JPEG decodes via mozjpeg (libjpeg-turbo core, plain C; amends 0017) |
| [0055](0055-corrupt-jpegs-fall-back-to-jpeg-decoder.md) | Corrupt or truncated JPEGs leave the mozjpeg path for jpeg-decoder (amends 0054) |
| [0056](0056-progressive-jpeg-coefficient-memory.md) | Progressive JPEG coefficient memory: serialize large decodes, refuse hostile ones |
| [0057](0057-regression-noise-floor.md) | Regressions must also exceed 5% of the budget's bound (amends 0026) |
| [0058](0058-third-party-licence-notices-via-cargo-about.md) | Third-party licence notices generated by cargo-about |
| [0059](0059-release-packages-build-heic-only-with-a-shippable-libheif.md) | Release packages build HEIC only where libheif is found and new enough (amends 0017, 0030) (superseded by 0089) |
| [0060](0060-packages-redistribute-only-what-they-license.md) | Release packages redistribute only what their notices and source offers cover (amends 0058) |
| [0061](0061-cleanup-journal-and-quarantine-moves.md) | Cleanup journal, catalog migration 14 and quarantine moves |
| [0062](0062-quarantine-moves-never-check-then-rename.md) | Quarantine moves never check-then-rename: MoveFileExW, link+unlink or refuse (amends 0061) |
| [0063](0063-cleanup-runs-know-their-source-by-identity.md) | Cleanup runs know their source by identity; quarantined files match by path (migration 15; amends 0061) |
| [0064](0064-durable-cleanup-journal-and-journal-file-reconciliation.md) | The cleanup journal's catalog half is durable; reconciliation reads journal.jsonl and hashes (amends 0061) |
| [0065](0065-one-file-changing-operation-at-a-time.md) | One file-changing operation (scan, export, cleanup) at a time per catalog |
| [0066](0066-quarantined-without-library-copy-is-pending.md) | A quarantined file without a valid library copy is pending; export reads it from quarantine (amends 0034) |
| [0067](0067-excluded-quarantined-files-need-a-library-copy-too.md) | An excluded quarantined file needs a valid library copy too (amends 0066) |
| [0068](0068-narrowed-and-discarded-cleanup-runs.md) | Cleanup plans are narrowed by marking moves unselected and discarded by state; no cleanup row is deleted (migration 16; amends 0061) |
| [0069](0069-capture-dates-from-folder-names.md) | Capture dates from folder names, with honest precision (amends 0022, 0043) |
| [0070](0070-day-of-a-time-instant-in-the-local-zone.md) | The day of a time instant is taken in the recorded offset, else the local zone (amends 0022) |
| [0071](0071-date-logic-version-and-existing-libraries.md) | A date-logic version re-dates catalog rows from stored facts; exported libraries keep their layout (migration 17) |
| [0072](0072-cli-scans-pause-on-battery-and-resume-on-mains.md) | CLI scans pause on battery and resume on mains power (amends 0053) |
| [0073](0073-pinned-appimage-runtime-with-its-own-notice.md) | The AppImage runtime is pinned and has its own notice (amends 0060) |
| [0074](0074-keeper-reasons-compare-against-the-best-different-copy.md) | Keeper reasons compare against the best different copy (amends 0014) |
| [0075](0075-wall-clock-budget-tests-run-alone.md) | Wall-clock budget tests run alone, in a timing pass after the other tests |
| [0076](0076-timing-pass-shares-the-main-build-and-covers-every-crate.md) | The timing pass shares the main pass's build and covers every crate (amends 0075) |
| [0077](0077-appimage-runtime-relinking-sources-attached.md) | The release attaches the AppImage runtime's relinking sources, all byte-stable (amends 0073) |
| [0078](0078-control-latency-compared-by-medians-in-ci.md) | CI compares control latency by the median of five runs, not the best of three (amends 0057) |
| [0079](0079-month-folder-plausibility-against-the-start-of-the-month.md) | A month folder's plausibility check compares against the start of the month (amends 0069) |
| [0080](0080-two-year-range-folders-are-not-month-dates.md) | Folders named for a range of two years are not month dates (amends 0069) |
| [0081](0081-keyed-quicktime-creation-dates-are-wall-clocks.md) | A keyed QuickTime creation date is a wall clock, also at +00:00 (amends 0070) |
| [0082](0082-a-scan-run-remembers-its-analysis-refresh.md) | A scan run remembers its analysis refresh (migration 18; amends 0071) |
| [0083](0083-a-two-year-range-folder-needs-evidence.md) | A folder is read as a range of two years only with text after it, outside its year (amends 0080) |
| [0084](0084-tiff-directories-beyond-the-probe-window.md) | TIFF directories beyond the probe window: from the analysis buffer, else bounded targeted reads (amends 0010) |
| [0085](0085-strip-wise-tiff-decoding.md) | TIFF stills decode strip by strip through the `tiff` crate, averaged down while decoding (amends 0017) |
| [0086](0086-one-pass-analysis-of-large-tiffs.md) | TIFFs above 64 MB are analysed in one sequential pass in their I/O lane (amends 0010, 0011) |
| [0087](0087-tiff-decode-budgets.md) | TIFF decode budgets: 12 MP analysis time and streamed working memory (extends 0026) |
| [0088](0088-tiff-decode-work-cap-and-stop-check.md) | A TIFF decode is capped at 4 GiB of work and checks pause and cancel before every strip or tile (amends 0085, 0086) |
| [0089](0089-heic-through-the-operating-systems-decoder.md) | HEIC through the operating system's decoder; no bundled HEVC decoder (supersedes 0059; amends 0017, 0030) |
| [0090](0090-avif-decoded-by-a-bundled-rav1d.md) | AVIF decoded by a bundled rav1d (amends 0017) |
| [0091](0091-rows-remember-a-missing-decoder.md) | Rows remember a missing decoder; rescans re-decode them (migration 19; amends 0007) |
| [0092](0092-a-group-already-in-the-library-is-not-copied-again.md) | A group already in the library is not copied again when its keeper changes; better copies are offered (amends 0023, 0071) |
| [0093](0093-a-better-copy-not-in-the-library-is-superseded.md) | A better copy not in the library is superseded, not covered: not accounted for, never cleaned up (amends 0092, 0024, 0023) |
| [0094](0094-avif-decoding-as-built-two-frame-contexts-and-a-watchdog.md) | AVIF decoding as built: two frame contexts, panics in rav1d abort, an OBU pre-check, fuzzing and a watchdog (amends 0090) |
| [0095](0095-heic-and-avif-decode-failures-keep-the-file.md) | HEIC and AVIF decode failures keep the file; decode memory is budgeted; libheif's size limit by version (amends 0091, 0089, 0009) |
| [0096](0096-cleanup-keeps-copies-better-than-a-chosen-keeper-and-rechecks-plans.md) | Cleanup keeps copies better than a keeper you chose, and re-checks a stored plan when it runs (amends 0093, 0024) |
| [0097](0097-avif-decodes-hold-their-budget-and-stop-on-cancel.md) | AVIF decodes stop waiting on cancel and hold their memory until their thread ends; the decode budget is FIFO (amends 0094, 0095) |
| [0098](0098-decode-failures-are-retried-when-transient-or-after-a-decoder-change.md) | HEIC and AVIF decode failures are retried when transient or after a decoder change (migration 20; amends 0091, 0095) |
| [0099](0099-cleanup-runs-recheck-when-the-catalog-changes-and-edits-wait.md) | A cleanup run re-checks its files when the catalog changes under it; keeper, group and exclusion edits wait while it runs (amends 0096, 0065) |
| [0100](0100-refused-avif-decodes-use-no-attempt-and-one-decode-budget-per-scanner.md) | A refused AVIF decode uses up no attempt; one decode budget serves every run of a scanner (amends 0098, 0097) |
| [0101](0101-fixture-file-operations-are-test-only.md) | Fixture file operations are test-only: the `scratch` cargo feature and a layering rule (amends 0004) |
