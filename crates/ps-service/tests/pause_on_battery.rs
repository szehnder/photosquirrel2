//! Pause on battery (`general.pause_on_battery`, ADR-0053) through the library facade, with a faked power
//! source, and the CLI's policy (ADR-0072) driven as the CLI's foreground loop drives it.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use ps_model::settings::Settings;
use ps_model::{RunStatus, ScanRunId};
use ps_service::library::Library;
use ps_service::power::{PowerEvent, PowerPause, PowerPolicy};
use ps_service::scan::{ScanOptions, ScanService};

/// A power source the test switches.
#[derive(Default)]
struct FakePower(std::sync::atomic::AtomicBool);

impl FakePower {
    fn set(&self, battery: bool) {
        self.0.store(battery, std::sync::atomic::Ordering::SeqCst);
    }
}

impl ps_service::power::PowerSource for FakePower {
    fn on_battery(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Wait (bounded) until `f` holds.
fn eventually(what: &str, bound: Duration, f: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + ps_fixtures::ci_slack(bound);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn status(lib: &Library) -> Option<ps_model::RunStatus> {
    lib.scan().unwrap().snapshot().map(|s| s.status)
}

#[test]
fn scenario_settings_scan_pauses_on_battery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..40u64 {
        std::fs::write(
            root.join(format!("p{i:02}.jpg")),
            ps_fixtures::media::jpeg(320, 240, i, 80),
        )
        .unwrap();
    }
    let data = tmp.path().join("data");
    // One slow reader: the scan runs for seconds.
    let slow = ScanOptions {
        max_readers: 1,
        read_delay: Some(Duration::from_millis(200)),
        ..ScanOptions::default()
    };
    let lib = Library::with_service(&data, ScanService::open(&data, slow).unwrap());
    let power = Arc::new(FakePower::default());
    lib.set_power_source(power.clone(), Duration::from_millis(20));
    let mut settings = Settings::default();
    settings.general.pause_on_battery = true;
    lib.apply_settings(&settings);

    let svc = lib.scan().unwrap();
    let id = svc.add_source(&root).unwrap().source.id;
    let run = svc.start(&[id]).unwrap();
    assert_eq!(status(&lib), Some(RunStatus::Running));

    // Unplugged: the scan pauses within a few seconds and says why.
    power.set(true);
    eventually("paused on battery", Duration::from_secs(3), || {
        status(&lib) == Some(RunStatus::Paused)
    });
    let note = lib.power_pause().expect("a reason");
    assert_eq!(note, PowerPause::OnBattery(run));
    assert!(
        note.message().contains("battery power"),
        "{}",
        note.message()
    );

    // Plugged in again: still paused (the user decides), and the dashboard offers to resume.
    power.set(false);
    eventually("mains offer", Duration::from_secs(3), || {
        lib.power_pause() == Some(PowerPause::MainsBack(run))
    });
    assert_eq!(status(&lib), Some(RunStatus::Paused));
    assert!(lib.power_pause().unwrap().message().contains("Resume"));
    svc.resume().unwrap();
    eventually("note cleared", Duration::from_secs(3), || {
        lib.power_pause().is_none()
    });
    assert_eq!(status(&lib), Some(RunStatus::Running));

    // With the setting off, battery power does not pause.
    settings.general.pause_on_battery = false;
    lib.apply_settings(&settings);
    power.set(true);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(status(&lib), Some(RunStatus::Running));
    assert!(lib.power_pause().is_none());
    svc.cancel().unwrap();
    svc.wait();
}

// ---- The CLI's policy (ADR-0072): the same rule, resuming by itself on mains power. ----

/// A slow scan (one reader, 100 ms per file) of 30 photos, driven by the test thread as the CLI's foreground
/// loop drives it.
struct CliScan {
    _tmp: tempfile::TempDir,
    svc: ScanService,
    run: ScanRunId,
}

fn cli_scan() -> CliScan {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..30u64 {
        std::fs::write(
            root.join(format!("p{i:02}.jpg")),
            ps_fixtures::media::jpeg(320, 240, i, 80),
        )
        .unwrap();
    }
    let data = tmp.path().join("data");
    let slow = ScanOptions {
        max_readers: 1,
        read_delay: Some(Duration::from_millis(100)),
        ..ScanOptions::default()
    };
    let svc = ScanService::open(&data, slow).unwrap();
    let id = svc.add_source(&root).unwrap().source.id;
    let run = svc.start(&[id]).unwrap();
    CliScan {
        _tmp: tmp,
        svc,
        run,
    }
}

impl CliScan {
    fn status(&self) -> Option<RunStatus> {
        self.svc.snapshot().map(|s| s.status)
    }

    fn processed(&self) -> u64 {
        self.svc.snapshot().map_or(0, |s| s.processed)
    }

    /// Tick `policy` every 20 ms for `span`; the events it returned.
    fn ticks(&self, policy: &mut PowerPolicy, on_battery: bool, span: Duration) -> Vec<PowerEvent> {
        let end = std::time::Instant::now() + span;
        let mut events = Vec::new();
        while std::time::Instant::now() < end {
            events.extend(policy.tick(&self.svc, on_battery));
            std::thread::sleep(Duration::from_millis(20));
        }
        events
    }

    /// Wait for the run to finish (no policy ticking); its final status.
    fn finish(&self) -> RunStatus {
        eventually("the run finishes", Duration::from_secs(30), || {
            self.svc.active().is_none()
        });
        self.svc.wait().expect("a summary").status
    }

    /// Pause for battery through `policy`; checks the event and the note.
    fn power_pause(&self, policy: &mut PowerPolicy) {
        assert_eq!(self.status(), Some(RunStatus::Running));
        assert_eq!(
            policy.tick(&self.svc, true),
            Some(PowerEvent::PausedOnBattery(self.run))
        );
        assert_eq!(self.status(), Some(RunStatus::Paused));
        assert_eq!(policy.note(), Some(PowerPause::OnBattery(self.run)));
    }

    fn cancel(&self) {
        let _ = self.svc.cancel();
        self.svc.wait();
    }
}

#[test]
fn scenario_settings_cli_scan_pauses_on_battery() {
    let scan = cli_scan();
    let mut policy = PowerPolicy::for_cli(true, false).expect("setting on: a policy");
    assert!(
        scan.ticks(&mut policy, false, Duration::from_millis(100))
            .is_empty()
    );
    scan.power_pause(&mut policy);
    // The file in flight finishes; after that nothing more is read while on battery, and the run stays active
    // (the command keeps running).
    let events = scan.ticks(&mut policy, true, Duration::from_millis(400));
    let settled = scan.processed();
    let events2 = scan.ticks(&mut policy, true, Duration::from_millis(600));
    assert!(
        events.is_empty() && events2.is_empty(),
        "{events:?} {events2:?}"
    );
    assert_eq!(scan.processed(), settled, "no files read while paused");
    assert_eq!(scan.status(), Some(RunStatus::Paused));
    assert_eq!(scan.svc.active(), Some(scan.run));
    scan.cancel();
}

#[test]
fn scenario_settings_cli_scan_resumes_on_mains_power() {
    let scan = cli_scan();
    let mut policy = PowerPolicy::for_cli(true, false).unwrap();
    scan.power_pause(&mut policy);
    scan.ticks(&mut policy, true, Duration::from_millis(200));
    assert_eq!(
        policy.tick(&scan.svc, false),
        Some(PowerEvent::ResumedOnMains(scan.run)),
        "mains power resumes the run the policy paused"
    );
    assert_eq!(scan.status(), Some(RunStatus::Running));
    assert_eq!(policy.note(), None);
    // The same run completes, with the policy still ticking on mains.
    let deadline = std::time::Instant::now() + ps_fixtures::ci_slack(Duration::from_secs(30));
    while scan.svc.active().is_some() {
        assert!(std::time::Instant::now() < deadline, "the run finishes");
        assert_eq!(policy.tick(&scan.svc, false), None);
        std::thread::sleep(Duration::from_millis(20));
    }
    let summary = scan.svc.wait().unwrap();
    assert_eq!(summary.run, scan.run);
    assert_eq!(summary.status, RunStatus::Completed);
}

/// The CLI's gate. The scenario `CLI setting off does not pause` is checked end to end by the binary test of
/// that name in `crates/photosquirrel-cli/tests/power.rs`.
#[test]
fn cli_policy_off_without_the_setting_or_with_the_flag() {
    // Setting off, or `--ignore-battery`: the CLI has no policy and never probes.
    assert!(PowerPolicy::for_cli(false, false).is_none());
    assert!(PowerPolicy::for_cli(false, true).is_none());
    assert!(PowerPolicy::for_cli(true, true).is_none());
    let scan = cli_scan();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(scan.status(), Some(RunStatus::Running));
    assert_eq!(scan.finish(), RunStatus::Completed);
}

#[test]
fn scenario_settings_cli_user_pause_not_auto_resumed() {
    let scan = cli_scan();
    let mut policy = PowerPolicy::for_cli(true, false).unwrap();
    // Paused for battery, then paused by the user (`scan pause` from another terminal).
    scan.power_pause(&mut policy);
    assert_eq!(scan.svc.pause().unwrap(), scan.run);
    policy.user_paused(scan.run);
    let events = scan.ticks(&mut policy, false, Duration::from_millis(300));
    assert!(
        events.is_empty(),
        "mains power does not resume it: {events:?}"
    );
    assert_eq!(scan.status(), Some(RunStatus::Paused));
    assert_eq!(
        policy.note(),
        None,
        "the pause is the user's, not a power pause"
    );
    // `scan resume` continues it.
    scan.svc.resume().unwrap();
    assert!(
        scan.ticks(&mut policy, false, Duration::from_millis(100))
            .is_empty()
    );
    assert_eq!(scan.status(), Some(RunStatus::Running));

    // Paused by the user before the battery: neither battery nor mains changes it.
    scan.svc.pause().unwrap();
    policy.user_paused(scan.run);
    assert!(
        scan.ticks(&mut policy, true, Duration::from_millis(100))
            .is_empty()
    );
    assert!(
        scan.ticks(&mut policy, false, Duration::from_millis(100))
            .is_empty()
    );
    assert_eq!(scan.status(), Some(RunStatus::Paused));
    scan.cancel();
}

#[test]
fn scenario_settings_cli_resume_on_battery_overrides() {
    let scan = cli_scan();
    let mut policy = PowerPolicy::for_cli(true, false).unwrap();
    scan.power_pause(&mut policy);
    // `scan resume` from another terminal while still on battery: the run keeps going.
    scan.svc.resume().unwrap();
    let events = scan.ticks(&mut policy, true, Duration::from_millis(400));
    assert!(events.is_empty(), "not paused again on battery: {events:?}");
    assert_eq!(scan.status(), Some(RunStatus::Running));
    assert_eq!(policy.note(), None);
    // Mains returns and is lost again: the rule applies afresh.
    assert_eq!(policy.tick(&scan.svc, false), None);
    assert_eq!(
        policy.tick(&scan.svc, true),
        Some(PowerEvent::PausedOnBattery(scan.run))
    );
    assert_eq!(scan.status(), Some(RunStatus::Paused));
    scan.cancel();
}
