# Design

## Context

Observed today (read from the code, nothing run):

- The probe and the rule live in `crates/ps-service/src/power.rs`: `PowerSource` (`on_battery()`),
  `SystemPower` (sysfs / IOKit / kernel32, ADR-0053), `SysfsPower { root }`, `PowerPause::{OnBattery, MainsBack}`
  with `message()`, `POLL` = 3 s, and the `pub(crate)` `PowerWatch`, whose `tick(&ScanService, on_battery)` holds
  the rule (pause a `Running` run on battery unless `overridden == Some(run)`; a run seen `Running` again on
  battery after a power pause is marked overridden; mains clears the override and turns `OnBattery` into
  `MainsBack`). `PowerWatch::ensure_started` spawns a `power-watch` thread over a `Weak<ScanService>`; only
  `Library` (`crates/ps-service/src/library.rs`, `power` field, `apply_settings`, `power_pause`,
  `set_power_source`) creates one. The app reaches it through `ui-workspace::scan_backend::power_note`.
- The CLI never builds a `Library`: `cmd::Ctx::service()` opens a bare `ScanService`, and
  `cmd::scan::foreground(ctx, svc, run)` polls every 100 ms (`TICK`): Ctrl-C (`control::interrupted`), requests
  from other terminals (`control::take_request`: pause/resume/cancel), heartbeat (`control::beat`, 250 ms, the
  `ProgressSnapshot`), output (`line("scan","snapshot",..)` every 200 ms with `--json`; a redrawn
  `progress_line` on a terminal; a line every 5 s otherwise). `progress_line` prints `, paused` for any pause.
  So the CLI does not pause on battery at all; the archived `settings-wiring` design says so on purpose.
- `general.pause_on_battery` defaults to off (`ps-model::settings::General`), and the CLI already loads the shared
  settings (`Ctx::settings`), so existing CLI tests (fresh config under `--data-dir`) are unaffected.
- `ScanEngine::resume` is a compare-and-swap from paused to running, so it can never undo a cancel; cancel wakes
  parked threads, so Ctrl-C while paused already works mechanically.

## Goals / Non-Goals

**Goals:**
- One pause rule for app and CLI (ADR-0072); the CLI adds only "resume what I paused when mains returns".
- Every scenario testable without a battery, on all three CI platforms.

**Non-Goals:**
- Changing the app's watcher behaviour or its 3 s poll; pausing `demo`, `ai`, `export` or `cleanup`.

## Decisions

- **Extract the rule into `ps_service::power::PowerPolicy`** (public, no thread): `PowerPolicy::new(resume:
  ResumeOnMains::{Offer, Auto})`, `tick(&mut self, &ScanService, on_battery: bool) -> Option<PowerEvent>`,
  `user_paused(run)`, `note() -> Option<PowerPause>` and `for_cli(pause_on_battery, ignore_battery) ->
  Option<PowerPolicy>` (the CLI's gate: `Auto` only with the setting on and no flag). `user_paused` holds the run
  until it is no longer paused: on mains a held run's power note is dropped (the pause is the user's now) and it is
  not resumed; a held run resumed by the user on battery counts as an override, as without the hold. `PowerEvent::{PausedOnBattery(run),
  ResumedOnMains(run), MainsBack(run)}`. `PowerWatch` keeps its thread and `Mutex<WatchState>` but delegates to a
  `PowerPolicy` with `Offer`, so `scenario_settings_scan_pauses_on_battery` keeps passing unchanged. With `Auto`,
  on mains while the note is `OnBattery(run)` and the run is still `Paused`, it calls `ScanService::resume()` and
  returns `ResumedOnMains`. Alternative considered: give the CLI a `Library` - it would bring the preview cache,
  stores and a second thread for one boolean, and the app's offer-only behaviour.
- **Probe selection: `ps_service::power::probe_from_env() -> Arc<dyn PowerSource>`** returns
  `SysfsPower { root }` when `PHOTOSQUIRREL_POWER_SUPPLY_DIR` is set and non-empty, else `SystemPower` (ADR-0072).
  The CLI uses it; the app keeps `SystemPower` (the env override is a CLI test and support hook, documented in the
  CLI reference; extending it to the app is not needed by any scenario). Using the override logs a warning, and
  so does a folder that is missing (it reads as mains power, so scans never pause); `doctor` shows the probe in
  use, the override folder and whether it exists. The sysfs reader skips anything that is not a regular file (a
  FIFO named `type` would block the poll forever) and reads at most 64 bytes per attribute.
- **CLI wiring in `cmd/scan.rs`.** `start()` reads `ctx.settings()?.general.pause_on_battery` and the new
  `--ignore-battery` flag (on `ScanStartArgs` and `ScanCommand::Resume` in `main.rs`; `scan resume` passes it to
  `start` when no scan is live; when one is, the flag cannot change that scan's rule and a stderr note says so) and passes `Option<PowerGuard>` to `foreground`, where `PowerGuard { policy,
  source, poll: 1 s, last_poll }`. `foreground` ticks it right after the run starts (so a scan started on
  battery pauses at once) and then every second, only while no cancel has been requested (Ctrl-C or a `cancel` request), so a cancel is never followed by a power action. Each
  loop pass reads the probe before it takes the pending control request and applies the reading after it:
  `scan pause` replies at once when the run is already paused for power, so a request queued before the probe
  saw mains return is always applied first and the user's pause wins over the automatic resume. A
  `pause` request calls `policy.user_paused(run)` after `svc.pause()`, so a later mains return does not resume
  it. A `resume` request on battery needs nothing new: the policy already treats a power-paused run seen running
  on battery as overridden. With `--ignore-battery` and the setting on, one stderr note is printed if the probe
  says battery at start ("on battery power: ignoring it (--ignore-battery), the scan does not pause"); with the setting off nothing is
  probed.
- **Output.** On `PausedOnBattery`: stderr (always, also with `--json`, since stderr is for humans and logs)
  `paused: this computer is on battery power (general.pause_on_battery); the scan resumes when mains power
  returns, or run with --ignore-battery`; on a terminal the progress line is cleared first and redrawn after.
  On `ResumedOnMains`: `mains power is back: resuming the scan`. With `--json`, a line
  `{"schema_version":1,"command":"scan","type":"power","data":{"event":"paused_on_battery"|"resumed_on_mains",
  "run":<id>,"at_ms":<unix ms>}}` (struct `PowerLine` in `cmd/scan.rs`), printed before the next snapshot.
  `progress_line` gains a reason: `[analyzing, paused on battery]` when the policy's note is `OnBattery`.
  While the snapshot status is `Paused` (for any reason), JSON snapshots and non-terminal lines are printed at
  most every 5 s (`PAUSED_EVERY`); a paused snapshot carries no new information, and hours of 200 ms lines are
  noise. The terminal redraw is unaffected.
- **`scan status`.** The heartbeat payload becomes `LiveScan { #[serde(flatten)] snapshot: ProgressSnapshot,
  power_pause: Option<String> }` (`"on_battery"` while noted; owned so `status` can read it back), written by
  `control::beat`; `status` reads `control::live::<LiveScan>` and prints the progress line, which reads
  `[<phase>, paused on battery]`; its JSON `live` object gains
  `power_pause`. `control_live` (pause/resume/cancel from another terminal) keeps reading the snapshot fields,
  which flattening leaves where they were.
- **Ctrl-C while paused**: unchanged path (`svc.cancel()`, exit 130, `Interrupted`); a test pins it, because the
  new tick must not resume a cancelled run.

## Risks / Trade-offs

- [The machine flaps between battery and mains] -> one poll per second and a pause/resume per flip; in-flight
  files finish either way (ADR-0008). Acceptable; a hysteresis can come later if users report it.
- [Auto-resume surprises a user who unplugged on purpose] -> the stderr line says how to stop it (`scan pause`
  from another terminal is never auto-resumed; Ctrl-C keeps the run resumable).
- [Probe override abused] -> it only reads a folder of small text files and can only cause or prevent a pause.
- [JSON consumers that assert only snapshot/summary types] -> power lines appear only with the setting on and on
  battery; documented in the CLI reference and ADR-0072.

## Migration Plan

None: the setting already exists and defaults to off; scripts that want the old behaviour pass
`--ignore-battery`.

## Effort

About one day: the policy extraction and its tests (~3 h), CLI wiring, output and heartbeat (~3 h), the binary
tests and README (~2 h), CI (~1 h). No catalog, UI or packaging change.

## Verification

- `cargo test -p ps-service --test pause_on_battery` (the app scenario unchanged, plus the policy's `Auto`
  scenarios on a slow `ScanService` with a fake source).
- `cargo test -p photosquirrel-cli --test power` (binary tests with `PHOTOSQUIRREL_POWER_SUPPLY_DIR` pointing at a
  temp folder holding `BAT0/{type,status}` and `AC/{type,online}`, flipped by rewriting `online`).
- `cargo test -p photosquirrel-cli --test cli scenario_cli_scan_streams_json_snapshots` (unchanged).
- `cargo xtask ci`, `cargo xtask spec-coverage`, `openspec validate cli-power-pause --strict`.
