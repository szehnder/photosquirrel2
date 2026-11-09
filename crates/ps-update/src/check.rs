//! One update check, and the context every update operation runs in (ADR-0124, ADR-0125).

use std::path::Path;
use std::sync::Arc;

use minisign_verify::PublicKey;
use semver::Version;

use crate::dir::UpdateDir;
use crate::state::{CheckRecord, UpdateState};
use crate::transport::{Endpoint, Http, Transport};
use crate::{Manifest, UpdateError, build, history, keys, manifest, version};

/// Free bytes for this user on the volume holding a path, `None` when unknown.
pub type FreeSpace = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// Everything an update operation needs. [`Updater::for_this_build`] is the real one; tests build their own.
#[derive(Clone)]
pub struct Updater {
    pub dir: UpdateDir,
    pub transport: Arc<dyn Transport>,
    pub endpoint: Endpoint,
    pub installed: Version,
    /// A developer build never makes a request (spec "Developer builds never check").
    pub release_build: bool,
    pub keys: Vec<PublicKey>,
    pub platform: String,
    pub free_space: FreeSpace,
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    Available(Manifest),
}

impl Updater {
    /// This build's updater: GitHub (or the loopback test server named by `PHOTOSQUIRREL_UPDATE_SERVER`), the
    /// built-in keys and this user's update folder.
    pub fn for_this_build(dir: UpdateDir) -> Result<Self, UpdateError> {
        let endpoint = Endpoint::from_env()?;
        if let Endpoint::Loopback(origin) = &endpoint {
            log::warn!(
                "updates come from the test server {origin} ({})",
                build::SERVER_ENV
            );
        }
        // A developer build talks to the loopback test server only (tests of the real programs); never to GitHub.
        let release_build = build::RELEASE_BUILD || matches!(endpoint, Endpoint::Loopback(_));
        Ok(Self {
            dir,
            transport: Arc::new(Http::new()),
            endpoint,
            installed: build::current_version(),
            release_build,
            keys: keys::trusted(),
            platform: build::platform().to_owned(),
            free_space: Arc::new(free_space),
        })
    }

    /// Fetch, verify and judge the manifest; record the result in the state and the history.
    pub fn check(&self) -> Result<Outcome, UpdateError> {
        self.check_at(crate::now_secs())
    }

    /// [`Updater::check`], recorded as made at `now` (seconds since the epoch).
    pub fn check_at(&self, now: i64) -> Result<Outcome, UpdateError> {
        if !self.release_build {
            return Err(UpdateError::DeveloperBuild);
        }
        let mut state = UpdateState::load(&self.dir);
        let result = self.fetch_and_judge(&mut state);
        let message = match &result {
            Ok(Outcome::UpToDate) => "up to date".to_owned(),
            Ok(Outcome::Available(m)) => format!("PhotoSquirrel {} is available", m.version),
            Err(e) => e.to_string(),
        };
        state.last_check = Some(CheckRecord {
            at: now,
            ok: result.is_ok(),
            message: message.clone(),
        });
        match &result {
            Ok(_) => {
                state.last_success_at = Some(now);
                state.failures_since = None;
            }
            Err(_) => {
                state.failures_since.get_or_insert(now);
            }
        }
        // The next scheduled check waits 24 h plus 0-60 min, drawn now (ADR-0124).
        state.jitter_s = jitter();
        state.save(&self.dir)?;
        let version = match &result {
            Ok(Outcome::Available(m)) => Some(m.version.as_str()),
            _ => None,
        };
        history::record(&self.dir, "check", version, &message);
        result
    }

    fn fetch_and_judge(&self, state: &mut UpdateState) -> Result<Outcome, UpdateError> {
        let body = self.transport.fetch(
            &self.endpoint,
            &self.endpoint.manifest_url(),
            manifest::MAX_MANIFEST_BYTES,
        )?;
        let m = manifest::verify(&body, &self.keys)?;
        let candidate = m.semver();
        let highest = state
            .highest_accepted
            .as_deref()
            .and_then(|v| Version::parse(v).ok());
        let skipped: Vec<Version> = state
            .skipped
            .iter()
            .filter_map(|v| Version::parse(v).ok())
            .collect();
        let offered = version::offered(&self.installed, &candidate, highest.as_ref(), &skipped);
        state.highest_accepted = Some(version::raise(highest.as_ref(), &candidate).to_string());
        if offered {
            state.available = Some(m.clone());
            Ok(Outcome::Available(m))
        } else {
            // Keep an earlier offer only while it is still newer than this build and not skipped.
            if state
                .available
                .as_ref()
                .is_some_and(|a| a.semver() <= self.installed || skipped.contains(&a.semver()))
            {
                state.available = None;
            }
            Ok(Outcome::UpToDate)
        }
    }
}

/// 0-3600 seconds, from the clock's low bits: spreading checks needs no cryptographic randomness.
fn jitter() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    u64::from(nanos) % 3_601
}

/// Free bytes for this user on the volume holding `path` (or its nearest existing ancestor); unknown on Windows,
/// which is never a reason to refuse (the download then fails on a full disk instead).
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    free_space_at(p)
}

#[cfg(unix)]
fn free_space_at(p: &Path) -> Option<u64> {
    let s = rustix::fs::statvfs(p).ok()?;
    Some(s.f_bavail.saturating_mul(s.f_frsize))
}

#[cfg(not(unix))]
fn free_space_at(_p: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::{Route, TestServer, envelope, sample_manifest};

    pub(crate) use crate::testing::updater;

    #[test]
    fn a_check_records_what_it_found() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", b"new")]);
        server.route(
            "/update-manifest.json",
            Route::Body(envelope(&m).into_bytes()),
        );
        let u = updater(&server, tmp.path(), "0.4.0");
        assert_eq!(u.check().expect("check"), Outcome::Available(m.clone()));
        let s = UpdateState::load(&u.dir);
        assert_eq!(s.available, Some(m));
        assert_eq!(s.highest_accepted.as_deref(), Some("0.4.1"));
        let last = s.last_check.expect("recorded");
        assert!(last.ok && last.message == "PhotoSquirrel 0.4.1 is available");
        assert!(s.jitter_s <= 3_600);
        // The same build after updating: nothing to offer.
        let u = updater(&server, tmp.path(), "0.4.1");
        assert_eq!(u.check().expect("check"), Outcome::UpToDate);
        assert_eq!(UpdateState::load(&u.dir).available, None);
    }

    #[test]
    fn scenario_app_updates_developer_build_refuses_to_check() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        let mut u = updater(&server, tmp.path(), "0.4.0");
        u.release_build = false;
        let e = u.check().expect_err("refused");
        assert_eq!(e.to_string(), "updates are off in developer builds");
        assert!(server.requests().is_empty());
    }

    #[test]
    fn a_failed_check_is_recorded_and_keeps_the_last_success() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        server.route("/update-manifest.json", Route::Status(503));
        let u = updater(&server, tmp.path(), "0.4.0");
        assert!(matches!(u.check(), Err(UpdateError::Status(503))));
        let s = UpdateState::load(&u.dir);
        assert!(!s.last_check.expect("recorded").ok);
        assert!(s.failures_since.is_some() && s.last_success_at.is_none());
    }
}
