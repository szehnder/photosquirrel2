//! `cargo xtask package`: release installers for the host OS (ADR-0030). Builds the `dist` profile, runs
//! cargo-packager with this OS's formats, adds a plain archive, renames everything to
//! `PhotoSquirrel-<version>-<os>-<arch>.<ext>` in `target/packages/`, smoke-tests the packaged CLI and writes a
//! `.sha256` beside each file. `--docker` does the Linux build in `packaging/linux/Dockerfile` (glibc 2.35
//! floor); `--universal` makes one arm64 + x86_64 macOS build.
//!
//! Every package carries `THIRD-PARTY-LICENSES`, generated from `Cargo.lock` by cargo-about (ADR-0058) and checked
//! in each finished package. HEIC decoding is built in only where a recent enough libheif is found; otherwise
//! the build leaves it out and writes `target/packages/heic-<os>.txt` for the release notes (ADR-0059).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const PRODUCT: &str = "PhotoSquirrel";
const PROFILE: &str = "dist";
const DOCKER_IMAGE: &str = "photosquirrel-packaging";
const MAC_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
/// The licence notices file in every package.
pub const LICENCES_FILE: &str = "THIRD-PARTY-LICENSES";
/// Where the notices are generated, relative to the workspace root. `crates/photosquirrel/Cargo.toml` lists this
/// path in the packager's `resources`, so it is not under `CARGO_TARGET_DIR` (which differs inside Docker).
const LICENCES_DIR: &str = "target/licenses";
/// The binaries shipped in every package; their dependency graphs make up the licence notices.
const SHIPPED: [&str; 2] = ["photosquirrel", "photosquirrel-cli"];
/// The oldest libheif the `heic` feature builds against (libheif-rs `v1_17`).
const LIBHEIF_MIN: (u32, u32) = (1, 17);

pub struct Options {
    pub docker: bool,
    pub universal: bool,
    pub tag: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    fn host() -> Result<Self> {
        Ok(match std::env::consts::OS {
            "linux" => Os::Linux,
            "macos" => Os::Macos,
            "windows" => Os::Windows,
            other => bail!("no installers for {other}"),
        })
    }

    fn name(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Os::Linux => "Linux",
            Os::Macos => "macOS",
            Os::Windows => "Windows",
        }
    }

    fn exe(self, name: &str) -> String {
        match self {
            Os::Windows => format!("{name}.exe"),
            _ => name.to_owned(),
        }
    }
}

/// What ends up in `target/packages/`, one per file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    AppImage,
    Deb,
    Dmg,
    Setup,
    TarGz,
    Zip,
}

impl Kind {
    /// The cargo-packager format that produces this kind, and the extension of its output.
    fn packager(self) -> Option<(&'static str, &'static str)> {
        match self {
            Kind::AppImage => Some(("appimage", ".AppImage")),
            Kind::Deb => Some(("deb", ".deb")),
            Kind::Dmg => Some(("dmg", ".dmg")),
            Kind::Setup => Some(("nsis", "-setup.exe")),
            Kind::TarGz | Kind::Zip => None,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Kind::AppImage => ".AppImage",
            Kind::Deb => ".deb",
            Kind::Dmg => ".dmg",
            Kind::Setup => "-setup.exe",
            Kind::TarGz => ".tar.gz",
            Kind::Zip => ".zip",
        }
    }
}

/// Every artifact built on `os`; the last one is the plain archive.
pub fn kinds(os: Os) -> &'static [Kind] {
    match os {
        Os::Linux => &[Kind::AppImage, Kind::Deb, Kind::TarGz],
        Os::Macos => &[Kind::Dmg, Kind::TarGz],
        Os::Windows => &[Kind::Setup, Kind::Zip],
    }
}

pub fn artifact_name(version: &str, os: Os, arch: &str, kind: Kind) -> String {
    format!("{PRODUCT}-{version}-{}-{arch}{}", os.name(), kind.suffix())
}

/// One line in `sha256sum` format: `sha256sum -c` accepts it.
pub fn checksum_line(bytes: &[u8], file_name: &str) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{hex}  {file_name}\n")
}

pub fn check_tag(tag: &str, version: &str) -> Result<()> {
    if tag != format!("v{version}") {
        bail!("tag {tag} does not match the workspace version {version} (expected v{version})");
    }
    Ok(())
}

/// `photosquirrel-cli --version` prints `photosquirrel-cli <version>`.
pub fn check_smoke(output: &str, version: &str) -> Result<()> {
    if output.split_whitespace().nth(1) != Some(version) {
        bail!(
            "packaged photosquirrel-cli reported {:?}, expected version {version}",
            output.trim()
        );
    }
    Ok(())
}

/// Whether this build decodes HEIC (ADR-0059).
#[derive(Debug, PartialEq, Eq)]
pub enum Heic {
    On,
    Off(String),
}

/// Decide HEIC support from what the probe found: `libheif` is the version pkg-config reports (or `vcpkg` when
/// the Windows vcpkg library is present), `None` when nothing was found.
pub fn heic_plan(os: Os, universal: bool, libheif: Option<&str>) -> Heic {
    if universal {
        return Heic::Off(
            "the universal macOS build cannot link a single-architecture libheif".to_owned(),
        );
    }
    let Some(found) = libheif else {
        return Heic::Off(format!(
            "libheif {}.{} or newer was not found on the {} build machine",
            LIBHEIF_MIN.0,
            LIBHEIF_MIN.1,
            os.label()
        ));
    };
    if found == "vcpkg" {
        return Heic::On;
    }
    let mut parts = found
        .trim()
        .split('.')
        .map(|p| p.parse::<u32>().unwrap_or(0));
    let version = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if version >= LIBHEIF_MIN {
        Heic::On
    } else {
        Heic::Off(format!(
            "the {} build machine has libheif {}, older than {}.{}",
            os.label(),
            found.trim(),
            LIBHEIF_MIN.0,
            LIBHEIF_MIN.1
        ))
    }
}

/// The release-notes line for a platform built without HEIC.
pub fn heic_note(os: Os, reason: &str) -> String {
    format!(
        "- **{}:** this build has no HEIC/HEIF decoding ({reason}). HEIC photos are still found, catalogued, \
         exactly de-duplicated and exported; they show \"preview unavailable\" and are not matched by look.\n",
        os.label()
    )
}

/// The `cargo build` arguments for this HEIC plan.
pub fn heic_build_args(heic: &Heic) -> Vec<&'static str> {
    match heic {
        Heic::On => vec!["--features", "photosquirrel/heic,photosquirrel-cli/heic"],
        Heic::Off(_) => Vec::new(),
    }
}

/// A file listing (one path per line, any separator) names the licence notices file.
pub fn listing_has_licences(listing: &str) -> bool {
    listing.lines().any(|line| {
        line.trim_end()
            .rsplit(['/', '\\', ' '])
            .next()
            .is_some_and(|name| name == LICENCES_FILE)
    })
}

/// One licence text and the crates it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: String,
    pub name: String,
    pub text: String,
    /// `name version`, sorted.
    pub crates: Vec<String>,
}

/// Merge cargo-about JSON reports (one per shipped binary) into notices: one per distinct licence text, our own
/// workspace crates left out.
pub fn merge_notices(reports: &[serde_json::Value]) -> Result<Vec<Notice>> {
    let mut notices: Vec<Notice> = Vec::new();
    for report in reports {
        let licences = report["licenses"]
            .as_array()
            .context("cargo-about report without `licenses`")?;
        for licence in licences {
            let field = |k: &str| licence[k].as_str().unwrap_or_default().to_owned();
            let (id, name, text) = (field("id"), field("name"), field("text"));
            let mut crates: Vec<String> = licence["used_by"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|u| &u["crate"])
                // Workspace crates have no registry source; they are PhotoSquirrel itself.
                .filter(|c| !c["source"].is_null())
                .map(|c| {
                    format!(
                        "{} {}",
                        c["name"].as_str().unwrap_or("?"),
                        c["version"].as_str().unwrap_or("?")
                    )
                })
                .collect();
            if crates.is_empty() {
                continue;
            }
            match notices.iter_mut().find(|n| n.id == id && n.text == text) {
                Some(n) => n.crates.append(&mut crates),
                None => notices.push(Notice {
                    id,
                    name,
                    text,
                    crates,
                }),
            }
        }
    }
    for n in &mut notices {
        n.crates.sort();
        n.crates.dedup();
    }
    notices.sort_by(|a, b| (&a.id, &a.crates).cmp(&(&b.id, &b.crates)));
    Ok(notices)
}

/// The text of `THIRD-PARTY-LICENSES`.
pub fn render_notices(version: &str, notices: &[Notice]) -> String {
    let rule = "=".repeat(78);
    let mut out = format!(
        "PhotoSquirrel {version}: third-party software notices\n\n\
         PhotoSquirrel is licensed under the Apache License 2.0 (see LICENSE). The programs in this package also \
         contain\nthe open-source components listed below, each under the licence text that follows its name.\n\
         This software is based in part on the work of the Independent JPEG Group.\n\n"
    );
    let mut ids: Vec<&str> = notices.iter().map(|n| n.id.as_str()).collect();
    ids.dedup();
    out.push_str("Licences used:\n");
    for id in ids {
        let count: usize = notices
            .iter()
            .filter(|n| n.id == id)
            .map(|n| n.crates.len())
            .sum();
        let noun = if count == 1 {
            "component"
        } else {
            "components"
        };
        out.push_str(&format!("  {id}: {count} {noun}\n"));
    }
    for n in notices {
        out.push_str(&format!(
            "\n{rule}\n{} ({})\nUsed by: {}\n{rule}\n\n{}\n",
            n.name,
            n.id,
            n.crates.join(", "),
            n.text.trim_end()
        ));
    }
    out
}

/// The notices must acknowledge mozjpeg-sys (libjpeg-turbo) under each of its three licences.
pub fn check_notices(notices: &[Notice]) -> Result<()> {
    for id in ["IJG", "Zlib", "BSD-3-Clause"] {
        let covered = notices
            .iter()
            .any(|n| n.id == id && n.crates.iter().any(|c| c.starts_with("mozjpeg-sys ")));
        if !covered {
            bail!("{LICENCES_FILE} has no {id} notice for mozjpeg-sys");
        }
    }
    Ok(())
}

pub fn run(root: &Path, opts: Options) -> Result<()> {
    let version = workspace_version(root)?;
    if let Some(tag) = &opts.tag {
        check_tag(tag, &version)?;
    }
    let os = Os::host()?;
    if opts.docker {
        if os != Os::Linux {
            bail!("--docker builds the Linux packages and needs a Linux host");
        }
        return in_docker(root, &opts);
    }
    if opts.universal && os != Os::Macos {
        bail!("--universal is for macOS");
    }

    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let out = root.join("target/packages");
    let staging = target_dir.join("packager");
    for dir in [&out, &staging] {
        if dir.exists() {
            fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
        }
        fs::create_dir_all(dir)?;
    }

    let heic = heic_plan(os, opts.universal, probe_libheif(os).as_deref());
    match &heic {
        Heic::On => eprintln!("HEIC: on (libheif found)"),
        Heic::Off(reason) => {
            eprintln!("HEIC: off ({reason})");
            fs::write(
                out.join(format!("heic-{}.txt", os.name())),
                heic_note(os, reason),
            )?;
        }
    }
    let features = heic_build_args(&heic);
    let (bins, arch) = if opts.universal {
        (
            universal_binaries(&target_dir, &features)?,
            "universal".to_owned(),
        )
    } else {
        build(&features)?;
        (target_dir.join(PROFILE), std::env::consts::ARCH.to_owned())
    };
    let licences = licence_notices(root, &target_dir, &version)?;

    let kinds = kinds(os);
    let formats: Vec<&str> = kinds
        .iter()
        .filter_map(|k| k.packager().map(|(f, _)| f))
        .collect();
    let installed = Command::new("cargo")
        .args(["packager", "--version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !installed {
        bail!("cargo-packager is missing: `cargo install cargo-packager --locked`");
    }
    // cargo-packager 0.11.8 parses `--binaries-dir` but ignores it; it finds the binaries from the profile
    // and target instead, so the universal build sits where `--target universal-apple-darwin` points.
    let mut packager = Command::new("cargo");
    packager
        .args([
            "packager",
            "-p",
            "photosquirrel",
            "--profile",
            PROFILE,
            "--formats",
        ])
        .arg(formats.join(","))
        .arg("--out-dir")
        .arg(&staging);
    if opts.universal {
        packager.args(["--target", "universal-apple-darwin"]);
    }
    crate::run("cargo packager", &mut packager)?;

    let mut written = Vec::new();
    for &kind in kinds {
        let name = artifact_name(&version, os, &arch, kind);
        let dest = out.join(&name);
        match kind.packager() {
            Some((_, ext)) => {
                let produced = find_with_suffix(&staging, ext)?;
                fs::copy(&produced, &dest)
                    .with_context(|| format!("copying {}", produced.display()))?;
            }
            None => archive(root, os, &bins, &licences, &name, &staging, &dest, &version)?,
        }
        verify_licences(os, kind, &dest, &staging)?;
        written.push(name);
    }

    for name in &written {
        let bytes = fs::read(out.join(name))?;
        fs::write(
            out.join(format!("{name}.sha256")),
            checksum_line(&bytes, name),
        )?;
    }
    println!("\nPackages in {}:", out.display());
    for name in &written {
        let size = fs::metadata(out.join(name))?.len();
        println!("  {name}  ({:.1} MB)", size as f64 / 1e6);
    }
    Ok(())
}

fn build(extra: &[&str]) -> Result<()> {
    crate::run(
        "build the dist profile",
        Command::new("cargo")
            .args([
                "build",
                "--profile",
                PROFILE,
                "-p",
                "photosquirrel",
                "-p",
                "photosquirrel-cli",
            ])
            .args(extra),
    )
}

/// Build both macOS architectures and join each binary with `lipo`.
fn universal_binaries(target_dir: &Path, features: &[&str]) -> Result<PathBuf> {
    for target in MAC_TARGETS {
        crate::run(
            "rustup target add",
            Command::new("rustup").args(["target", "add", target]),
        )?;
        let mut args = vec!["--target", target];
        args.extend_from_slice(features);
        build(&args)?;
    }
    let out = target_dir.join("universal-apple-darwin").join(PROFILE);
    fs::create_dir_all(&out)?;
    for bin in ["photosquirrel", "photosquirrel-cli"] {
        let mut lipo = Command::new("lipo");
        lipo.arg("-create").arg("-output").arg(out.join(bin));
        for target in MAC_TARGETS {
            lipo.arg(target_dir.join(target).join(PROFILE).join(bin));
        }
        crate::run("lipo", &mut lipo)?;
    }
    Ok(out)
}

/// Look for libheif the way libheif-sys will: pkg-config on Linux and macOS, vcpkg on Windows.
fn probe_libheif(os: Os) -> Option<String> {
    if os == Os::Windows {
        let root = PathBuf::from(std::env::var_os("VCPKG_ROOT")?);
        let lib = root.join("installed/x64-windows-static-md/lib/heif.lib");
        return lib.is_file().then(|| "vcpkg".to_owned());
    }
    let out = Command::new("pkg-config")
        .args(["--modversion", "libheif"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Generate `THIRD-PARTY-LICENSES` with cargo-about for each shipped binary's dependency graph (all release
/// targets, see `about.toml`), check it and return its path.
fn licence_notices(root: &Path, target_dir: &Path, version: &str) -> Result<PathBuf> {
    let installed = Command::new("cargo")
        .args(["about", "--version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !installed {
        bail!(
            "cargo-about is missing: `cargo install cargo-about --version 0.9.2 --features cli --locked`"
        );
    }
    let mut reports = Vec::new();
    for bin in SHIPPED {
        let json = target_dir.join(format!("about-{bin}.json"));
        crate::run(
            &format!("cargo about ({bin})"),
            Command::new("cargo")
                .args([
                    "about", "generate", "--locked", "--fail", "--format", "json",
                ])
                .arg("-c")
                .arg(root.join("about.toml"))
                .arg("-m")
                .arg(root.join("crates").join(bin).join("Cargo.toml"))
                .arg("-o")
                .arg(&json),
        )?;
        reports.push(serde_json::from_str(&fs::read_to_string(&json)?)?);
    }
    let notices = merge_notices(&reports)?;
    check_notices(&notices)?;
    let dir = root.join(LICENCES_DIR);
    fs::create_dir_all(&dir)?;
    let path = dir.join(LICENCES_FILE);
    fs::write(&path, render_notices(version, &notices))?;
    eprintln!(
        "licence notices: {} ({} licence texts)",
        path.display(),
        notices.len()
    );
    Ok(path)
}

/// Check a finished package contains `THIRD-PARTY-LICENSES`, using the platform's own tools.
fn verify_licences(os: Os, kind: Kind, package: &Path, staging: &Path) -> Result<()> {
    let listing = |cmd: &mut Command| -> Result<String> {
        let out = cmd
            .output()
            .with_context(|| format!("listing {}", package.display()))?;
        if !out.status.success() {
            bail!("listing {} failed: {out:?}", package.display());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let found = match kind {
        Kind::TarGz => listing(Command::new("tar").arg("-tzf").arg(package))?,
        Kind::Zip => {
            let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            listing(
                Command::new(PathBuf::from(system).join("System32").join("tar.exe"))
                    .arg("-tf")
                    .arg(package),
            )?
        }
        Kind::Deb => match listing(Command::new("dpkg-deb").arg("-c").arg(package)) {
            Ok(text) => text,
            // No dpkg (a non-Debian host): unpack the data member with binutils' ar.
            Err(_) => {
                let members = listing(Command::new("ar").arg("t").arg(package))?;
                let data = members
                    .lines()
                    .find(|m| m.starts_with("data.tar"))
                    .context("the .deb has no data.tar member")?;
                let out = Command::new("ar")
                    .arg("p")
                    .arg(package)
                    .arg(data)
                    .output()?;
                let file = staging.join(data);
                fs::write(&file, out.stdout)?;
                let text = listing(Command::new("tar").arg("-tf").arg(&file));
                fs::remove_file(&file).ok();
                text?
            }
        },
        Kind::AppImage => {
            let dir = staging.join("appimage-check");
            fs::create_dir_all(&dir)?;
            listing(
                Command::new(package)
                    .arg("--appimage-extract")
                    .current_dir(&dir),
            )?;
            let root = dir.join("squashfs-root");
            let text = walk(&root)?.join("\n");
            fs::remove_dir_all(&dir).ok();
            text
        }
        Kind::Dmg => {
            let mount = staging.join("dmg-check");
            fs::create_dir_all(&mount)?;
            crate::run(
                "mount the dmg",
                Command::new("hdiutil")
                    .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
                    .arg(&mount)
                    .arg(package),
            )?;
            let text = walk(&mount).map(|files| files.join("\n"));
            crate::run(
                "unmount the dmg",
                Command::new("hdiutil").arg("detach").arg(&mount),
            )?;
            text?
        }
        // 7-Zip lists the files an NSIS installer would write.
        Kind::Setup => listing(Command::new("7z").arg("l").arg(package))?,
    };
    if !listing_has_licences(&found) {
        bail!(
            "{} has no {LICENCES_FILE} ({} package)",
            package.display(),
            os.label()
        );
    }
    eprintln!("licence notices present: {}", package.display());
    Ok(())
}

/// Every file under `dir`, recursively, without following symlinks.
fn walk(dir: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(entry.path());
            } else {
                files.push(entry.path().display().to_string());
            }
        }
    }
    Ok(files)
}

/// The plain archive: a folder with both binaries, the licence, the third-party notices and the README, then
/// the smoke test on the binary that went in.
#[allow(clippy::too_many_arguments)]
fn archive(
    root: &Path,
    os: Os,
    bins: &Path,
    licences: &Path,
    name: &str,
    staging: &Path,
    dest: &Path,
    version: &str,
) -> Result<()> {
    let folder = name
        .strip_suffix(".tar.gz")
        .or_else(|| name.strip_suffix(".zip"))
        .unwrap_or(name);
    let dir = staging.join(folder);
    fs::create_dir_all(&dir)?;
    for bin in ["photosquirrel", "photosquirrel-cli"] {
        let file = os.exe(bin);
        fs::copy(bins.join(&file), dir.join(&file))
            .with_context(|| format!("copying {file} from {}", bins.display()))?;
    }
    for doc in ["LICENSE", "README.md"] {
        fs::copy(root.join(doc), dir.join(doc))?;
    }
    fs::copy(licences, dir.join(LICENCES_FILE))?;

    let cli = dir.join(os.exe("photosquirrel-cli"));
    let output = Command::new(&cli)
        .arg("--version")
        .output()
        .with_context(|| format!("running {}", cli.display()))?;
    if !output.status.success() {
        bail!("packaged photosquirrel-cli --version failed: {output:?}");
    }
    check_smoke(&String::from_utf8_lossy(&output.stdout), version)?;
    eprintln!(
        "smoke test: {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );

    // Windows' own bsdtar writes a zip when given -a and a .zip name. Name it explicitly: in CI's bash
    // steps Git's GNU tar comes first on PATH, and it can't write zips. GNU and BSD tar write .tar.gz with -z.
    let mut tar = match os {
        Os::Windows => {
            let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let mut tar = Command::new(PathBuf::from(system).join("System32").join("tar.exe"));
            tar.arg("-a").arg("-cf");
            tar
        }
        _ => {
            let mut tar = Command::new("tar");
            tar.arg("-czf");
            tar
        }
    };
    tar.arg(dest).arg("-C").arg(staging).arg(folder);
    crate::run("archive", &mut tar)
}

fn find_with_suffix(dir: &Path, suffix: &str) -> Result<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(suffix))
        {
            found.push(path);
        }
    }
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("cargo-packager wrote no *{suffix} in {}", dir.display()),
        _ => bail!("several *{suffix} in {}: {found:?}", dir.display()),
    }
}

/// Run this command again inside the Linux packaging image, as the calling user, with the repository at
/// /work. The container keeps its own target directory and caches under target/docker.
fn in_docker(root: &Path, opts: &Options) -> Result<()> {
    crate::run(
        "build the packaging image",
        Command::new("docker")
            .args(["build", "-t", DOCKER_IMAGE, "-f"])
            .arg(root.join("packaging/linux/Dockerfile"))
            .arg(root.join("packaging/linux")),
    )?;
    let cache = root.join("target/docker");
    for dir in ["home", "cargo-home"] {
        fs::create_dir_all(cache.join(dir))?;
    }
    let id = |flag: &str| -> Result<String> {
        let out = Command::new("id").arg(flag).output()?;
        Ok(String::from_utf8(out.stdout)?.trim().to_owned())
    };
    let mut docker = Command::new("docker");
    docker
        .args(["run", "--rm", "--user"])
        .arg(format!("{}:{}", id("-u")?, id("-g")?))
        .arg("-v")
        .arg(format!("{}:/work", root.display()))
        .args([
            "-w",
            "/work",
            "-e",
            "HOME=/work/target/docker/home",
            "-e",
            "CARGO_HOME=/work/target/docker/cargo-home",
            "-e",
            "CARGO_TARGET_DIR=/work/target/docker",
        ]);
    if std::env::var_os("CI").is_some() {
        docker.args(["-e", "CI=true"]);
    }
    // Keep the host's build parallelism limit (AGENTS.md "Machine resources").
    if let Some(jobs) = std::env::var_os("CARGO_BUILD_JOBS") {
        docker
            .arg("-e")
            .arg(format!("CARGO_BUILD_JOBS={}", jobs.to_string_lossy()));
    }
    docker.args([DOCKER_IMAGE, "cargo", "xtask", "package"]);
    if let Some(tag) = &opts.tag {
        docker.args(["--tag", tag]);
    }
    crate::run("package inside the Linux image", &mut docker)
}

fn workspace_version(root: &Path) -> Result<String> {
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    manifest["workspace"]["package"]["version"]
        .as_str()
        .map(str::to_owned)
        .context("workspace.package.version missing from Cargo.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf()
    }

    fn packager_metadata() -> toml::Value {
        let text =
            fs::read_to_string(root().join("crates/photosquirrel/Cargo.toml")).expect("manifest");
        let manifest: toml::Value = toml::from_str(&text).expect("toml");
        manifest["package"]["metadata"]["packager"].clone()
    }

    #[test]
    fn scenario_release_packaging_package_formats_per_platform() {
        assert_eq!(kinds(Os::Linux), [Kind::AppImage, Kind::Deb, Kind::TarGz]);
        assert_eq!(kinds(Os::Macos), [Kind::Dmg, Kind::TarGz]);
        assert_eq!(kinds(Os::Windows), [Kind::Setup, Kind::Zip]);
        // Only the archive is ours; everything else comes from a cargo-packager format.
        for os in [Os::Linux, Os::Macos, Os::Windows] {
            let (archive, installers) = kinds(os).split_last().expect("non-empty");
            assert!(archive.packager().is_none());
            assert!(installers.iter().all(|k| k.packager().is_some()));
        }
    }

    #[test]
    fn scenario_release_packaging_packages_contain_the_app_and_the_cli() {
        let binaries = packager_metadata()["binaries"].clone();
        let binaries = binaries.as_array().expect("binaries list");
        let main: Vec<_> = binaries
            .iter()
            .filter(|b| b["main"].as_bool() == Some(true))
            .map(|b| b["path"].as_str())
            .collect();
        assert_eq!(main, [Some("photosquirrel")]);
        assert!(
            binaries
                .iter()
                .any(|b| b["path"].as_str() == Some("photosquirrel-cli"))
        );
    }

    #[test]
    fn scenario_release_packaging_artifact_names_include_version_os_and_architecture() {
        assert_eq!(
            artifact_name("0.1.0", Os::Linux, "x86_64", Kind::AppImage),
            "PhotoSquirrel-0.1.0-linux-x86_64.AppImage"
        );
        assert_eq!(
            artifact_name("0.1.0", Os::Windows, "x86_64", Kind::Setup),
            "PhotoSquirrel-0.1.0-windows-x86_64-setup.exe"
        );
        assert_eq!(
            artifact_name("0.1.0", Os::Macos, "universal", Kind::Dmg),
            "PhotoSquirrel-0.1.0-macos-universal.dmg"
        );
    }

    #[test]
    fn scenario_release_packaging_every_artifact_has_a_checksum() {
        let dir = std::env::temp_dir().join(format!("photosquirrel-sha-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let name = "PhotoSquirrel-0.1.0-linux-x86_64.tar.gz";
        fs::write(dir.join(name), b"abc").expect("artifact");
        let line = checksum_line(b"abc", name);
        assert_eq!(
            line,
            format!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  {name}\n")
        );
        fs::write(dir.join(format!("{name}.sha256")), &line).expect("checksum");
        if let Ok(status) = Command::new("sha256sum")
            .args(["-c", &format!("{name}.sha256")])
            .current_dir(&dir)
            .output()
        {
            assert!(status.status.success(), "sha256sum -c rejected {line}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scenario_release_packaging_mismatched_tag_is_rejected() {
        assert!(check_tag("v0.1.0", "0.1.0").is_ok());
        let err = check_tag("v0.2.0", "0.1.0")
            .expect_err("mismatch")
            .to_string();
        assert!(err.contains("v0.2.0") && err.contains("0.1.0"), "{err}");
        assert!(check_tag("0.1.0", "0.1.0").is_err());
    }

    #[test]
    fn scenario_release_packaging_debian_package_declares_runtime_libraries() {
        let depends = packager_metadata()["deb"]["depends"].clone();
        let depends: Vec<_> = depends
            .as_array()
            .expect("depends list")
            .iter()
            .filter_map(|d| d.as_str())
            .collect();
        for lib in [
            "libdbus-1-3",
            "libxcb1",
            "libxkbcommon0",
            "libxkbcommon-x11-0",
            "libvulkan1",
        ] {
            assert!(depends.contains(&lib), "{lib} missing from {depends:?}");
        }
    }

    #[test]
    fn scenario_release_packaging_wrong_binary_fails_the_smoke_test() {
        assert!(check_smoke("photosquirrel-cli 0.1.0\n", "0.1.0").is_ok());
        assert!(check_smoke("photosquirrel-cli 0.0.9\n", "0.1.0").is_err());
        assert!(check_smoke("", "0.1.0").is_err());
    }

    #[test]
    fn scenario_release_packaging_linux_build_image_pins_glibc_floor() {
        let text =
            fs::read_to_string(root().join("packaging/linux/Dockerfile")).expect("Dockerfile");
        let from: Vec<_> = text.lines().filter(|l| l.starts_with("FROM ")).collect();
        assert_eq!(from, ["FROM ubuntu:22.04"]);
    }

    #[test]
    fn scenario_release_packaging_heic_fallback_noted() {
        assert_eq!(heic_plan(Os::Linux, false, Some("1.23.3")), Heic::On);
        assert_eq!(heic_plan(Os::Windows, false, Some("vcpkg")), Heic::On);
        assert_eq!(
            heic_build_args(&Heic::On),
            ["--features", "photosquirrel/heic,photosquirrel-cli/heic"]
        );
        // Ubuntu 22.04's libheif is 1.12; no libheif at all; the universal macOS build.
        for (os, universal, found) in [
            (Os::Linux, false, Some("1.12.0")),
            (Os::Windows, false, None),
            (Os::Macos, true, Some("1.19.5")),
        ] {
            let Heic::Off(reason) = heic_plan(os, universal, found) else {
                panic!("{os:?} {found:?} should build without HEIC");
            };
            assert!(heic_build_args(&Heic::Off(reason.clone())).is_empty());
            let note = heic_note(os, &reason);
            assert!(note.contains(os.label()) && note.contains("HEIC") && note.contains(&reason));
        }
        // The release job folds the per-platform notes into the release notes, and attaches only packages.
        let workflow = release_workflow();
        assert!(
            workflow.contains("packages/heic-*.txt"),
            "heic notes not folded in"
        );
        assert!(workflow.contains("packages/PhotoSquirrel-*"));
    }

    fn release_workflow() -> String {
        fs::read_to_string(root().join(".github/workflows/release.yml")).expect("release.yml")
    }

    #[test]
    fn scenario_release_packaging_release_workflow_covers_every_platform() {
        let text = release_workflow();
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        assert!(
            lines.contains(&"tags: [\"v*\"]"),
            "not triggered by v* tags"
        );
        for (name, os, args) in [
            ("linux", "ubuntu-", "--docker"),
            ("macos", "macos-", "--universal"),
            ("windows", "windows-", "\"\""),
        ] {
            let at = lines
                .iter()
                .position(|l| *l == format!("- name: {name}"))
                .unwrap_or_else(|| panic!("no {name} matrix entry"));
            assert!(
                lines[at + 1].starts_with(&format!("os: {os}")),
                "{name}: {}",
                lines[at + 1]
            );
            assert_eq!(lines[at + 2], format!("args: {args}"));
        }
        assert!(text.contains("cargo xtask package $args"));
        // One release job, after every package job, publishing on tags only.
        let creates: Vec<_> = lines
            .iter()
            .filter(|l| l.starts_with("gh release create"))
            .collect();
        assert_eq!(creates.len(), 1, "{creates:?}");
        let release = text.split("\n  release:\n").nth(1).expect("release job");
        assert!(release.contains("needs: package"));
        assert!(release.contains("if: startsWith(github.ref, 'refs/tags/v')"));
        assert!(release.contains("merge-multiple: true"));
    }

    fn about_report(entries: &[(&str, &str, &[(&str, Option<&str>)])]) -> serde_json::Value {
        let licenses: Vec<_> = entries
            .iter()
            .map(|(id, text, crates)| {
                let used_by: Vec<_> = crates
                    .iter()
                    .map(|(name, source)| {
                        serde_json::json!({"crate": {"name": name, "version": "1.0.0", "source": source}})
                    })
                    .collect();
                serde_json::json!({"id": id, "name": format!("{id} licence"), "text": text, "used_by": used_by})
            })
            .collect();
        serde_json::json!({ "licenses": licenses })
    }

    #[test]
    fn scenario_release_packaging_licence_notices_acknowledge_mozjpeg_sys() {
        let registry = Some("registry+https://github.com/rust-lang/crates.io-index");
        let app = about_report(&[
            (
                "IJG",
                "IJG text: based in part on the work of the Independent JPEG Group",
                &[("mozjpeg-sys", registry)],
            ),
            ("Zlib", "zlib text", &[("mozjpeg-sys", registry)]),
            ("BSD-3-Clause", "bsd text", &[("mozjpeg-sys", registry)]),
            (
                "Apache-2.0",
                "apache text",
                &[("ps-model", None), ("anyhow", registry)],
            ),
        ]);
        let cli = about_report(&[
            (
                "Apache-2.0",
                "apache text",
                &[("anyhow", registry), ("clap", registry)],
            ),
            ("MIT", "mit text", &[("ps-media", None)]),
        ]);
        let notices = merge_notices(&[app, cli]).expect("merge");
        check_notices(&notices).expect("mozjpeg-sys covered");
        let apache = notices
            .iter()
            .find(|n| n.id == "Apache-2.0")
            .expect("apache");
        assert_eq!(apache.crates, ["anyhow 1.0.0", "clap 1.0.0"]);
        assert!(
            notices.iter().all(|n| n.id != "MIT"),
            "workspace-only licence kept"
        );
        let text = render_notices("0.1.0", &notices);
        for needle in [
            "IJG text",
            "zlib text",
            "bsd text",
            "Used by: mozjpeg-sys 1.0.0",
            "Independent JPEG Group",
        ] {
            assert!(text.contains(needle), "{needle} missing");
        }
        assert!(!text.contains("ps-model"));

        let without_zlib: Vec<_> = notices.iter().filter(|n| n.id != "Zlib").cloned().collect();
        let err = check_notices(&without_zlib)
            .expect_err("Zlib missing")
            .to_string();
        assert!(err.contains("Zlib") && err.contains("mozjpeg-sys"), "{err}");

        assert!(listing_has_licences(
            "-rw-r--r-- root/root 1 2026 ./usr/lib/photosquirrel/THIRD-PARTY-LICENSES\n"
        ));
        assert!(listing_has_licences(
            "2026-10-21 ....A  12  10  THIRD-PARTY-LICENSES\n"
        ));
        assert!(listing_has_licences(
            "PhotoSquirrel-0.1.0-windows-x86_64\\THIRD-PARTY-LICENSES"
        ));
        assert!(!listing_has_licences(
            "LICENSE\nREADME.md\nTHIRD-PARTY-LICENSES.bak\n"
        ));
    }

    #[test]
    fn scenario_release_packaging_readme_screenshots_exist() {
        let readme = fs::read_to_string(root().join("README.md")).expect("README.md");
        let mut images = Vec::new();
        for part in readme.split("](").skip(1) {
            let target = part.split(')').next().unwrap_or_default();
            if target.ends_with(".png") {
                images.push(target.to_owned());
            }
        }
        for image in &images {
            assert!(
                root().join(image).is_file(),
                "README references missing {image}"
            );
        }
        for view in ["gallery", "scan-dashboard", "duplicates", "export"] {
            assert!(
                images.iter().any(|i| i.contains(view)),
                "README shows no {view} screenshot: {images:?}"
            );
        }
        for promise in ["read-only", "copies", "telemetry"] {
            assert!(
                readme.contains(promise),
                "README does not mention {promise}"
            );
        }
    }
}
