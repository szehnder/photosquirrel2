# Performance budgets: how to measure

PhotoSquirrel's performance budgets (ADR-0026) are acceptance criteria. `perf-budgets.toml` defines them,
`cargo xtask bench` measures them, and `perf-baseline.json` records the values measured on the reference
machine. This page describes that machine, the procedure, and how to read the output.

## The reference machine

Absolute budgets apply only here (`cargo xtask bench --check` without `--ci`).

| | |
|---|---|
| Model | Laptop, on AC power, power profile `balanced` (`powerprofilesctl get`) |
| CPU | AMD Ryzen 7 4800H: 8 cores / 16 threads, 2.9 GHz base, boost enabled, 4 MB L2, 8 MB L3 |
| RAM | 30 GB (32 GB installed), no swap; `/tmp` is a 16 GB tmpfs |
| Disk | Samsung PM981a 1 TB NVMe SSD (`MZVLB1T0HBLR`), ext4 |
| GPU | AMD Radeon Vega (Renoir, integrated) + NVIDIA GeForce GTX 1660 Ti Mobile |
| Display | 1920x1080 at 144 Hz (frame interval 6.94 ms), KDE Plasma on Wayland |
| OS | Manjaro Linux, kernel 6.6, Rust 1.99.0 |

The machine is also a desktop in daily use, so measure when it is otherwise idle: no other build running, no
indexer or VM busy. Every budget test prints `load average:` so a noisy run is visible in the output.

## Procedure

1. Close other builds (`pgrep -a cargo` prints nothing) and leave the machine on AC power.
2. Warm caches: every test-based source builds or reuses its corpus under the target directory and reads it
   once before timing (the analysis bench reads its 50 distinct 12 MP JPEGs, the rescan bench walks its tree
   once). For a manual run over a folder, read it twice first: `find corpus -type f -exec cat {} + >/dev/null`.
3. Run the full check, from a terminal on the desktop session: app budgets open the release app's window, and
   the gallery render bench needs it visible and focused (Wayland sends no frames to a hidden window).

   ```
   cargo xtask bench --check                      # every budget: absolute + against perf-baseline.json
   cargo xtask bench --check --only cold-start    # one budget (repeat --only for more)
   cargo xtask bench --check --update-baseline    # record: writes the baseline only if the check passed
   cargo xtask bench --check --ci                 # what CI runs: relative regressions only, no windows
   cargo xtask bench --list                       # every budget, its bound, owner and source
   ```

4. Re-record the baseline only with a commit whose message says why (new reference machine, an intended
   trade-off, a faster implementation). Never loosen a budget to pass; a missed budget reopens the owning
   change, or a new ADR amends the budget with measurements (as ADR-0050 did).

## How to read the output

Each budget prints one line while it runs:

```
    scan-analysis-12mp                 4.60 ms   (budget 6.67 ms, 10 s)
```

the measured value in the budget's unit, the bound, and how long the measurement took. Then every failure is
listed as `FAIL <budget>: ...`:

- `... exceeds the budget of ...`: above the absolute bound (local mode only).
- `... is N% worse than the baseline ...`: more than 15% above `perf-baseline.json`, and above it by more than 5% of the budget's bound (both modes; ADR-0057).
- `... no measurement ...`: the source printed no `measure <budget>=<value> <unit>` line (a test that failed
  before measuring, or an app that did not draw its first frame). This always fails.

All budgets are lower-is-better; throughputs are stored as time per unit (catalog insert: 20 us per row =
50,000 rows/s; analysis: 6.67 ms of wall time per file = 150 files/s on 16 threads).

Sources (`source.kind` in `perf-budgets.toml`):

- `test`: an integration test run with `cargo test --release ... -- --exact <name>`, printing a `measure` line.
  The test also asserts its own budget in optimized builds, so `cargo test --release` catches a miss too.
- `app`: the release app with a measurement flag, run `runs` times on a fresh data directory; the median
  (or the maximum, for `metric = "max"`) of its `measure` lines. The 1M-item synthetic catalog used by
  `idle-rss` and `first-gallery-frame` is built once into the temp folder.
- `criterion`: a criterion benchmark's median (none at the moment; criterion benches stay for exploration).

## CI

The `bench-regressions` job in `.github/workflows/ci.yml` runs after the full Linux CI job. Runners are not the
reference machine and differ from one another (the same commit measured 17 ms and 54 ms for
`near-dup-index-build` on two runs), so the job first measures the base commit (the pull request's base, or the
previous head of main) on the same runner with `cargo xtask bench --ci --update-baseline --best-of 3`, then
checks the head with `cargo xtask bench --check --ci --baseline perf-baseline-ci.json`: relative regressions above
15% only, no app (window) budgets, and a suspected regression re-measured twice with the best run counted (the
base gets the same best-of-three, so a noisy-high base does not hide a regression). A missing or unreadable base
file fails the job; a budget the base has no value for (one new in the head) is listed as a warning. Before the
head is built, every tracked file is touched, so `include_str!` inputs are rebuilt too. The committed
`perf-baseline.json` is never compared with runner numbers.

`control-latency-p95` sets `ci_runs = 5` (ADR-0078). In CI it is measured five times for the base and five for the
head, and the **medians** are compared, not the best runs; a suspected regression measures the head five more times
and the median of all ten counts. Its single runs vary by about 25% on runners: the p95 of 20 alternating
pause/cancel trials is, in effect, the ninth of ten cancel latencies, whose teardown (durable catalog flushes, thread
joins) varies most. With best-of-three, one low base run failed identical code about one run in twenty. The base file
records each run under `samples`; a base without them (measured by an older xtask) is compared by its best run, with
a warning.

Absolute numbers on a runner mean little, and the job does not fail on them. In run 39965164278,
`scan-analysis-12mp` measured 19.0 ms against the 6.67 ms bound because that bound is for 16 hardware threads
(150 files/s) and a runner has 4: 4.83 ms x 16 / 4 = 19.3 ms, the same for base and head. The budget test asserts
its bound in release builds and fails there, so the job logs `(... failed)` and still uses the printed measurement.
Only the relative check (and a missing measurement) fails the job. The same run's `replan_50k_exported_within_budget
failed` once at 502 ms because the test still asserted ADR-0044's 500 ms after ADR-0050 raised the budget to 800 ms;
the test now asserts 800 ms, and an xtask unit test keeps every `budget(...)` bound in a budget test equal to
`perf-budgets.toml`.

## Budgets

| Budget | Bound | ADR-0026 | Owner | Reference value |
|---|---|---|---|---|
| `phash-dhash-per-image` | median <= 0.3 ms | yes | content-hashing | 0.1504 ms |
| `near-dup-lookup-p95` | p95 <= 1 ms | yes | content-hashing | 0.0151 ms |
| `near-dup-index-build` | min <= 1000 ms | component | content-hashing | 139.9 ms |
| `analysis-decode-thumb` | median <= 100 ms | component | media-analysis | 37.46 ms |
| `catalog-insert` | mean <= 20 us | yes | catalog | 14.797 us |
| `catalog-id-list-1m` | median <= 300 ms | yes | catalog | 127.425 ms |
| `cold-start` | median <= 400 ms | yes | app-shell | 258.9 ms |
| `idle-rss` | max <= 250 MB | yes | app-shell | 209.6 MB |
| `scan-analysis-12mp` | mean <= 6.67 ms | yes | scanning | 4.833 ms |
| `rescan-nochange-100k` | median <= 4 s | yes | scanning | 0.408 s |
| `control-latency-p95` | p95 <= 250 ms | yes | scanning | 106.92 ms |
| `dedup-regroup-1k-of-1m` | max <= 250 ms | component | duplicate-groups | 27.884 ms |
| `gallery-scroll-p95` | p95 <= 8.3 ms | yes | gallery | 7.85 ms |
| `first-gallery-frame` | median <= 600 ms | yes | gallery | 568 ms |
| `export-replan-50k` | median <= 800 ms | component | export | 547.045 ms |
| `tiff-analysis-decode` | median <= 1000 ms | component | tiff-support | 319.56 ms |
| `tiff-stream-memory` | max <= 4 MB | component | tiff-support | 0.94 MB |

Recorded 2026-10-21 with `cargo xtask bench --check --update-baseline` (balanced power profile, on AC, load
average below 2). Repeated runs on this machine vary by up to about 60% for the sub-millisecond hashing
budgets and 10-25% for the I/O-bound ones (catalog, export re-plan), so compare a regression with a second
run before acting on it; CI re-measures a suspected regression twice and keeps the best run.

The two TIFF budgets were recorded 2026-10-24 on the same machine with `cargo xtask bench --check
--update-baseline --only tiff-analysis-decode --only tiff-stream-memory` (ADR-0087). `tiff-stream-memory` is the
decoder's own accounting (its largest strip, compressed plus decoded, plus the downsampling accumulator), not the
resident size of a process: it is deterministic, but allocator overhead and the `tiff` crate's internal buffers
are not in it. The process-level figure is the peak resident size quoted below.

### Large TIFF scans (ADR-0084, ADR-0086)

TIFFs whose directories follow their pixel data cost no extra read up to 64 MB: the scanner reads them in full
before classifying them, as it would anyway. Above 64 MB the probe adds a few targeted directory reads (at most
1 MiB, usually one or two reads of a few KB), and the file is hashed and decoded in one sequential pass in its I/O
lane, holding one strip plus the downsampled image; the lane reserves one 64 MB buffer for it. Measured on the
reference machine (release build, file in the page cache): a 979 MB 14000x10000 16-bit LZW scan with its directory
at the end took 8.0 s to scan alone, almost all of it LZW decompression, with a peak resident size of 25 MB for
the whole CLI process. A format folder of 22 files (16 TIFFs of 0.8-979 MB in every layout, 5 HEIC/AVIF, one
JPEG; 1.6 GB) scanned in 9.4 s.

A TIFF decode is also bounded in work (ADR-0088): a directory whose strips or tiles decode to more than 4 GiB, or
claim more than 4 GiB of compressed data (a block shared by several chunks counted for each), is refused from its
header before any chunk is read, so a small file declaring gigapixels costs microseconds; at the cap a decode takes
about 18 s of one core. Pause and cancel are checked before every strip or tile, on CPU workers and in the one-pass
lane decode alike.

### Re-dating an existing catalog (ADR-0071)

When the capture-date rules change, the first rescan of each source re-dates its unchanged rows from stored facts
(path, modification time, metadata JSON) without reading any file. It is not a budget: it runs once per rule
change, chunked and resumable. Measured on the reference machine (release build,
`cargo test --release -p ps-scan --test redate_scale -- --ignored --nocapture`): a synthetic catalog of 1,000,000
rows, 30% of them dated only by their file time, re-dates its 300,000 stale rows in 14.9 s with 20 tags per row
(every date changed, Los Angeles zone), on top of 3.2 s for the diff's page reads; the no-change rescan of 100k
files stayed at 0.43 s. With a realistic 200 tags per row (real JPEG and RAW files list 150-300; a 13 GB
catalog) the same pass takes 85-110 s, on top of 73-81 s for the diff's page reads: reading the date facts is
13-17 s of it (`Readers::date_facts` deserialises only the date fields and the date tags; for rows in the page
cache it reads 60,000 rows in 1.6 s against 2.6 s for the whole metadata JSON), the rest is SQLite rewriting
each updated row with its large metadata column. All 1M rows stale would take about 5 minutes, once. The opt-in analysis refresh of rows from before catalog
migration 13 reads up to 256 KB per row: minutes on an SSD, hours for 1M rows on a USB hard drive or a NAS.

`gallery-scroll-p95` needs a visible window: Wayland sends no frames to the render bench while the desktop
session is locked (it reports "frames stalled" and prints no measurement, which fails the check). It was
recorded separately from an unlocked session on 2026-10-21 with
`cargo xtask bench --check --update-baseline --only gallery-scroll-p95` (7.85 ms against 8.3 ms).
