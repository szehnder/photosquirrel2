//! Pause on battery (`general.pause_on_battery`, ADR-0053): the power-source probe and the watcher that pauses a
//! running scan when the machine switches to battery power and offers to resume when mains power returns.
//!
//! [`PowerSource`] is one question, asked every few seconds; [`SystemPower`] answers it per platform without a
//! dependency (sysfs on Linux, IOKit on macOS, kernel32 on Windows) and tests fake it. Unknown means mains, so the
//! setting never pauses a scan by mistake.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use ps_model::{RunStatus, ScanRunId};

use crate::scan::ScanService;

/// Where the machine draws power from.
pub trait PowerSource: Send + Sync {
    /// Running on battery right now (`false` when on mains or unknown).
    fn on_battery(&self) -> bool;
}

/// The platform's power source.
#[derive(Clone, Debug, Default)]
pub struct SystemPower;

impl PowerSource for SystemPower {
    fn on_battery(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            SysfsPower::default().on_battery()
        }
        #[cfg(target_os = "macos")]
        {
            macos::on_battery()
        }
        #[cfg(windows)]
        {
            windows::on_battery()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            false
        }
    }
}

/// Linux `power_supply` class in sysfs (any root, so tests can lay out a fake one on every platform).
#[derive(Clone, Debug)]
pub struct SysfsPower {
    pub root: PathBuf,
}

impl Default for SysfsPower {
    fn default() -> Self {
        Self {
            root: PathBuf::from("/sys/class/power_supply"),
        }
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
}

impl PowerSource for SysfsPower {
    /// On battery when no mains (or USB) supply is online and a battery is discharging.
    fn on_battery(&self) -> bool {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return false;
        };
        let mut mains_online = false;
        let mut discharging = false;
        for e in entries.flatten() {
            let dir = e.path();
            match read_trimmed(&dir.join("type")).as_deref() {
                Some("Mains" | "USB") => {
                    mains_online |= read_trimmed(&dir.join("online")).as_deref() == Some("1");
                }
                Some("Battery") => {
                    discharging |=
                        read_trimmed(&dir.join("status")).as_deref() == Some("Discharging");
                }
                _ => {}
            }
        }
        discharging && !mains_online
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        /// `IOPSGetTimeRemainingEstimate` (IOKit/ps/IOPowerSources.h, macOS 10.7+): seconds of battery left,
        /// `kIOPSTimeRemainingUnknown` (-1) while on battery and estimating, `kIOPSTimeRemainingUnlimited` (-2)
        /// on external power.
        safe fn IOPSGetTimeRemainingEstimate() -> f64;
    }

    const UNLIMITED: f64 = -2.0;

    pub fn on_battery() -> bool {
        // Anything but "unlimited" (an estimate, or unknown while estimating) is battery.
        (IOPSGetTimeRemainingEstimate() - UNLIMITED).abs() > 0.5
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    /// `SYSTEM_POWER_STATUS` (winbase.h).
    #[repr(C)]
    #[derive(Default)]
    #[allow(dead_code)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }

    pub fn on_battery() -> bool {
        let mut status = SystemPowerStatus::default();
        // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS for the duration of the call.
        let ok = unsafe { GetSystemPowerStatus(&mut status) } != 0;
        // ACLineStatus: 0 offline (battery), 1 online, 255 unknown.
        ok && status.ac_line_status == 0
    }
}

/// Why a scan is paused for power, for the dashboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerPause {
    /// The scan was paused because the machine switched to battery power.
    OnBattery(ScanRunId),
    /// It is still paused and mains power is back: offer to resume.
    MainsBack(ScanRunId),
}

impl PowerPause {
    pub fn run(self) -> ScanRunId {
        match self {
            PowerPause::OnBattery(r) | PowerPause::MainsBack(r) => r,
        }
    }

    /// What the dashboard says.
    pub fn message(self) -> &'static str {
        match self {
            PowerPause::OnBattery(_) => {
                "Paused: this computer is on battery power (Settings, General: pause scans on battery)."
            }
            PowerPause::MainsBack(_) => "Mains power is back. Resume the scan?",
        }
    }
}

/// How often the watcher asks the power source.
pub const POLL: Duration = Duration::from_secs(3);

#[derive(Default)]
struct WatchState {
    note: Option<PowerPause>,
    /// The user resumed this run while on battery: leave it running until mains power returns.
    overridden: Option<ScanRunId>,
}

/// The watcher's shared state; the thread holds the engine weakly and stops when the library is dropped.
pub(crate) struct PowerWatch {
    enabled: AtomicBool,
    started: AtomicBool,
    source: Mutex<(Arc<dyn PowerSource>, Duration)>,
    state: Mutex<WatchState>,
}

impl Default for PowerWatch {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            started: AtomicBool::new(false),
            source: Mutex::new((Arc::new(SystemPower), POLL)),
            state: Mutex::new(WatchState::default()),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl PowerWatch {
    /// Use another power source and poll interval (tests). Takes effect for a watcher not yet started.
    pub(crate) fn set_source(&self, source: Arc<dyn PowerSource>, poll: Duration) {
        *lock(&self.source) = (source, poll);
    }

    pub(crate) fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
        if !on {
            *lock(&self.state) = WatchState::default();
        }
    }

    pub(crate) fn note(&self) -> Option<PowerPause> {
        lock(&self.state).note
    }

    /// Start the watcher thread over `engine` once (when enabled).
    pub(crate) fn ensure_started(self: &Arc<Self>, engine: &Arc<ScanService>) {
        if !self.enabled.load(Ordering::SeqCst) || self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let (source, poll) = lock(&self.source).clone();
        let watch = Arc::downgrade(self);
        let engine = Arc::downgrade(engine);
        let spawned = std::thread::Builder::new()
            .name("power-watch".into())
            .spawn(move || run(watch, engine, source, poll));
        if let Err(e) = spawned {
            log::error!("could not start the power watcher: {e}");
            self.started.store(false, Ordering::SeqCst);
        }
    }

    /// One poll: pause a running scan on battery, note when mains power returns.
    fn tick(&self, engine: &ScanService, on_battery: bool) {
        let mut st = lock(&self.state);
        if !self.enabled.load(Ordering::SeqCst) {
            *st = WatchState::default();
            return;
        }
        let snap = engine.snapshot().filter(|_| engine.active().is_some());
        let active = snap.as_ref().map(|s| (s.run, s.status));
        // Forget a note whose run is no longer paused (resumed, cancelled, finished).
        if let Some(note) = st.note
            && active != Some((note.run(), RunStatus::Paused))
        {
            if on_battery && active == Some((note.run(), RunStatus::Running)) {
                st.overridden = Some(note.run());
            }
            st.note = None;
        }
        if !on_battery {
            st.overridden = None;
            if let Some(PowerPause::OnBattery(run)) = st.note {
                st.note = Some(PowerPause::MainsBack(run));
            }
            return;
        }
        if let Some((run, RunStatus::Running)) = active
            && st.overridden != Some(run)
        {
            match engine.pause() {
                Ok(run) => {
                    log::info!("scan run {run} paused: on battery power");
                    st.note = Some(PowerPause::OnBattery(run));
                }
                Err(e) => log::warn!("could not pause the scan on battery: {e}"),
            }
        }
    }
}

fn run(
    watch: Weak<PowerWatch>,
    engine: Weak<ScanService>,
    source: Arc<dyn PowerSource>,
    poll: Duration,
) {
    loop {
        std::thread::sleep(poll);
        let (Some(watch), Some(engine)) = (watch.upgrade(), engine.upgrade()) else {
            return;
        };
        if watch.enabled.load(Ordering::SeqCst) {
            watch.tick(&engine, source.on_battery());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (f, v) in files {
            std::fs::write(dir.join(f), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn sysfs_reports_battery_only_when_discharging_without_mains() {
        let tmp = tempfile::tempdir().unwrap();
        let p = SysfsPower {
            root: tmp.path().to_path_buf(),
        };
        assert!(!p.on_battery(), "no supplies: a desktop, mains");
        supply(
            tmp.path(),
            "BAT0",
            &[("type", "Battery"), ("status", "Discharging")],
        );
        assert!(p.on_battery());
        supply(tmp.path(), "AC", &[("type", "Mains"), ("online", "1")]);
        assert!(!p.on_battery(), "mains online wins");
        supply(tmp.path(), "AC", &[("type", "Mains"), ("online", "0")]);
        assert!(p.on_battery());
        supply(
            tmp.path(),
            "BAT0",
            &[("type", "Battery"), ("status", "Charging")],
        );
        assert!(!p.on_battery());
        let missing = SysfsPower {
            root: tmp.path().join("nope"),
        };
        assert!(!missing.on_battery(), "unknown means mains");
    }

    #[test]
    fn system_power_answers() {
        // Whatever the machine is on, the probe returns without failing.
        let _ = SystemPower.on_battery();
    }
}
