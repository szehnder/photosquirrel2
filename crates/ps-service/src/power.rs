//! Pause on battery (`general.pause_on_battery`, ADR-0053): the power-source probe and the watcher that pauses a
//! running scan when the machine switches to battery power and offers to resume when mains power returns.
//!
//! [`PowerPolicy`] is the rule both the app's watcher thread and the CLI's foreground loop apply (ADR-0072): the
//! app offers to resume on mains power, the CLI resumes by itself.
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

/// What a power poll did, for the CLI's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    /// The running scan was paused because the machine is on battery power.
    PausedOnBattery(ScanRunId),
    /// Mains power is back and the scan the policy paused was resumed ([`ResumeOnMains::Auto`]).
    ResumedOnMains(ScanRunId),
    /// Mains power is back and the scan the policy paused is still paused: offer to resume
    /// ([`ResumeOnMains::Offer`]).
    MainsBack(ScanRunId),
}

/// What happens to a scan paused for battery when mains power returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeOnMains {
    /// The app: note it and let the user decide.
    Offer,
    /// The CLI (ADR-0072): resume it, unless the user has since made the pause theirs.
    Auto,
}

/// The pause-on-battery rule without a thread (ADR-0053, ADR-0072): the app's watcher and the CLI's foreground
/// loop call [`PowerPolicy::tick`] with what the probe said. A running scan is paused on battery; a run the user
/// resumed on battery is left running until mains power returns; on mains, a run the policy paused is offered
/// for resume or resumed, as configured. A pause the user asked for ([`PowerPolicy::user_paused`]) is never
/// resumed by the policy.
#[derive(Debug)]
pub struct PowerPolicy {
    resume: ResumeOnMains,
    note: Option<PowerPause>,
    /// The user resumed this run while on battery: leave it running until mains power returns.
    overridden: Option<ScanRunId>,
    /// The user paused this run: never resume it on mains.
    held: Option<ScanRunId>,
}

impl PowerPolicy {
    pub fn new(resume: ResumeOnMains) -> Self {
        Self {
            resume,
            note: None,
            overridden: None,
            held: None,
        }
    }

    /// A foreground CLI scan's policy (ADR-0072): [`ResumeOnMains::Auto`] when `general.pause_on_battery` is on
    /// and `--ignore-battery` was not given, else none (nothing is probed).
    pub fn for_cli(pause_on_battery: bool, ignore_battery: bool) -> Option<Self> {
        (pause_on_battery && !ignore_battery).then(|| Self::new(ResumeOnMains::Auto))
    }

    /// Why the active scan is paused for power, if it is.
    pub fn note(&self) -> Option<PowerPause> {
        self.note
    }

    /// The user paused `run` (before or during a power pause): mains power must not resume it.
    pub fn user_paused(&mut self, run: ScanRunId) {
        self.held = Some(run);
    }

    /// One poll: pause a running scan on battery; on mains, offer or resume a scan this policy paused.
    pub fn tick(&mut self, engine: &ScanService, on_battery: bool) -> Option<PowerEvent> {
        let snap = engine.snapshot().filter(|_| engine.active().is_some());
        let active = snap.as_ref().map(|s| (s.run, s.status));
        // A user pause ends when its run is no longer paused (resumed, cancelled, finished).
        if let Some(run) = self.held
            && active != Some((run, RunStatus::Paused))
        {
            self.held = None;
        }
        // Forget a note whose run is no longer paused (resumed, cancelled, finished).
        if let Some(note) = self.note
            && active != Some((note.run(), RunStatus::Paused))
        {
            if on_battery && active == Some((note.run(), RunStatus::Running)) {
                self.overridden = Some(note.run());
            }
            self.note = None;
        }
        if !on_battery {
            self.overridden = None;
            let Some(PowerPause::OnBattery(run)) = self.note else {
                return None;
            };
            if self.held == Some(run) {
                // The user made the pause theirs: it is no longer a power pause.
                self.note = None;
                return None;
            }
            return match self.resume {
                ResumeOnMains::Offer => {
                    self.note = Some(PowerPause::MainsBack(run));
                    Some(PowerEvent::MainsBack(run))
                }
                ResumeOnMains::Auto => match engine.resume() {
                    Ok(_) => {
                        log::info!("scan run {run} resumed: mains power is back");
                        self.note = None;
                        Some(PowerEvent::ResumedOnMains(run))
                    }
                    Err(e) => {
                        // Keep the note: the next poll tries again.
                        log::warn!("could not resume the scan on mains power: {e}");
                        None
                    }
                },
            };
        }
        if let Some((run, RunStatus::Running)) = active
            && self.overridden != Some(run)
        {
            match engine.pause() {
                Ok(run) => {
                    log::info!("scan run {run} paused: on battery power");
                    self.note = Some(PowerPause::OnBattery(run));
                    return Some(PowerEvent::PausedOnBattery(run));
                }
                Err(e) => log::warn!("could not pause the scan on battery: {e}"),
            }
        }
        None
    }
}

/// The variable that replaces the platform probe with [`SysfsPower`] over a folder (ADR-0072).
pub const POWER_SUPPLY_DIR_ENV: &str = "PHOTOSQUIRREL_POWER_SUPPLY_DIR";

/// The folder [`probe_from_env`] reads, from the variable's value: `None` when unset or empty.
fn supply_dir(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The power probe for the CLI: [`SysfsPower`] over `PHOTOSQUIRREL_POWER_SUPPLY_DIR` when it is set and not
/// empty (on every platform, so the behaviour is tested without a battery), else [`SystemPower`].
pub fn probe_from_env() -> Arc<dyn PowerSource> {
    match supply_dir(std::env::var_os(POWER_SUPPLY_DIR_ENV)) {
        Some(root) => Arc::new(SysfsPower { root }),
        None => Arc::new(SystemPower),
    }
}

/// The watcher's shared state; the thread holds the engine weakly and stops when the library is dropped.
pub(crate) struct PowerWatch {
    enabled: AtomicBool,
    started: AtomicBool,
    source: Mutex<(Arc<dyn PowerSource>, Duration)>,
    policy: Mutex<PowerPolicy>,
}

impl Default for PowerWatch {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            started: AtomicBool::new(false),
            source: Mutex::new((Arc::new(SystemPower), POLL)),
            policy: Mutex::new(PowerPolicy::new(ResumeOnMains::Offer)),
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
            *lock(&self.policy) = PowerPolicy::new(ResumeOnMains::Offer);
        }
    }

    pub(crate) fn note(&self) -> Option<PowerPause> {
        lock(&self.policy).note()
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

    /// One poll through the shared rule (the app offers to resume, it never resumes by itself).
    fn tick(&self, engine: &ScanService, on_battery: bool) {
        let mut policy = lock(&self.policy);
        if !self.enabled.load(Ordering::SeqCst) {
            *policy = PowerPolicy::new(ResumeOnMains::Offer);
            return;
        }
        policy.tick(engine, on_battery);
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
    fn probe_override_reads_the_given_folder() {
        let tmp = tempfile::tempdir().unwrap();
        supply(
            tmp.path(),
            "BAT0",
            &[("type", "Battery"), ("status", "Discharging")],
        );
        supply(tmp.path(), "AC", &[("type", "Mains"), ("online", "0")]);
        let dir = supply_dir(Some(tmp.path().as_os_str().to_owned())).expect("a folder");
        assert_eq!(dir, tmp.path());
        assert!(
            SysfsPower { root: dir }.on_battery(),
            "BAT0 discharging, no mains online"
        );
        // Unset or empty: the platform probe.
        assert_eq!(supply_dir(None), None);
        assert_eq!(supply_dir(Some(std::ffi::OsString::new())), None);
        // Whatever the variable holds in this process, the probe answers.
        let _ = probe_from_env().on_battery();
    }

    #[test]
    fn system_power_answers() {
        // Whatever the machine is on, the probe returns without failing.
        let _ = SystemPower.on_battery();
    }
}
