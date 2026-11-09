//! How this copy of PhotoSquirrel was installed, and whether it can update itself (ADR-0126, spec "Installation
//! kind detected").
//!
//! Worked out from the running executable: an AppImage (`$APPIMAGE`), a macOS app bundle, a Windows per-user
//! installation (its uninstall key's `InstallLocation`), a Debian package (dpkg's file list), a plain archive, or a
//! developer build. Only the first three update themselves, and only where the installation is writable.

use std::path::{Path, PathBuf};

/// The package name of the Debian package (`photo-squirrel`, from cargo-packager's product name).
pub const DEB_PACKAGE: &str = "photo-squirrel";
/// The uninstall key cargo-packager's NSIS installer writes for a per-user installation.
pub const WINDOWS_UNINSTALL_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\PhotoSquirrel";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    AppImage,
    MacApp,
    WindowsSetup,
    Deb,
    Archive,
    Developer,
}

impl Kind {
    pub fn id(self) -> &'static str {
        match self {
            Kind::AppImage => "appimage",
            Kind::MacApp => "macos-app",
            Kind::WindowsSetup => "windows-setup",
            Kind::Deb => "deb",
            Kind::Archive => "archive",
            Kind::Developer => "developer",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::AppImage => "AppImage",
            Kind::MacApp => "macOS app",
            Kind::WindowsSetup => "Windows installation",
            Kind::Deb => "Debian package",
            Kind::Archive => "unpacked archive",
            Kind::Developer => "developer build",
        }
    }

    /// The manifest package this kind installs or downloads (`None`: link only).
    pub fn package(self) -> Option<&'static str> {
        match self {
            Kind::AppImage => Some("appimage"),
            Kind::MacApp => Some("app.tar.gz"),
            Kind::WindowsSetup => Some("setup"),
            Kind::Deb => Some("deb"),
            Kind::Archive | Kind::Developer => None,
        }
    }
}

/// This installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub kind: Kind,
    /// What an install replaces: the AppImage file, the `.app` bundle, the installation folder; for the other
    /// kinds, the executable's folder.
    pub location: PathBuf,
    /// Why it cannot install updates itself, in words for the user; `None` when it can.
    pub cannot_install: Option<String>,
}

impl Installation {
    pub fn can_install(&self) -> bool {
        self.cannot_install.is_none()
    }
}

/// What detection looks at. [`Probe::this_process`] reads the real values; tests build their own.
pub struct Probe {
    /// The running executable, canonical.
    pub exe: PathBuf,
    pub release_build: bool,
    /// `$APPIMAGE` and `$APPDIR`, set by the AppImage runtime.
    pub appimage: Option<PathBuf>,
    pub appdir: Option<PathBuf>,
    /// dpkg's list of the files of the `photo-squirrel` package, if installed.
    pub dpkg_list: Option<String>,
    /// The per-user Windows installation's `InstallLocation`, if registered.
    pub windows_install_location: Option<PathBuf>,
    pub writable: Box<dyn Fn(&Path) -> bool>,
    pub read_only_volume: Box<dyn Fn(&Path) -> bool>,
}

impl Probe {
    pub fn this_process() -> Self {
        let exe = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .unwrap_or_default();
        Self {
            exe,
            release_build: crate::build::RELEASE_BUILD,
            appimage: std::env::var_os("APPIMAGE").map(PathBuf::from),
            appdir: std::env::var_os("APPDIR").map(PathBuf::from),
            dpkg_list: std::fs::read_to_string(format!("/var/lib/dpkg/info/{DEB_PACKAGE}.list"))
                .ok(),
            windows_install_location: windows_install_location(),
            writable: Box::new(writable),
            read_only_volume: Box::new(read_only_volume),
        }
    }
}

/// Detect the installation `probe` describes.
pub fn detect(p: &Probe) -> Installation {
    let exe_dir = p.exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let found = |kind, location: PathBuf, cannot_install: Option<String>| Installation {
        kind,
        location,
        cannot_install,
    };
    let no_permission = |dir: &Path| format!("No permission to write {}", dir.display());

    if !p.release_build {
        return found(Kind::Developer, exe_dir, Some("Developer build".into()));
    }
    if let (Some(file), Some(appdir)) = (&p.appimage, &p.appdir)
        && file.is_file()
        && p.exe.starts_with(appdir)
    {
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let reason = (!(p.writable)(&dir)).then(|| no_permission(&dir));
        return found(Kind::AppImage, file.clone(), reason);
    }
    if let Some(bundle) = mac_bundle(&p.exe) {
        let text = bundle.to_string_lossy();
        let parent = bundle.parent().map(Path::to_path_buf).unwrap_or_default();
        let reason = if text.contains("/AppTranslocation/") {
            Some("macOS moved the app to a temporary location; move it to Applications".to_owned())
        } else if (p.read_only_volume)(&bundle) {
            Some("Running from the disk image; drag PhotoSquirrel to Applications first".to_owned())
        } else if !(p.writable)(&parent) {
            Some(no_permission(&parent))
        } else {
            None
        };
        return found(Kind::MacApp, bundle, reason);
    }
    if let Some(location) = &p.windows_install_location
        && same_dir(location, &exe_dir)
    {
        let reason = (!(p.writable)(location)).then(|| no_permission(location));
        return found(Kind::WindowsSetup, location.clone(), reason);
    }
    if p.exe.starts_with("/usr/")
        && p.dpkg_list
            .as_deref()
            .is_some_and(|l| l.lines().any(|line| Path::new(line.trim()) == p.exe))
    {
        return found(
            Kind::Deb,
            exe_dir,
            Some("Installed by the system package manager".into()),
        );
    }
    found(Kind::Archive, exe_dir, Some("Unpacked archive".into()))
}

/// `<X>.app` when `exe` is `<X>.app/Contents/MacOS/<name>`.
fn mac_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension().is_some_and(|e| e == "app");
    is_bundle.then(|| bundle.to_path_buf())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let norm = |p: &Path| {
            p.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .trim_end_matches(['\\', '/'])
                .to_lowercase()
        };
        norm(a) == norm(b)
    } else {
        a == b
    }
}

/// Parse `reg query <key> /v InstallLocation` output: the value, without the quotes the installer writes.
pub fn parse_reg_install_location(output: &str) -> Option<PathBuf> {
    let line = output
        .lines()
        .find(|l| l.trim_start().starts_with("InstallLocation"))?;
    let value = line.split_once("REG_SZ")?.1.trim().trim_matches('"');
    (!value.is_empty()).then(|| PathBuf::from(value))
}

#[cfg(windows)]
fn windows_install_location() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .args(["query", WINDOWS_UNINSTALL_KEY, "/v", "InstallLocation"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| parse_reg_install_location(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
        .and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)))
}

#[cfg(not(windows))]
fn windows_install_location() -> Option<PathBuf> {
    None
}

/// Whether this user may create files in `dir`.
pub fn writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        rustix::fs::access(dir, rustix::fs::Access::WRITE_OK).is_ok()
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(dir).is_ok_and(|m| m.is_dir() && !m.permissions().readonly())
    }
}

/// Whether `path` is on a read-only volume (a mounted disk image).
pub fn read_only_volume(path: &Path) -> bool {
    #[cfg(unix)]
    {
        rustix::fs::statvfs(path)
            .is_ok_and(|s| s.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(exe: &Path) -> Probe {
        Probe {
            exe: exe.to_path_buf(),
            release_build: true,
            appimage: None,
            appdir: None,
            dpkg_list: None,
            windows_install_location: None,
            writable: Box::new(|_| true),
            read_only_volume: Box::new(|_| false),
        }
    }

    #[test]
    fn scenario_app_updates_installation_kinds_reported() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();

        // An AppImage with its runtime's environment: installs itself.
        let image = root.join("Apps/PhotoSquirrel-0.4.0-linux-x86_64.AppImage");
        std::fs::create_dir_all(image.parent().expect("dir")).expect("dir");
        std::fs::write(&image, b"appimage").expect("image");
        let mount = root.join("mnt/.mount_PhotoS");
        let mut p = probe(&mount.join("usr/bin/photosquirrel"));
        p.appimage = Some(image.clone());
        p.appdir = Some(mount.clone());
        let i = detect(&p);
        assert_eq!(
            (i.kind, &i.location, i.can_install()),
            (Kind::AppImage, &image, true)
        );
        // ...unless its folder is not writable.
        p.writable = Box::new(|_| false);
        assert!(
            detect(&p)
                .cannot_install
                .expect("reason")
                .starts_with("No permission to write")
        );

        // A bundle in a writable folder: installs itself.
        let bundle = root.join("Applications/PhotoSquirrel.app");
        let i = detect(&probe(&bundle.join("Contents/MacOS/photosquirrel")));
        assert_eq!(
            (i.kind, &i.location, i.can_install()),
            (Kind::MacApp, &bundle, true)
        );
        // The CLI inside the bundle reports the same installation.
        assert_eq!(
            detect(&probe(&bundle.join("Contents/MacOS/photosquirrel-cli"))).kind,
            Kind::MacApp
        );
        // On a read-only path (the DMG), and translocated: they cannot.
        let mut ro = probe(&bundle.join("Contents/MacOS/photosquirrel"));
        ro.read_only_volume = Box::new(|_| true);
        assert_eq!(
            detect(&ro).cannot_install.as_deref(),
            Some("Running from the disk image; drag PhotoSquirrel to Applications first")
        );
        let moved = probe(Path::new(
            "/private/var/folders/x/T/AppTranslocation/ABC/d/PhotoSquirrel.app/Contents/MacOS/photosquirrel",
        ));
        assert_eq!(
            detect(&moved).cannot_install.as_deref(),
            Some("macOS moved the app to a temporary location; move it to Applications")
        );

        // A per-user installation registered for the user: installs itself.
        let installed = root.join("Local/PhotoSquirrel");
        let mut w = probe(&installed.join("photosquirrel.exe"));
        w.windows_install_location = Some(installed.clone());
        let i = detect(&w);
        assert_eq!(
            (i.kind, &i.location, i.can_install()),
            (Kind::WindowsSetup, &installed, true)
        );

        // A package-owned path: never.
        let mut d = probe(Path::new("/usr/bin/photosquirrel"));
        d.dpkg_list =
            Some("/.\n/usr\n/usr/bin\n/usr/bin/photosquirrel\n/usr/bin/photosquirrel-cli\n".into());
        let i = detect(&d);
        assert_eq!(i.kind, Kind::Deb);
        assert_eq!(
            i.cannot_install.as_deref(),
            Some("Installed by the system package manager")
        );

        // A plain unpacked archive: never.
        let i = detect(&probe(
            &root.join("Downloads/PhotoSquirrel-0.4.0-linux-x86_64/photosquirrel"),
        ));
        assert_eq!(
            (i.kind, i.cannot_install.as_deref()),
            (Kind::Archive, Some("Unpacked archive"))
        );

        // A developer build, whatever its layout.
        let mut dev = probe(&bundle.join("Contents/MacOS/photosquirrel"));
        dev.release_build = false;
        assert_eq!(detect(&dev).kind, Kind::Developer);
    }

    #[test]
    fn the_registry_value_is_read_without_its_quotes() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PhotoSquirrel\r\n    \
                   InstallLocation    REG_SZ    \"C:\\Users\\s\\AppData\\Local\\PhotoSquirrel\"\r\n\r\n";
        assert_eq!(
            parse_reg_install_location(out),
            Some(PathBuf::from(r"C:\Users\s\AppData\Local\PhotoSquirrel"))
        );
        assert_eq!(parse_reg_install_location("ERROR: not found"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_writable_folder_is_writable() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(writable(tmp.path()));
        assert!(!read_only_volume(tmp.path()));
    }
}
