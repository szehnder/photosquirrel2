//! Installs on fake installations in temporary folders. The "programs" are shell scripts that answer
//! `--version`, so these tests run on Unix; the Windows installer path runs the same code with a script standing
//! in for the NSIS installer.
#![cfg(unix)]

use std::cell::{Cell, RefCell};
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::state::Ready;

thread_local! {
    /// Make the bundle exchange fail as macOS App Management would.
    pub(super) static REFUSE_EXCHANGE: Cell<bool> = const { Cell::new(false) };
    /// A stand-in for `codesign`.
    pub(super) static CODESIGN: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

const OLD: &str = "0.4.0";
const NEW: &str = "0.4.1";

fn script(path: &Path, body: &str) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn sha_of(path: &Path) -> String {
    let mut h = Sha256::new();
    crate::download::hash_file(path, &mut h).unwrap();
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn program(path: &Path, name: &str, version: &str) {
    script(path, &format!("echo '{name} {version}'"));
}

/// What a program at `path` says it is.
fn version_of(path: &Path) -> String {
    let out = Command::new(path).arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Fake {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    dir: UpdateDir,
    location: PathBuf,
    kind: Kind,
}

impl Fake {
    fn new(kind: Kind, payload_cli_version: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let dir = UpdateDir::at(root.join("updates"));
        let ready = dir.ready_dir(NEW);
        fs::create_dir_all(&ready).unwrap();
        let (location, payload) = match kind {
            Kind::AppImage => {
                let location = root.join("Apps/PhotoSquirrel-0.4.0-linux-x86_64.AppImage");
                program(&location, "photosquirrel", OLD);
                let payload = ready.join("PhotoSquirrel-0.4.1-linux-x86_64.AppImage");
                program(&payload, "photosquirrel", payload_cli_version);
                (location, payload)
            }
            Kind::MacApp => {
                let location = root.join("Applications/PhotoSquirrel.app");
                bundle(&location, OLD, OLD);
                let src = root.join("payload-src/PhotoSquirrel.app");
                bundle(&src, NEW, payload_cli_version);
                let payload = ready.join("PhotoSquirrel-0.4.1-macos-universal.app.tar.gz");
                let ok = Command::new("tar")
                    .arg("-czf")
                    .arg(&payload)
                    .arg("-C")
                    .arg(root.join("payload-src"))
                    .arg("PhotoSquirrel.app")
                    .status()
                    .unwrap();
                assert!(ok.success());
                (location, payload)
            }
            Kind::WindowsSetup => {
                let location = root.join("Local/PhotoSquirrel");
                program(&location.join("photosquirrel.exe"), "photosquirrel", OLD);
                program(
                    &location.join("photosquirrel-cli.exe"),
                    "photosquirrel-cli",
                    OLD,
                );
                fs::write(location.join("THIRD-PARTY-LICENSES"), "old notices").unwrap();
                let payload = ready.join("PhotoSquirrel-0.4.1-windows-x86_64-setup.exe");
                // The "installer" writes the new programs into the installation, as NSIS does.
                let loc = location.display();
                script(
                    &payload,
                    &format!(
                        "[ \"$1\" = /S ] && [ \"$2\" = /NS ] || exit 3\n\
                         printf '#!/bin/sh\\necho photosquirrel {NEW}\\n' > '{loc}/photosquirrel.exe'\n\
                         printf '#!/bin/sh\\necho photosquirrel-cli {payload_cli_version}\\n' > '{loc}/photosquirrel-cli.exe'\n\
                         printf 'new notices' > '{loc}/THIRD-PARTY-LICENSES'"
                    ),
                );
                (location, payload)
            }
            _ => unreachable!(),
        };
        let fake = Self {
            _tmp: tmp,
            root,
            dir,
            location,
            kind,
        };
        let plan = Plan {
            action: Action::Install,
            version: NEW.into(),
            current: OLD.into(),
            kind: kind.id().into(),
            location: fake.location.clone(),
            payload: payload.clone(),
            current_max_catalog_schema: 27,
            relaunch: false,
            wait_pid: None,
            downloads: Some(fake.root.join("Downloads")),
            sha256: Some(sha_of(&payload)),
            size: Some(fs::metadata(&payload).unwrap().len()),
            step: Step::Planned,
        };
        plan.save(&fake.plan()).unwrap();
        let state = UpdateState {
            ready: Some(Ready {
                version: NEW.into(),
                kind: "x".into(),
                sha256: sha_of(&payload),
                size: fs::metadata(&payload).unwrap().len(),
                file: payload,
            }),
            ..Default::default()
        };
        state.save(&fake.dir).unwrap();
        fake
    }

    fn plan(&self) -> PathBuf {
        Plan::file(&self.dir, NEW)
    }

    fn finish(&self, stop: StopAfter) -> Result<Step, UpdateError> {
        finish(&self.dir, &self.plan(), Duration::from_secs(1), stop)
    }

    /// The installed app's answer to `--version`.
    fn installed(&self) -> String {
        match self.kind {
            Kind::AppImage => version_of(&self.location),
            Kind::MacApp => version_of(&self.location.join("Contents/MacOS/photosquirrel")),
            _ => version_of(&self.location.join("photosquirrel.exe")),
        }
    }

    /// Every file of the installation and its bytes.
    fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        snap(&self.location, &mut out);
        out.sort();
        out
    }
}

fn snap(p: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    if p.is_dir() {
        for e in fs::read_dir(p).unwrap() {
            snap(&e.unwrap().path(), out);
        }
    } else {
        out.push((p.to_path_buf(), fs::read(p).unwrap()));
    }
}

fn bundle(at: &Path, app: &str, cli: &str) {
    let bin = at.join("Contents/MacOS");
    program(&bin.join("photosquirrel"), "photosquirrel", app);
    program(&bin.join("photosquirrel-cli"), "photosquirrel-cli", cli);
    fs::create_dir_all(at.join("Contents/Resources")).unwrap();
    fs::write(
        at.join("Contents/Info.plist"),
        format!("<plist>{app}</plist>"),
    )
    .unwrap();
}

const KINDS: [Kind; 3] = [Kind::AppImage, Kind::MacApp, Kind::WindowsSetup];

#[test]
fn an_install_replaces_the_installation_and_records_the_trial() {
    for kind in KINDS {
        let f = Fake::new(kind, NEW);
        assert_eq!(f.finish(None).unwrap(), Step::Done, "{kind:?}");
        assert_eq!(f.installed(), format!("photosquirrel {NEW}"), "{kind:?}");
        let state = UpdateState::load(&f.dir);
        assert_eq!(
            state.trial,
            Some(Trial {
                version: NEW.into(),
                starts: 0,
                ok: false
            })
        );
        let kept = state.kept.expect("kept");
        assert_eq!((kept.version.as_str(), kept.max_catalog_schema), (OLD, 27));
        assert!(
            kept.path.exists(),
            "{kind:?}: the kept copy {} exists",
            kept.path.display()
        );
        assert_eq!(state.ready, None);
        // No staging left beside the installation.
        let parent = f.location.parent().unwrap();
        let leftovers: Vec<_> = fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{kind:?}: {leftovers:?}");
        // Running it again does nothing more.
        assert_eq!(f.finish(None).unwrap(), Step::Done);
    }
}

#[test]
fn scenario_app_updates_broken_new_binary_rolled_back_at_install() {
    for kind in KINDS {
        let f = Fake::new(kind, "0.0.9");
        let before = f.snapshot();
        let e = f.finish(None).expect_err("refused");
        assert!(e.to_string().contains("instead of"), "{kind:?}: {e}");
        assert_eq!(
            f.snapshot(),
            before,
            "{kind:?}: the previous version, byte for byte"
        );
        let state = UpdateState::load(&f.dir);
        let failure = state.last_failure.clone().expect("recorded");
        assert_eq!(
            (failure.version.as_str(), failure.current.as_str()),
            (NEW, OLD)
        );
        assert!(state.is_skipped(NEW), "{kind:?}: not offered again");
        let history = crate::history::read(&f.dir);
        assert!(
            history
                .iter()
                .any(|h| h.event == "failure" && h.detail.contains("put back"))
        );
    }
}

#[test]
fn scenario_app_updates_swap_is_complete_or_not_at_all() {
    for kind in KINDS {
        for stop in [Step::BackedUp, Step::Staged, Step::Swapped] {
            let f = Fake::new(kind, NEW);
            assert!(
                matches!(f.finish(Some(stop)), Err(UpdateError::Stopped)),
                "{kind:?} {stop:?}"
            );
            let now = f.installed();
            assert!(
                now == format!("photosquirrel {OLD}") || now == format!("photosquirrel {NEW}"),
                "{kind:?} after {stop:?}: {now}"
            );
            if kind != Kind::WindowsSetup {
                // One whole version: the CLI agrees with the app.
                let cli = match kind {
                    Kind::MacApp => {
                        version_of(&f.location.join("Contents/MacOS/photosquirrel-cli"))
                    }
                    _ => now.replace("photosquirrel", "photosquirrel-cli"),
                };
                assert_eq!(
                    cli,
                    now.replace("photosquirrel", "photosquirrel-cli"),
                    "{kind:?} {stop:?}"
                );
            }
            assert_eq!(
                f.finish(None).unwrap(),
                Step::Done,
                "{kind:?} {stop:?}: the rerun finishes"
            );
            assert_eq!(f.installed(), format!("photosquirrel {NEW}"));
        }
    }
}

#[test]
fn scenario_app_updates_running_cli_postpones_the_install() {
    let f = Fake::new(Kind::AppImage, NEW);
    let before = f.snapshot();
    // A CLI of this installation is running: it holds the lock shared.
    let held = hold_running(&f.dir, &f.location).expect("lock");
    assert!(running(&f.dir, &f.location));
    let e = f.finish(None).expect_err("postponed");
    assert!(matches!(e, UpdateError::StillRunning));
    assert_eq!(f.snapshot(), before, "no installed file changed");
    assert!(
        UpdateState::load(&f.dir).ready.is_some(),
        "the update is still ready"
    );
    let history = crate::history::read(&f.dir);
    assert_eq!(
        history.last().map(|h| h.detail.as_str()),
        Some("postponed: PhotoSquirrel is still running")
    );
    drop(held);
    assert!(!running(&f.dir, &f.location));
    assert_eq!(
        f.finish(None).unwrap(),
        Step::Done,
        "the next attempt installs"
    );
}

#[test]
fn scenario_app_updates_refused_swap_falls_back_to_a_manual_install() {
    let f = Fake::new(Kind::MacApp, NEW);
    let before = f.snapshot();
    REFUSE_EXCHANGE.with(|r| r.set(true));
    let e = f.finish(None).expect_err("refused");
    REFUSE_EXCHANGE.with(|r| r.set(false));
    let UpdateError::Fallback(saved) = e else {
        panic!("{e}");
    };
    assert_eq!(f.snapshot(), before, "the installed bundle is unchanged");
    assert!(
        saved.starts_with(f.root.join("Downloads")),
        "{}",
        saved.display()
    );
    assert!(saved.is_file());
    let state = UpdateState::load(&f.dir);
    assert!(
        state.fallback_offered.contains(&NEW.to_owned()),
        "not retried at the next quit"
    );
    assert!(
        !state.is_skipped(NEW),
        "the user can still install it by hand"
    );
}

/// A fake `codesign`: a bundle's Team ID is the text of its `team` file; `adhoc` or no file is an ad-hoc
/// signature (no Team ID), and only a bundle with a `team` file verifies.
fn fake_codesign(root: &Path) -> PathBuf {
    let path = root.join("bin/codesign");
    script(
        &path,
        "case \"$1\" in\n\
         -dv) t=$(cat \"$2/team\" 2>/dev/null)\n\
              if [ -n \"$t\" ] && [ \"$t\" != adhoc ]; then echo \"TeamIdentifier=$t\" >&2;\n\
              else echo 'TeamIdentifier=not set' >&2; fi ;;\n\
         --verify) [ -f \"$3/team\" ] ;;\n\
         esac",
    );
    path
}

/// Sign the installed bundle with `installed` and the offered one with `offered`, then re-pack the payload.
fn sign_bundles(f: &Fake, installed: &str, offered: &str) {
    fs::write(f.location.join("team"), installed).unwrap();
    let src = f.root.join("payload-src");
    fs::write(src.join("PhotoSquirrel.app/team"), offered).unwrap();
    let payload = f
        .dir
        .ready_dir(NEW)
        .join("PhotoSquirrel-0.4.1-macos-universal.app.tar.gz");
    let ok = Command::new("tar")
        .arg("-czf")
        .arg(&payload)
        .arg("-C")
        .arg(&src)
        .arg("PhotoSquirrel.app")
        .status()
        .unwrap();
    assert!(ok.success());
    // The manifest would describe the re-packed payload.
    let mut plan = Plan::load(&f.plan()).unwrap();
    plan.sha256 = Some(sha_of(&payload));
    plan.size = Some(fs::metadata(&payload).unwrap().len());
    plan.save(&f.plan()).unwrap();
}

#[test]
fn a_payload_changed_after_its_download_is_not_installed() {
    for kind in KINDS {
        let f = Fake::new(kind, NEW);
        let before = f.snapshot();
        let plan = Plan::load(&f.plan()).unwrap();
        // Same size, other bytes: only the hash tells.
        let mut bytes = fs::read(&plan.payload).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&plan.payload, bytes).unwrap();
        let e = f.finish(None).expect_err("refused");
        assert!(matches!(e, UpdateError::Mismatch), "{kind:?}: {e}");
        assert_eq!(
            f.snapshot(),
            before,
            "{kind:?}: the installation is unchanged"
        );
        assert!(UpdateState::load(&f.dir).is_skipped(NEW), "{kind:?}");
    }
}

#[test]
fn a_plan_that_points_outside_the_update_folder_is_refused() {
    let f = Fake::new(Kind::AppImage, NEW);
    let before = f.snapshot();
    // A version that is a path, and a payload elsewhere.
    for change in [0, 1] {
        let mut plan = Plan::load(&f.plan()).unwrap();
        if change == 0 {
            plan.current = "../..".into();
        } else {
            let elsewhere = f.root.join("elsewhere.AppImage");
            fs::copy(&plan.payload, &elsewhere).unwrap();
            plan.payload = elsewhere;
        }
        let path = f.dir.root().join("crafted.json");
        plan.save(&path).unwrap();
        let e = finish(&f.dir, &path, Duration::from_secs(1), None).expect_err("refused");
        assert!(e.to_string().contains("plan:"), "{e}");
        assert_eq!(f.snapshot(), before);
    }
    assert!(f.root.join("Apps").is_dir(), "nothing outside was removed");
}

#[test]
fn a_version_is_never_installed_over_itself_and_the_planner_holds_the_lock() {
    let f = Fake::new(Kind::AppImage, NEW);
    let installation = crate::kind::Installation {
        kind: Kind::AppImage,
        location: f.location.clone(),
        cannot_install: None,
    };
    // 0.4.1 is ready: an installed 0.4.1 (or newer) does not plan it.
    for current in [NEW, "0.5.0"] {
        let e = plan_install(&f.dir, &installation, current, 27, false, None).expect_err("refused");
        assert!(e.to_string().contains("is not newer"), "{e}");
    }
    // 0.4.0 does, and from then on this process counts as running until it exits.
    plan_install(&f.dir, &installation, OLD, 27, false, None).unwrap();
    assert!(
        running(&f.dir, &f.location),
        "the helper waits for this process"
    );
    release_exit_locks();
    assert!(!running(&f.dir, &f.location));
    // After the install the offer is gone.
    let mut state = UpdateState::load(&f.dir);
    state.available = Some(crate::testing::sample_manifest(NEW, &[]));
    state.save(&f.dir).unwrap();
    assert_eq!(f.finish(None).unwrap(), Step::Done);
    assert!(UpdateState::load(&f.dir).available.is_none());
}

#[test]
fn scenario_app_updates_bundle_from_another_signer_refused() {
    for offered in ["adhoc", "OTHERTEAM1"] {
        let f = Fake::new(Kind::MacApp, NEW);
        CODESIGN.with(|c| *c.borrow_mut() = Some(fake_codesign(&f.root)));
        sign_bundles(&f, "TEAMID1234", offered);
        let before = f.snapshot();
        let e = f.finish(None).expect_err("refused");
        CODESIGN.with(|c| *c.borrow_mut() = None);
        assert!(
            e.to_string()
                .contains("the update is not signed by the same developer"),
            "{offered}: {e}"
        );
        assert_eq!(
            f.snapshot(),
            before,
            "{offered}: the installed bundle is unchanged"
        );
        assert!(
            UpdateState::load(&f.dir).is_skipped(NEW),
            "{offered}: the version is skipped"
        );
    }
    // The same Team ID installs.
    let f = Fake::new(Kind::MacApp, NEW);
    CODESIGN.with(|c| *c.borrow_mut() = Some(fake_codesign(&f.root)));
    sign_bundles(&f, "TEAMID1234", "TEAMID1234");
    let done = f.finish(None);
    CODESIGN.with(|c| *c.borrow_mut() = None);
    assert_eq!(done.unwrap(), Step::Done);
}

#[test]
fn a_rollback_puts_the_kept_version_back() {
    for kind in KINDS {
        let f = Fake::new(kind, NEW);
        f.finish(None).unwrap();
        let kept = UpdateState::load(&f.dir).kept.unwrap();
        let plan = Plan {
            action: Action::Rollback,
            version: OLD.into(),
            current: NEW.into(),
            kind: kind.id().into(),
            location: f.location.clone(),
            payload: kept.path.clone(),
            current_max_catalog_schema: 27,
            relaunch: false,
            wait_pid: None,
            downloads: None,
            sha256: None,
            size: None,
            step: Step::Planned,
        };
        let path = f.dir.root().join("rollback.json");
        plan.save(&path).unwrap();
        assert_eq!(
            finish(&f.dir, &path, Duration::from_secs(1), None).unwrap(),
            Step::Done,
            "{kind:?}"
        );
        assert_eq!(f.installed(), format!("photosquirrel {OLD}"), "{kind:?}");
        let state = UpdateState::load(&f.dir);
        assert!(
            state.is_skipped(NEW),
            "{kind:?}: the newer version is not offered again automatically"
        );
        assert_eq!((state.kept, state.trial), (None, None));
    }
}
