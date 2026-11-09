//! Installing a verified update after the app has exited, and putting the previous version back (ADR-0126; spec
//! "Install waits for every program of the installation", "Install keeps the previous version and checks the new
//! one", "Blocked bundle swap falls back").
//!
//! The work is described by a [`Plan`] saved beside the payload and carried out by the installed CLI's copy
//! (`update finish`). Every step is recorded in the plan, and each replacement is one atomic operation (a rename
//! over the AppImage, an exchange of bundles, or the installer's own run), so a killed helper leaves either the
//! old or the new version, and running it again finishes the work.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dir::UpdateDir;
use crate::kind::Kind;
use crate::state::{Failure, Kept, Trial, UpdateState, write_atomic};
use crate::{UpdateError, deliver, history};

/// The longest the installer waits for the programs of the installation to exit.
pub const WAIT_FOR_EXIT: Duration = Duration::from_secs(10 * 60);
/// The longest the Windows installer may run.
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What the helper does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Replace the installation with `payload`.
    Install,
    /// Put the kept version back.
    Rollback,
}

/// How far the helper got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Planned,
    /// The installed version is copied to `previous/`.
    BackedUp,
    /// The new version is prepared beside the installation and passed `--version`.
    Staged,
    /// The installation is the new version.
    Swapped,
    /// The installed new version passed `--version`; the state records the trial.
    Done,
    /// The old version was put back.
    Undone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub action: Action,
    /// The version installed afterwards (the new one, or for a rollback the kept one).
    pub version: String,
    /// The version installed now.
    pub current: String,
    /// [`Kind::id`] of the installation.
    pub kind: String,
    /// The AppImage file, the `.app` bundle or the installation folder.
    pub location: PathBuf,
    /// The verified payload (install), or the kept copy (rollback).
    pub payload: PathBuf,
    /// The newest catalog format the version installed now opens.
    pub current_max_catalog_schema: u32,
    /// Start the app afterwards ("Restart now", an automatic rollback).
    pub relaunch: bool,
    /// Exit of this process first.
    pub wait_pid: Option<u32>,
    /// Where a refused macOS swap saves the update (the user's Downloads folder).
    #[serde(default)]
    pub downloads: Option<PathBuf>,
    pub step: Step,
}

impl Plan {
    pub fn file(dir: &UpdateDir, version: &str) -> PathBuf {
        dir.ready_dir(version).join("plan.json")
    }

    pub fn load(path: &Path) -> Result<Self, UpdateError> {
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(|e| UpdateError::BadManifest(format!("plan: {e}")))
    }

    pub fn save(&self, path: &Path) -> Result<(), UpdateError> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(path, &bytes)?;
        Ok(())
    }

    fn kind(&self) -> Result<Kind, UpdateError> {
        [Kind::AppImage, Kind::MacApp, Kind::WindowsSetup]
            .into_iter()
            .find(|k| k.id() == self.kind)
            .ok_or_else(|| {
                UpdateError::BadManifest(format!("a {} cannot install updates", self.kind))
            })
    }
}

/// The plan to install the ready version over `installation`, saved beside its payload.
pub fn plan_install(
    dir: &UpdateDir,
    installation: &crate::kind::Installation,
    current: &str,
    current_max_catalog_schema: u32,
    relaunch: bool,
    wait_pid: Option<u32>,
) -> Result<PathBuf, UpdateError> {
    if let Some(reason) = &installation.cannot_install {
        return Err(UpdateError::Failed(reason.clone()));
    }
    let state = UpdateState::load(dir);
    let ready = state
        .ready
        .ok_or_else(|| UpdateError::Failed("no update is ready to install".into()))?;
    let plan = Plan {
        action: Action::Install,
        version: ready.version.clone(),
        current: current.to_owned(),
        kind: installation.kind.id().to_owned(),
        location: installation.location.clone(),
        payload: ready.file,
        current_max_catalog_schema,
        relaunch,
        wait_pid,
        downloads: deliver::downloads_dir(),
        step: Step::Planned,
    };
    let path = Plan::file(dir, &ready.version);
    plan.save(&path)?;
    Ok(path)
}

/// The plan to put the kept version back over `installation`.
pub fn plan_rollback(
    dir: &UpdateDir,
    installation: &crate::kind::Installation,
    current: &str,
    relaunch: bool,
    wait_pid: Option<u32>,
) -> Result<PathBuf, UpdateError> {
    let state = UpdateState::load(dir);
    let kept = state
        .kept
        .ok_or_else(|| UpdateError::Failed("no previous version is kept".into()))?;
    let plan = Plan {
        action: Action::Rollback,
        version: kept.version.clone(),
        current: current.to_owned(),
        kind: installation.kind.id().to_owned(),
        location: installation.location.clone(),
        payload: kept.path,
        current_max_catalog_schema: kept.max_catalog_schema,
        relaunch,
        wait_pid,
        downloads: None,
        step: Step::Planned,
    };
    let path = dir.root().join("rollback").join("plan.json");
    plan.save(&path)?;
    Ok(path)
}

/// Copy the installed CLI into the update folder and start it, detached, to carry out `plan`
/// (`photosquirrel-cli update finish --plan <plan> --update-dir <dir>`). The copy is the old, known-good code and
/// is not among the files being replaced. Its output goes to `helper.log` in the update folder.
pub fn spawn_helper(
    cli: &Path,
    dir: &UpdateDir,
    current: &str,
    plan: &Path,
) -> Result<(), UpdateError> {
    let helper_dir = dir.helper_dir(current);
    fs::create_dir_all(&helper_dir)?;
    let name = cli.file_name().ok_or_else(|| missing(cli))?;
    let helper = helper_dir.join(name);
    fs::copy(cli, &helper)?;
    set_executable(&helper)?;
    let log = File::options()
        .create(true)
        .append(true)
        .open(dir.root().join("helper.log"))?;
    let mut cmd = Command::new(&helper);
    cmd.args(["update", "finish", "--plan"])
        .arg(plan)
        .arg("--update-dir")
        .arg(dir.root())
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    detach(&mut cmd, true);
    if let Err(e) = cmd.spawn() {
        // A job object that forbids breaking away refuses the flag: start it without.
        if !cfg!(windows) {
            return Err(e.into());
        }
        detach(&mut cmd, false);
        cmd.spawn()?;
    }
    Ok(())
}

#[cfg(unix)]
fn detach(cmd: &mut Command, _breakaway: bool) {
    use std::os::unix::process::CommandExt;
    // Its own process group: it outlives the app and a terminal's Ctrl-C.
    cmd.process_group(0);
}

#[cfg(windows)]
fn detach(cmd: &mut Command, breakaway: bool) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let flags = DETACHED_PROCESS | CREATE_NO_WINDOW;
    cmd.creation_flags(if breakaway {
        flags | CREATE_BREAKAWAY_FROM_JOB
    } else {
        flags
    });
}

#[cfg(not(any(unix, windows)))]
fn detach(_cmd: &mut Command, _breakaway: bool) {}

/// The per-installation running lock: every program of the installation holds it shared while it runs; the
/// installer holds it exclusively while it replaces files.
pub fn lock_file(dir: &UpdateDir, location: &Path) -> PathBuf {
    let digest = Sha256::digest(location.to_string_lossy().as_bytes());
    let key: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    dir.installs_dir().join(format!("{key}.lock"))
}

/// Hold the running lock shared for as long as the returned file lives. `None` (and a log line) when it cannot
/// be taken: running is never refused because of it.
pub fn hold_running(dir: &UpdateDir, location: &Path) -> Option<File> {
    let path = lock_file(dir, location);
    let f = fs::create_dir_all(dir.installs_dir())
        .and_then(|()| {
            File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
        })
        .ok()?;
    match f.try_lock_shared() {
        Ok(()) => Some(f),
        Err(e) => {
            log::info!("installation lock not taken ({e}): an update may be installing");
            None
        }
    }
}

/// Whether a program of the installation is running (someone holds the lock).
pub fn running(dir: &UpdateDir, location: &Path) -> bool {
    let path = lock_file(dir, location);
    let Ok(f) = File::options().write(true).open(&path) else {
        return false;
    };
    match f.try_lock() {
        Ok(()) => {
            let _ = f.unlock();
            false
        }
        Err(_) => true,
    }
}

fn take_exclusive(dir: &UpdateDir, location: &Path, wait: Duration) -> Result<File, UpdateError> {
    fs::create_dir_all(dir.installs_dir())?;
    let f = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_file(dir, location))?;
    let deadline = Instant::now() + wait;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(f),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            Err(_) => return Err(UpdateError::StillRunning),
        }
    }
}

/// Run `program --version` and require `<name> <version>`.
pub fn smoke(program: &Path, name: &str, version: &str) -> Result<(), UpdateError> {
    let out = Command::new(program)
        .arg("--version")
        .output()
        .map_err(|e| UpdateError::Failed(format!("{} did not start: {e}", program.display())))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut words = text.split_whitespace();
    if out.status.success() && words.next() == Some(name) && words.next() == Some(version) {
        Ok(())
    } else {
        Err(UpdateError::Failed(format!(
            "{} reported {:?} instead of {name} {version}",
            program.display(),
            text.trim()
        )))
    }
}

/// Test hook: stop after this step, as if the helper had been killed.
pub type StopAfter = Option<Step>;

/// Carry out the plan at `plan_path`: wait for the installation's programs, replace the installation (or put the
/// kept version back), check it, and record the outcome. A failure puts the previous version back.
pub fn finish(
    dir: &UpdateDir,
    plan_path: &Path,
    wait: Duration,
    stop_after: StopAfter,
) -> Result<Step, UpdateError> {
    let mut plan = Plan::load(plan_path)?;
    let _lock = match take_exclusive(dir, &plan.location, wait) {
        Ok(l) => l,
        Err(e) => {
            history::record(
                dir,
                "postponed",
                Some(&plan.version),
                "postponed: PhotoSquirrel is still running",
            );
            return Err(e);
        }
    };
    if matches!(plan.step, Step::Done | Step::Undone) {
        return Ok(plan.step);
    }
    let result = match plan.action {
        Action::Install => install(dir, plan_path, &mut plan, stop_after),
        Action::Rollback => rollback(dir, plan_path, &mut plan, stop_after),
    };
    match result {
        Ok(step) => Ok(step),
        Err(UpdateError::Stopped) => Err(UpdateError::Stopped),
        Err(UpdateError::Fallback(file)) => {
            // The OS refused the bundle swap: nothing was changed, the update waits in Downloads.
            let mut state = UpdateState::load(dir);
            if !state.fallback_offered.contains(&plan.version) {
                state.fallback_offered.push(plan.version.clone());
            }
            state.save(dir)?;
            history::record(
                dir,
                "failure",
                Some(&plan.version),
                &format!(
                    "macOS refused to replace the app; the update is in {}",
                    file.display()
                ),
            );
            Err(UpdateError::Fallback(file))
        }
        Err(e) => {
            let undone = undo(dir, &plan);
            plan.step = Step::Undone;
            plan.save(plan_path)?;
            let mut state = UpdateState::load(dir);
            if plan.action == Action::Install {
                state.skip(&plan.version);
                state.ready = None;
            }
            state.last_failure = Some(Failure {
                at: crate::now_secs(),
                version: plan.version.clone(),
                current: plan.current.clone(),
                message: e.to_string(),
                reason: crate::state::FailureReason::Install,
            });
            state.save(dir)?;
            history::record(
                dir,
                "failure",
                Some(&plan.version),
                &format!(
                    "{e}; previous version {}",
                    if undone.is_ok() {
                        "put back"
                    } else {
                        "NOT put back"
                    }
                ),
            );
            undone?;
            Err(e)
        }
    }
}

fn reached(
    plan: &mut Plan,
    plan_path: &Path,
    step: Step,
    stop_after: StopAfter,
) -> Result<(), UpdateError> {
    plan.step = step;
    plan.save(plan_path)?;
    if stop_after == Some(step) {
        return Err(UpdateError::Stopped);
    }
    Ok(())
}

fn install(
    dir: &UpdateDir,
    plan_path: &Path,
    plan: &mut Plan,
    stop: StopAfter,
) -> Result<Step, UpdateError> {
    let kind = plan.kind()?;
    let previous = dir.previous_dir(&plan.current);
    if plan.step < Step::BackedUp {
        if previous.exists() {
            fs::remove_dir_all(&previous)?;
        }
        fs::create_dir_all(&previous)?;
        backup(kind, &plan.location, &previous)?;
        reached(plan, plan_path, Step::BackedUp, stop)?;
    }
    if plan.step < Step::Staged {
        stage(kind, plan)?;
        reached(plan, plan_path, Step::Staged, stop)?;
    }
    if plan.step < Step::Swapped {
        swap(kind, dir, plan)?;
        reached(plan, plan_path, Step::Swapped, stop)?;
    }
    check_installed(kind, &plan.location, &plan.version)?;
    if kind == Kind::MacApp {
        // After the exchange the old bundle sits in the staging folder: keep it in `previous/`.
        keep_old_bundle(&staged_bundle(plan), &previous.join("PhotoSquirrel.app"))?;
    }
    cleanup_staging(kind, plan);

    // Record the trial and the kept copy; the copy before it is no longer needed.
    let mut state = UpdateState::load(dir);
    if let Some(old) = state.kept.take()
        && old.version != plan.current
    {
        fs::remove_dir_all(dir.previous_dir(&old.version)).ok();
    }
    state.kept = Some(Kept {
        version: plan.current.clone(),
        path: kept_path(kind, &previous, &plan.location),
        max_catalog_schema: plan.current_max_catalog_schema,
    });
    state.trial = Some(Trial {
        version: plan.version.clone(),
        starts: 0,
        ok: false,
    });
    state.ready = None;
    state.install_on_quit = false;
    state.save(dir)?;
    history::record(
        dir,
        "install",
        Some(&plan.version),
        &format!(
            "installed over {} ({})",
            plan.current,
            plan.location.display()
        ),
    );
    reached(plan, plan_path, Step::Done, None)?;
    relaunch(kind, plan);
    Ok(Step::Done)
}

fn rollback(
    dir: &UpdateDir,
    plan_path: &Path,
    plan: &mut Plan,
    stop: StopAfter,
) -> Result<Step, UpdateError> {
    let kind = plan.kind()?;
    if plan.step < Step::Staged {
        stage_kept(kind, plan)?;
        reached(plan, plan_path, Step::Staged, stop)?;
    }
    if plan.step < Step::Swapped {
        swap(kind, dir, plan)?;
        reached(plan, plan_path, Step::Swapped, stop)?;
    }
    check_installed(kind, &plan.location, &plan.version)?;
    cleanup_staging(kind, plan);
    let mut state = UpdateState::load(dir);
    state.skip(&plan.current);
    state.kept = None;
    state.trial = None;
    state.ready = None;
    state.available = None;
    state.save(dir)?;
    history::record(
        dir,
        "rollback",
        Some(&plan.version),
        &format!("{} replaced by the kept {}", plan.current, plan.version),
    );
    reached(plan, plan_path, Step::Done, None)?;
    relaunch(kind, plan);
    Ok(Step::Done)
}

/// Where the staged replacement lives: beside the installation, so the swap is one rename on one volume.
fn staging(kind: Kind, plan: &Plan) -> PathBuf {
    let parent = plan.location.parent().unwrap_or(Path::new("."));
    let name = plan
        .location
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match kind {
        Kind::AppImage => parent.join(format!(".{name}.update")),
        Kind::MacApp => parent.join(format!(".PhotoSquirrel-update-{}", plan.version)),
        _ => parent.join(format!(".{name}.update")),
    }
}

/// The new bundle inside the staging folder.
fn staged_bundle(plan: &Plan) -> PathBuf {
    staging(Kind::MacApp, plan).join(
        plan.location
            .file_name()
            .unwrap_or(std::ffi::OsStr::new("PhotoSquirrel.app")),
    )
}

fn backup(kind: Kind, location: &Path, previous: &Path) -> Result<(), UpdateError> {
    match kind {
        Kind::AppImage => {
            let name = location.file_name().ok_or_else(|| missing(location))?;
            let to = previous.join(name);
            fs::copy(location, &to)?;
            if fs::metadata(&to)?.len() != fs::metadata(location)?.len() {
                return Err(UpdateError::Failed("the backup copy is incomplete".into()));
            }
        }
        // A bundle needs no copy: the exchange keeps the old one whole.
        Kind::MacApp => {}
        Kind::WindowsSetup => copy_tree(location, previous)?,
        _ => {}
    }
    Ok(())
}

fn stage(kind: Kind, plan: &Plan) -> Result<(), UpdateError> {
    let staged = staging(kind, plan);
    match kind {
        Kind::AppImage => {
            fs::copy(&plan.payload, &staged)?;
            File::open(&staged)?.sync_all()?;
            set_executable(&staged)?;
            smoke(&staged, "photosquirrel", &plan.version)
        }
        Kind::MacApp => {
            if staged.exists() {
                fs::remove_dir_all(&staged)?;
            }
            fs::create_dir_all(&staged)?;
            let status = Command::new("tar")
                .arg("-xzf")
                .arg(&plan.payload)
                .arg("-C")
                .arg(&staged)
                .status()?;
            let bundle = staged_bundle(plan);
            if !status.success() || !bundle.is_dir() {
                return Err(UpdateError::Failed(
                    "the update archive holds no PhotoSquirrel.app".into(),
                ));
            }
            check_same_signer(&plan.location, &bundle)?;
            check_installed(kind, &bundle, &plan.version)
        }
        // The installer itself is the staged form; it runs in the swap.
        _ => Ok(()),
    }
}

fn stage_kept(kind: Kind, plan: &Plan) -> Result<(), UpdateError> {
    let staged = staging(kind, plan);
    match kind {
        Kind::AppImage => {
            fs::copy(&plan.payload, &staged)?;
            set_executable(&staged)?;
            smoke(&staged, "photosquirrel", &plan.version)
        }
        Kind::MacApp => {
            if staged.exists() {
                fs::remove_dir_all(&staged)?;
            }
            fs::create_dir_all(&staged)?;
            // The kept bundle may be on another volume: copy it beside the installation first.
            copy_tree(&plan.payload, &staged_bundle(plan))?;
            check_installed(kind, &staged_bundle(plan), &plan.version)
        }
        _ => Ok(()),
    }
}

fn swap(kind: Kind, dir: &UpdateDir, plan: &Plan) -> Result<(), UpdateError> {
    match kind {
        Kind::AppImage => {
            fs::rename(staging(kind, plan), &plan.location)?;
            Ok(())
        }
        Kind::MacApp => match exchange(&staged_bundle(plan), &plan.location) {
            Ok(()) => Ok(()),
            Err(e)
                if e.kind() == io::ErrorKind::PermissionDenied
                    && plan.action == Action::Install =>
            {
                let downloads = plan
                    .downloads
                    .clone()
                    .or_else(deliver::downloads_dir)
                    .unwrap_or_else(|| dir.root().to_path_buf());
                let name = plan
                    .payload
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "PhotoSquirrel.app.tar.gz".into());
                let saved = deliver::copy_new(&plan.payload, &downloads, &name)?;
                fs::remove_dir_all(staging(kind, plan)).ok();
                deliver::reveal(&saved);
                Err(UpdateError::Fallback(saved))
            }
            Err(e) => Err(e.into()),
        },
        Kind::WindowsSetup => match plan.action {
            Action::Install => run_installer(&plan.payload),
            Action::Rollback => {
                copy_tree(&plan.payload, &plan.location)?;
                set_display_version(&plan.version);
                Ok(())
            }
        },
        _ => Err(UpdateError::Failed(format!(
            "a {} cannot install updates",
            kind.label()
        ))),
    }
}

/// Put the version that was installed before back, after a failure.
fn undo(dir: &UpdateDir, plan: &Plan) -> Result<(), UpdateError> {
    let Ok(kind) = plan.kind() else {
        return Ok(());
    };
    match plan.action {
        Action::Install => {
            if plan.step < Step::Swapped && kind != Kind::WindowsSetup {
                // Nothing replaced yet: drop the staged copy.
                cleanup_staging(kind, plan);
                return Ok(());
            }
            let previous = dir.previous_dir(&plan.current);
            match kind {
                Kind::AppImage => {
                    let name = plan
                        .location
                        .file_name()
                        .ok_or_else(|| missing(&plan.location))?;
                    let tmp = staging(kind, plan);
                    fs::copy(previous.join(name), &tmp)?;
                    set_executable(&tmp)?;
                    fs::rename(&tmp, &plan.location)?;
                }
                Kind::MacApp => {
                    // The old bundle is in the staging folder after the exchange: exchange back.
                    exchange(&staged_bundle(plan), &plan.location)?;
                    cleanup_staging(kind, plan);
                }
                Kind::WindowsSetup if previous.is_dir() => {
                    copy_tree(&previous, &plan.location)?;
                    set_display_version(&plan.current);
                }
                _ => {}
            }
            Ok(())
        }
        // A rollback that failed leaves the newer version, which still starts as far as anyone knows.
        Action::Rollback => {
            if plan.step >= Step::Swapped && kind == Kind::MacApp {
                exchange(&staged_bundle(plan), &plan.location)?;
            }
            cleanup_staging(kind, plan);
            Ok(())
        }
    }
}

fn cleanup_staging(kind: Kind, plan: &Plan) {
    let staged = staging(kind, plan);
    match kind {
        Kind::AppImage => {
            fs::remove_file(&staged).ok();
        }
        Kind::MacApp => {
            fs::remove_dir_all(&staged).ok();
        }
        _ => {}
    }
}

/// The installed version answers `--version`: the app, and the CLI where the package exposes it.
fn check_installed(kind: Kind, location: &Path, version: &str) -> Result<(), UpdateError> {
    match kind {
        Kind::AppImage => smoke(location, "photosquirrel", version),
        Kind::MacApp => {
            let bin = location.join("Contents/MacOS");
            smoke(&bin.join("photosquirrel"), "photosquirrel", version)?;
            smoke(&bin.join("photosquirrel-cli"), "photosquirrel-cli", version)
        }
        Kind::WindowsSetup => {
            smoke(
                &location.join("photosquirrel.exe"),
                "photosquirrel",
                version,
            )?;
            smoke(
                &location.join("photosquirrel-cli.exe"),
                "photosquirrel-cli",
                version,
            )
        }
        _ => Ok(()),
    }
}

/// Where the kept copy of the replaced version is.
fn kept_path(kind: Kind, previous: &Path, location: &Path) -> PathBuf {
    match kind {
        Kind::AppImage => previous.join(location.file_name().unwrap_or_default()),
        Kind::WindowsSetup => previous.to_path_buf(),
        _ => previous.join("PhotoSquirrel.app"),
    }
}

/// Move the replaced bundle into the update folder (a rename on one volume, otherwise a copy).
fn keep_old_bundle(from: &Path, to: &Path) -> Result<(), UpdateError> {
    if to.exists() {
        fs::remove_dir_all(to)?;
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(from, to).is_err() {
        copy_tree(from, to)?;
    }
    Ok(())
}

/// Swap two paths atomically (macOS `renamex_np(RENAME_SWAP)`, Linux `renameat2(RENAME_EXCHANGE)`).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    #[cfg(test)]
    if tests::REFUSE_EXCHANGE.with(std::cell::Cell::get) {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    renameat_with(CWD, a, CWD, b, RenameFlags::EXCHANGE).map_err(io::Error::from)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn exchange(_a: &Path, _b: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no atomic exchange on this platform",
    ))
}

/// A bundle signed with a Developer ID accepts only a bundle with the same Team ID (ADR-0128). Unsigned (ad-hoc)
/// installations, and systems without `codesign`, rely on the manifest signature alone.
fn check_same_signer(installed: &Path, staged: &Path) -> Result<(), UpdateError> {
    let Some(team) = team_id(installed) else {
        return Ok(());
    };
    let verified = Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(staged)
        .status()
        .is_ok_and(|s| s.success());
    if verified && team_id(staged).as_deref() == Some(team.as_str()) {
        Ok(())
    } else {
        Err(UpdateError::Failed(
            "the update is not signed by the same developer".into(),
        ))
    }
}

/// The TeamIdentifier of a signed bundle, if it has one.
fn team_id(bundle: &Path) -> Option<String> {
    let out = Command::new("codesign")
        .arg("-dv")
        .arg(bundle)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "not set")
        .map(str::to_owned)
}

/// Run the NSIS installer silently: no new shortcuts, no relaunch (`/S /NS`), and wait for it.
fn run_installer(setup: &Path) -> Result<(), UpdateError> {
    let mut child = Command::new(setup).args(["/S", "/NS"]).spawn()?;
    let deadline = Instant::now() + INSTALLER_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(UpdateError::Failed(format!(
                    "the installer ended with {status}"
                )))
            };
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err(UpdateError::Failed(
                "the installer did not finish in 10 minutes".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Keep Windows' Apps list in step after a rollback.
fn set_display_version(version: &str) {
    if cfg!(windows) {
        let _ = Command::new("reg")
            .args([
                "add",
                crate::kind::WINDOWS_UNINSTALL_KEY,
                "/v",
                "DisplayVersion",
                "/d",
                version,
                "/f",
            ])
            .status();
    }
}

fn relaunch(kind: Kind, plan: &Plan) {
    if !plan.relaunch {
        return;
    }
    let program = match kind {
        Kind::AppImage => plan.location.clone(),
        Kind::MacApp => plan.location.join("Contents/MacOS/photosquirrel"),
        Kind::WindowsSetup => plan.location.join("photosquirrel.exe"),
        _ => return,
    };
    if let Err(e) = Command::new(&program).spawn() {
        log::warn!("could not start {}: {e}", program.display());
    }
}

/// Copy a folder's files into `to` (created), replacing files of the same name there.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let meta = fs::symlink_metadata(&src)?;
        if meta.is_dir() {
            copy_tree(&src, &dst)?;
        } else if meta.file_type().is_symlink() {
            #[cfg(unix)]
            {
                fs::remove_file(&dst).ok();
                std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
            }
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

fn set_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn missing(path: &Path) -> UpdateError {
    UpdateError::Failed(format!("{} has no file name", path.display()))
}

#[cfg(test)]
mod tests;
