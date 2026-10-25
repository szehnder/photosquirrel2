//! `cargo xtask package`: release installers for the host OS (ADR-0030). Builds the `dist` profile, runs
//! cargo-packager with this OS's formats, adds a plain archive, renames everything to
//! `PhotoSquirrel-<version>-<os>-<arch>.<ext>` in `target/packages/`, smoke-tests the packaged CLI and writes a
//! `.sha256` beside each file. `--docker` does the Linux build in `packaging/linux/Dockerfile` (glibc 2.35
//! floor); `--universal` makes one arm64 + x86_64 macOS build.
//!
//! Every package carries `THIRD-PARTY-LICENSES`, generated from `Cargo.lock` by cargo-about (ADR-0058) and checked
//! in each finished package. No package contains an HEVC decoder (ADR-0089): packaging fails when the shipped
//! programs' dependency graph has an HEVC decoder crate or a Linux program links libheif or libde265; HEIC is
//! decoded by the operating system at run time. AVIF is decoded by the bundled rav1d with its assembly (ADR-0090):
//! packaging refuses an x86_64 build without nasm, and the notices must name rav1d. Packages
//! carry nothing their notices do not cover (ADR-0060): the notices offer the source of the statically linked LGPL
//! crates, the AppImage bundles no system library, and the Windows programs need no Visual C++ runtime. The AppImage
//! starts with the runtime pinned in `packaging/linux/appimage-runtime.toml`, whose components the Linux notices name
//! (ADR-0073).

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
/// Crates that are (or bundle) an HEVC decoder: none may be in a shipped program's dependency graph (ADR-0089).
pub const HEVC_DENY: &[&str] = &[
    "libheif-rs",
    "libheif-sys",
    "libde265",
    "ffmpeg",
    "x265",
    "hevc",
];
/// Shared libraries no shipped Linux program may need (ADR-0089: libheif is loaded at run time, never linked).
pub const NEEDED_DENY: &[&str] = &["libheif", "libde265"];

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
    let mut words = output.split_whitespace();
    if words.next() != Some("photosquirrel-cli") || words.next() != Some(version) {
        bail!(
            "packaged photosquirrel-cli reported {:?}, expected version {version}",
            output.trim()
        );
    }
    Ok(())
}

/// Crates of a dependency graph (`cargo tree --prefix none` lines: `name vX.Y.Z ...`) that are HEVC decoders.
pub fn hevc_components(tree: &str) -> Vec<String> {
    let mut v: Vec<String> = tree
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|name| HEVC_DENY.iter().any(|d| name.starts_with(d)))
        .map(str::to_owned)
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Fail when the graph has an HEVC decoder crate.
pub fn check_hevc_free(tree: &str) -> Result<()> {
    let found = hevc_components(tree);
    if !found.is_empty() {
        bail!(
            "the shipped programs depend on an HEVC decoder ({}); PhotoSquirrel ships none (ADR-0089)",
            found.join(", ")
        );
    }
    Ok(())
}

/// The `NEEDED` entries of `readelf -d` output.
pub fn needed_libs(readelf: &str) -> Vec<String> {
    readelf
        .lines()
        .filter(|l| l.contains("(NEEDED)"))
        .filter_map(|l| {
            let start = l.find('[')? + 1;
            let end = l[start..].find(']')? + start;
            Some(l[start..end].to_owned())
        })
        .collect()
}

/// Fail when a Linux program links libheif or an HEVC decoder.
pub fn check_needed(program: &str, needed: &[String]) -> Result<()> {
    let bad: Vec<&String> = needed
        .iter()
        .filter(|n| NEEDED_DENY.iter().any(|d| n.starts_with(d)))
        .collect();
    if !bad.is_empty() {
        bail!(
            "{program} links {}: the system HEIF library is loaded at run time only (ADR-0089)",
            bad.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

/// The shipped programs' normal dependency graph for every package target.
fn shipped_tree(root: &Path) -> Result<String> {
    let out = Command::new("cargo")
        .current_dir(root)
        .args([
            "tree",
            "--locked",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--target",
            "all",
            "-p",
            "photosquirrel",
            "-p",
            "photosquirrel-cli",
        ])
        .output()
        .context("cargo tree")?;
    if !out.status.success() {
        bail!("cargo tree: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// rav1d's x86_64 assembly needs nasm (ADR-0090): packages are never built without it.
fn check_avif_assembly(universal: bool) -> Result<()> {
    // The universal macOS build has an x86_64 half; aarch64 assembly needs only the C toolchain.
    if !universal && std::env::consts::ARCH != "x86_64" {
        return Ok(());
    }
    let ok = Command::new("nasm")
        .arg("-v")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        bail!(
            "nasm is missing: packages are built with AVIF assembly (the `avif-asm` feature, ADR-0090); install nasm"
        );
    }
    Ok(())
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

/// The public repository: the complete corresponding source of a release is its `v<version>` tag.
pub const REPOSITORY: &str = "https://github.com/szehnder/photosquirrel2";
/// Where each platform's package job lists the LGPL crates whose source the release attaches (ADR-0060). Every
/// platform has its own name, so the release job keeps all three lists and merges them
/// (`packaging/merge-lgpl-sources.sh`) into `lgpl-sources.txt`.
pub fn lgpl_sources_file(os: Os) -> String {
    format!("lgpl-sources-{}.txt", os.name())
}

/// The crates under an LGPL licence, as `name version`, sorted. They are linked statically, so each release
/// offers their source and says how to relink (LGPL-2.1 section 6, ADR-0060).
pub fn lgpl_crates(notices: &[Notice]) -> Vec<String> {
    let mut crates: Vec<String> = notices
        .iter()
        .filter(|n| n.id.starts_with("LGPL-"))
        .flat_map(|n| n.crates.iter().cloned())
        .collect();
    crates.sort();
    crates.dedup();
    crates
}

/// The section at the top of `THIRD-PARTY-LICENSES` for the statically linked LGPL components: where the
/// complete source of this release and of each component is, and how to relink against a modified copy.
pub fn render_lgpl_section(version: &str, notices: &[Notice]) -> String {
    let lgpl = lgpl_crates(notices);
    if lgpl.is_empty() {
        return String::new();
    }
    let rule = "-".repeat(78);
    let mut out = format!("LGPL components\n{rule}\n");
    for krate in &lgpl {
        let (name, ver) = krate.split_once(' ').unwrap_or((krate, "?"));
        let licence = notices
            .iter()
            .find(|n| n.id.starts_with("LGPL-") && n.crates.contains(krate))
            .map_or("LGPL", |n| n.id.as_str());
        out.push_str(&format!(
            "{name} {ver} ({licence}) is statically linked into photosquirrel and photosquirrel-cli.\n\
             - Its source: https://crates.io/crates/{name}/{ver} (download \
             https://crates.io/api/v1/crates/{name}/{ver}/download);\n  \
             the GitHub Release of PhotoSquirrel {version} also carries it as {name}-{ver}.crate.\n"
        ));
    }
    out.push_str(&format!(
        "- The complete corresponding source of PhotoSquirrel {version}, with the Cargo.lock that pins every\n  \
         component version and the pinned Rust toolchain (rust-toolchain.toml):\n  \
         {REPOSITORY}/tree/v{version}\n\
         - To relink against a modified copy: unpack the source above and the component's .crate file\n  \
         (`tar -xzf <name>-<version>.crate`), change the component, add to the workspace Cargo.toml\n\n    \
         [patch.crates-io]\n    \
         <name> = {{ path = \"../<name>-<version>\" }}\n\n  \
         and run `cargo build --profile dist -p photosquirrel -p photosquirrel-cli`. The rebuilt programs\n  \
         are in target/dist.\n\n"
    ));
    out
}

/// The text of `THIRD-PARTY-LICENSES`. The Linux notices pass the AppImage runtime pin, which adds its section
/// after the LGPL one and its licence texts at the end (ADR-0073).
pub fn render_notices(version: &str, notices: &[Notice], runtime: Option<&RuntimePin>) -> String {
    let rule = "=".repeat(78);
    let mut out = format!(
        "PhotoSquirrel {version}: third-party software notices\n\n\
         PhotoSquirrel is licensed under the Apache License 2.0 (see LICENSE). The programs in this package also \
         contain\nthe open-source components listed below, each under the licence text that follows its name.\n\
         This software is based in part on the work of the Independent JPEG Group.\n\
         The source of every component is published on crates.io at the version shown.\n\n"
    );
    out.push_str(&render_lgpl_section(version, notices));
    if let Some(pin) = runtime {
        out.push_str(&render_runtime_section(version, pin));
    }
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
    if let Some(pin) = runtime {
        out.push_str(&render_runtime_texts(pin));
    }
    out
}

/// The notices must acknowledge mozjpeg-sys (libjpeg-turbo) under each of its three licences, and the bundled
/// AV1 decoder rav1d under BSD-2-Clause (ADR-0090).
pub fn check_notices(notices: &[Notice]) -> Result<()> {
    let rav1d = notices
        .iter()
        .any(|n| n.id == "BSD-2-Clause" && n.crates.iter().any(|c| c.starts_with("rav1d ")));
    if !rav1d {
        bail!("{LICENCES_FILE} has no BSD-2-Clause notice for rav1d (the bundled AV1 decoder)");
    }
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

/// Shared libraries in an extracted AppImage (`.so` files under `usr/lib*`) that `THIRD-PARTY-LICENSES` does not
/// name. The packager config excludes every system library, so the AppImage should bundle none; one that slips in
/// fails packaging until it is excluded or its notice is added (ADR-0060).
pub fn bundled_libs_without_notice(files: &[String], notices: &str) -> Vec<String> {
    let mut missing: Vec<String> = files
        .iter()
        .filter(|f| f.contains("/usr/lib"))
        .filter_map(|f| f.rsplit(['/', '\\']).next())
        .filter(|name| name.ends_with(".so") || name.contains(".so."))
        .filter(|name| !notices.contains(*name))
        .map(str::to_owned)
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

/// The DLL names a Windows PE executable imports (its import directory).
pub fn pe_imports(bytes: &[u8]) -> Result<Vec<String>> {
    let u16_at = |at: usize| -> Result<usize> {
        let b = bytes.get(at..at + 2).context("PE file truncated")?;
        Ok(u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let u32_at = |at: usize| -> Result<usize> {
        let b = bytes.get(at..at + 4).context("PE file truncated")?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    if bytes.get(..2) != Some(b"MZ") {
        bail!("not a PE file (no MZ header)");
    }
    let pe = u32_at(0x3c)?;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        bail!("not a PE file (no PE signature)");
    }
    let coff = pe + 4;
    let sections = u16_at(coff + 2)?;
    let optional = coff + 20;
    let dirs = match u16_at(optional)? {
        0x10b => optional + 96,
        0x20b => optional + 112,
        magic => bail!("unknown PE optional header magic {magic:#x}"),
    };
    let section_table = optional + u16_at(coff + 16)?;
    let to_offset = |rva: usize| -> Result<usize> {
        for i in 0..sections {
            let header = section_table + i * 40;
            let size = u32_at(header + 8)?.max(u32_at(header + 16)?);
            let va = u32_at(header + 12)?;
            if (va..va + size).contains(&rva) {
                return Ok(rva - va + u32_at(header + 20)?);
            }
        }
        bail!("RVA {rva:#x} is in no section")
    };
    let import_rva = u32_at(dirs + 8)?;
    let mut imports = Vec::new();
    if import_rva == 0 {
        return Ok(imports);
    }
    let mut descriptor = to_offset(import_rva)?;
    loop {
        let name_rva = u32_at(descriptor + 12)?;
        if name_rva == 0 {
            break;
        }
        let start = to_offset(name_rva)?;
        let len = bytes[start..]
            .iter()
            .position(|&b| b == 0)
            .context("unterminated import name")?;
        imports.push(String::from_utf8_lossy(&bytes[start..start + len]).into_owned());
        descriptor += 20;
    }
    Ok(imports)
}

/// A shipped Windows program must not need the Visual C++ runtime, which Windows does not include and the
/// packages do not carry: `.cargo/config.toml` links the CRT statically (`+crt-static`, ADR-0060).
pub fn check_windows_imports(program: &str, imports: &[String]) -> Result<()> {
    let runtime: Vec<&String> = imports
        .iter()
        .filter(|dll| {
            let dll = dll.to_ascii_lowercase();
            dll.starts_with("vcruntime") || dll.starts_with("msvcp")
        })
        .collect();
    if !runtime.is_empty() {
        bail!(
            "{program} imports the Visual C++ runtime ({runtime:?}), which the packages do not ship; build with \
             -C target-feature=+crt-static"
        );
    }
    Ok(())
}

/// The AppImage runtime pin (ADR-0073), relative to the workspace root.
pub const RUNTIME_PIN: &str = "packaging/linux/appimage-runtime.toml";
/// The licence texts the pin's `text` fields name, relative to the workspace root.
const RUNTIME_TEXTS: &str = "packaging/linux/appimage-runtime";
/// The sections appimagetool fills in after copying the runtime: an MD5 digest of the image, update information
/// and the signature with its key. They are zero in the pristine runtime and left out of its pinned hash.
const RUNTIME_TOOL_SECTIONS: [&str; 4] = [".digest_md5", ".upd_info", ".sha256_sig", ".sig_key"];

/// `packaging/linux/appimage-runtime.toml`: the AppImage runtime, its components, the AppRun launcher and the source
/// archives the release attaches for the LGPL components.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RuntimePin {
    pub runtime: PinnedRuntime,
    #[serde(rename = "component")]
    pub components: Vec<RuntimeComponent>,
    pub apprun: PinnedAppRun,
    #[serde(rename = "source", default)]
    pub sources: Vec<PinnedSource>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PinnedRuntime {
    pub release: String,
    pub commit: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RuntimeComponent {
    /// The name the runtime's help text gives the component.
    pub name: String,
    pub title: String,
    pub version: String,
    pub licence: String,
    /// The licence text file under `packaging/linux/appimage-runtime/`.
    pub text: String,
    pub upstream: String,
    /// The licence text itself, read by [`RuntimePin::load`].
    #[serde(skip)]
    pub body: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PinnedAppRun {
    pub title: String,
    pub url: String,
    pub sha256: String,
    pub licence: String,
    pub text: String,
    pub upstream: String,
    #[serde(skip)]
    pub body: String,
}

/// A source archive the release attaches: a byte-stable download (`url`: an upstream release file), or, for code
/// published only as a git repository, a `git archive` of a pinned commit whose tree hash is checked (ADR-0077).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PinnedSource {
    /// The `[[component]]` whose source this is.
    pub name: String,
    pub version: String,
    /// The file name the release attaches it under.
    pub file: String,
    /// A release file that never changes once published.
    #[serde(default)]
    pub url: Option<String>,
    /// The repository a `git archive` is made from, with `commit` and `tree`.
    #[serde(default)]
    pub git: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    /// The commit's tree hash: what the archive must contain, whatever git version writes it.
    #[serde(default)]
    pub tree: Option<String>,
    /// SHA-256 of the attached file.
    pub sha256: String,
}

impl PinnedSource {
    /// Where the source is published: the download URL, or the repository's tree at the commit.
    pub fn location(&self) -> String {
        match (&self.url, &self.git, &self.commit) {
            (Some(url), _, _) => url.clone(),
            (None, Some(git), Some(commit)) => {
                format!("{}/tree/{commit}", git.trim_end_matches(".git"))
            }
            _ => String::new(),
        }
    }

    /// The directory a `git archive` puts every file under: the file name without `.tar`.
    pub fn archive_prefix(&self) -> String {
        format!("{}/", self.file.trim_end_matches(".tar"))
    }
}

impl RuntimePin {
    pub fn parse(text: &str) -> Result<Self> {
        let pin: RuntimePin =
            toml::from_str(text).with_context(|| format!("parsing {RUNTIME_PIN}"))?;
        let hex = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let sums = [&pin.runtime.sha256, &pin.apprun.sha256]
            .into_iter()
            .chain(pin.sources.iter().map(|s| &s.sha256));
        for sum in sums {
            if !hex(sum) {
                bail!("{RUNTIME_PIN}: {sum:?} is not a SHA-256");
            }
        }
        let git_hash = |s: &Option<String>| {
            s.as_deref()
                .is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        for s in &pin.sources {
            match (&s.url, &s.git) {
                (Some(_), None) if s.commit.is_none() && s.tree.is_none() => {}
                (None, Some(_)) => {
                    if !git_hash(&s.commit) || !git_hash(&s.tree) {
                        bail!(
                            "{RUNTIME_PIN}: the git [[source]] {} needs a full `commit` and `tree` hash",
                            s.file
                        );
                    }
                    if !s.file.ends_with(".tar") {
                        bail!(
                            "{RUNTIME_PIN}: the git [[source]] {} must be an uncompressed .tar",
                            s.file
                        );
                    }
                }
                _ => bail!(
                    "{RUNTIME_PIN}: the [[source]] {} needs either `url` or `git`, `commit` and `tree`",
                    s.file
                ),
            }
            if !pin
                .components
                .iter()
                .any(|c| c.name == s.name && c.version == s.version)
            {
                bail!(
                    "{RUNTIME_PIN}: the [[source]] {} is for {} {}, which is not a [[component]]",
                    s.file,
                    s.name,
                    s.version
                );
            }
        }
        for c in pin
            .components
            .iter()
            .filter(|c| c.licence.starts_with("LGPL"))
        {
            if !pin
                .sources
                .iter()
                .any(|s| s.name == c.name && s.version == c.version)
            {
                bail!(
                    "{RUNTIME_PIN}: the LGPL component {} {} has no [[source]] at that version",
                    c.name,
                    c.version
                );
            }
        }
        Ok(pin)
    }

    /// The pin with every licence text read from `packaging/linux/appimage-runtime/`.
    pub fn load(root: &Path) -> Result<Self> {
        let mut pin = Self::parse(&fs::read_to_string(root.join(RUNTIME_PIN))?)?;
        let read = |name: &str| -> Result<String> {
            let path = root.join(RUNTIME_TEXTS).join(name);
            let text =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            if text.trim().is_empty() {
                bail!("{} is empty", path.display());
            }
            Ok(text)
        };
        for c in &mut pin.components {
            c.body = read(&c.text)?;
        }
        pin.apprun.body = read(&pin.apprun.text)?;
        Ok(pin)
    }

    pub fn short_commit(&self) -> &str {
        self.runtime.commit.get(..7).unwrap_or(&self.runtime.commit)
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The sections of an ELF64 little-endian file: (name, file offset, size in the file).
fn elf_sections(elf: &[u8]) -> Result<Vec<(String, usize, usize)>> {
    let u32_at = |at: usize| -> Result<usize> {
        let b = elf.get(at..at + 4).context("ELF file truncated")?;
        Ok(u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
    };
    let u64_at = |at: usize| -> Result<usize> {
        let b = elf.get(at..at + 8).context("ELF file truncated")?;
        usize::try_from(u64::from_le_bytes(b.try_into().expect("8 bytes")))
            .context("ELF offset too large")
    };
    let (shoff, shentsize, shnum, shstrndx) = elf_header(elf)?;
    let header = |i: usize| shoff + i * shentsize;
    let strtab = u64_at(header(shstrndx) + 24)?;
    let mut sections = Vec::with_capacity(shnum);
    for i in 0..shnum {
        let at = header(i);
        let name_at = strtab + u32_at(at)?;
        let len = elf
            .get(name_at..)
            .and_then(|rest| rest.iter().position(|&b| b == 0))
            .context("ELF section name unterminated")?;
        let name = String::from_utf8_lossy(&elf[name_at..name_at + len]).into_owned();
        // SHT_NOBITS (8) occupies no file bytes.
        let size = if u32_at(at + 4)? == 8 {
            0
        } else {
            u64_at(at + 32)?
        };
        sections.push((name, u64_at(at + 24)?, size));
    }
    Ok(sections)
}

/// `(e_shoff, e_shentsize, e_shnum, e_shstrndx)` of an ELF64 little-endian file.
fn elf_header(elf: &[u8]) -> Result<(usize, usize, usize, usize)> {
    if elf.get(..4) != Some(b"\x7fELF".as_slice()) {
        bail!("the AppImage does not start with an ELF runtime");
    }
    if elf.get(4) != Some(&2) || elf.get(5) != Some(&1) {
        bail!("the AppImage runtime is not a 64-bit little-endian ELF file");
    }
    let u16_at = |at: usize| u16::from_le_bytes([elf[at], elf[at + 1]]) as usize;
    if elf.len() < 0x40 {
        bail!("ELF header truncated");
    }
    let shoff = usize::try_from(u64::from_le_bytes(
        elf[0x28..0x30].try_into().expect("8 bytes"),
    ))
    .context("e_shoff too large")?;
    Ok((shoff, u16_at(0x3a), u16_at(0x3c), u16_at(0x3e)))
}

/// The runtime at the front of an AppImage: the ELF image up to the end of its section header table, where the
/// squashfs image (`hsqs`) starts.
pub fn appimage_runtime(appimage: &[u8]) -> Result<&[u8]> {
    let (shoff, shentsize, shnum, _) = elf_header(appimage)?;
    let end = shoff + shentsize * shnum;
    let runtime = appimage
        .get(..end)
        .context("the AppImage ends inside its runtime")?;
    if appimage.get(end..end + 4) != Some(b"hsqs".as_slice()) {
        bail!("no squashfs image (hsqs) right after the AppImage runtime at byte {end}");
    }
    Ok(runtime)
}

/// SHA-256 of the runtime with the sections appimagetool fills in zeroed, so the pristine runtime file and the copy
/// in a built AppImage hash alike.
pub fn normalized_runtime_sha256(runtime: &[u8]) -> Result<String> {
    let mut bytes = runtime.to_vec();
    let sections = elf_sections(runtime)?;
    for wanted in RUNTIME_TOOL_SECTIONS {
        let (_, offset, size) = sections
            .iter()
            .find(|(name, _, _)| name == wanted)
            .with_context(|| format!("the AppImage runtime has no {wanted} section"))?;
        bytes
            .get_mut(*offset..offset + size)
            .with_context(|| format!("{wanted} lies outside the runtime"))?
            .fill(0);
    }
    Ok(sha256_hex(&bytes))
}

/// The components the runtime's help text lists (`  * <name>, licensed under the terms of`), in its order.
pub fn runtime_components(runtime: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(runtime)
        .split(['\0', '\n'])
        .filter_map(|line| {
            line.trim()
                .strip_prefix("* ")?
                .strip_suffix(", licensed under the terms of")
                .map(str::to_owned)
        })
        .collect()
}

/// A built AppImage's runtime is the pinned one and declares the components the notice names.
pub fn check_runtime(pin: &RuntimePin, appimage: &[u8]) -> Result<()> {
    let runtime = appimage_runtime(appimage)?;
    let found = normalized_runtime_sha256(runtime)?;
    if found != pin.runtime.sha256 {
        bail!(
            "the AppImage runtime is not the pinned one: pinned SHA-256 {}, found {found} ({} bytes, pinned {}). \
             If the AppImage tooling now ships another runtime, update {RUNTIME_PIN} and its notice texts in \
             {RUNTIME_TEXTS}/ (ADR-0073)",
            pin.runtime.sha256,
            runtime.len(),
            pin.runtime.size
        );
    }
    check_runtime_components(pin, &runtime_components(runtime))
}

/// The runtime's declared components must be exactly the pin's, so the notice names every one.
pub fn check_runtime_components(pin: &RuntimePin, declared: &[String]) -> Result<()> {
    let pinned: Vec<&str> = pin.components.iter().map(|c| c.name.as_str()).collect();
    if declared != pinned.as_slice() {
        bail!(
            "the AppImage runtime declares the components {declared:?}, but {RUNTIME_PIN} names {pinned:?}: update \
             the pin and its notice texts in {RUNTIME_TEXTS}/ (ADR-0073)"
        );
    }
    Ok(())
}

/// The "AppImage runtime" overview section of `THIRD-PARTY-LICENSES` (Linux notices only).
pub fn render_runtime_section(version: &str, pin: &RuntimePin) -> String {
    let rule = "-".repeat(78);
    let mut out = format!(
        "AppImage runtime\n{rule}\n\
         This section concerns the AppImage package only; the .deb and .tar.gz packages do not contain it.\n\
         The AppImage starts with the AppImage runtime {} (type2-runtime release {},\n\
         https://github.com/AppImage/type2-runtime/tree/{}), a program that mounts the\n\
         image and starts PhotoSquirrel. It contains:\n",
        pin.short_commit(),
        pin.runtime.release,
        pin.runtime.commit
    );
    for c in &pin.components {
        out.push_str(&format!(
            "  - {} {} ({}), {}\n",
            c.title, c.version, c.licence, c.upstream
        ));
    }
    out.push_str(&format!(
        "The image also contains the {} launcher ({}), {}.\n",
        pin.apprun.title, pin.apprun.licence, pin.apprun.upstream
    ));
    for c in pin
        .components
        .iter()
        .filter(|c| c.licence.starts_with("LGPL"))
    {
        let sources: Vec<&PinnedSource> = pin
            .sources
            .iter()
            .filter(|s| s.name == c.name && s.version == c.version)
            .collect();
        out.push_str(&format!(
            "{} {} ({}) is statically linked into the runtime.\n- Its source, as the runtime was built from it:\n",
            c.title, c.version, c.licence
        ));
        for s in &sources {
            out.push_str(&format!("  {}\n", s.location()));
        }
        out.push_str(&format!(
            "  the GitHub Release of PhotoSquirrel {version} also carries {}.\n\
             - The runtime's build scripts, which apply that patch and link it statically:\n  \
             https://github.com/AppImage/type2-runtime/tree/{}/scripts\n",
            sources
                .iter()
                .map(|s| s.file.as_str())
                .collect::<Vec<_>>()
                .join(" and "),
            pin.runtime.commit
        ));
        // The rest of the program that links it: what relinking against a modified copy needs.
        let rest: Vec<&PinnedSource> = pin
            .sources
            .iter()
            .filter(|s| {
                pin.components
                    .iter()
                    .any(|o| o.name == s.name && o.version == s.version && o.name != c.name)
            })
            .collect();
        if !rest.is_empty() {
            out.push_str(&format!(
                "- To relink the runtime against a modified {}: the source of the code linked with it, which the\n  \
                 GitHub Release of PhotoSquirrel {version} also carries:\n",
                c.title
            ));
            for s in &rest {
                let title = pin
                    .components
                    .iter()
                    .find(|o| o.name == s.name)
                    .map_or(s.name.as_str(), |o| o.title.as_str());
                out.push_str(&format!(
                    "  {title} {}: {} ({})\n",
                    s.version,
                    s.file,
                    s.location()
                ));
            }
            out.push_str(
                "  Unpack them with the libfuse source, apply the patch, and build with the runtime's scripts\n  \
                 (scripts/common/install-dependencies.sh, then scripts/build-runtime.sh) using your libfuse.\n",
            );
        }
    }
    out.push_str("The licence texts follow the other notices at the end of this file.\n\n");
    out
}

/// The licence texts of the runtime's components and AppRun, at the end of `THIRD-PARTY-LICENSES`.
pub fn render_runtime_texts(pin: &RuntimePin) -> String {
    let rule = "=".repeat(78);
    let mut out = String::new();
    let entries = pin
        .components
        .iter()
        .map(|c| (format!("{} {}", c.title, c.version), &c.licence, &c.body))
        .chain(std::iter::once((
            pin.apprun.title.clone(),
            &pin.apprun.licence,
            &pin.apprun.body,
        )));
    for (title, licence, body) in entries {
        out.push_str(&format!(
            "\n{rule}\n{title} ({licence})\nPart of the AppImage runtime and launcher\n{rule}\n\n{}\n",
            body.trim_end()
        ));
    }
    out
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

    check_hevc_free(&shipped_tree(root)?)?;
    check_avif_assembly(opts.universal)?;
    // The programs' default features (`avif-asm`) and nothing else: no build option adds an HEVC decoder.
    let features: Vec<&str> = Vec::new();
    let (bins, arch) = if opts.universal {
        (
            universal_binaries(&target_dir, &features)?,
            "universal".to_owned(),
        )
    } else {
        build(&features)?;
        (target_dir.join(PROFILE), std::env::consts::ARCH.to_owned())
    };
    if os == Os::Linux {
        for bin in SHIPPED {
            let path = bins.join(bin);
            let out = Command::new("readelf")
                .arg("-d")
                .arg(&path)
                .output()
                .context("readelf (binutils) is needed to check the Linux programs")?;
            if !out.status.success() {
                bail!(
                    "readelf -d {}: {}",
                    path.display(),
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            let needed = needed_libs(&String::from_utf8_lossy(&out.stdout));
            check_needed(bin, &needed)?;
            eprintln!("{bin} needs: {}", needed.join(", "));
        }
    }
    if os == Os::Windows {
        for bin in SHIPPED {
            let exe = os.exe(bin);
            let imports = pe_imports(&fs::read(bins.join(&exe))?)
                .with_context(|| format!("reading the imports of {exe}"))?;
            check_windows_imports(&exe, &imports)?;
            eprintln!("{exe} imports: {}", imports.join(", "));
        }
    }
    let runtime = if os == Os::Linux {
        Some(RuntimePin::load(root)?)
    } else {
        None
    };
    let licences = licence_notices(root, os, &target_dir, &out, &version, runtime.as_ref())?;

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
    if let Some(pin) = &runtime {
        let file = appimage_tools(&target_dir, pin, &arch)?;
        packager.env("LDAI_RUNTIME_FILE", file);
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
        verify_licences(os, kind, &dest, &staging, &version, runtime.as_ref())?;
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

/// Download the pinned AppImage runtime and AppRun (ADR-0073), check their SHA-256 and pre-place AppRun in
/// cargo-packager's tool cache, where it would otherwise fetch one itself. Returns the runtime file for
/// appimagetool (`LDAI_RUNTIME_FILE`).
fn appimage_tools(target_dir: &Path, pin: &RuntimePin, arch: &str) -> Result<PathBuf> {
    if arch != "x86_64" {
        bail!("{RUNTIME_PIN} pins the x86_64 AppImage runtime only; this build is {arch}");
    }
    let dir = target_dir.join("packager-tools");
    fs::create_dir_all(&dir)?;
    let runtime = fetch_pinned(
        &dir.join("runtime-x86_64"),
        &pin.runtime.url,
        &pin.runtime.sha256,
    )?;
    let apprun = fetch_pinned(
        &dir.join("AppRun-x86_64"),
        &pin.apprun.url,
        &pin.apprun.sha256,
    )?;
    // cargo-packager 0.11.8 keeps its tools in `dirs::cache_dir()/.cargo-packager/AppImage` and downloads only
    // what is missing there.
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .context("neither XDG_CACHE_HOME nor HOME is set")?
        .join(".cargo-packager/AppImage");
    fs::create_dir_all(&cache)?;
    let placed = cache.join("AppRun-x86_64");
    fs::copy(&apprun, &placed).with_context(|| format!("placing {}", placed.display()))?;
    set_executable(&placed)?;
    eprintln!(
        "AppImage runtime: {} (type2-runtime {}), AppRun pinned in {}",
        pin.short_commit(),
        pin.runtime.release,
        cache.display()
    );
    Ok(runtime)
}

/// `path`, downloaded from `url` unless it is already there with the pinned hash; fails on any other hash.
fn fetch_pinned(path: &Path, url: &str, sha256: &str) -> Result<PathBuf> {
    let cached = fs::read(path).ok().filter(|b| sha256_hex(b) == sha256);
    if cached.is_none() {
        eprintln!("downloading {url}");
        let bytes = ureq::get(url)
            .call()
            .with_context(|| format!("downloading {url}"))?
            .into_body()
            .with_config()
            .limit(64 << 20)
            .read_to_vec()
            .with_context(|| format!("reading {url}"))?;
        let found = sha256_hex(&bytes);
        if found != sha256 {
            bail!(
                "{url} has SHA-256 {found}, but {RUNTIME_PIN} pins {sha256}: the upstream file changed; review it \
                 and update the pin and its notice texts (ADR-0073)"
            );
        }
        fs::write(path, &bytes)?;
    }
    set_executable(path)?;
    Ok(path.to_path_buf())
}

/// `cargo xtask runtime-sources --out <dir>`: every `[[source]]` of the AppImage runtime pin in `out`, each checked
/// against its SHA-256 (and a git source against its tree hash first). The release attaches them (ADR-0073,
/// ADR-0077).
pub fn runtime_sources(root: &Path, out: &Path) -> Result<()> {
    let pin = RuntimePin::parse(&fs::read_to_string(root.join(RUNTIME_PIN))?)?;
    if pin.sources.is_empty() {
        bail!("{RUNTIME_PIN} lists no [[source]]");
    }
    fs::create_dir_all(out)?;
    // Absolute: git runs in the scratch repository.
    let out = &std::env::current_dir()?.join(out);
    let work = crate::temp_dir(root)?.join(format!("runtime-sources-{}", std::process::id()));
    for s in &pin.sources {
        let path = out.join(&s.file);
        if let Some(url) = &s.url {
            eprintln!("downloading {url}");
            let bytes = ureq::get(url)
                .call()
                .with_context(|| format!("downloading {url}"))?
                .into_body()
                .with_config()
                .limit(64 << 20)
                .read_to_vec()
                .with_context(|| format!("reading {url}"))?;
            check_source_sha256(s, &bytes)?;
            fs::write(&path, &bytes)?;
        } else {
            fs::remove_dir_all(&work).ok();
            let made = git_archive(s, &work, &path);
            fs::remove_dir_all(&work).ok();
            made?;
        }
        eprintln!("{}: SHA-256 {} as pinned", s.file, s.sha256);
    }
    Ok(())
}

fn check_source_sha256(s: &PinnedSource, bytes: &[u8]) -> Result<()> {
    let found = sha256_hex(bytes);
    if found != s.sha256 {
        bail!(
            "{} has SHA-256 {found}, but {RUNTIME_PIN} pins {}: review the upstream change and update the pin",
            s.file,
            s.sha256
        );
    }
    Ok(())
}

/// A deterministic `git archive` of a git `[[source]]` into `path`, made in the scratch repository `work`: the
/// commit is fetched by hash, its tree must be the pinned tree, and the tar (uncompressed, fixed umask and line
/// endings, mtimes from the commit) must have the pinned SHA-256.
fn git_archive(s: &PinnedSource, work: &Path, path: &Path) -> Result<()> {
    let (Some(repo), Some(commit), Some(tree)) = (&s.git, &s.commit, &s.tree) else {
        bail!("{}: not a git [[source]]", s.file);
    };
    fs::create_dir_all(work)?;
    let git = |args: &[&str]| -> Result<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(work)
            .args(["-c", "core.autocrlf=false", "-c", "tar.umask=0002"])
            .args(args)
            .output()
            .context("running git")?;
        if !out.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    git(&["init", "-q"])?;
    eprintln!("fetching {repo} at {commit}");
    git(&["fetch", "-q", "--depth", "1", repo, commit])?;
    let found = git(&["rev-parse", &format!("{commit}^{{tree}}")])?;
    if &found != tree {
        bail!(
            "{repo} commit {commit} has tree {found}, but {RUNTIME_PIN} pins {tree} for {}",
            s.file
        );
    }
    let prefix = format!("--prefix={}", s.archive_prefix());
    let output = format!("--output={}", path.display());
    git(&["archive", "--format=tar", &prefix, &output, commit])?;
    let bytes = fs::read(path)?;
    if let Err(e) = check_source_sha256(s, &bytes) {
        fs::remove_file(path).ok();
        bail!(
            "{e:#} (its tree is the pinned {tree}: this git wrote the same files in a different tar layout)"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("making {} executable", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<()> {
    Ok(())
}

/// Generate `THIRD-PARTY-LICENSES` with cargo-about for each shipped binary's dependency graph (all release
/// targets, see `about.toml`), check it and return its path. The LGPL crates go to `out/lgpl-sources-<os>.txt`,
/// one `name version` per line, for the release job to attach their source (ADR-0060).
fn licence_notices(
    root: &Path,
    os: Os,
    target_dir: &Path,
    out: &Path,
    version: &str,
    runtime: Option<&RuntimePin>,
) -> Result<PathBuf> {
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
    fs::write(&path, render_notices(version, &notices, runtime))?;
    let lgpl = lgpl_crates(&notices);
    fs::write(
        out.join(lgpl_sources_file(os)),
        lgpl.iter().map(|c| format!("{c}\n")).collect::<String>(),
    )?;
    eprintln!(
        "licence notices: {} ({} licence texts)",
        path.display(),
        notices.len()
    );
    Ok(path)
}

/// Check a finished package contains `THIRD-PARTY-LICENSES`, using the platform's own tools.
fn verify_licences(
    os: Os,
    kind: Kind,
    package: &Path,
    staging: &Path,
    version: &str,
    runtime: Option<&RuntimePin>,
) -> Result<()> {
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
            let pin = runtime.context("the AppImage check needs the runtime pin")?;
            let checked = check_appimage(&root, package, version, pin);
            let text = walk(&root).map(|files| files.join("\n"));
            fs::remove_dir_all(&dir).ok();
            checked?;
            text?
        }
        Kind::Dmg => {
            let mount = staging.join("dmg-check");
            fs::create_dir_all(&mount)?;
            // The DMG carries the licence agreement (`licenseFile`), which `hdiutil attach` shows through a
            // pager and asks to accept; answer it on stdin.
            let mut attach = Command::new("hdiutil")
                .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
                .arg(&mount)
                .arg(package)
                .env("PAGER", "cat")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()
                .context("running hdiutil attach")?;
            if let Some(mut stdin) = attach.stdin.take() {
                use std::io::Write as _;
                stdin.write_all(b"Y\n").ok();
            }
            if !attach.wait()?.success() {
                bail!("hdiutil could not mount {}", package.display());
            }
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

/// An AppImage starts with the pinned runtime and carries the pinned AppRun (ADR-0073); extracted, it bundles no
/// shared library without a notice, its notices have the runtime section, its programs find every library they
/// link on this (build) machine, and its CLI runs.
fn check_appimage(root: &Path, package: &Path, version: &str, pin: &RuntimePin) -> Result<()> {
    check_runtime(pin, &fs::read(package)?)?;
    let apprun = sha256_hex(&fs::read(root.join("AppRun")).context("the AppImage has no AppRun")?);
    if apprun != pin.apprun.sha256 {
        bail!(
            "the AppImage's AppRun is not the pinned one: pinned SHA-256 {}, found {apprun}; update {RUNTIME_PIN} \
             and its notice texts (ADR-0073)",
            pin.apprun.sha256
        );
    }
    let files = walk(root)?;
    let notices = fs::read_to_string(root.join("usr/lib/photosquirrel").join(LICENCES_FILE))
        .with_context(|| format!("the AppImage has no usr/lib/photosquirrel/{LICENCES_FILE}"))?;
    if !notices.contains(&render_runtime_section(version, pin)) {
        bail!(
            "the AppImage's {LICENCES_FILE} has no AppImage runtime section for the pinned runtime"
        );
    }
    let missing = bundled_libs_without_notice(&files, &notices);
    if !missing.is_empty() {
        bail!(
            "the AppImage bundles {missing:?} without a notice in {LICENCES_FILE}: add them to \
             `appimage.excludedLibs` in crates/photosquirrel/Cargo.toml or add their notices"
        );
    }
    for bin in SHIPPED {
        let program = root.join("usr/bin").join(bin);
        let out = Command::new("ldd").arg(&program).output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let unresolved: Vec<&str> = text.lines().filter(|l| l.contains("not found")).collect();
        if !out.status.success() || !unresolved.is_empty() {
            bail!("{bin} in the AppImage has unresolved libraries: {unresolved:?}");
        }
    }
    let cli = root.join("usr/bin/photosquirrel-cli");
    let out = Command::new(&cli).arg("--version").output()?;
    check_smoke(&String::from_utf8_lossy(&out.stdout), version)?;
    eprintln!(
        "AppImage: pinned runtime {} and AppRun, no bundled libraries without notices; its CLI runs",
        pin.short_commit()
    );
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
    crate::run("package inside the Linux image", &mut docker)?;
    heif_smoke(root)
}

/// `doctor --json` of a packaged CLI: whether HEIC is available, and the first fix command when it is not.
pub fn doctor_heic(json: &str) -> Result<(bool, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).context("doctor --json output")?;
    let d = &v["data"];
    let heic = d["decoders"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"] == "heic"))
        .context("no heic entry in doctor --json")?;
    let available = heic["available"].as_bool().context("heic.available")?;
    if d["heic"].as_bool() != Some(available) {
        bail!("doctor --json: `heic` and `decoders[heic].available` disagree");
    }
    let fix = heic["fix"]["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|s| s["command"].as_str().map(str::to_owned));
    Ok((available, fix))
}

/// The Ubuntu 22.04 smoke test of the packaged CLI (task 9.3, 9.7): in a throwaway container of the packaging
/// image without libheif, `doctor` exits 0 and reports HEIC unavailable with the apt fix; after `apt install
/// libheif1` (1.12, the oldest supported), it reports HEIC available, decoding the embedded sample.
fn heif_smoke(root: &Path) -> Result<()> {
    let cli = "/work/target/docker/dist/photosquirrel-cli";
    let run = |script: &str| -> Result<String> {
        let out = Command::new("docker")
            .args(["run", "--rm", "-v"])
            .arg(format!("{}:/work:ro", root.display()))
            .args([DOCKER_IMAGE, "sh", "-c", script])
            .output()
            .context("docker run")?;
        if !out.status.success() {
            bail!(
                "smoke container failed: {}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let without = run(&format!("{cli} --data-dir /tmp/d --json doctor"))?;
    let (available, fix) = doctor_heic(&without)?;
    if available || fix.as_deref() != Some("sudo apt install libheif1") {
        bail!("without libheif, doctor should report HEIC unavailable with the apt fix: {without}");
    }
    let with = run(&format!(
        "apt-get update -qq >/dev/null && apt-get install -y -qq libheif1 >/dev/null && \
         dpkg-query -W -f='libheif ${{Version}}\\n' libheif1 >&2 && {cli} --data-dir /tmp/d --json doctor"
    ))?;
    let (available, _) = doctor_heic(&with)?;
    if !available {
        bail!("with Ubuntu 22.04's libheif 1.12, doctor should report HEIC available: {with}");
    }
    eprintln!(
        "HEIC smoke: unavailable without libheif (apt fix), available with Ubuntu 22.04's libheif"
    );
    Ok(())
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
        // The right version from the wrong program (the app, not the CLI) fails too.
        assert!(check_smoke("photosquirrel 0.1.0\n", "0.1.0").is_err());
        assert!(check_smoke("0.1.0 photosquirrel-cli\n", "0.1.0").is_err());
    }

    #[test]
    fn scenario_release_packaging_linux_build_image_pins_glibc_floor() {
        let text =
            fs::read_to_string(root().join("packaging/linux/Dockerfile")).expect("Dockerfile");
        let from: Vec<_> = text.lines().filter(|l| l.starts_with("FROM ")).collect();
        // Pinned to a digest, so a rebuilt image is the same base.
        assert_eq!(from.len(), 1, "{from:?}");
        let digest = from[0]
            .strip_prefix("FROM ubuntu:22.04@sha256:")
            .unwrap_or_else(|| panic!("not a digest-pinned ubuntu:22.04: {}", from[0]));
        assert!(digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn scenario_release_packaging_heic_fallback_noted() {
        // The template explains HEIC and AVIF per platform.
        let notes = fs::read_to_string(root().join("packaging/release-notes.md")).expect("notes");
        let section = notes
            .split("### HEIC and AVIF photos")
            .nth(1)
            .expect("a HEIC and AVIF section");
        let section = section.split("\n### ").next().unwrap_or(section);
        for needle in [
            "macOS",
            "Windows",
            "HEIF Image Extensions",
            "HEVC Video Extensions",
            "https://apps.microsoft.com/detail/9pmmsr1cgpwg",
            "Linux",
            "sudo apt install libheif1 libheif-plugin-libde265",
            "libheif-freeworld",
            "pacman",
            "Packman",
            "AVIF",
            "photosquirrel-cli doctor",
        ] {
            assert!(
                section.contains(needle),
                "{needle} missing from the HEIC section"
            );
        }
        // Packaging writes no note about a build without HEIC, and the workflow appends none.
        let source = fs::read_to_string(root().join("xtask/src/package.rs")).expect("source");
        assert!(!source.contains(&["heic-", "{}.txt"].concat()));
        let workflow = release_workflow();
        assert!(!workflow.contains("heic-"), "no HEIC notes folded in");
        assert!(workflow.contains("packages/PhotoSquirrel-*"));
    }

    /// The packaging container's check (`cargo xtask package --docker` runs `heif_smoke`: the packaged doctor in
    /// Ubuntu 22.04 with its libheif 1.12, and without any). Here: the evaluation of doctor's output it uses.
    #[test]
    fn scenario_decoder_support_oldest_supported_library_decodes() {
        let with = r#"{"schema_version":1,"data":{"heic":true,"decoders":[{"name":"heic","available":true,
            "detail":"libheif 1.12.0","backend":"libheif 1.12.0","version":"1.12.0"}]}}"#;
        assert_eq!(doctor_heic(with).expect("parse"), (true, None));
        let without = r#"{"schema_version":1,"data":{"heic":false,"decoders":[{"name":"heic","available":false,
            "detail":"x","fix":{"summary":"Install libheif1","steps":[{"text":"Install libheif",
            "command":"sudo apt install libheif1"},{"text":"Then restart"}],"restart":true}}]}}"#;
        assert_eq!(
            doctor_heic(without).expect("parse"),
            (false, Some("sudo apt install libheif1".into()))
        );
        let inconsistent =
            r#"{"data":{"heic":true,"decoders":[{"name":"heic","available":false}]}}"#;
        assert!(doctor_heic(inconsistent).is_err());
        // The packaging run calls the smoke test after building.
        let source = fs::read_to_string(root().join("xtask/src/package.rs")).expect("source");
        assert!(source.contains("    heif_smoke(root)\n"));
    }

    #[test]
    fn scenario_release_packaging_bundled_hevc_decoder_fails_packaging() {
        let graph = "photosquirrel v0.2.0 (/w/crates/photosquirrel)\nlibheif-rs v3.0.0\nlibheif-sys v3.1.0\nrav1d v1.1.0\n";
        let err = check_hevc_free(graph)
            .expect_err("an HEVC decoder")
            .to_string();
        assert!(
            err.contains("libheif-rs") && err.contains("libheif-sys"),
            "{err}"
        );
        assert!(check_hevc_free("ffmpeg-next v7.0.0\n").is_err());
        let readelf = " 0x0000000000000001 (NEEDED)             Shared library: [libheif.so.1]\n                        0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]\n";
        let needed = needed_libs(readelf);
        assert_eq!(needed, ["libheif.so.1", "libc.so.6"]);
        let err = check_needed("photosquirrel-cli", &needed)
            .expect_err("links libheif")
            .to_string();
        assert!(
            err.contains("photosquirrel-cli") && err.contains("libheif.so.1"),
            "{err}"
        );
        // The graph of the current release is free of HEVC decoders.
        check_hevc_free(&shipped_tree(&root()).expect("cargo tree")).expect("no HEVC decoder");
        // And a program of this build needs no HEIF library (this test binary links what the programs link
        // from the workspace's C code: libc and friends only).
        #[cfg(target_os = "linux")]
        if let Ok(out) = Command::new("readelf")
            .arg("-d")
            .arg(std::env::current_exe().expect("exe"))
            .output()
        {
            let needed = needed_libs(&String::from_utf8_lossy(&out.stdout));
            assert!(!needed.is_empty(), "readelf output parsed");
            check_needed("xtask tests", &needed).expect("no libheif");
        }
    }

    #[test]
    fn scenario_release_packaging_notices_name_the_av1_decoder() {
        let registry = Some("registry+https://github.com/rust-lang/crates.io-index");
        let mozjpeg: [(&str, &str, &[(&str, Option<&str>)]); 3] = [
            ("IJG", "ijg text", &[("mozjpeg-sys", registry)]),
            ("Zlib", "zlib text", &[("mozjpeg-sys", registry)]),
            ("BSD-3-Clause", "bsd3 text", &[("mozjpeg-sys", registry)]),
        ];
        let rav1d_users: [(&str, Option<&str>); 1] = [("rav1d", registry)];
        let mut with = mozjpeg.to_vec();
        with.push((
            "BSD-2-Clause",
            "Copyright (c) 2018-2024, VideoLAN and dav1d authors; rav1d authors. Redistribution and use...",
            &rav1d_users,
        ));
        let app = about_report(&with);
        let cli = about_report(&with);
        let notices = merge_notices(&[app, cli]).expect("merge");
        check_notices(&notices).expect("rav1d named");
        let text = render_notices("0.3.0", &notices, None);
        assert!(text.contains("Used by: rav1d 1.0.0"), "{text}");
        assert!(text.contains("VideoLAN and dav1d authors"));
        let err = check_notices(&merge_notices(&[about_report(&mozjpeg)]).expect("merge"))
            .expect_err("rav1d missing")
            .to_string();
        assert!(
            err.contains("rav1d") && err.contains("BSD-2-Clause"),
            "{err}"
        );
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
        // The release job checks for every package of every platform, and a checksum for each.
        let total: usize = [Os::Linux, Os::Macos, Os::Windows]
            .map(|os| kinds(os).len())
            .iter()
            .sum();
        assert!(
            text.contains(&format!(
                "\"$count\" -ne {total} ] || [ \"$sums\" -ne {total} ]"
            )),
            "the release job does not expect {total} packages"
        );
        // One release job, after every package job, publishing on tags only.
        let creates: Vec<_> = lines
            .iter()
            .filter(|l| l.starts_with("gh release create"))
            .collect();
        assert_eq!(creates.len(), 1, "{creates:?}");
        let release = text.split("\n  release:\n").nth(1).expect("release job");
        assert!(release.contains("needs: [package, runtime-sources]"));
        // Publishing needs a pushed tag: a manual run from a tag ref is still a dry run.
        assert!(
            release.contains(
                "if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"
            )
        );
        assert!(release.contains("merge-multiple: true"));
        assert_least_privilege(&text, Some("release"));
    }

    /// The workflow token is read-only except where `write_job` publishes, no checkout keeps the token in
    /// `.git/config`, and every action is pinned to a commit.
    fn assert_least_privilege(text: &str, write_job: Option<&str>) {
        let lines: Vec<&str> = text.lines().collect();
        let top = lines
            .iter()
            .position(|l| *l == "permissions:")
            .expect("top-level permissions");
        assert_eq!(lines[top + 1].trim(), "contents: read");
        let writes: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains("contents: write"))
            .map(|(i, _)| i)
            .collect();
        match write_job {
            None => assert!(writes.is_empty(), "contents: write in {writes:?}"),
            Some(job) => {
                assert_eq!(writes.len(), 1, "contents: write at {writes:?}");
                let job_line = lines[..writes[0]]
                    .iter()
                    .rposition(|l| l.len() > 2 && l.starts_with("  ") && !l.starts_with("   "))
                    .expect("job of the write permission");
                assert_eq!(lines[job_line].trim(), format!("{job}:"));
            }
        }
        let mut checkouts = 0;
        for (i, line) in lines.iter().enumerate() {
            let Some(uses) = line
                .trim()
                .strip_prefix("- uses: ")
                .or(line.trim().strip_prefix("uses: "))
            else {
                continue;
            };
            let (action, rest) = uses.split_once('@').expect("versioned action");
            let sha = rest.split_whitespace().next().unwrap_or_default();
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{action} is not pinned to a commit: {line}"
            );
            assert!(rest.contains(" # "), "{action}: no version comment");
            if action == "actions/checkout" {
                checkouts += 1;
                let with: Vec<&str> = lines[i + 1..]
                    .iter()
                    .take_while(|l| !l.trim_start().starts_with("- "))
                    .map(|l| l.trim())
                    .collect();
                assert!(
                    with.contains(&"persist-credentials: false"),
                    "checkout at line {} keeps its credentials",
                    i + 1
                );
            }
        }
        assert!(checkouts > 0);
    }

    #[test]
    fn ci_workflow_is_least_privilege() {
        let text = fs::read_to_string(root().join(".github/workflows/ci.yml")).expect("ci.yml");
        assert_least_privilege(&text, None);
    }

    const HELP: &str = "Licenses:\n  * runtime, licensed under the terms of\n    https://x/LICENSE\n  * musl libc, \
                        licensed under the terms of\n    https://x\n  * libfuse, licensed under the terms of\n    \
                        https://x/LGPL2.txt\n  * squashfuse, licensed under the terms of\n    https://x\n  * libzstd, \
                        licensed under the terms of\n    https://x\n  * zlib, licensed under the terms of\n    https://x\n";

    /// A minimal ELF64 runtime: a `.text` section holding `help`, the four sections appimagetool fills in (filled
    /// with `fill`), the section name table and the section header table at the end, followed by a squashfs magic.
    fn fake_appimage(help: &str, fill: u8) -> Vec<u8> {
        let names = b"\0.shstrtab\0.text\0.digest_md5\0.upd_info\0.sha256_sig\0.sig_key\0";
        let name_at = |n: &str| {
            names
                .windows(n.len())
                .position(|w| w == n.as_bytes())
                .expect("name") as u32
        };
        let mut elf = vec![0u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        let mut sections = vec![(0u32, 0u32, 0usize, 0usize)];
        let mut push = |elf: &mut Vec<u8>, name: &str, kind: u32, data: &[u8]| {
            let offset = elf.len();
            elf.extend_from_slice(data);
            sections.push((name_at(name), kind, offset, data.len()));
        };
        push(&mut elf, ".text", 1, help.as_bytes());
        for (name, size) in [
            (".digest_md5", 16),
            (".upd_info", 32),
            (".sha256_sig", 32),
            (".sig_key", 64),
        ] {
            push(&mut elf, name, 1, &vec![fill; size]);
        }
        push(&mut elf, ".shstrtab", 3, names);
        let shoff = elf.len();
        for (name, kind, offset, size) in &sections {
            let mut header = [0u8; 64];
            header[..4].copy_from_slice(&name.to_le_bytes());
            header[4..8].copy_from_slice(&kind.to_le_bytes());
            header[24..32].copy_from_slice(&(*offset as u64).to_le_bytes());
            header[32..40].copy_from_slice(&(*size as u64).to_le_bytes());
            elf.extend_from_slice(&header);
        }
        elf[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
        elf[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
        elf[0x3c..0x3e].copy_from_slice(&(sections.len() as u16).to_le_bytes());
        elf[0x3e..0x40].copy_from_slice(&((sections.len() - 1) as u16).to_le_bytes());
        elf.extend_from_slice(b"hsqs and the image");
        elf
    }

    fn pin_for(runtime_sha256: &str) -> RuntimePin {
        let mut pin = RuntimePin::load(&root()).expect("pin");
        pin.runtime.sha256 = runtime_sha256.to_owned();
        pin
    }

    #[test]
    fn runtime_pin_parses_and_names_every_component() {
        let pin = RuntimePin::load(&root()).expect("the runtime pin parses");
        let names: Vec<&str> = pin.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "runtime",
                "musl libc",
                "libfuse",
                "squashfuse",
                "libzstd",
                "zlib"
            ]
        );
        for c in &pin.components {
            assert!(!c.version.is_empty() && !c.licence.is_empty(), "{c:?}");
            assert!(c.body.len() > 200, "{} licence text too short", c.name);
        }
        let licence = |name: &str| {
            pin.components
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.licence.as_str())
        };
        assert_eq!(licence("libfuse"), Some("LGPL-2.1"));
        assert_eq!(licence("squashfuse"), Some("BSD-2-Clause"));
        assert_eq!(licence("libzstd"), Some("BSD-3-Clause"));
        assert_eq!(licence("zlib"), Some("Zlib"));
        assert!(pin.apprun.body.contains("MIT"));
        // The libfuse source is pinned at the version the runtime reports (3.15.0), with the patch it applies.
        let libfuse = pin
            .components
            .iter()
            .find(|c| c.name == "libfuse")
            .expect("libfuse");
        let sources: Vec<&PinnedSource> =
            pin.sources.iter().filter(|s| s.name == "libfuse").collect();
        assert!(!sources.is_empty());
        assert!(sources.iter().all(|s| s.version == libfuse.version));
        assert!(
            sources
                .iter()
                .any(|s| s.file == format!("fuse-{}.tar.xz", libfuse.version))
        );
        // A tagged release asset, never the moving `continuous` one.
        assert!(
            pin.runtime
                .url
                .contains(&format!("/download/{}/", pin.runtime.release)),
            "{}",
            pin.runtime.url
        );
        assert!(!pin.runtime.url.contains("continuous"));
        assert!(
            pin.runtime.commit.starts_with(pin.short_commit()) && pin.short_commit().len() == 7
        );
        // An LGPL component without a pinned source is rejected.
        let text = fs::read_to_string(root().join(RUNTIME_PIN)).expect("pin");
        let without_sources = text.split("\n[[source]]").next().expect("head");
        let err = RuntimePin::parse(without_sources).expect_err("no LGPL source");
        assert!(err.to_string().contains("libfuse"), "{err}");
    }

    #[test]
    fn scenario_release_packaging_changed_appimage_runtime_fails_packaging() {
        let pristine = fake_appimage(HELP, 0);
        let runtime = appimage_runtime(&pristine).expect("runtime");
        assert_eq!(&pristine[runtime.len()..runtime.len() + 4], b"hsqs");
        let pinned = normalized_runtime_sha256(runtime).expect("hash");
        // The pristine runtime file has the tool sections zeroed, so its plain hash is the pin.
        assert_eq!(pinned, sha256_hex(runtime));
        assert_eq!(runtime_components(runtime).len(), 6);
        let pin = pin_for(&pinned);
        check_runtime(&pin, &pristine).expect("the pinned runtime passes");

        // Only the update information, digest and signature differ: still the pinned runtime.
        let signed = fake_appimage(HELP, 0xab);
        assert_ne!(sha256_hex(&signed), sha256_hex(&pristine));
        check_runtime(&pin, &signed).expect("tool-filled sections are ignored");

        // A runtime with different code fails, naming both checksums and the files to update.
        let changed = fake_appimage(&HELP.replace("https://x/LICENSE", "https://y/LICENSE"), 0);
        let found =
            normalized_runtime_sha256(appimage_runtime(&changed).expect("runtime")).expect("hash");
        let err = check_runtime(&pin, &changed)
            .expect_err("changed runtime")
            .to_string();
        for needle in [pinned.as_str(), found.as_str(), RUNTIME_PIN, RUNTIME_TEXTS] {
            assert!(err.contains(needle), "{needle} missing from: {err}");
        }

        // No squashfs right after the section header table, or not an ELF file at all.
        let mut truncated = pristine.clone();
        truncated.truncate(runtime.len());
        assert!(appimage_runtime(&truncated).is_err());
        assert!(appimage_runtime(b"#!/bin/sh\nexit 0\n").is_err());

        // The pin is the runtime file the release URL serves (checked when that file is at hand).
        // `cargo xtask package --docker` keeps it under target/docker.
        let real = ["target/docker", "target"]
            .map(|dir| root().join(dir).join("packager-tools/runtime-x86_64"))
            .into_iter()
            .find_map(|path| fs::read(path).ok());
        if let Some(bytes) = real {
            let real_pin = RuntimePin::load(&root()).expect("pin");
            assert_eq!(sha256_hex(&bytes), real_pin.runtime.sha256);
            assert_eq!(
                normalized_runtime_sha256(&bytes).expect("hash"),
                real_pin.runtime.sha256
            );
            assert_eq!(bytes.len() as u64, real_pin.runtime.size);
        }
        // And a package built with it passes the check (the AppImage of the workspace version, when built).
        let version = workspace_version(&root()).expect("version");
        let appimage = artifact_name(&version, Os::Linux, "x86_64", Kind::AppImage);
        assert!(
            appimage.ends_with(".AppImage") && appimage.contains(&version),
            "{appimage}"
        );
        if let Ok(bytes) = fs::read(root().join("target/packages").join(&appimage)) {
            check_runtime(&RuntimePin::load(&root()).expect("pin"), &bytes)
                .expect("the built AppImage");
        }
    }

    #[test]
    fn scenario_release_packaging_appimage_runtime_notice_matches_the_pinned_runtime() {
        let pin = RuntimePin::load(&root()).expect("pin");
        let registry = Some("registry+https://github.com/rust-lang/crates.io-index");
        let notices = merge_notices(&[about_report(&[
            ("LGPL-2.1", "lgpl text", &[("rawler", registry)]),
            ("MIT", "mit text", &[("anyhow", registry)]),
        ])])
        .expect("merge");
        let text = render_notices("0.1.0", &notices, Some(&pin));
        let section = text.find("AppImage runtime\n").expect("runtime section");
        assert!(text.find("LGPL components").expect("LGPL") < section);
        assert!(section < text.find("Licences used:").expect("overview"));
        for needle in [
            "AppImage runtime dd6cebe",
            "AppImage type2-runtime dd6cebe (MIT)",
            "type2-runtime release 20251108",
            "musl libc 1.2.5",
            "libfuse 3.15.0 (LGPL-2.1)",
            "squashfuse 0.5.2 (BSD-2-Clause)",
            "Zstandard (libzstd) 1.5.6 (BSD-3-Clause)",
            "zlib 1.3.1 (Zlib)",
            "AppImageKit AppRun launcher (MIT)",
            "https://github.com/libfuse/libfuse/releases/download/fuse-3.15.0/fuse-3.15.0.tar.xz",
            "fuse-3.15.0.tar.xz and libfuse-3.15.0-type2-runtime-mount.c.diff",
            // What relinking against a modified libfuse needs: the rest of the program, attached too.
            "To relink the runtime against a modified libfuse",
            "AppImage type2-runtime dd6cebe: type2-runtime-dd6cebe.tar \
             (https://github.com/AppImage/type2-runtime/tree/dd6cebedcbddde9c82f89b011e8e1d40b6e43868)",
            "squashfuse 0.5.2: squashfuse-0.5.2.tar.gz \
             (https://github.com/vasi/squashfuse/releases/download/0.5.2/squashfuse-0.5.2.tar.gz)",
            "concerns the AppImage package only",
            // The licence texts themselves.
            "GNU LESSER GENERAL PUBLIC LICENSE",
            "Rich Felker",
            "Jean-loup Gailly and Mark Adler",
            "For Zstandard software",
            "Dave\nVasilevsky",
            "probonopd",
        ] {
            assert!(text.contains(needle), "{needle} missing from the notices");
        }
        // Without the pin (macOS, Windows) there is no runtime section.
        assert!(!render_notices("0.1.0", &notices, None).contains("AppImage runtime"));

        // The runtime's declared components must equal the notice's.
        let declared = runtime_components(HELP.as_bytes());
        check_runtime_components(&pin, &declared).expect("same components");
        let mut extra = declared.clone();
        extra.push("liblzma".to_owned());
        let err = check_runtime_components(&pin, &extra)
            .expect_err("an unlisted component")
            .to_string();
        assert!(
            err.contains("liblzma") && err.contains(RUNTIME_PIN),
            "{err}"
        );
        assert!(check_runtime_components(&pin, &declared[1..]).is_err());
    }

    #[test]
    fn scenario_release_packaging_release_attaches_the_appimage_runtime_lgpl_source() {
        let workflow = release_workflow();
        // A read-only job fetches and checks the pinned sources with xtask; the publishing job compiles nothing.
        let fetch = workflow
            .split("\n  runtime-sources:\n")
            .nth(1)
            .and_then(|j| j.split("\n  release:\n").next())
            .expect("runtime-sources job");
        assert!(fetch.contains("cargo xtask runtime-sources --out runtime-source"));
        assert!(fetch.contains("name: runtime-source"));
        assert!(
            !fetch.contains("permissions:"),
            "the fetch job is read-only"
        );
        let release = workflow
            .split("\n  release:\n")
            .nth(1)
            .expect("release job");
        assert!(release.contains("needs: [package, runtime-sources]"));
        assert!(
            !release.contains("cargo "),
            "the publishing job compiles nothing"
        );
        for needle in [
            "name: runtime-source\n          path: packages/runtime-source",
            "packaging/linux/appimage-runtime.toml",
            "tomllib",
            "pin[\"source\"]",
            "sha256sum -c",
            "packages/runtime-source/*",
        ] {
            assert!(release.contains(needle), "the release job lacks {needle}");
        }
        // The archives are attached to the release and kept with the dry run's notes.
        let create = release
            .lines()
            .find(|l| l.trim().starts_with("gh release create"))
            .expect("gh release create");
        assert!(create.contains("packages/runtime-source/*"), "{create}");
        let upload = release
            .split("name: release-notes")
            .nth(1)
            .expect("notes artifact");
        assert!(upload.contains("packages/runtime-source/*"));
        // The step fails without a pinned source.
        assert!(release.contains("lists no [[source]]"));
        // The pin attaches, besides libfuse, the source of the program that links it and of squashfuse, so the
        // runtime can be relinked: the runtime at its pinned commit and squashfuse at its component version.
        let pin = RuntimePin::load(&root()).expect("pin");
        let runtime = pin
            .sources
            .iter()
            .find(|s| s.name == "runtime")
            .expect("runtime source");
        assert_eq!(runtime.commit.as_deref(), Some(pin.runtime.commit.as_str()));
        assert_eq!(runtime.version, pin.short_commit());
        assert!(runtime.tree.is_some() && runtime.url.is_none());
        let squashfuse = pin
            .sources
            .iter()
            .find(|s| s.name == "squashfuse")
            .expect("squashfuse source");
        let component = pin
            .components
            .iter()
            .find(|c| c.name == "squashfuse")
            .expect("squashfuse");
        assert_eq!(squashfuse.version, component.version);
        // An upstream release file, never a generated tag archive (`/archive/`), whose bytes are not promised.
        for s in &pin.sources {
            if let Some(url) = &s.url {
                assert!(
                    url.contains("/releases/download/")
                        || url.contains("raw.githubusercontent.com"),
                    "{url}"
                );
                assert!(!url.contains("/archive/"), "{url}");
            }
        }
        // Every pinned source is reachable from the notes and README.
        let notes = fs::read_to_string(root().join("packaging/release-notes.md")).expect("notes");
        assert!(notes.contains("AppImage runtime") && notes.contains("{{runtime_sources}}"));
        assert!(notes.contains("relink") && notes.contains("squashfuse"));
        let readme = fs::read_to_string(root().join("README.md")).expect("README");
        assert!(readme.contains("AppImage runtime") && readme.contains("libfuse"));
        for s in &pin.sources {
            if s.name != "libfuse" || s.file.ends_with(".tar.xz") {
                assert!(
                    readme.contains(&format!("`{}`", s.file)),
                    "README lacks {}",
                    s.file
                );
            }
        }
    }

    /// The pin rejects a source that is neither a download nor a fully pinned git archive, or belongs to no component.
    #[test]
    fn runtime_pin_sources_are_byte_stable() {
        let text = fs::read_to_string(root().join(RUNTIME_PIN)).expect("pin");
        let mut pin = RuntimePin::parse(&text).expect("pin");
        assert!(
            pin.sources
                .iter()
                .all(|s| s.url.is_some() != s.git.is_some())
        );
        let commit = "dd6cebedcbddde9c82f89b011e8e1d40b6e43868";
        let tree = "c7cdda89406bda7e04456dd5df8801e42d811ccb";
        let git = |extra: &str| {
            format!(
                "{text}\n[[source]]\nname = \"runtime\"\nversion = \"dd6cebe\"\nfile = \"x.tar\"\n\
                 git = \"https://example.invalid/r\"\nsha256 = \"{}\"\n{extra}",
                "0".repeat(64)
            )
        };
        RuntimePin::parse(&git(&format!("commit = \"{commit}\"\ntree = \"{tree}\"\n")))
            .expect("valid");
        for (extra, needle) in [
            (format!("commit = \"{commit}\"\n"), "`commit` and `tree`"),
            (
                format!("commit = \"dd6cebe\"\ntree = \"{tree}\"\n"),
                "`commit` and `tree`",
            ),
            (
                format!("commit = \"{commit}\"\ntree = \"{tree}\"\nurl = \"https://x\"\n"),
                "either `url` or",
            ),
        ] {
            let err = RuntimePin::parse(&git(&extra))
                .expect_err(&extra)
                .to_string();
            assert!(err.contains(needle), "{extra}: {err}");
        }
        let gz = git(&format!("commit = \"{commit}\"\ntree = \"{tree}\"\n"))
            .replace("\"x.tar\"", "\"x.tar.gz\"");
        assert!(
            RuntimePin::parse(&gz)
                .expect_err("compressed")
                .to_string()
                .contains(".tar")
        );
        let stray = text.replace(
            "name = \"squashfuse\"\nversion = \"0.5.2\"\nfile",
            "name = \"squashfuse\"\nversion = \"0.5.1\"\nfile",
        );
        assert_ne!(stray, text);
        let err = RuntimePin::parse(&stray)
            .expect_err("no such component")
            .to_string();
        assert!(err.contains("not a [[component]]"), "{err}");
        pin.sources.retain(|s| s.git.is_some());
        assert_eq!(pin.sources[0].archive_prefix(), "type2-runtime-dd6cebe/");
    }

    /// A git source is archived deterministically from its pinned commit: the same bytes every time, refused when
    /// the commit's tree or the archive's hash is not the pinned one.
    #[test]
    fn git_source_archive_is_deterministic_and_checked() {
        let base = crate::temp_dir(&root())
            .expect("tmp")
            .join(format!("git-source-{}", std::process::id()));
        fs::remove_dir_all(&base).ok();
        let repo = base.join("repo");
        fs::create_dir_all(repo.join("src")).expect("repo");
        fs::write(repo.join("src/a.c"), "int main(void) { return 0; }\r\n").expect("file");
        fs::write(repo.join("README"), "x\n").expect("file");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_DATE", "2025-11-08T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2025-11-08T00:00:00Z")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "uploadpack.allowAnySHA1InWant", "true"]);
        git(&["add", "."]);
        git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c"]);
        let commit = git(&["rev-parse", "HEAD"]);
        let tree = git(&["rev-parse", "HEAD^{tree}"]);
        let mut source = PinnedSource {
            name: "runtime".into(),
            version: "x".into(),
            file: "r-x.tar".into(),
            url: None,
            git: Some(format!("file://{}", repo.display())),
            commit: Some(commit),
            tree: Some(tree.clone()),
            sha256: "0".repeat(64),
        };
        let out = base.join("out.tar");
        // The wrong hash: refused, and nothing is left behind; the error names the tree as checked.
        let err = git_archive(&source, &base.join("w1"), &out)
            .expect_err("hash")
            .to_string();
        assert!(err.contains("pins 0000") && err.contains(&tree), "{err}");
        assert!(!out.exists());
        // Learn the archive's hash from the refusal, pin it, then archive twice: byte-identical, prefixed, with the
        // file's own line endings.
        let probe = PinnedSource {
            sha256: "f".repeat(64),
            ..source.clone()
        };
        let err = git_archive(&probe, &base.join("w2"), &out)
            .expect_err("probe")
            .to_string();
        source.sha256 = err
            .split("has SHA-256 ")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .expect("the found hash")
            .to_owned();
        git_archive(&source, &base.join("w3"), &out).expect("pinned archive");
        let first = fs::read(&out).expect("tar");
        fs::remove_file(&out).expect("rm");
        git_archive(&source, &base.join("w4"), &out).expect("again");
        assert_eq!(
            fs::read(&out).expect("tar"),
            first,
            "the archive is byte-stable"
        );
        let names = String::from_utf8_lossy(&first);
        assert!(
            names.contains("r-x/src/a.c") && names.contains("\r\n"),
            "prefix and line endings kept"
        );
        // The wrong tree: refused before archiving.
        let wrong = PinnedSource {
            tree: Some("1".repeat(40)),
            ..source.clone()
        };
        let err = git_archive(&wrong, &base.join("w5"), &base.join("o2.tar"))
            .expect_err("tree")
            .to_string();
        assert!(err.contains(&format!("has tree {tree}")), "{err}");
        assert!(!base.join("o2.tar").exists());
        fs::remove_dir_all(&base).ok();
    }

    /// Run `packaging/merge-lgpl-sources.sh` over `lists` (platform, contents; `None` leaves the file out).
    #[cfg(unix)]
    fn merge_lists(lists: &[(&str, Option<&str>)]) -> (bool, String, PathBuf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("photosquirrel-lgpl-{}-{n}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).expect("temp dir");
        for (os, list) in lists {
            if let Some(list) = list {
                fs::write(dir.join(format!("lgpl-sources-{os}.txt")), list).expect("list");
            }
        }
        let out = Command::new("bash")
            .arg(root().join("packaging/merge-lgpl-sources.sh"))
            .arg(&dir)
            .args(lists.iter().map(|(os, _)| os))
            .output()
            .expect("bash");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text, dir)
    }

    #[cfg(unix)]
    #[test]
    fn scenario_release_packaging_lgpl_source_lists_merged_per_platform() {
        let list = "rawler 0.7.1\n";
        let (ok, text, dir) = merge_lists(&[
            ("linux", Some(list)),
            ("macos", Some(list)),
            ("windows", Some("rawler 0.7.1\r\n")),
        ]);
        assert!(ok, "{text}");
        assert_eq!(
            fs::read_to_string(dir.join("lgpl-sources.txt")).expect("merged"),
            "rawler 0.7.1\n"
        );
        for os in ["linux", "macos", "windows"] {
            assert!(
                dir.join(format!("lgpl-sources-{os}.txt")).is_file(),
                "{os} list lost"
            );
        }
        fs::remove_dir_all(&dir).ok();
        // The release job merges before it reads the merged list, and keeps the per-platform lists.
        let workflow = release_workflow();
        let merge = workflow
            .find("packaging/merge-lgpl-sources.sh packages linux macos windows")
            .expect("release.yml merges the lists");
        assert!(
            merge
                < workflow
                    .find("done < packages/lgpl-sources.txt")
                    .expect("download loop")
        );
        assert!(workflow.contains("packages/lgpl-sources-*.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn scenario_release_packaging_differing_lgpl_source_lists_fail_the_release() {
        let list = "rawler 0.7.1\n";
        let (ok, text, dir) = merge_lists(&[
            ("linux", Some(list)),
            ("macos", Some("other-lgpl 1.0.0\nrawler 0.7.1\n")),
            ("windows", Some(list)),
        ]);
        assert!(!ok, "a differing list passed: {text}");
        // macOS lists a component Linux and Windows lack; each platform says which side it is on, and the union is
        // still written for the error.
        assert!(
            text.contains("  + other-lgpl 1.0.0 (not listed by linux, windows)"),
            "{text}"
        );
        assert_eq!(
            text.matches("  - other-lgpl 1.0.0 (listed by macos)")
                .count(),
            2,
            "{text}"
        );
        assert_eq!(
            fs::read_to_string(dir.join("lgpl-sources.txt")).expect("merged"),
            "other-lgpl 1.0.0\nrawler 0.7.1\n"
        );
        fs::remove_dir_all(&dir).ok();

        let (ok, text, dir) = merge_lists(&[
            ("linux", Some(list)),
            ("macos", None),
            ("windows", Some(list)),
        ]);
        assert!(!ok, "a missing list passed: {text}");
        assert!(text.contains("macos") && text.contains("missing"), "{text}");
        fs::remove_dir_all(&dir).ok();
    }

    /// Only Windows lists an extra component: Windows is named as listing what the others do not, and Linux and
    /// macOS as lacking it, listed by Windows alone; a component every platform lists is never reported.
    #[cfg(unix)]
    #[test]
    fn lgpl_merge_names_the_platform_with_an_extra_component() {
        let list = "rawler 0.7.1\n";
        let (ok, text, dir) = merge_lists(&[
            ("linux", Some(list)),
            ("macos", Some(list)),
            ("windows", Some("rawler 0.7.1\r\nwin-only 2.0.0\r\n")),
        ]);
        fs::remove_dir_all(&dir).ok();
        assert!(!ok, "{text}");
        let section = |os: &str| -> String {
            let head = format!("error: the {os} LGPL source list differs");
            text.split("error: ")
                .map(|s| format!("error: {s}"))
                .find(|s| s.starts_with(&head))
                .unwrap_or_default()
        };
        let windows = section("windows");
        assert!(
            windows.contains("  + win-only 2.0.0 (not listed by linux, macos)"),
            "{text}"
        );
        assert!(!windows.contains("  - "), "{text}");
        for os in ["linux", "macos"] {
            let s = section(os);
            assert!(
                s.contains("  - win-only 2.0.0 (listed by windows)"),
                "{os}: {text}"
            );
            assert!(
                !s.contains("  + "),
                "{os} is not the platform with the extra: {text}"
            );
        }
        assert!(!text.contains("rawler 0.7.1 ("), "{text}");
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
            ("BSD-2-Clause", "bsd2 text", &[("rav1d", registry)]),
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
        let text = render_notices("0.1.0", &notices, None);
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
    fn scenario_release_packaging_lgpl_components_offer_their_source() {
        let registry = Some("registry+https://github.com/rust-lang/crates.io-index");
        let report = about_report(&[
            ("LGPL-2.1", "lgpl text", &[("rawler", registry)]),
            ("MIT", "mit text", &[("anyhow", registry)]),
        ]);
        let notices = merge_notices(&[report]).expect("merge");
        assert_eq!(lgpl_crates(&notices), ["rawler 1.0.0"]);
        // Each platform writes its own list, so the release keeps all three.
        assert_eq!(lgpl_sources_file(Os::Linux), "lgpl-sources-linux.txt");
        assert_eq!(lgpl_sources_file(Os::Macos), "lgpl-sources-macos.txt");
        assert_eq!(lgpl_sources_file(Os::Windows), "lgpl-sources-windows.txt");
        let text = render_notices("0.1.0", &notices, None);
        let section = text.find("LGPL components").expect("LGPL section");
        assert!(
            section < text.find("Licences used:").expect("overview"),
            "the LGPL section is not at the top"
        );
        for needle in [
            "rawler 1.0.0 (LGPL-2.1) is statically linked",
            "https://crates.io/api/v1/crates/rawler/1.0.0/download",
            "rawler-1.0.0.crate",
            "https://github.com/szehnder/photosquirrel2/tree/v0.1.0",
            "Cargo.lock",
            "rust-toolchain.toml",
            "[patch.crates-io]",
            "cargo build --profile dist",
        ] {
            assert!(text.contains(needle), "{needle} missing from:\n{text}");
        }
        // Without an LGPL component there is no section.
        let mit = merge_notices(&[about_report(&[("MIT", "mit", &[("anyhow", registry)])])])
            .expect("merge");
        assert!(!render_notices("0.1.0", &mit, None).contains("LGPL components"));

        // about.toml accepts LGPL-2.1 for rawler only.
        let about: toml::Value =
            toml::from_str(&fs::read_to_string(root().join("about.toml")).expect("about.toml"))
                .expect("toml");
        let global: Vec<_> = about["accepted"]
            .as_array()
            .expect("accepted")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(global.iter().all(|l| !l.contains("GPL")), "{global:?}");
        assert_eq!(
            about["rawler"]["accepted"].as_array().map(Vec::as_slice),
            Some([toml::Value::from("LGPL-2.1")].as_slice())
        );

        // The release job attaches each LGPL crate's source, checked against Cargo.lock, and fails without it.
        let workflow = release_workflow();
        for needle in [
            "packages/lgpl-sources.txt",
            "https://crates.io/api/v1/crates/$name/$ver/download",
            "sha256sum -c",
            "packages/*.crate",
        ] {
            assert!(workflow.contains(needle), "release.yml lacks {needle}");
        }
    }

    #[test]
    fn scenario_release_packaging_appimage_bundles_no_library_without_a_notice() {
        let files: Vec<String> = [
            "squashfs-root/usr/bin/photosquirrel",
            "squashfs-root/usr/lib/photosquirrel/THIRD-PARTY-LICENSES",
            "squashfs-root/usr/lib/libgcrypt.so.20",
            "squashfs-root/usr/lib/x86_64-linux-gnu/libfoo.so",
            "squashfs-root/usr/share/doc/libcap2/copyright",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(
            bundled_libs_without_notice(&files, "notices"),
            ["libfoo.so", "libgcrypt.so.20"]
        );
        assert_eq!(
            bundled_libs_without_notice(&files, "libgcrypt.so.20 ... libfoo.so"),
            Vec::<String>::new()
        );
        // The packager excludes the libraries every glibc desktop has, so none are bundled.
        let excluded = packager_metadata()["appimage"]["excludedLibs"].clone();
        let excluded: Vec<_> = excluded
            .as_array()
            .expect("appimage.excludedLibs")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        for lib in [
            "libgcrypt.so*",
            "libsystemd.so*",
            "libdbus-1.so*",
            "libxkbcommon.so*",
            "libxkbcommon-x11.so*",
            "libzstd.so*",
        ] {
            assert!(excluded.contains(&lib), "{lib} not excluded: {excluded:?}");
        }
    }

    /// A minimal PE32+ file whose import directory names `dlls`.
    fn fake_pe(dlls: &[&str]) -> Vec<u8> {
        let mut pe = vec![0u8; 0x1200];
        let put32 =
            |pe: &mut Vec<u8>, at: usize, v: u32| pe[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let put16 =
            |pe: &mut Vec<u8>, at: usize, v: u16| pe[at..at + 2].copy_from_slice(&v.to_le_bytes());
        pe[..2].copy_from_slice(b"MZ");
        put32(&mut pe, 0x3c, 0x40);
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        let coff = 0x44;
        put16(&mut pe, coff, 0x8664);
        put16(&mut pe, coff + 2, 1);
        put16(&mut pe, coff + 16, 240);
        let optional = coff + 20;
        put16(&mut pe, optional, 0x20b);
        put32(&mut pe, optional + 108, 16);
        put32(&mut pe, optional + 112 + 8, 0x1000);
        put32(&mut pe, optional + 112 + 12, ((dlls.len() + 1) * 20) as u32);
        let section = optional + 240;
        for (field, v) in [(8, 0x1000), (12, 0x1000), (16, 0x1000), (20, 0x200)] {
            put32(&mut pe, section + field, v);
        }
        let mut name_rva = 0x1100;
        for (i, dll) in dlls.iter().enumerate() {
            put32(&mut pe, 0x200 + i * 20 + 12, name_rva);
            let at = 0x200 + (name_rva as usize - 0x1000);
            pe[at..at + dll.len()].copy_from_slice(dll.as_bytes());
            name_rva += dll.len() as u32 + 1;
        }
        pe
    }

    #[test]
    fn scenario_release_packaging_windows_programs_do_not_import_the_msvc_runtime() {
        let dynamic = fake_pe(&[
            "KERNEL32.dll",
            "VCRUNTIME140.dll",
            "api-ms-win-crt-heap-l1-1-0.dll",
        ]);
        let imports = pe_imports(&dynamic).expect("imports");
        assert_eq!(
            imports,
            [
                "KERNEL32.dll",
                "VCRUNTIME140.dll",
                "api-ms-win-crt-heap-l1-1-0.dll"
            ]
        );
        let err = check_windows_imports("photosquirrel.exe", &imports)
            .expect_err("VCRUNTIME140 is not shipped")
            .to_string();
        assert!(
            err.contains("VCRUNTIME140.dll") && err.contains("crt-static"),
            "{err}"
        );
        let imports = pe_imports(&fake_pe(&["KERNEL32.dll", "MSVCP140.dll"])).expect("imports");
        assert!(check_windows_imports("photosquirrel-cli.exe", &imports).is_err());
        let imports = pe_imports(&fake_pe(&["KERNEL32.dll", "USER32.dll"])).expect("imports");
        assert!(check_windows_imports("photosquirrel.exe", &imports).is_ok());
        assert!(pe_imports(b"not a PE file").is_err());
        // The static CRT is configured for the Windows release target.
        let config = fs::read_to_string(root().join(".cargo/config.toml")).expect("config");
        let config: toml::Value = toml::from_str(&config).expect("toml");
        let flags: Vec<_> = config["target"]["x86_64-pc-windows-msvc"]["rustflags"]
            .as_array()
            .expect("rustflags")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(flags.contains(&"target-feature=+crt-static"), "{flags:?}");
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
