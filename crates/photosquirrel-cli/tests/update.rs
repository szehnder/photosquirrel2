//! Spec `cli`, "Update commands" and "Doctor report" (update state), and the CLI half of spec `settings` "Update
//! mode persists and is shared". The real binary against a loopback test server; the update folder, the
//! Downloads folder and the installation kind are test overrides (debug builds only), so nothing outside a temp
//! folder is touched.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use ps_update::testing::{Route, TestServer, envelope, sample_manifest};
use serde_json::Value;

struct Env {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    updates: PathBuf,
    downloads: PathBuf,
    server: TestServer,
    installation: Option<String>,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        Self {
            data: root.join("data"),
            updates: root.join("updates"),
            downloads: root.join("Downloads"),
            _tmp: tmp,
            server: TestServer::start(),
            installation: None,
        }
    }

    fn root(&self) -> &Path {
        self.data.parent().unwrap()
    }

    fn mode(&self, mode: &str) {
        std::fs::create_dir_all(self.data.join("config")).unwrap();
        std::fs::write(
            self.data.join("config/settings.toml"),
            format!("settings_version = 4\n[updates]\nmode = \"{mode}\"\n"),
        )
        .unwrap();
    }

    /// The server offers 99.0.0 with a `kind` package for this platform.
    fn offer(&self, kind: &str, payload: &[u8]) {
        let m = sample_manifest("99.0.0", &[(ps_update::build::platform(), kind, payload)]);
        self.server.route(
            "/update-manifest.json",
            Route::Body(envelope(&m).into_bytes()),
        );
        let name = &m.packages[ps_update::build::platform()][kind].name;
        self.server
            .route(&format!("/{name}"), Route::Body(payload.to_vec()));
    }

    fn cli(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"));
        c.env("PHOTOSQUIRREL_DATA_DIR", &self.data)
            .env("PHOTOSQUIRREL_UPDATE_DIR", &self.updates)
            .env("PHOTOSQUIRREL_DOWNLOADS_DIR", &self.downloads)
            .env("PHOTOSQUIRREL_UPDATE_SERVER", self.server.url())
            .env_remove("PHOTOSQUIRREL_CONFIG_DIR")
            .stdin(Stdio::null());
        match &self.installation {
            Some(i) => c.env("PHOTOSQUIRREL_UPDATE_TEST_INSTALLATION", i),
            None => c.env_remove("PHOTOSQUIRREL_UPDATE_TEST_INSTALLATION"),
        };
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cli().args(args).output().unwrap()
    }
}

fn json(o: &Output) -> Value {
    let s = String::from_utf8_lossy(&o.stdout);
    let v: Value = serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("{e}: {s}"));
    assert_eq!(v["schema_version"], 1, "{s}");
    v
}

fn ok(o: &Output) -> &Output {
    assert!(
        o.status.success(),
        "exited {:?}\nstdout: {}\nstderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    o
}

#[test]
fn scenario_cli_update_status_works_offline() {
    let env = Env::new();
    env.mode("automatic");
    let v = json(ok(&env.run(&["--json", "update", "status"])));
    let d = &v["data"];
    assert_eq!(d["mode"], "automatic");
    assert_eq!(d["installed"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["kind"], "developer", "a test build is a developer build");
    assert!(d["history"].is_array());
    assert_eq!(d["network_used"], false);
    assert!(env.server.requests().is_empty(), "no connection");
}

#[test]
fn scenario_cli_update_check_reports_an_available_version() {
    let env = Env::new();
    env.mode("off");
    env.offer("appimage", b"payload");
    let v = json(ok(&env.run(&["--json", "update", "check"])));
    let d = &v["data"];
    assert_eq!(d["available"], "99.0.0");
    assert!(
        d["notes_url"]
            .as_str()
            .unwrap()
            .ends_with("/releases/tag/v99.0.0")
    );
    assert!(d["can_install"].is_boolean());
    assert_eq!(env.server.requests().len(), 1, "one request");
    // The mode is still off: the explicit check changed nothing.
    let status = json(ok(&env.run(&["--json", "update", "status"])));
    assert_eq!(status["data"]["mode"], "off");
    assert_eq!(status["data"]["available"], "99.0.0");
}

#[test]
fn scenario_cli_install_refused_while_the_app_runs() {
    let mut env = Env::new();
    let location = env.root().join("Apps/PhotoSquirrel.AppImage");
    std::fs::create_dir_all(location.parent().unwrap()).unwrap();
    std::fs::write(&location, b"the installed appimage").unwrap();
    env.installation = Some(format!("appimage={}", location.display()));
    env.offer("appimage", b"the new appimage");
    // The app of this installation is running: it holds the installation's lock.
    let dir = ps_update::UpdateDir::at(&env.updates);
    std::fs::create_dir_all(dir.root()).unwrap();
    let held = ps_update::install::hold_running(&dir, &location).expect("lock");
    let o = env.run(&["update", "install", "--yes"]);
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("quit PhotoSquirrel first"), "{err}");
    assert_eq!(std::fs::read(&location).unwrap(), b"the installed appimage");
    drop(held);
}

#[test]
fn scenario_cli_install_refused_for_a_package_managed_installation() {
    let mut env = Env::new();
    env.installation = Some("deb=/usr/bin".into());
    env.offer("deb", b"a debian package");
    let o = env.run(&["update", "install", "--yes"]);
    assert_eq!(o.status.code(), Some(1));
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    let saved = std::fs::read_dir(&env.downloads)
        .unwrap()
        .map(|e| e.unwrap().path())
        .next()
        .expect("a file in Downloads");
    assert_eq!(std::fs::read(&saved).unwrap(), b"a debian package");
    assert!(out.contains(&saved.display().to_string()), "{out}");
    assert!(err.contains("sudo apt install '"), "{err}");
    assert!(err.contains(&saved.display().to_string()), "{err}");
}

#[test]
fn scenario_cli_doctor_reports_update_state_offline() {
    let mut env = Env::new();
    env.mode("notify");
    let location = env.root().join("Local/PhotoSquirrel");
    std::fs::create_dir_all(&location).unwrap();
    env.installation = Some(format!("windows-setup={}", location.display()));
    let state = ps_update::UpdateState {
        last_check: Some(ps_update::state::CheckRecord {
            at: 1_800_000_000,
            ok: true,
            message: "up to date".into(),
        }),
        ..Default::default()
    };
    state.save(&ps_update::UpdateDir::at(&env.updates)).unwrap();
    let v = json(ok(&env.run(&["--json", "doctor"])));
    let u = &v["data"]["updates"];
    assert_eq!(u["mode"], "notify");
    assert_eq!(u["kind"], "windows-setup");
    assert_eq!(u["can_install"], true);
    assert_eq!(u["last_check_at"], 1_800_000_000);
    assert_eq!(u["last_check"], "up to date");
    assert_eq!(v["data"]["network_used"], false);
    assert!(env.server.requests().is_empty(), "no connection");
}

#[test]
fn scenario_cli_scans_never_check_for_updates() {
    let env = Env::new();
    env.mode("automatic");
    env.offer("appimage", b"payload");
    let corpus = ps_fixtures::shared_corpus(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "cli-corpus-small-42",
        42,
        ps_fixtures::Preset::Small,
    )
    .canonicalize()
    .unwrap();
    let library = env.root().join("Library");
    for args in [
        vec!["sources", "add", corpus.join("drive-a").to_str().unwrap()],
        vec!["scan"],
        vec!["groups", "list"],
        vec!["export", "dry-run", "--library", library.to_str().unwrap()],
    ] {
        ok(&env.run(&args));
    }
    assert!(env.server.requests().is_empty(), "no update request");
}

#[test]
fn update_mode_set_in_settings_is_what_the_cli_reports() {
    // Spec settings "Update mode persists and is shared": the app's saved mode is the CLI's.
    let env = Env::new();
    env.mode("notify");
    let v = json(ok(&env.run(&["--json", "update", "status"])));
    assert_eq!(v["data"]["mode"], "notify");
}
