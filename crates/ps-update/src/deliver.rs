//! Handing a verified update to the user for an installation PhotoSquirrel does not change itself (spec "Updates
//! for installations it cannot change", "Blocked bundle swap falls back"; ADR-0126).
//!
//! The verified payload is copied as a new file into the user's Downloads folder (never replacing a file there),
//! and for a Debian package the one command that installs it is returned. Nothing runs with elevated rights.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use crate::check::Updater;
use crate::kind::Kind;
use crate::{Manifest, UpdateError, history};

/// A verified update the user installs by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualInstall {
    pub file: PathBuf,
    /// The command that installs it (`.deb`), to show with a copy button.
    pub command: Option<String>,
}

/// The user's Downloads folder, or the home folder when the platform names none.
pub fn downloads_dir() -> Option<PathBuf> {
    let dirs = directories::UserDirs::new()?;
    Some(
        dirs.download_dir()
            .map_or_else(|| dirs.home_dir().to_path_buf(), Path::to_path_buf),
    )
}

impl Updater {
    /// Download and verify the package for `kind` and save it as a new file in `downloads`.
    pub fn prepare_manual_install(
        &self,
        manifest: &Manifest,
        kind: Kind,
        downloads: &Path,
    ) -> Result<ManualInstall, UpdateError> {
        let package = kind.package().ok_or_else(|| {
            UpdateError::BadManifest(format!(
                "a {} is updated from the release page",
                kind.label()
            ))
        })?;
        let ready = self.download(manifest, package)?;
        let name = ready
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file = copy_new(&ready.file, downloads, &name)?;
        let command = (kind == Kind::Deb).then(|| apt_command(&file));
        history::record(
            &self.dir,
            "download",
            Some(&manifest.version),
            &format!("saved for a manual install: {}", file.display()),
        );
        Ok(ManualInstall { file, command })
    }
}

/// `sudo apt install '<path>'`, quoted for a POSIX shell.
pub fn apt_command(file: &Path) -> String {
    let path = file.to_string_lossy().replace('\'', r"'\''");
    format!("sudo apt install '{path}'")
}

/// Copy `src` into `dir` as a new file named `name`, or `name (1)`, `name (2)`... when taken. An existing file
/// is never replaced.
pub fn copy_new(src: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let (stem, ext) = split_name(name);
    for n in 0..1000 {
        let candidate = if n == 0 {
            dir.join(name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match File::options()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut out) => {
                let mut input = File::open(src)?;
                io::copy(&mut input, &mut out)?;
                out.sync_all()?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("too many copies of {name} in {}", dir.display()),
    ))
}

/// `PhotoSquirrel-0.4.1-linux-x86_64.deb` -> (`PhotoSquirrel-0.4.1-linux-x86_64`, `.deb`); `x.app.tar.gz` keeps
/// `.app.tar.gz` together.
fn split_name(name: &str) -> (&str, &str) {
    for ext in [".app.tar.gz", ".tar.gz"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return (stem, &name[stem.len()..]);
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Show `file` in the platform's file manager. Failure is not an error worth stopping for.
pub fn reveal(file: &Path) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-R")
            .arg(file)
            .spawn()
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", file.display()))
            .spawn()
    } else {
        std::process::Command::new("xdg-open")
            .arg(file.parent().unwrap_or(file))
            .spawn()
    };
    if let Err(e) = result {
        log::warn!("could not show {}: {e}", file.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::tests::updater;
    use crate::testing::{Route, TestServer, sample_manifest};

    #[test]
    fn scenario_app_updates_debian_package_gets_a_verified_file_and_a_command() {
        let server = TestServer::start();
        let tmp = tempfile::tempdir().expect("tmp");
        let body = b"a debian package".to_vec();
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "deb", &body)]);
        let name = m.packages["linux-x86_64"]["deb"].name.clone();
        server.route(&format!("/{name}"), Route::Body(body.clone()));
        let u = updater(&server, &tmp.path().join("updates"), "0.4.0");
        let downloads = tmp.path().join("Downloads");
        // A file of the same name is already there: it is kept.
        fs::create_dir_all(&downloads).expect("dir");
        fs::write(downloads.join(&name), b"mine").expect("existing");
        let got = u
            .prepare_manual_install(&m, Kind::Deb, &downloads)
            .expect("prepared");
        assert_eq!(fs::read(downloads.join(&name)).expect("kept"), b"mine");
        assert_eq!(
            fs::read(&got.file).expect("saved"),
            body,
            "the verified package"
        );
        assert_eq!(got.file, downloads.join(name.replace(".deb", " (1).deb")));
        let command = got.command.expect("command");
        assert!(command.starts_with("sudo apt install '"), "{command}");
        assert!(
            command.ends_with(&format!("{}'", got.file.display())),
            "{command}"
        );
        // An archive installation gets no package at all.
        assert!(
            u.prepare_manual_install(&m, Kind::Archive, &downloads)
                .is_err()
        );
    }

    #[test]
    fn names_keep_their_extensions() {
        assert_eq!(split_name("a.app.tar.gz"), ("a", ".app.tar.gz"));
        assert_eq!(split_name("a.tar.gz"), ("a", ".tar.gz"));
        assert_eq!(split_name("a.deb"), ("a", ".deb"));
        assert_eq!(split_name("noext"), ("noext", ""));
        assert_eq!(
            apt_command(Path::new("/h/it's.deb")),
            r"sudo apt install '/h/it'\''s.deb'"
        );
    }
}
