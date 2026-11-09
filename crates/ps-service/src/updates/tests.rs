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
