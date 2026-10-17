//! `cargo xtask package`: release installers for the host OS (ADR-0030). Builds the `dist` profile, runs
//! cargo-packager with this OS's formats, adds a plain archive, renames everything to
//! `PhotoSquirrel-<version>-<os>-<arch>.<ext>` in `target/packages/`, smoke-tests the packaged CLI and writes a
//! `.sha256` beside each file. `--docker` does the Linux build in `packaging/linux/Dockerfile` (glibc 2.35
//! floor); `--universal` makes one arm64 + x86_64 macOS build.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const PRODUCT: &str = "PhotoSquirrel";
const PROFILE: &str = "dist";
const DOCKER_IMAGE: &str = "photosquirrel-packaging";
const MAC_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

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
    let (bins, arch) = if opts.universal {
        (universal_binaries(&target_dir)?, "universal".to_owned())
    } else {
        build(&[])?;
        (target_dir.join(PROFILE), std::env::consts::ARCH.to_owned())
    };

    let out = root.join("target/packages");
    let staging = target_dir.join("packager");
    for dir in [&out, &staging] {
        if dir.exists() {
            fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
        }
        fs::create_dir_all(dir)?;
    }

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
            None => archive(root, os, &bins, &name, &staging, &dest, &version)?,
        }
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
fn universal_binaries(target_dir: &Path) -> Result<PathBuf> {
    for target in MAC_TARGETS {
        crate::run(
            "rustup target add",
            Command::new("rustup").args(["target", "add", target]),
        )?;
        build(&["--target", target])?;
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

/// The plain archive: a folder with both binaries, the licence and the README, then the smoke test on the
/// binary that went in.
fn archive(
    root: &Path,
    os: Os,
    bins: &Path,
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
}
