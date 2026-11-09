//! Update checks for the app (spec `app-updates`; ADR-0124, ADR-0126).
//!
//! [`UpdateService`] decides when to check, download and offer an update, and keeps one coalesced
//! [`UpdateStatus`] snapshot for the UI (ADR-0019). The work happens in [`UpdateService::tick`], called by the
//! service's own thread every minute with the wall clock, or by tests with any time they like. Nothing is ever
//! requested while the mode is undecided or off, before 60 s after start, or more than once per 24 h (plus the
//! jitter drawn at the last check) across restarts.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ps_model::settings::UpdateMode;
use ps_update::check::{Outcome, Updater};
use ps_update::install;
use ps_update::kind::Probe;
use ps_update::kind::{Installation, Kind};
use ps_update::state::{CheckRecord, Failure, FailureReason, UpdateState};
use ps_update::{UpdateDir, UpdateError, history};

/// No request before this many seconds after start ("Quiet start").
pub const FIRST_CHECK_AFTER_S: i64 = 60;
/// Scheduled checks are at least a day apart, plus the jitter drawn at the last check.
pub const CHECK_INTERVAL_S: i64 = 24 * 60 * 60;
/// Failed scheduled checks stay silent until they have failed on three days in a row.
pub const QUIET_FAILURES_S: i64 = 2 * 24 * 60 * 60;
/// How often the service's thread wakes to compare the clock.
pub const TICK: Duration = Duration::from_secs(60);

/// The current update mode, read from the app's settings in memory.
pub type ModeSource = Arc<dyn Fn() -> UpdateMode + Send + Sync>;

/// What file-changing operation runs now (ADR-0065), in words for "Available when <it> finishes": `"the scan"`,
/// `"the export"`, `"the cleanup"`; `None` when none does.
pub type BusyProbe = Arc<dyn Fn() -> Option<&'static str> + Send + Sync>;

/// A mode the app's settings write and the service reads, from any thread.
#[derive(Debug, Clone, Default)]
pub struct ModeCell(Arc<std::sync::atomic::AtomicU8>);

impl ModeCell {
    pub fn new(mode: UpdateMode) -> Self {
        let cell = Self::default();
        cell.set(mode);
        cell
    }

    pub fn set(&self, mode: UpdateMode) {
        let v = match mode {
            UpdateMode::Undecided => 0,
            UpdateMode::Automatic => 1,
            UpdateMode::Notify => 2,
            UpdateMode::Off => 3,
        };
        self.0.store(v, Ordering::Relaxed);
    }

    pub fn get(&self) -> UpdateMode {
        match self.0.load(Ordering::Relaxed) {
            1 => UpdateMode::Automatic,
            2 => UpdateMode::Notify,
            3 => UpdateMode::Off,
            _ => UpdateMode::Undecided,
        }
    }

    pub fn source(&self) -> ModeSource {
        let me = self.clone();
        Arc::new(move || me.get())
    }
}

/// What the UI shows about updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub mode: UpdateMode,
    pub installed: String,
    pub kind: Kind,
    /// Why this installation cannot install updates itself, or `None`.
    pub cannot_install: Option<String>,
    pub last_check: Option<CheckRecord>,
    pub last_success_at: Option<i64>,
    /// A newer version, verified and offered.
    pub available: Option<Available>,
    /// The version downloaded, verified and waiting to be installed.
    pub ready: Option<String>,
    pub busy: Option<Busy>,
    /// The kept previous version, for "Go back to".
    pub kept: Option<String>,
    /// The notice to show now, if any.
    pub notice: Option<Notice>,
    /// Scheduled checks have failed on three days in a row.
    pub failing: bool,
    /// "Restart now" waits for this operation to finish (`"the scan"`...).
    pub restart_blocked_by: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub version: String,
    pub urgent: bool,
    pub notes: String,
    pub notes_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Checking,
    Downloading,
}

/// One notice at a time, most important first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// "Couldn't install PhotoSquirrel <version>. You're still on <current>."
    Failed(Failure),
    /// "Updated to <version>" with What's new, once.
    Updated(String),
    /// Downloaded and verified: installs on quit, or Restart now.
    Ready(Available),
    /// Available, and this installation installs it when the user asks (notify mode).
    Available(Available),
    /// Available, but this installation cannot install it (deb: download and command; archive: link).
    Manual(Available),
}

impl Notice {
    pub fn text(&self) -> String {
        match self {
            Notice::Failed(f) => match f.reason {
                FailureReason::Install => format!(
                    "Couldn't install PhotoSquirrel {}. You're still on {}.",
                    f.version, f.current
                ),
                FailureReason::Start => format!(
                    "PhotoSquirrel {} could not start, so {} was put back.",
                    f.version, f.current
                ),
                FailureReason::StartKept => format!(
                    "PhotoSquirrel {} had trouble starting. Your catalog was already upgraded, so going back \
                     is not possible. Please report it: https://github.com/szehnder/photosquirrel2/issues",
                    f.version
                ),
            },
            Notice::Updated(v) => format!("Updated to PhotoSquirrel {v}"),
            Notice::Ready(a) => format!(
                "PhotoSquirrel {} is ready. It installs when you quit.",
                a.version
            ),
            Notice::Available(a) | Notice::Manual(a) => {
                format!("PhotoSquirrel {} is available.", a.version)
            }
        }
    }
}

struct Shared {
    updater: Updater,
    installation: Installation,
    mode: ModeSource,
    busy_probe: Mutex<Option<BusyProbe>>,
    /// The installed CLI that becomes the installer helper.
    cli: Mutex<Option<std::path::PathBuf>>,
    started_at: i64,
    status: Mutex<UpdateStatus>,
    busy: Mutex<Option<Busy>>,
    stop: AtomicBool,
    wake: (Mutex<bool>, Condvar),
}

/// The app's update service. Cheap to clone.
#[derive(Clone)]
pub struct UpdateService {
    shared: Arc<Shared>,
}

impl UpdateService {
    /// A service for `installation`, started at `started_at` (seconds since the epoch). Remembers `data_dir` for
    /// the rollback guard. No thread runs until [`UpdateService::start`].
    pub fn new(
        updater: Updater,
        installation: Installation,
        mode: ModeSource,
        started_at: i64,
        data_dir: Option<&Path>,
    ) -> Self {
        if let Some(dir) = data_dir {
            let mut state = UpdateState::load(&updater.dir);
            if state.remember_catalog(dir)
                && let Err(e) = state.save(&updater.dir)
            {
                log::warn!("update state not saved: {e}");
            }
        }
        let status = compute_status(&updater, &installation, mode(), None, started_at);
        Self {
            shared: Arc::new(Shared {
                updater,
                installation,
                mode,
                busy_probe: Mutex::new(None),
                cli: Mutex::new(None),
                started_at,
                status: Mutex::new(status),
                busy: Mutex::new(None),
                stop: AtomicBool::new(false),
                wake: (Mutex::new(false), Condvar::new()),
            }),
        }
    }

    /// Run [`UpdateService::tick`] on a background thread every [`TICK`] (and when woken) until stopped.
    pub fn start(&self) {
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("ps-updates".into())
            .spawn(move || {
                while !me.shared.stop.load(Ordering::Relaxed) {
                    me.tick(ps_update::now_secs());
                    let (lock, cv) = &me.shared.wake;
                    let woken = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let (mut woken, _) = cv
                        .wait_timeout_while(woken, TICK, |w| !*w)
                        .unwrap_or_else(|e| e.into_inner());
                    *woken = false;
                }
            });
        if let Err(e) = spawned {
            log::error!("update thread not started: {e}");
        }
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.wake();
    }

    fn wake(&self) {
        let (lock, cv) = &self.shared.wake;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }

    pub fn status(&self) -> UpdateStatus {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn installation(&self) -> &Installation {
        &self.shared.installation
    }

    /// Tell the service how to see a running scan, export or cleanup (ADR-0065).
    pub fn set_busy_probe(&self, probe: BusyProbe) {
        *self
            .shared
            .busy_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(probe);
        self.refresh(ps_update::now_secs());
    }

    /// The installed CLI the installer helper is copied from ([`installed_cli`] in the app).
    pub fn set_cli(&self, cli: std::path::PathBuf) {
        *self.shared.cli.lock().unwrap_or_else(|e| e.into_inner()) = Some(cli);
    }

    fn cli(&self) -> Result<std::path::PathBuf, UpdateError> {
        self.shared
            .cli
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| UpdateError::Failed("no installed CLI to install with".into()))
    }

    fn blocked_by(&self) -> Option<&'static str> {
        let probe = self
            .shared
            .busy_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        probe.and_then(|p| p())
    }

    /// "Restart now": install the ready update once the app has quit, then start the new version. Refused while a
    /// scan, export or cleanup runs. The caller quits the app when this returns `Ok`.
    pub fn restart_now(&self) -> Result<(), UpdateError> {
        if let Some(op) = self.blocked_by() {
            return Err(UpdateError::Failed(format!("Available when {op} finishes")));
        }
        self.install_after_quit(true)
    }

    /// The app is quitting: install the ready update after it has exited, when one is ready and wanted (automatic
    /// mode, or "Install on quit"), the installation installs itself, and that version's install was not already
    /// handed to the user. Returns whether the installer was started.
    pub fn on_quit(&self) -> bool {
        let mode = (self.shared.mode)();
        let state = UpdateState::load(&self.shared.updater.dir);
        let Some(ready) = state.ready.as_ref() else {
            return false;
        };
        let wanted = mode == UpdateMode::Automatic || state.install_on_quit;
        if !wanted
            || !self.shared.installation.can_install()
            || state.fallback_offered.contains(&ready.version)
            || state.is_skipped(&ready.version)
        {
            return false;
        }
        match self.install_after_quit(false) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("update not installed at quit: {e}");
                false
            }
        }
    }

    fn install_after_quit(&self, relaunch: bool) -> Result<(), UpdateError> {
        let cli = self.cli()?;
        install_after_exit(
            &self.shared.updater,
            &self.shared.installation,
            &cli,
            relaunch,
        )
    }

    /// Check, download and hand the install to the helper now, for `--update-now` (the release round trip): the
    /// caller exits right after.
    pub fn update_now(&self) -> Result<String, UpdateError> {
        let manifest = match self.shared.updater.check()? {
            Outcome::Available(m) => m,
            Outcome::UpToDate => return Err(UpdateError::Failed("already up to date".into())),
        };
        let package = self
            .shared
            .installation
            .kind
            .package()
            .filter(|_| self.shared.installation.can_install())
            .ok_or_else(|| {
                UpdateError::Failed(
                    self.shared
                        .installation
                        .cannot_install
                        .clone()
                        .unwrap_or_else(|| "this installation does not update itself".into()),
                )
            })?;
        self.shared.updater.download(&manifest, package)?;
        self.install_after_quit(false)?;
        Ok(manifest.version)
    }

    /// Do whatever is due at `now`: a scheduled check, then (automatic mode, an installation that installs
    /// itself) the download of what it found.
    pub fn tick(&self, now: i64) {
        let mode = (self.shared.mode)();
        let due = check_due(
            mode,
            self.shared.started_at,
            now,
            &UpdateState::load(&self.shared.updater.dir),
        );
        if due && self.shared.updater.release_build {
            let _ = self.run_check(now);
        }
        if mode != UpdateMode::Undecided {
            self.download_if_wanted(mode);
        }
        self.refresh(now);
    }

    /// "Check for updates": one check now, whatever the mode (not in a developer build).
    pub fn check_now(&self) -> Result<Outcome, UpdateError> {
        let r = self.run_check(ps_update::now_secs());
        let mode = (self.shared.mode)();
        if mode != UpdateMode::Undecided {
            self.download_if_wanted(mode);
        }
        self.refresh(ps_update::now_secs());
        r
    }

    /// The user chose to install the available version (notify mode): download it now and install on quit.
    pub fn install_on_quit(&self) -> Result<(), UpdateError> {
        let mut state = UpdateState::load(&self.shared.updater.dir);
        state.install_on_quit = true;
        state.save(&self.shared.updater.dir)?;
        let r = self.download_available();
        self.refresh(ps_update::now_secs());
        r
    }

    /// Hide the current notice for its version (the one-time "Updated", a failure, or an urgent banner).
    pub fn dismiss(&self) {
        let dir = &self.shared.updater.dir;
        let mut state = UpdateState::load(dir);
        match self.status().notice {
            Some(Notice::Failed(_)) => state.last_failure = None,
            Some(Notice::Updated(v)) => state.announced = Some(v),
            Some(Notice::Ready(a) | Notice::Available(a) | Notice::Manual(a))
                if !state.dismissed.contains(&a.version) =>
            {
                state.dismissed.push(a.version);
            }
            _ => {}
        }
        if let Err(e) = state.save(dir) {
            log::warn!("update state not saved: {e}");
        }
        self.refresh(ps_update::now_secs());
    }

    /// "Skip this version": never offered again automatically.
    pub fn skip(&self, version: &str) {
        let dir = &self.shared.updater.dir;
        let mut state = UpdateState::load(dir);
        state.skip(version);
        if state
            .available
            .as_ref()
            .is_some_and(|a| a.version == version)
        {
            state.available = None;
        }
        if let Err(e) = state.save(dir) {
            log::warn!("update state not saved: {e}");
        }
        history::record(dir, "skip", Some(version), "skipped by the user");
        self.refresh(ps_update::now_secs());
    }

    fn run_check(&self, now: i64) -> Result<Outcome, UpdateError> {
        self.set_busy(Some(Busy::Checking));
        let r = self.shared.updater.check_at(now);
        self.set_busy(None);
        r
    }

    fn download_if_wanted(&self, mode: UpdateMode) {
        let state = UpdateState::load(&self.shared.updater.dir);
        let wanted = mode == UpdateMode::Automatic || state.install_on_quit;
        if wanted && self.shared.installation.can_install() && state.available.is_some() {
            let _ = self.download_available();
        }
    }

    fn download_available(&self) -> Result<(), UpdateError> {
        let state = UpdateState::load(&self.shared.updater.dir);
        let (Some(manifest), Some(package)) =
            (state.available, self.shared.installation.kind.package())
        else {
            return Ok(());
        };
        if state
            .ready
            .as_ref()
            .is_some_and(|r| r.version == manifest.version)
        {
            return Ok(());
        }
        if !self.shared.installation.can_install() {
            return Ok(());
        }
        self.set_busy(Some(Busy::Downloading));
        let r = self.shared.updater.download(&manifest, package).map(|_| ());
        self.set_busy(None);
        if let Err(e) = &r {
            log::warn!("update download: {e}");
        }
        r
    }

    fn set_busy(&self, busy: Option<Busy>) {
        *self.shared.busy.lock().unwrap_or_else(|e| e.into_inner()) = busy;
        let mut status = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
        status.busy = busy;
    }

    fn refresh(&self, now: i64) {
        let busy = *self.shared.busy.lock().unwrap_or_else(|e| e.into_inner());
        let mut status = compute_status(
            &self.shared.updater,
            &self.shared.installation,
            (self.shared.mode)(),
            busy,
            now,
        );
        status.restart_blocked_by = self.blocked_by();
        *self.shared.status.lock().unwrap_or_else(|e| e.into_inner()) = status;
    }
}

/// Whether a scheduled check is due at `now`.
pub fn check_due(mode: UpdateMode, started_at: i64, now: i64, state: &UpdateState) -> bool {
    if !mode.checks() || now < started_at + FIRST_CHECK_AFTER_S {
        return false;
    }
    match &state.last_check {
        None => true,
        Some(c) => now >= c.at + CHECK_INTERVAL_S + i64::try_from(state.jitter_s).unwrap_or(0),
    }
}

/// The status of `installation` from the state on disk, without the service (the CLI's `update status`).
pub fn compute_status(
    updater: &Updater,
    installation: &Installation,
    mode: UpdateMode,
    busy: Option<Busy>,
    now: i64,
) -> UpdateStatus {
    let state = UpdateState::load(&updater.dir);
    let installed = updater.installed.to_string();
    let available = state
        .available
        .as_ref()
        .filter(|m| m.semver() > updater.installed && !state.is_skipped(&m.version))
        .map(|m| Available {
            version: m.version.clone(),
            urgent: m.urgent,
            notes: m.notes.clone(),
            notes_url: m.notes_url.clone(),
        });
    let ready = state
        .ready
        .as_ref()
        .filter(|r| available.as_ref().is_some_and(|a| a.version == r.version) && r.file.is_file())
        .map(|r| r.version.clone());
    let failing = state
        .failures_since
        .is_some_and(|since| now - since >= QUIET_FAILURES_S);
    let notice = notice(
        &state,
        &installed,
        installation,
        mode,
        available.as_ref(),
        ready.as_deref(),
    );
    UpdateStatus {
        mode,
        installed,
        kind: installation.kind,
        cannot_install: installation.cannot_install.clone(),
        last_check: state.last_check.clone(),
        last_success_at: state.last_success_at,
        available,
        ready,
        busy,
        kept: state.kept.as_ref().map(|k| k.version.clone()),
        notice,
        failing,
        restart_blocked_by: None,
    }
}

fn notice(
    state: &UpdateState,
    installed: &str,
    installation: &Installation,
    mode: UpdateMode,
    available: Option<&Available>,
    ready: Option<&str>,
) -> Option<Notice> {
    if let Some(f) = &state.last_failure {
        return Some(Notice::Failed(f.clone()));
    }
    if let Some(t) = &state.trial
        && t.ok
        && t.version == installed
        && state.announced.as_deref() != Some(installed)
    {
        return Some(Notice::Updated(installed.to_owned()));
    }
    let a = available?;
    // Dismissed for this version: hidden, urgent or not (spec "Urgent updates").
    if state.dismissed.contains(&a.version) {
        return None;
    }
    if ready == Some(a.version.as_str()) {
        return Some(Notice::Ready(a.clone()));
    }
    if !installation.can_install() {
        return Some(Notice::Manual(a.clone()));
    }
    match mode {
        UpdateMode::Notify => Some(Notice::Available(a.clone())),
        // Automatic: the download is on its way; urgent updates still say so.
        UpdateMode::Automatic if a.urgent => Some(Notice::Available(a.clone())),
        UpdateMode::Off | UpdateMode::Undecided => Some(Notice::Available(a.clone())),
        UpdateMode::Automatic => None,
    }
}

/// Record a failed install for the notice.
pub fn record_failure(updater: &Updater, version: &str, message: &str) {
    record(
        &updater.dir,
        Failure {
            at: ps_update::now_secs(),
            version: version.to_owned(),
            current: updater.installed.to_string(),
            message: message.to_owned(),
            reason: FailureReason::Install,
        },
    );
}

fn record(dir: &UpdateDir, failure: Failure) {
    let mut state = UpdateState::load(dir);
    let (version, message) = (failure.version.clone(), failure.message.clone());
    state.last_failure = Some(failure);
    if let Err(e) = state.save(dir) {
        log::warn!("update state not saved: {e}");
    }
    history::record(dir, "failure", Some(&version), &message);
}

/// This build's updater, with this user's update folder.
pub fn updater_for_this_build() -> Result<Updater, UpdateError> {
    let dir = UpdateDir::for_user()
        .ok_or_else(|| UpdateError::Failed("this system has no home folder for updates".into()))?;
    Updater::for_this_build(dir)
}

/// How this copy of PhotoSquirrel was installed.
pub fn this_installation() -> Installation {
    ps_update::kind::test_override()
        .unwrap_or_else(|| ps_update::kind::detect(&Probe::this_process()))
}

/// Hold this installation's running lock while the returned file lives: an install waits for every app and CLI
/// process of the installation (ADR-0126). `None` when it cannot be taken; running is never refused for it.
pub fn hold_running() -> Option<std::fs::File> {
    let dir = UpdateDir::for_user()?;
    install::hold_running(&dir, &this_installation().location)
}

/// The installed CLI beside the running program (`photosquirrel-cli`, `.exe` on Windows): the helper's source.
pub fn installed_cli() -> Result<std::path::PathBuf, UpdateError> {
    let exe = std::env::current_exe()?;
    let name = if cfg!(windows) {
        "photosquirrel-cli.exe"
    } else {
        "photosquirrel-cli"
    };
    let cli = exe
        .parent()
        .map(|d| d.join(name))
        .ok_or_else(|| UpdateError::Failed("no folder for the running program".into()))?;
    if cli.is_file() {
        Ok(cli)
    } else {
        Err(UpdateError::Failed(format!("{} is missing", cli.display())))
    }
}

/// Install the ready update once this process has exited: write the plan and start the helper (on quit, or
/// "Restart now" with `relaunch`).
pub fn install_after_exit(
    updater: &Updater,
    installation: &Installation,
    cli: &Path,
    relaunch: bool,
) -> Result<(), UpdateError> {
    let plan = install::plan_install(
        &updater.dir,
        installation,
        &updater.installed.to_string(),
        ps_catalog::latest_schema_version(),
        relaunch,
        Some(std::process::id()),
    )?;
    install::spawn_helper(cli, &updater.dir, &updater.installed.to_string(), &plan)
}

/// Whether the kept previous version can be put back: refused once a catalog this installation used has a
/// format newer than the kept version opens (spec "Guarded manual rollback").
pub fn rollback_allowed(dir: &UpdateDir, current: &str) -> Result<(), String> {
    let state = UpdateState::load(dir);
    let kept = state
        .kept
        .ok_or_else(|| "no previous version is kept".to_owned())?;
    for data in &state.catalogs {
        let file = data.join(ps_catalog::CATALOG_FILE);
        match ps_catalog::db::stored_version(&file) {
            Ok(Some(v)) if v > kept.max_catalog_schema => {
                return Err(format!(
                    "the catalog was upgraded by {current}; {} cannot open it ({})",
                    kept.version,
                    data.display()
                ));
            }
            Ok(_) => {}
            Err(e) => log::warn!("rollback check of {}: {e}", file.display()),
        }
    }
    Ok(())
}

/// Put the kept version back once this process has exited, if the catalog guard allows it.
pub fn rollback_after_exit(
    updater: &Updater,
    installation: &Installation,
    cli: &Path,
    relaunch: bool,
) -> Result<(), UpdateError> {
    let current = updater.installed.to_string();
    rollback_allowed(&updater.dir, &current).map_err(UpdateError::Failed)?;
    if let Some(reason) = &installation.cannot_install {
        return Err(UpdateError::Failed(reason.clone()));
    }
    let plan = install::plan_rollback(
        &updater.dir,
        installation,
        &current,
        relaunch,
        Some(std::process::id()),
    )?;
    install::spawn_helper(cli, &updater.dir, &current, &plan)
}

/// What the start of this process means for a newly installed version (spec "Failed start restores the previous
/// version").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrialStart {
    /// Not a trial start.
    Normal,
    /// A trial start: open the catalog for writing only after the first frame, then call [`trial_succeeded`].
    Trial,
    /// Two trial starts failed: the kept version is being put back; exit now.
    RollingBack,
    /// Two trial starts failed but the catalog was already upgraded: carry on, with a notice.
    Kept,
}

/// Count this start of a newly installed version, very early (before the window and the catalog). After two
/// trial starts that did not reach their first frame, start putting the kept version back.
pub fn trial_start(
    updater: &Updater,
    installation: &Installation,
    cli: Option<&Path>,
) -> TrialStart {
    let dir = &updater.dir;
    let installed = updater.installed.to_string();
    let mut state = UpdateState::load(dir);
    let Some(trial) = state
        .trial
        .as_mut()
        .filter(|t| t.version == installed && !t.ok)
    else {
        return TrialStart::Normal;
    };
    trial.starts += 1;
    let starts = trial.starts;
    if let Err(e) = state.save(dir) {
        log::warn!("update state not saved: {e}");
    }
    if starts < 3 {
        history::record(
            dir,
            "trial",
            Some(&installed),
            &format!("trial start {starts}"),
        );
        return TrialStart::Trial;
    }
    let kept = state
        .kept
        .as_ref()
        .map(|k| k.version.clone())
        .unwrap_or_default();
    let guard = rollback_allowed(dir, &installed);
    let rolled = guard.clone().map_err(UpdateError::Failed).and_then(|()| {
        let cli = cli
            .ok_or_else(|| UpdateError::Failed("no CLI to put the previous version back".into()))?;
        rollback_after_exit(updater, installation, cli, true)
    });
    match rolled {
        Ok(()) => {
            // The previous version shows this when it starts.
            record(
                dir,
                Failure {
                    at: ps_update::now_secs(),
                    version: installed.clone(),
                    current: kept,
                    message: "the new version did not start twice in a row".into(),
                    reason: FailureReason::Start,
                },
            );
            TrialStart::RollingBack
        }
        Err(e) => {
            let mut state = UpdateState::load(dir);
            state.trial = None;
            if let Err(e) = state.save(dir) {
                log::warn!("update state not saved: {e}");
            }
            record(
                dir,
                Failure {
                    at: ps_update::now_secs(),
                    version: installed.clone(),
                    current: installed,
                    message: e.to_string(),
                    reason: FailureReason::StartKept,
                },
            );
            TrialStart::Kept
        }
    }
}

/// The trial start drew its first frame: the new version is kept.
pub fn trial_succeeded(dir: &UpdateDir) {
    let mut state = UpdateState::load(dir);
    if let Some(t) = state.trial.as_mut().filter(|t| !t.ok) {
        t.ok = true;
        let v = t.version.clone();
        if let Err(e) = state.save(dir) {
            log::warn!("update state not saved: {e}");
        }
        history::record(dir, "trial", Some(&v), "started successfully");
    }
}

#[cfg(test)]
mod tests;
