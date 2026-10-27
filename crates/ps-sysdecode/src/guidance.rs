//! What to install when HEIC cannot be decoded, for the detected system (ADR-0089, design D6). A pure function
//! of the [`OsInfo`] and the [`Finding`]; every Linux and Windows fix ends with restarting the app and
//! rescanning.

use ps_model::{Fix, FixStep};

use crate::os::{OsFamily, OsInfo};

/// The oldest libheif PhotoSquirrel uses (Ubuntu 22.04, the packages' glibc floor).
pub const LIBHEIF_MIN: (u32, u32) = (1, 12);

/// Microsoft Store product ids.
pub const STORE_HEIF: &str = "9PMMSR1CGPWG";
pub const STORE_HEVC: &str = "9NMZLZ57R3T7";
pub const STORE_HEVC_OEM: &str = "9N4WGH0Z6VHQ";

/// Why HEIC cannot be decoded on this machine, as precisely as detection could tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// No HEIF library could be loaded (Linux and other Unix systems).
    NoLibrary,
    /// The HEIF library is older than [`LIBHEIF_MIN`].
    TooOld { version: String },
    /// The HEIF library has no HEVC decoder.
    NoHevc { version: String },
    /// Windows older than 10 version 1809 (build 17763).
    WindowsTooOld,
    /// Windows: which extensions are missing.
    WindowsMissing { heif: bool, hevc: bool },
    /// A Windows N edition without Media Foundation (no Media Feature Pack).
    WindowsNoMediaFoundation,
    /// A decoder that loads but fails the self-test, with no more specific finding.
    SelfTestFailed { backend: String, detail: String },
    /// Detection did not finish in time.
    Timeout,
    /// `PHOTOSQUIRREL_HEIF_DECODER=none`.
    Forced,
}

/// The precise finding of the Windows platform checks (D5), from the build number, whether Media Foundation
/// is missing, whether a WIC decoder for the HEIF container is registered and how many HEVC video decoders
/// Media Foundation lists (`None`: it could not be asked). `None` when everything needed is registered.
pub fn windows_finding(build: u32, no_mf: bool, heif: bool, hevc: Option<u32>) -> Option<Finding> {
    if build != 0 && build < 17763 {
        return Some(Finding::WindowsTooOld);
    }
    if no_mf || hevc.is_none() {
        return Some(Finding::WindowsNoMediaFoundation);
    }
    let missing_hevc = hevc == Some(0);
    (!heif || missing_hevc).then_some(Finding::WindowsMissing {
        heif: !heif,
        hevc: missing_hevc,
    })
}

/// The reason, in a few words, for reports.
pub fn reason(f: &Finding) -> String {
    match f {
        Finding::NoLibrary => {
            "no HEIC decoder on this system: the HEIF library (libheif) is not installed".into()
        }
        Finding::TooOld { version } => format!(
            "libheif {version} is too old; PhotoSquirrel needs {}.{} or newer",
            LIBHEIF_MIN.0, LIBHEIF_MIN.1
        ),
        Finding::NoHevc { version } => {
            format!("libheif {version} is installed but has no HEVC decoder")
        }
        Finding::WindowsTooOld => "HEIC previews need Windows 10 version 1809 or newer".into(),
        Finding::WindowsMissing {
            heif: true,
            hevc: true,
        } => "no HEIC decoder on this system: the HEIF Image Extensions and an HEVC decoder (HEVC Video \
              Extensions) are not installed"
            .into(),
        Finding::WindowsMissing { heif: true, .. } => {
            "no HEIC decoder on this system: the HEIF Image Extensions are not installed".into()
        }
        Finding::WindowsMissing { .. } => {
            "HEIF Image Extensions found, but no HEVC decoder (HEVC Video Extensions) is installed".into()
        }
        Finding::WindowsNoMediaFoundation => {
            "this Windows N edition has no Media Foundation (the Media Feature Pack is not installed)"
                .into()
        }
        Finding::SelfTestFailed { backend, detail } => {
            format!("{backend} is present but cannot decode the HEIC sample ({detail})")
        }
        Finding::Timeout => "the system decoder did not answer".into(),
        Finding::Forced => "no HEIC decoder on this system (PHOTOSQUIRREL_HEIF_DECODER=none)".into(),
    }
}

fn step(text: &str) -> FixStep {
    FixStep {
        text: text.into(),
        ..FixStep::default()
    }
}

fn cmd(text: &str, command: &str) -> FixStep {
    FixStep {
        text: text.into(),
        command: Some(command.into()),
        ..FixStep::default()
    }
}

/// A Microsoft Store step: the plain web URL always shown, the Store link opened only on a click.
fn store(text: &str, id: &str) -> FixStep {
    FixStep {
        text: text.into(),
        command: None,
        url: Some(format!(
            "https://apps.microsoft.com/detail/{}",
            id.to_ascii_lowercase()
        )),
        open: Some(format!("ms-windows-store://pdp/?ProductId={id}")),
    }
}

/// The step every Linux and Windows fix ends with. The "Why no preview?" dialog shows its own closing line instead
/// ("Then restart PhotoSquirrel and fill in previews", ADR-0109), so it recognises this one by its text.
pub const RESTART_STEP: &str = "Then restart PhotoSquirrel and rescan your sources";

fn restart() -> FixStep {
    step(RESTART_STEP)
}

const RPMFUSION_FEDORA: &str = "sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm";
const RPMFUSION_EL: &str = "sudo dnf install https://mirrors.rpmfusion.org/free/el/rpmfusion-free-release-$(rpm -E %rhel).noarch.rpm";

/// The fix for `f` on `os`.
pub fn guidance(os: &OsInfo, f: &Finding) -> Fix {
    match os.family {
        OsFamily::MacOs => macos(f),
        OsFamily::Windows => windows(os, f),
        OsFamily::Linux | OsFamily::OtherUnix => unix(os, f),
    }
}

fn macos(f: &Finding) -> Fix {
    match f {
        Finding::Forced => Fix {
            summary: "Unset PHOTOSQUIRREL_HEIF_DECODER to use macOS's own HEIC decoder".into(),
            steps: vec![step("Nothing to install: macOS decodes HEIC itself")],
            restart: true,
        },
        Finding::SelfTestFailed { .. } | Finding::Timeout => Fix {
            summary: "Nothing to install: macOS decodes HEIC itself".into(),
            steps: vec![step(
                "This should not happen: please report a bug with the output of `photosquirrel-cli doctor`",
            )],
            restart: false,
        },
        _ => Fix {
            summary: "Nothing to install: macOS decodes HEIC itself".into(),
            steps: Vec::new(),
            restart: false,
        },
    }
}

fn windows(os: &OsInfo, f: &Finding) -> Fix {
    let heif = || {
        store(
            "Install the free HEIF Image Extensions from the Microsoft Store",
            STORE_HEIF,
        )
    };
    let hevc = || {
        [
            store(
                "Install the HEVC Video Extensions ($0.99) from the Microsoft Store",
                STORE_HEVC,
            ),
            store(
                "or, if your PC maker provides it, the free HEVC Video Extensions from Device Manufacturer",
                STORE_HEVC_OEM,
            ),
        ]
    };
    let mut steps = Vec::new();
    let summary;
    match f {
        Finding::WindowsTooOld => {
            summary = "Update Windows".to_owned();
            steps.push(step(
                "HEIC previews need Windows 10 version 1809 or newer: update Windows (Settings > Windows Update)",
            ));
        }
        Finding::WindowsNoMediaFoundation => {
            summary =
                "Install the Media Feature Pack, then the HEIF and HEVC extensions".to_owned();
            steps.push(step(
                "Install the Media Feature Pack (Settings > Apps > Optional features > Add a feature)",
            ));
            steps.push(heif());
            steps.extend(hevc());
        }
        Finding::WindowsMissing {
            heif: true,
            hevc: missing_hevc,
        } => {
            if *missing_hevc {
                summary =
                    "Install the HEIF Image Extensions and the HEVC Video Extensions".to_owned();
                steps.push(heif());
                steps.extend(hevc());
            } else {
                summary = "Install the HEIF Image Extensions".to_owned();
                steps.push(heif());
            }
        }
        Finding::WindowsMissing { heif: false, .. } => {
            summary = "Install the HEVC Video Extensions".to_owned();
            steps.extend(hevc());
        }
        Finding::Forced => {
            summary = "Unset PHOTOSQUIRREL_HEIF_DECODER to use the Windows HEIC decoder".to_owned();
        }
        _ => {
            summary = "Install (or repair) the HEIF Image Extensions and the HEVC Video Extensions"
                .to_owned();
            steps.push(heif());
            steps.extend(hevc());
        }
    }
    if os.windows_build.is_some_and(|b| b < 17763) && *f != Finding::WindowsTooOld {
        steps.insert(
            0,
            step("HEIC previews need Windows 10 version 1809 or newer: update Windows first"),
        );
    }
    steps.push(restart());
    Fix {
        summary,
        steps,
        restart: true,
    }
}

/// The first libheif version Debian and Ubuntu package with its decoders split into plugins
/// (`libheif-plugin-libde265`). Upstream has plugins from 1.14, but the distributions split them only with 1.16:
/// Debian bookworm (1.15.1) and Ubuntu 23.04 lunar (1.14.2) ship `libheif1` with libde265 built in and have no
/// plugin package; Ubuntu 23.10 mantic (1.16.2), 24.04 noble (1.17.6), 24.10 oracular (1.18.1), Debian trixie
/// (1.19.8) and bookworm-backports (1.19.7) package the plugin (packages.debian.org and Launchpad, checked
/// 2026-10-25; re-review fix 9).
const DEBIAN_PLUGIN_SINCE: (u32, u32) = (1, 16);
/// Ubuntu releases whose libheif has the HEVC decoder built into `libheif1` (no plugin package).
const UBUNTU_BUILT_IN_HEVC: &[&str] = &["bionic", "focal", "jammy", "kinetic", "lunar"];
/// The same for Debian (bookworm without backports).
const DEBIAN_BUILT_IN_HEVC: &[&str] = &["buster", "bullseye", "bookworm"];

/// Whether this Debian-family system packages libheif's HEVC decoder as `libheif-plugin-libde265`. In order: the
/// libheif version detection found (the plugin package exists from [`DEBIAN_PLUGIN_SINCE`], so a bookworm with
/// backports' 1.19 needs it); the Ubuntu base release a derivative names (`UBUNTU_CODENAME`: Linux Mint 21 and
/// Pop!_OS 22.04 are jammy); the Debian base release (`DEBIAN_CODENAME`, Debian's `VERSION_CODENAME`); Ubuntu's
/// (23.10 and later) and Debian's (13 and later) own version numbers. Unknown: the plugin (every current release).
fn debian_family_has_hevc_plugin(os: &OsInfo, f: &Finding) -> bool {
    let detected = match f {
        Finding::NoHevc { version } | Finding::TooOld { version } => parse_version(version),
        _ => None,
    };
    if let Some(v) = detected {
        return v >= DEBIAN_PLUGIN_SINCE;
    }
    if let Some(c) = os.ubuntu_codename.as_deref() {
        return !UBUNTU_BUILT_IN_HEVC.contains(&c);
    }
    if let Some(c) = os.debian_codename.as_deref() {
        return !DEBIAN_BUILT_IN_HEVC.contains(&c);
    }
    let version = os.version_id.as_deref().and_then(parse_version);
    match os.id.as_deref() {
        Some("ubuntu") => version.is_none_or(|v| v >= (23, 10)),
        Some("debian") => os.major().is_none_or(|m| m >= 13),
        _ => true,
    }
}

/// `1.12.0` -> (1, 12).
fn parse_version(v: &str) -> Option<(u32, u32)> {
    let mut it = v.split('.');
    Some((
        it.next()?.trim().parse().ok()?,
        it.next()?.trim().parse().ok()?,
    ))
}

/// Linux and other Unix systems: the distribution's libheif with an HEVC decoder.
fn unix(os: &OsInfo, f: &Finding) -> Fix {
    if *f == Finding::Forced {
        return Fix {
            summary: "Unset PHOTOSQUIRREL_HEIF_DECODER to use the system HEIF library".into(),
            steps: vec![restart()],
            restart: true,
        };
    }
    let no_hevc = matches!(f, Finding::NoHevc { .. });
    let mut steps = Vec::new();
    if let Finding::TooOld { version } = f {
        steps.push(step(&format!(
            "libheif {version} is too old; PhotoSquirrel needs {}.{} or newer. Update your system, or install a \
             newer libheif:",
            LIBHEIF_MIN.0, LIBHEIF_MIN.1
        )));
    }
    let summary;
    if os.is_like(&["arch", "manjaro", "endeavouros"]) {
        summary = "Install libheif and the libde265 HEVC decoder".to_owned();
        steps.push(cmd(
            "Install libheif and its HEVC decoder",
            "sudo pacman -S --needed libheif libde265",
        ));
    } else if os.is_like(&["fedora"]) && !os.is_like(&["rhel", "centos"]) {
        summary = "Install libheif with HEVC from RPM Fusion".to_owned();
        if !no_hevc {
            steps.push(cmd("Install libheif", "sudo dnf install libheif"));
        }
        steps.push(cmd(
            "Fedora's libheif has no HEVC decoder: enable the RPM Fusion free repository",
            RPMFUSION_FEDORA,
        ));
        steps.push(cmd(
            "Install RPM Fusion's libheif with HEVC",
            "sudo dnf install libheif-freeworld",
        ));
    } else if os.is_like(&["rhel", "centos", "rocky", "almalinux"]) {
        summary = "Install libheif with HEVC from EPEL and RPM Fusion".to_owned();
        steps.push(cmd("Enable EPEL", "sudo dnf install epel-release"));
        steps.push(cmd(
            "Enable the RPM Fusion free repository for Enterprise Linux",
            RPMFUSION_EL,
        ));
        steps.push(cmd(
            "Install RPM Fusion's libheif with HEVC",
            "sudo dnf install libheif-freeworld",
        ));
    } else if os.is_like(&["opensuse", "suse"])
        || os.id.as_deref().is_some_and(|i| i.starts_with("opensuse"))
    {
        summary = "Install libheif with HEVC from the Packman repository".to_owned();
        let dist = if os.id.as_deref() == Some("opensuse-tumbleweed") {
            "openSUSE_Tumbleweed".to_owned()
        } else {
            format!(
                "openSUSE_Leap_{}",
                os.version_id.as_deref().unwrap_or("$releasever")
            )
        };
        steps.push(FixStep {
            text: "openSUSE's libheif has no HEVC decoder: add the Packman repository".into(),
            command: Some(format!(
                "sudo zypper addrepo -cfp 90 https://ftp.gwdg.de/pub/linux/misc/packman/suse/{dist}/ packman"
            )),
            url: Some("https://en.opensuse.org/Additional_package_repositories#Packman".into()),
            open: None,
        });
        steps.push(cmd("Refresh the repositories", "sudo zypper refresh"));
        steps.push(cmd(
            "Install libheif1 and its HEVC decoder plugin (libheif-ffmpeg) from Packman",
            "sudo zypper install --from packman --allow-vendor-change libheif1 libheif-ffmpeg",
        ));
    } else if os.is_like(&["debian", "ubuntu"]) {
        // Ubuntu 22.04 and 23.04 and Debian 12 (and their derivatives) ship libheif with libde265 built in; later
        // releases package the HEVC decoder as a plugin, `libheif-plugin-libde265`, which does not exist before.
        if debian_family_has_hevc_plugin(os, f) {
            summary = "Install libheif1 and its HEVC plugin".to_owned();
            steps.push(cmd(
                "Install libheif and its HEVC decoder plugin",
                "sudo apt install libheif1 libheif-plugin-libde265",
            ));
        } else {
            summary = "Install libheif1".to_owned();
            steps.push(cmd(
                "Install libheif (its HEVC decoder is included)",
                "sudo apt install libheif1",
            ));
        }
    } else {
        summary = format!(
            "Install libheif {}.{} or newer with an HEVC decoder",
            LIBHEIF_MIN.0, LIBHEIF_MIN.1
        );
        steps.push(step(&format!(
            "Install libheif {}.{} or newer with an HEVC (libde265) decoder from your distribution",
            LIBHEIF_MIN.0, LIBHEIF_MIN.1
        )));
    }
    steps.push(restart());
    Fix {
        summary,
        steps,
        restart: true,
    }
}
