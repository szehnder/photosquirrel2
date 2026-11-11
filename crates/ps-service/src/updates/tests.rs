#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ps_model::settings::UpdateMode;
use ps_update::kind::{Installation, Kind};
use ps_update::state::{CheckRecord, UpdateState};
use ps_update::testing::{Route, TestServer, envelope, sample_manifest, updater};

use super::*;

const HOUR: i64 = 60 * 60;
const DAY: i64 = 24 * HOUR;

/// A switchable mode, standing in for the app's settings.
fn mode(m: UpdateMode) -> (ModeSource, Arc<Mutex<UpdateMode>>) {
    let cell = Arc::new(Mutex::new(m));
    let c = cell.clone();
    (Arc::new(move || *c.lock().unwrap()), cell)
}

fn appimage(dir: &Path) -> Installation {
    Installation {
        kind: Kind::AppImage,
        location: dir.join("PhotoSquirrel.AppImage"),
        cannot_install: None,
    }
}

fn archive(dir: &Path) -> Installation {
    Installation {
        kind: Kind::Archive,
        location: dir.to_path_buf(),
        cannot_install: Some("Unpacked archive".into()),
    }
}

struct Fixture {
    server: TestServer,
    _tmp: tempfile::TempDir,
    dir: PathBuf,
}

impl Fixture {
    /// A server offering 0.4.1 with an AppImage of `payload`.
    fn new(urgent: bool) -> Self {
        let server = TestServer::start();
        let payload = b"the 0.4.1 appimage".to_vec();
        let mut m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", &payload)]);
        m.urgent = urgent;
        server.route(
            "/update-manifest.json",
            Route::Body(envelope(&m).into_bytes()),
        );
        let name = m.packages["linux-x86_64"]["appimage"].name.clone();
        server.route(&format!("/{name}"), Route::Body(payload));
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("updates");
        Self {
            server,
            _tmp: tmp,
            dir,
        }
    }

    fn service(&self, m: ModeSource, inst: Installation, started_at: i64) -> UpdateService {
        UpdateService::new(
            updater(&self.server, &self.dir, "0.4.0"),
            inst,
            m,
            started_at,
            None,
        )
    }

    fn manifest_requests(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|r| r.path == "/update-manifest.json")
            .count()
    }

    fn payload_requests(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|r| r.path != "/update-manifest.json")
            .count()
    }
}

/// Tick every 10 minutes of app time from `from` to `to`.
fn run(s: &UpdateService, from: i64, to: i64) {
    let mut t = from;
    while t <= to {
        s.tick(t);
        t += 10 * 60;
    }
}

#[test]
fn scenario_app_updates_undecided_mode_sends_nothing() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Undecided);
    let s = f.service(m, appimage(&f.dir), t0);
    run(&s, t0, t0 + 48 * HOUR);
    assert!(f.server.requests().is_empty(), "no request while undecided");
    assert_eq!(s.status().mode, UpdateMode::Undecided);
    assert_eq!(s.status().mode.id(), "undecided");
}

#[test]
fn scenario_app_updates_first_check_after_60_seconds() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Automatic);
    let s = f.service(m, appimage(&f.dir), t0);
    for t in [t0, t0 + 1, t0 + 30, t0 + 59] {
        s.tick(t);
    }
    assert_eq!(f.manifest_requests(), 0, "nothing in the first 60 s");
    let mut t = t0 + 60;
    while t <= t0 + 60 + 5 * 60 {
        s.tick(t);
        t += 15;
    }
    assert_eq!(
        f.manifest_requests(),
        1,
        "exactly one check in the next 5 minutes"
    );
}

#[test]
fn scenario_app_updates_at_most_one_check_a_day_across_restarts() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    // A check completed 3 hours before this start (no jitter, to make the boundary exact).
    let dir = ps_update::UpdateDir::at(&f.dir);
    let state = UpdateState {
        last_check: Some(CheckRecord {
            at: t0 - 3 * HOUR,
            ok: true,
            message: "up to date".into(),
        }),
        ..Default::default()
    };
    state.save(&dir).unwrap();
    let (m, _) = mode(UpdateMode::Notify);
    let s = f.service(m, appimage(&f.dir), t0);
    run(&s, t0, t0 + 21 * HOUR - 1);
    assert_eq!(f.manifest_requests(), 0, "no request until 21 hours later");
    run(&s, t0 + 21 * HOUR, t0 + 22 * HOUR);
    assert_eq!(f.manifest_requests(), 1, "then the daily check");
}

#[test]
fn scenario_app_updates_manual_check_with_mode_off() {
    let f = Fixture::new(false);
    let t0 = ps_update::now_secs();
    let (m, _) = mode(UpdateMode::Off);
    let s = f.service(m, appimage(&f.dir), t0);
    let outcome = s.check_now().unwrap();
    assert!(matches!(outcome, Outcome::Available(ref m) if m.version == "0.4.1"));
    assert_eq!(f.manifest_requests(), 1);
    assert_eq!(
        s.status().last_check.unwrap().message,
        "PhotoSquirrel 0.4.1 is available"
    );
    run(&s, t0, t0 + 48 * HOUR);
    assert_eq!(f.manifest_requests(), 1, "no further request with mode off");
    assert_eq!(f.payload_requests(), 0, "nothing downloaded with mode off");
}

#[test]
fn scenario_app_updates_notify_mode_waits_for_the_user() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Notify);
    let s = f.service(m, appimage(&f.dir), t0);
    run(&s, t0, t0 + HOUR);
    let status = s.status();
    assert_eq!(f.manifest_requests(), 1);
    assert_eq!(
        f.payload_requests(),
        0,
        "no download until the user chooses"
    );
    let notice = status.notice.unwrap();
    assert!(
        matches!(notice, Notice::Available(ref a) if a.version == "0.4.1" && a.notes.contains("0.4.1"))
    );
    // "Install on quit": downloaded and verified now, ready for the quit.
    s.install_on_quit().unwrap();
    assert_eq!(f.payload_requests(), 1);
    let status = s.status();
    assert_eq!(status.ready.as_deref(), Some("0.4.1"));
    assert!(matches!(status.notice, Some(Notice::Ready(_))));
}

#[test]
fn automatic_mode_downloads_in_the_background() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Automatic);
    let s = f.service(m, appimage(&f.dir), t0);
    run(&s, t0, t0 + HOUR);
    assert_eq!(f.payload_requests(), 1);
    let status = s.status();
    assert_eq!(status.ready.as_deref(), Some("0.4.1"));
    assert_eq!(
        status.notice.unwrap().text(),
        "PhotoSquirrel 0.4.1 is ready. It installs when you quit."
    );
}

#[test]
fn scenario_app_updates_archive_installation_notified_only() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Automatic);
    let s = f.service(m, archive(&f.dir), t0);
    run(&s, t0, t0 + 2 * HOUR);
    assert_eq!(f.payload_requests(), 0, "nothing is downloaded");
    match s.status().notice {
        Some(Notice::Manual(a)) => {
            assert_eq!(a.version, "0.4.1");
            assert!(
                a.notes_url.ends_with("/releases/tag/v0.4.1"),
                "{}",
                a.notes_url
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn scenario_app_updates_offline_user_is_not_nagged() {
    let f = Fixture::new(false);
    f.server.route("/update-manifest.json", Route::Status(503));
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Automatic);
    let s = f.service(m, appimage(&f.dir), t0);
    // Two days of failed checks: nothing shown, Settings has the failure.
    run(&s, t0, t0 + DAY + 2 * HOUR);
    assert_eq!(f.manifest_requests(), 2);
    let status = s.status();
    assert_eq!(status.notice, None);
    assert!(!status.failing);
    let last = status.last_check.unwrap();
    assert!(!last.ok && last.message.contains("503"), "{}", last.message);
    assert_eq!(status.last_success_at, None);
    // The third day in a row: a quiet note for the status bar.
    // (Each check draws up to an hour of jitter: the third falls before 2 days and 4 hours.)
    run(&s, t0 + DAY + 2 * HOUR + 600, t0 + 2 * DAY + 4 * HOUR);
    assert_eq!(f.manifest_requests(), 3);
    assert!(s.status().failing);
}

#[test]
fn scenario_app_updates_urgent_notice_persists() {
    let f = Fixture::new(true);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Notify);
    let s = f.service(m.clone(), appimage(&f.dir), t0);
    run(&s, t0, t0 + HOUR);
    assert!(matches!(s.status().notice, Some(Notice::Available(ref a)) if a.urgent));
    // Every start shows it again until it is acted on or dismissed.
    for restart in 1..=3 {
        let again = f.service(m.clone(), appimage(&f.dir), t0 + restart * DAY);
        assert!(
            matches!(again.status().notice, Some(Notice::Available(ref a)) if a.urgent),
            "start {restart}"
        );
    }
    s.dismiss();
    assert_eq!(s.status().notice, None);
    let after = f.service(m, appimage(&f.dir), t0 + 5 * DAY);
    assert_eq!(after.status().notice, None, "dismissed for that version");
}

#[test]
fn a_failure_and_the_updated_notice_are_shown_once() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Automatic);
    let s = f.service(m, appimage(&f.dir), t0);
    record_failure(
        &updater(&f.server, &f.dir, "0.4.0"),
        "0.4.1",
        "the new version did not start",
    );
    s.tick(t0);
    let n = s.status().notice.unwrap();
    assert_eq!(
        n.text(),
        "Couldn't install PhotoSquirrel 0.4.1. You're still on 0.4.0."
    );
    s.dismiss();
    assert_eq!(s.status().notice, None);
}

#[test]
fn the_data_directory_is_remembered_for_the_rollback_guard() {
    let f = Fixture::new(false);
    let (m, _) = mode(UpdateMode::Off);
    let data = f.dir.join("catalog");
    UpdateService::new(
        updater(&f.server, &f.dir, "0.4.0"),
        appimage(&f.dir),
        m,
        0,
        Some(&data),
    );
    let state = UpdateState::load(&ps_update::UpdateDir::at(&f.dir));
    assert_eq!(state.catalogs, vec![data]);
}

// Trial starts and rollback (spec "Failed start restores the previous version", "Guarded manual rollback").
// The installation is a fake AppImage and the CLI a script recording how the helper was started (Unix).
#[cfg(unix)]
mod trial {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use ps_update::install::{self, Action, Plan};
    use ps_update::state::{Kept, Trial};

    use super::*;

    fn script(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    struct Installed {
        f: Fixture,
        installation: Installation,
        cli: PathBuf,
        calls: PathBuf,
        data: PathBuf,
    }

    /// 0.4.1 installed over 0.4.0, which is kept; the catalog in `data` has format `catalog_version`.
    fn installed(catalog_version: Option<u32>) -> Installed {
        let f = Fixture::new(false);
        let root = f.dir.parent().unwrap().to_path_buf();
        let location = root.join("Apps/PhotoSquirrel.AppImage");
        script(&location, "echo 'photosquirrel 0.4.1'");
        let kept = f.dir.join("previous/0.4.0/PhotoSquirrel.AppImage");
        script(&kept, "echo 'photosquirrel 0.4.0'");
        let calls = root.join("helper-calls.txt");
        let cli = root.join("bin/photosquirrel-cli");
        script(&cli, &format!("echo \"$@\" > '{}'", calls.display()));
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        if let Some(v) = catalog_version {
            let conn = rusqlite::Connection::open(data.join(ps_catalog::CATALOG_FILE)).unwrap();
            conn.pragma_update(None, "user_version", v).unwrap();
        }
        let dir = UpdateDir::at(&f.dir);
        let state = UpdateState {
            trial: Some(Trial {
                version: "0.4.1".into(),
                starts: 0,
                ok: false,
            }),
            kept: Some(Kept {
                version: "0.4.0".into(),
                path: kept,
                max_catalog_schema: 27,
            }),
            catalogs: vec![data.clone()],
            ..Default::default()
        };
        state.save(&dir).unwrap();
        let installation = Installation {
            kind: Kind::AppImage,
            location,
            cannot_install: None,
        };
        Installed {
            f,
            installation,
            cli,
            calls,
            data,
        }
    }

    fn new_version(i: &Installed) -> Updater {
        updater(&i.f.server, &i.f.dir, "0.4.1")
    }

    fn wait_for(path: &Path) -> String {
        for _ in 0..200 {
            if let Ok(t) = std::fs::read_to_string(path)
                && !t.is_empty()
            {
                return t;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("{} was not written", path.display());
    }

    #[test]
    fn scenario_app_updates_new_version_that_cannot_start_is_rolled_back() {
        let i = installed(Some(27));
        let u = new_version(&i);
        // Two starts that never draw a frame...
        assert_eq!(
            trial_start(&u, &i.installation, Some(&i.cli)),
            TrialStart::Trial
        );
        assert_eq!(
            trial_start(&u, &i.installation, Some(&i.cli)),
            TrialStart::Trial
        );
        // ...and the third puts the previous version back through the helper.
        assert_eq!(
            trial_start(&u, &i.installation, Some(&i.cli)),
            TrialStart::RollingBack
        );
        let args = wait_for(&i.calls);
        assert!(args.starts_with("update finish --plan "), "{args}");
        let plan_path = PathBuf::from(args.split_whitespace().nth(3).unwrap());
        let plan = Plan::load(&plan_path).unwrap();
        assert_eq!(
            (plan.action, plan.version.as_str(), plan.relaunch),
            (Action::Rollback, "0.4.0", true)
        );
        // What the helper then does: the previous version is back and 0.4.1 is not offered again.
        let mut plan = plan;
        plan.relaunch = false;
        plan.save(&plan_path).unwrap();
        // This test is the app and then the helper: the app's lock ends with its process.
        ps_update::install::release_exit_locks();
        install::finish(&u.dir, &plan_path, Duration::from_secs(1), None).unwrap();
        let out = std::process::Command::new(&i.installation.location)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "photosquirrel 0.4.0"
        );
        let state = UpdateState::load(&u.dir);
        assert!(state.is_skipped("0.4.1"));
        // The previous version's next start says what happened.
        let old = updater(&i.f.server, &i.f.dir, "0.4.0");
        let status = compute_status(&old, &i.installation, UpdateMode::Automatic, None, 0);
        assert_eq!(
            status.notice.unwrap().text(),
            "PhotoSquirrel 0.4.1 could not start, so 0.4.0 was put back."
        );
    }

    #[test]
    fn a_trial_after_a_catalog_upgrade_keeps_the_new_version() {
        let i = installed(Some(30));
        let u = new_version(&i);
        for _ in 0..2 {
            trial_start(&u, &i.installation, Some(&i.cli));
        }
        assert_eq!(
            trial_start(&u, &i.installation, Some(&i.cli)),
            TrialStart::Kept
        );
        assert!(!i.calls.exists(), "no helper started");
        let status = compute_status(&u, &i.installation, UpdateMode::Automatic, None, 0);
        assert!(
            status
                .notice
                .unwrap()
                .text()
                .contains("going back is not possible")
        );
        assert_eq!(
            trial_start(&u, &i.installation, Some(&i.cli)),
            TrialStart::Normal,
            "asked once"
        );
    }

    #[test]
    fn scenario_app_updates_trial_start_opens_the_catalog_after_the_first_frame() {
        // No catalog yet: opening creates and migrates one, which a trial start must not do before its first
        // frame (the app holds the library back from `main()` and lets it go at the first frame).
        let i = installed(None);
        let u = new_version(&i);
        let library = Arc::new(crate::library::Library::new(&i.data));
        library.defer_open();
        assert_eq!(
            trial_start(&u, &i.installation, Some(&i.cli)),
            TrialStart::Trial
        );
        let lib = library.clone();
        let summary = std::thread::spawn(move || lib.summary().map(|s| s.photos));
        std::thread::sleep(Duration::from_millis(300));
        let catalog = i.data.join(ps_catalog::CATALOG_FILE);
        assert!(
            !catalog.exists(),
            "the catalog is not opened before the first frame"
        );
        assert!(
            !summary.is_finished(),
            "the summary waits for the first frame"
        );
        assert!(library.opened().is_none());
        // The first frame: the trial is recorded, then the catalog opens and is migrated.
        trial_succeeded(&u.dir);
        assert!(UpdateState::load(&u.dir).trial.unwrap().ok);
        library.allow_open();
        assert_eq!(summary.join().unwrap().unwrap(), 0);
        assert_eq!(
            ps_catalog::db::stored_version(&catalog).unwrap(),
            Some(ps_catalog::latest_schema_version())
        );
    }

    #[test]
    fn scenario_app_updates_successful_trial_keeps_the_new_version() {
        let i = installed(Some(27));
        let u = new_version(&i);
        assert_eq!(
            trial_start(&u, &i.installation, Some(&i.cli)),
            TrialStart::Trial
        );
        trial_succeeded(&u.dir);
        assert_eq!(
            trial_start(&u, &i.installation, Some(&i.cli)),
            TrialStart::Normal
        );
        let (m, _) = mode(UpdateMode::Automatic);
        let s = UpdateService::new(u.clone(), i.installation.clone(), m, 0, None);
        let n = s.status().notice.unwrap();
        assert_eq!(n, Notice::Updated("0.4.1".into()));
        s.dismiss();
        assert_eq!(s.status().notice, None, "once");
        assert!(
            UpdateState::load(&u.dir).kept.is_some(),
            "the kept copy stays"
        );
    }

    #[test]
    fn scenario_app_updates_rollback_refused_after_a_catalog_upgrade() {
        let i = installed(Some(28));
        let u = new_version(&i);
        let e = rollback_after_exit(&u, &i.installation, &i.cli, false).unwrap_err();
        let text = e.to_string();
        assert!(
            text.starts_with("the catalog was upgraded by 0.4.1; 0.4.0 cannot open it"),
            "{text}"
        );
        assert!(text.contains(&i.data.display().to_string()), "{text}");
        assert!(!i.calls.exists(), "nothing is changed");
        let out = std::process::Command::new(&i.installation.location)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "photosquirrel 0.4.1"
        );
    }

    #[test]
    fn scenario_app_updates_rollback_before_a_catalog_upgrade() {
        let i = installed(Some(27));
        let u = new_version(&i);
        rollback_after_exit(&u, &i.installation, &i.cli, false).unwrap();
        let args = wait_for(&i.calls);
        let plan_path = PathBuf::from(args.split_whitespace().nth(3).unwrap());
        // This test is the app and then the helper: the app's lock ends with its process.
        ps_update::install::release_exit_locks();
        install::finish(&u.dir, &plan_path, Duration::from_secs(1), None).unwrap();
        let out = std::process::Command::new(&i.installation.location)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "photosquirrel 0.4.0"
        );
        assert!(
            UpdateState::load(&u.dir).is_skipped("0.4.1"),
            "not offered again automatically"
        );
    }

    #[test]
    fn scenario_app_updates_failed_install_explained() {
        let i = installed(Some(27));
        let u = updater(&i.f.server, &i.f.dir, "0.4.0");
        record_failure(&u, "0.4.1", "the new version reported the wrong version");
        let status = compute_status(&u, &i.installation, UpdateMode::Automatic, None, 0);
        assert_eq!(
            status.notice.unwrap().text(),
            "Couldn't install PhotoSquirrel 0.4.1. You're still on 0.4.0."
        );
        // Details: the update history holds the failure.
        let history = ps_update::history::read(&u.dir);
        assert!(
            history
                .iter()
                .any(|h| h.event == "failure" && h.version.as_deref() == Some("0.4.1"))
        );
    }
}

#[test]
fn scenario_app_updates_restart_now_waits_for_a_scan() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Automatic);
    let s = f.service(m, appimage(&f.dir), t0);
    run(&s, t0, t0 + HOUR);
    assert_eq!(s.status().ready.as_deref(), Some("0.4.1"));
    // A scan runs: Restart now says why it waits, and does nothing.
    let scanning = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let probe = scanning.clone();
    s.set_busy_probe(Arc::new(move || {
        probe
            .load(std::sync::atomic::Ordering::Relaxed)
            .then_some("the scan")
    }));
    s.tick(t0 + HOUR + 60);
    assert_eq!(s.status().restart_blocked_by, Some("the scan"));
    let e = s.restart_now().unwrap_err();
    assert_eq!(e.to_string(), "Available when the scan finishes");
    // The scan completes: it is available.
    scanning.store(false, std::sync::atomic::Ordering::Relaxed);
    s.tick(t0 + HOUR + 120);
    assert_eq!(s.status().restart_blocked_by, None);
}

/// The version an update just installed starts with that update's offer and payload still on record: it must not
/// download or install itself again (gate review H1).
#[test]
fn a_freshly_installed_version_does_not_install_itself_again() {
    let f = Fixture::new(false);
    let t0 = 1_800_000_000;
    let (m, _) = mode(UpdateMode::Automatic);
    // 0.4.0 finds and downloads 0.4.1...
    let old = f.service(m.clone(), appimage(&f.dir), t0);
    run(&old, t0, t0 + HOUR);
    let state = UpdateState::load(&UpdateDir::at(&f.dir));
    assert!(state.available.is_some() && state.ready.is_some());
    let downloads = f.payload_requests();
    // ...and now 0.4.1 runs with that state.
    let new = UpdateService::new(
        updater(&f.server, &f.dir, "0.4.1"),
        appimage(&f.dir),
        m,
        t0 + 2 * HOUR,
        None,
    );
    let state = UpdateState::load(&UpdateDir::at(&f.dir));
    assert!(state.available.is_none() && state.ready.is_none());
    new.set_cli(f.dir.join("no-such-cli"));
    run(&new, t0 + 2 * HOUR, t0 + 3 * HOUR);
    assert_eq!(
        f.payload_requests(),
        downloads,
        "nothing is downloaded again"
    );
    assert!(!new.on_quit(), "nothing is installed at quit");
    assert_eq!(new.status().ready, None);
}

#[test]
fn the_mode_cell_is_read_from_any_thread() {
    let cell = ModeCell::new(UpdateMode::Notify);
    let source = cell.source();
    assert_eq!(source(), UpdateMode::Notify);
    cell.set(UpdateMode::Off);
    assert_eq!(
        std::thread::spawn(move || source()).join().unwrap(),
        UpdateMode::Off
    );
}

/// Quitting with a ready update in automatic mode starts the installer helper; what it then does leaves the
/// catalog and settings alone and installs the new version (spec "Quit installs a ready update").
#[cfg(unix)]
#[test]
fn scenario_app_updates_quit_installs_a_ready_update() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let f = Fixture::new(false);
    let root = f.dir.parent().unwrap().to_path_buf();
    // The installed AppImage, the server's new one, and a CLI that records how the helper was started.
    let location = root.join("Apps/PhotoSquirrel.AppImage");
    let write_script = |p: &Path, body: &str| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    write_script(&location, "echo 'photosquirrel 0.4.0'");
    let payload = "#!/bin/sh\necho 'photosquirrel 0.4.1'\n";
    let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", payload.as_bytes())]);
    f.server.route(
        "/update-manifest.json",
        Route::Body(envelope(&m).into_bytes()),
    );
    let name = m.packages["linux-x86_64"]["appimage"].name.clone();
    f.server.route(
        &format!("/{name}"),
        Route::Body(payload.as_bytes().to_vec()),
    );
    let calls = root.join("helper-calls.txt");
    let cli = root.join("bin/photosquirrel-cli");
    write_script(&cli, &format!("echo \"$@\" > '{}'", calls.display()));
    let data = root.join("data");
    std::fs::create_dir_all(data.join("config")).unwrap();
    std::fs::write(data.join("config/settings.toml"), "settings_version = 4\n").unwrap();
    std::fs::write(data.join(ps_catalog::CATALOG_FILE), b"catalog bytes").unwrap();

    let (mode_source, _) = mode(UpdateMode::Automatic);
    let installation = Installation {
        kind: Kind::AppImage,
        location: location.clone(),
        cannot_install: None,
    };
    let t0 = 1_800_000_000;
    let s = f.service(mode_source, installation, t0);
    s.set_cli(cli);
    run(&s, t0, t0 + HOUR);
    assert_eq!(s.status().ready.as_deref(), Some("0.4.1"));
    assert!(s.on_quit(), "the installer starts at quit");
    assert!(!s.on_quit(), "one installer, however often quit is called");
    // The helper was started to carry out the plan; it runs once the app has exited.
    let mut args = String::new();
    for _ in 0..200 {
        args = std::fs::read_to_string(&calls).unwrap_or_default();
        if !args.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(args.starts_with("update finish --plan "), "{args}");
    let plan = PathBuf::from(args.split_whitespace().nth(3).unwrap());
    // This test is the app and then the helper: the app's lock ends with its process.
    ps_update::install::release_exit_locks();
    ps_update::install::finish(
        &ps_update::UpdateDir::at(&f.dir),
        &plan,
        Duration::from_secs(1),
        None,
    )
    .unwrap();
    let out = std::process::Command::new(&location).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "photosquirrel 0.4.1"
    );
    assert_eq!(
        std::fs::read(data.join(ps_catalog::CATALOG_FILE)).unwrap(),
        b"catalog bytes"
    );
    assert_eq!(
        std::fs::read_to_string(data.join("config/settings.toml")).unwrap(),
        "settings_version = 4\n"
    );
}
