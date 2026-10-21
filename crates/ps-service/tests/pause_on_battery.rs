//! Pause on battery (`general.pause_on_battery`, ADR-0053) through the library facade, with a faked power
//! source.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use ps_model::settings::Settings;
use ps_service::library::Library;
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
    use ps_model::RunStatus;
    use ps_service::power::PowerPause;

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
