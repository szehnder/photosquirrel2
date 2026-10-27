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
| 24 | M8 Formats | `tiff-support` | media-probe, media-metadata, thumbnails, scanning, content-identity, duplicate-detection, test-corpus, performance-budgets | **corpus gate unchanged**; RAW probes unchanged; every TIFF of the format folder (end directories, 16-bit, float, CMYK, planar, alpha, tiled, multipage, JPEG YCbCr pyramid, up to 1 GB) thumbed with true dimensions and, when it shows the same picture upright, grouped with its JPEG copy; one read per file; streamed TIFF decode memory and decode time within their budgets |
| 25 | M8 | `heic-system-decoders` | decoder-support, media-probe, thumbnails, incremental-rescan, cli, scan-progress, photo-inspector, release-packaging, export, completion-report | **corpus gate unchanged**; no shipped binary contains an HEVC decoder or links libheif; HEIC decodes through ImageIO (macOS) and the system libheif 1.12+ (Linux, real-decoder CI), Windows reports the missing extensions with Store links; AVIF decodes on all three runners with no system library; a rescan after a decoder appears reads only the waiting files once and groups each HEIC with its resized JPEG; **better-copy gate**: an export after a keeper change copies 0 files by default and lists the better copies, and `--upgrade` copies them beside the old copies, which stay byte-identical |
| 26 | M9 Review polish | `excluded-view-filters` | gallery, ai-review, catalog, performance-budgets | **corpus gate unchanged**; old catalog migrated with AI decisions and categories without reading file bytes; Excluded view opens with unsupported files hidden and the hidden count shown, filters to AI-excluded documents, shows them in Grid and List with thumbnails, and puts them back in bulk (they stay after a rescan and AI re-run); `excluded-view-open` within budget on 1M files; screenshot review |
| 27 | M9 | `burst-frame-identity` | media-metadata, duplicate-detection, incremental-rescan, cleanup, test-corpus | **corpus gate at 1.0000 with 0 violations on a re-recorded baseline that includes converted RAW bursts** (CR2 + camera JPEG + Adobe DNG with the first frame's `00` sub-second dropped), which fail the gate without the fix; on the owner's SD card no group mixes two Canon file numbers; an old catalog re-reads only RAW headers once and splits merged frames; a quarantined file whose group changed is pending with a restore hint |
| 28 | M9 | `preview-placeholders` | thumbnails, gallery, photo-inspector, incremental-rescan, cli | **corpus gate unchanged**; no blank tiles: every placeholder drawn with format, reason mark and info button at every tile size, gallery-scroll p95 unchanged; the "Why no preview?" dialog right for each reason; Fill in previews reads only waiting files once (no walk), groups filled-in HEIC with JPEG copies, skips offline sources; a 0.2.0 catalog of TIFF variants offers and completes Fill in previews; screenshot review |
| 29 | 0.3.x | `quiet-decoder-logs` | cli, scan-progress | a scan of a Canon card prints no rawler lens-database warnings by default (`RUST_LOG` still shows them); errors still reach the summary |
| 30 | 0.3.x | `adobe-rgb-color` | thumbnails, photo-inspector | Adobe RGB JPEGs and RAW previews (Canon `_MG_` files, EXIF InteropIndex R03 or an embedded profile) are converted to sRGB for thumbnails and the loupe; sRGB files unchanged; gallery-scroll p95 and thumbnail budgets unchanged |
| 31 | 0.3.x | `stable-ids-on-fat` | incremental-rescan, sources | after unmounting and remounting an exFAT/FAT card or drive, a rescan of unchanged files re-reads none of them (inode numbers are not stable there, ADR-0007); a renamed or replaced file is still detected |

## Later (not v1; propose when needed)
`similar-shots` (burst suggestions UI), `justified-layout`, `video-perceptual-dedup`, `face-clusters`,
`quick-rescan-dir-mtime`, `map-view`.
