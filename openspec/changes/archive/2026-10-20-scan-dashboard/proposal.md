# Proposal

## Why

Scans of a NAS or a stack of USB drives take hours. The user wants to watch them like a flight deck: what each
stage and each drive is doing, how fast, how long is left, the photos being found, the errors, and full control
to pause, cancel and resume, without the dashboard itself slowing the scan or the UI.

## What Changes

- Scan dashboard view: start a scan (choose sources), rescan diff summary before analysis, per-stage counts and
  bars, per-source rows (status, lane size, current folder per reader, MB/s, files/s), overall throughput and
  ETA, recent finds strip (last 64 photos), errors list (last 32, plus link to all), run summary on completion.
- Pause, resume and cancel controls; interrupted-run resume; compact progress in the status bar when the
  dashboard is not visible.
- Rendering from coalesced snapshots sampled at most every 100 ms (ADR-0019), constant UI cost.

## Capabilities

### New Capabilities
- `scan-progress`: the live scan dashboard and scan controls in the desktop application.

### Modified Capabilities
- None.

## Impact

- ADRs: 0007, 0008, 0009, 0019, 0026, 0027, 0028.
- Crates: `ui-workspace` (dashboard view, status-bar progress), `ui-gallery` (thumbnail strip reuse),
  `ps-service` (ETA estimator, run summaries, error listing).

## Out of scope

- Scheduling scans, notifications when the app is closed, AI queue progress (`ai-insights`), export progress
  (`export`).
