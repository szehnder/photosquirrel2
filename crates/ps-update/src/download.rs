//! Downloading a verified payload (spec "Verified payload", "Resumable bounded download").
//!
//! A payload is streamed into `download/<name>.part` and through SHA-256, resumed with a range request when a
//! part exists, and moved to `ready/<version>/` only when its size and hash equal the manifest's. One version is
//! staged at a time.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};

use sha2::{Digest, Sha256};

use crate::check::Updater;
use crate::state::{Ready, UpdateState};
use crate::transport::copy_limited;
use crate::{Manifest, UpdateError, history};

/// The free space a download needs: the payload, an unpacked copy and the kept previous version.
pub fn space_needed(size: u64) -> u64 {
    size.saturating_mul(3)
}

impl Updater {
    /// Download and verify the `kind` package of `manifest` for this platform, and record it as ready.
    pub fn download(&self, manifest: &Manifest, kind: &str) -> Result<Ready, UpdateError> {
        if !self.release_build {
            return Err(UpdateError::DeveloperBuild);
        }
        let package = manifest.package(&self.platform, kind).ok_or_else(|| {
            UpdateError::BadManifest(format!(
                "PhotoSquirrel {} has no {kind} package for {}",
                manifest.version, self.platform
            ))
        })?;
        let ready_dir = self.dir.ready_dir(&manifest.version);
        let target = ready_dir.join(&package.name);
        let state = UpdateState::load(&self.dir);
        if let Some(r) = state.ready.as_ref().filter(|r| {
            r.version == manifest.version && r.sha256 == package.sha256 && r.file.is_file()
        }) {
            return Ok(r.clone());
        }

        let parts = self.dir.download_dir();
        fs::create_dir_all(&parts)?;
        let needed = space_needed(package.size);
        if let Some(available) = (self.free_space)(&parts).filter(|a| *a < needed) {
            let e = UpdateError::NoSpace { needed, available };
            history::record(
                &self.dir,
                "failure",
                Some(&manifest.version),
                &e.to_string(),
            );
            return Err(e);
        }

        let part = parts.join(format!("{}.part", package.name));
        let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > package.size {
            fs::remove_file(&part)?;
            have = 0;
        }
        let mut hasher = Sha256::new();
        if have > 0 {
            hash_file(&part, &mut hasher)?;
        }
        // A part that is already complete needs no request (a range past the end would be refused for ever).
        let written = if have == package.size {
            0
        } else {
            let url = self.endpoint.package_url(package);
            let opened = self
                .transport
                .open(&self.endpoint, &url, (have > 0).then_some(have))?;
            let mut file = if opened.partial {
                OpenOptions::new().append(true).open(&part)?
            } else {
                // The whole file came back: start over.
                hasher = Sha256::new();
                have = 0;
                File::create(&part)?
            };
            let mut sink = HashingWriter {
                file: &mut file,
                hasher: &mut hasher,
            };
            let written = match copy_limited(opened.body, &mut sink, package.size - have) {
                Ok(n) => n,
                Err(UpdateError::TooLarge(_)) => {
                    drop(file);
                    fs::remove_file(&part).ok();
                    return self.mismatch(&manifest.version);
                }
                // A dropped connection keeps the part for the next attempt.
                Err(e) => return Err(e),
            };
            file.sync_all()?;
            written
        };
        if have + written != package.size {
            return Err(UpdateError::Network(format!(
                "the download stopped after {} of {} bytes",
                have + written,
                package.size
            )));
        }
        let sum: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if sum != package.sha256.to_ascii_lowercase() {
            fs::remove_file(&part).ok();
            return self.mismatch(&manifest.version);
        }

        // Only one version is staged: drop older ones, then move the verified file in.
        let ready_root = self.dir.root().join("ready");
        if let Ok(entries) = fs::read_dir(&ready_root) {
            for e in entries.flatten() {
                if e.file_name() != std::ffi::OsStr::new(&manifest.version) {
                    fs::remove_dir_all(e.path()).ok();
                }
            }
        }
        fs::create_dir_all(&ready_dir)?;
        fs::rename(&part, &target)?;
        let ready = Ready {
            version: manifest.version.clone(),
            kind: kind.to_owned(),
            file: target,
            sha256: sum,
            size: package.size,
        };
        // The state as it is now: the user may have skipped or dismissed something during the transfer.
        let mut state = UpdateState::load(&self.dir);
        state.ready = Some(ready.clone());
        state.save(&self.dir)?;
        history::record(
            &self.dir,
            "download",
            Some(&manifest.version),
            &format!("{} verified ({} bytes)", package.name, package.size),
        );
        Ok(ready)
    }

    fn mismatch<T>(&self, version: &str) -> Result<T, UpdateError> {
        let e = UpdateError::Mismatch;
        history::record(&self.dir, "failure", Some(version), &e.to_string());
        Err(e)
    }
}

struct HashingWriter<'a> {
    file: &'a mut File,
    hasher: &'a mut Sha256,
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub(crate) fn hash_file(path: &std::path::Path, hasher: &mut Sha256) -> io::Result<()> {
    let mut f = File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::check::tests::updater;
    use crate::testing::{Route, TestServer, sample_manifest};

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn a_download_is_verified_and_staged() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        let body = payload(100_000);
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", &body)]);
        let name = &m.packages["linux-x86_64"]["appimage"].name;
        server.route(&format!("/{name}"), Route::Body(body.clone()));
        let u = updater(&server, tmp.path(), "0.4.0");
        let ready = u.download(&m, "appimage").expect("download");
        assert_eq!(fs::read(&ready.file).expect("ready"), body);
        assert_eq!(UpdateState::load(&u.dir).ready, Some(ready.clone()));
        // A second call reuses the staged file without a request.
        let before = server.requests().len();
        assert_eq!(u.download(&m, "appimage").expect("again"), ready);
        assert_eq!(server.requests().len(), before);
    }

    #[test]
    fn scenario_app_updates_corrupted_download_never_installed() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        let body = payload(50_000);
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", &body)]);
        let name = m.packages["linux-x86_64"]["appimage"].name.clone();
        let mut bad = body.clone();
        bad[1234] ^= 0xff;
        server.route(&format!("/{name}"), Route::Body(bad));
        let u = updater(&server, tmp.path(), "0.4.0");
        let e = u.download(&m, "appimage").expect_err("mismatch");
        assert_eq!(
            e.to_string(),
            "downloaded update does not match its signature"
        );
        assert!(
            !u.dir.download_dir().join(format!("{name}.part")).exists(),
            "removed"
        );
        assert!(!u.dir.ready_dir("0.4.1").join(&name).exists());
        assert_eq!(UpdateState::load(&u.dir).ready, None);
        // The next attempt downloads it again from the start (no range request) and succeeds.
        server.route(&format!("/{name}"), Route::Body(body.clone()));
        u.download(&m, "appimage").expect("second try");
        let last = server.requests().pop().expect("request");
        assert_eq!(last.header("range"), None);
    }

    #[test]
    fn scenario_app_updates_interrupted_download_resumes() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        let body = payload(200_000);
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", &body)]);
        let name = m.packages["linux-x86_64"]["appimage"].name.clone();
        server.route(&format!("/{name}"), Route::CutAfter(body.clone(), 80_000));
        let u = updater(&server, tmp.path(), "0.4.0");
        assert!(
            u.download(&m, "appimage").is_err(),
            "the connection dropped"
        );
        let part = u.dir.download_dir().join(format!("{name}.part"));
        assert_eq!(fs::metadata(&part).expect("part kept").len(), 80_000);
        server.route(&format!("/{name}"), Route::Body(body.clone()));
        let ready = u.download(&m, "appimage").expect("resumed");
        let last = server.requests().pop().expect("request");
        assert_eq!(last.header("range"), Some("bytes=80000-"));
        assert_eq!(fs::read(ready.file).expect("ready"), body);
    }

    #[test]
    fn scenario_app_updates_low_disk_space_defers_the_download() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        let body = payload(10_000);
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", &body)]);
        let mut u = updater(&server, tmp.path(), "0.4.0");
        u.free_space = Arc::new(|_| Some(25_000));
        let e = u.download(&m, "appimage").expect_err("no space");
        assert!(
            matches!(
                e,
                UpdateError::NoSpace {
                    needed: 30_000,
                    available: 25_000
                }
            ),
            "{e}"
        );
        assert!(e.to_string().contains("30000 bytes needed"));
        assert!(server.requests().is_empty(), "nothing downloaded");
    }

    #[test]
    fn an_oversized_payload_is_refused() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        let body = payload(1_000);
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", &body)]);
        let name = m.packages["linux-x86_64"]["appimage"].name.clone();
        server.route(&format!("/{name}"), Route::Endless(5_000));
        let u = updater(&server, tmp.path(), "0.4.0");
        assert!(matches!(
            u.download(&m, "appimage"),
            Err(UpdateError::Mismatch)
        ));
        assert!(!u.dir.download_dir().join(format!("{name}.part")).exists());
    }

    #[test]
    fn only_one_version_is_staged() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        for v in ["0.4.1", "0.4.2"] {
            let body = payload(1_000);
            let m = sample_manifest(v, &[("linux-x86_64", "appimage", &body)]);
            let name = m.packages["linux-x86_64"]["appimage"].name.clone();
            server.route(&format!("/{name}"), Route::Body(body));
            updater(&server, tmp.path(), "0.4.0")
                .download(&m, "appimage")
                .expect("download");
        }
        let dir = crate::UpdateDir::at(tmp.path());
        assert!(!dir.ready_dir("0.4.1").exists());
        assert!(dir.ready_dir("0.4.2").exists());
    }
}
