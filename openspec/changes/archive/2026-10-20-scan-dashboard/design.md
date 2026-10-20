# Design

## Context

The engine already exposes `ProgressSnapshot` (ADR-0019) and run control (ADR-0008) through `ps-service`
(ADR-0027). This change is presentation plus two small service additions (ETA estimation and run summaries).
Lane behaviour shown in the UI is ADR-0009; diff counts ADR-0007; frame budget ADR-0026; visuals ADR-0028.

## Goals / Non-Goals

**Goals:**
- Rich, calm, information-dense dashboard; constant cost per frame.

**Non-Goals:**
- Historical charts across runs; system notifications.

## Decisions

- **Modules.** `ui-workspace::views::scan` (`ScanDashboard` entity), `ui-workspace::status_progress`,
  `ps-service::scans::{eta, summary, errors}`.
- **Sampling.** A single `cx.spawn` loop per window sleeps 100 ms (timer), takes one snapshot and stores it in the
  dashboard entity; views render from the stored snapshot only. When no run is active the loop stops.
- **ETA.** `ps-service::scans::eta`: exponentially weighted MB/s (alpha for a 10 s half-life) over 1 s samples;
  remaining bytes = discovered-but-unanalyzed candidate bytes from the walk; "estimating" until 10 samples.
  Pure function, unit-tested with synthetic samples.
- **Recent strip.** Reuses `ui-gallery::textures` and the decode scheduler with a small private LRU (64).
- **Simulated runs for tests.** A `FakeScanService` produces snapshots at configured rates so UI tests and the
  constant-cost scenario do not need real I/O.
- **Throughput charts.** Small sparkline per source (last 60 samples) drawn in one paint layer.

## Risks / Trade-offs

- [ETA jitter on mixed HDD/NAS sources] -> per-source ETA, overall = max; smoothing.
- [Folder paths are long] -> middle-ellipsis with full path in a tooltip.

## Verification

- `cargo test -p ui-workspace scan` and `cargo test -p ps-service eta` (all `scenario_scan_progress_*`).
- `cargo xtask screenshot --all-surfaces` (dashboard during a fake run, summary, errors).
- `cargo xtask ci`, `openspec validate scan-dashboard --strict`.
