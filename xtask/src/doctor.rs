//! `cargo xtask doctor`: check that this machine can build and run PhotoSquirrel, and say how to fix what
//! is missing. Only needs Rust: xtask itself links no system libraries.

use std::process::{Command, Stdio};

use anyhow::{Result, bail};

/// A system library the build links, by pkg-config name, with its package on common distros.
struct Lib {
    pkg_config: &'static str,
    debian: &'static str,
    fedora: &'static str,
    arch: &'static str,
}

/// What `target/release/photosquirrel` links on Linux (`ldd`); fonts, Wayland and Vulkan are loaded at
/// runtime and SQLite is bundled.
const LINUX_LIBS: &[Lib] = &[
    Lib {
        pkg_config: "dbus-1",
        debian: "libdbus-1-dev",
        fedora: "dbus-devel",
        arch: "dbus",
    },
    Lib {
        pkg_config: "xcb",
        debian: "libxcb1-dev",
        fedora: "libxcb-devel",
        arch: "libxcb",
    },
    Lib {
        pkg_config: "xkbcommon",
        debian: "libxkbcommon-dev",
        fedora: "libxkbcommon-devel",
        arch: "libxkbcommon",
    },
    Lib {
        pkg_config: "xkbcommon-x11",
        debian: "libxkbcommon-x11-dev",
        fedora: "libxkbcommon-x11-devel",
        arch: "libxkbcommon-x11",
    },
];

#[derive(Default)]
struct Report {
    problems: usize,
}

impl Report {
    fn ok(&self, what: &str, detail: &str) {
        println!("  ok    {what:<22} {detail}");
    }
    fn fail(&mut self, what: &str, fix: &str) {
        self.problems += 1;
        println!("  MISS  {what:<22} {fix}");
    }
    fn optional(&self, what: &str, why: &str) {
        println!("  --    {what:<22} {why}");
    }
}

pub fn run() -> Result<()> {
    let mut r = Report::default();

    println!("Build (required)");
    match output("rustc", &["--version"]) {
        Some(v) => r.ok("rust", &v),
        None => r.fail("rust", "install rustup from https://rustup.rs"),
    }
    if cfg!(target_os = "linux") {
        linux(&mut r);
    } else if cfg!(target_os = "macos") {
        match output("xcode-select", &["-p"]) {
            Some(p) => r.ok("xcode tools", &p),
            None => r.fail("xcode tools", "run `xcode-select --install`"),
        }
    } else if cfg!(windows) {
        r.ok(
            "platform",
            "Windows needs only the MSVC build tools rustup installs",
        );
    }

    println!(
        "
Optional decoders"
    );
    match output("ffmpeg", &["-version"]) {
        Some(v) => r.ok("ffmpeg", v.lines().next().unwrap_or_default()),
        None => r.optional(
            "ffmpeg",
            "video poster frames (ADR-0017): install ffmpeg from your package manager",
        ),
    }
    match output("pkg-config", &["--modversion", "libheif"]) {
        Some(v) => r.ok("libheif", &v),
        None => r.optional("libheif", "HEIC previews with `--features heic` (ADR-0017): apt install libheif-dev / brew install libheif"),
    }
    match output("lms", &["version"]) {
        Some(_) => r.ok("LM Studio CLI", "lms found"),
        None => r.optional(
            "LM Studio",
            "local AI insights (ADR-0020): https://lmstudio.ai",
        ),
    }

    println!("\nContributing (optional)");
    match output("openspec", &["--version"]) {
        Some(v) => r.ok("openspec", &v),
        None => r.optional(
            "openspec",
            "spec validation in `cargo xtask ci`: `npm i -g @fission-ai/openspec` (Node >= 20.19)",
        ),
    }

    match output("cargo", &["packager", "--version"]) {
        Some(v) => r.ok("cargo-packager", &v),
        None => r.optional(
            "cargo-packager",
            "installers with `cargo xtask package` (ADR-0030): `cargo install cargo-packager --locked`",
        ),
    }

    println!();
    if r.problems > 0 {
        bail!("{} required item(s) missing; see MISS above", r.problems);
    }
    println!("Ready. Try `cargo run --release -p photosquirrel`.");
    Ok(())
}

fn linux(r: &mut Report) {
    match output("cc", &["--version"]) {
        Some(v) => r.ok("c compiler", v.lines().next().unwrap_or_default()),
        None => r.fail(
            "c compiler",
            "install gcc or clang (build-essential / base-devel)",
        ),
    }
    if output("pkg-config", &["--version"]).is_none() {
        r.fail("pkg-config", "install pkg-config (pkgconf)");
        return;
    }
    r.ok("pkg-config", "");
    let missing: Vec<&Lib> = LINUX_LIBS
        .iter()
        .filter(|lib| {
            let found = Command::new("pkg-config")
                .args(["--exists", lib.pkg_config])
                .status()
                .is_ok_and(|s| s.success());
            if found {
                r.ok(lib.pkg_config, "");
            }
            !found
        })
        .collect();
    for lib in &missing {
        r.fail(lib.pkg_config, "development headers not found");
    }
    if !missing.is_empty() {
        let list = |f: fn(&Lib) -> &'static str| {
            missing.iter().map(|l| f(l)).collect::<Vec<_>>().join(" ")
        };
        println!("\n  Install them with one of:");
        println!("    sudo apt install {}", list(|l| l.debian));
        println!("    sudo dnf install {}", list(|l| l.fedora));
        println!("    sudo pacman -S {}", list(|l| l.arch));
        println!();
    }
}

/// First line of a command's stdout, or `None` when it is missing or fails.
fn output(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    })
}
