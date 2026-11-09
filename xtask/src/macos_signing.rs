//! `cargo xtask macos-signed`: check the signed, notarized and stapled macOS app and DMG that
//! `packaging/macos/sign.sh` wrote, and rebuild the macOS archive from the signed app (ADR-0128).
//!
//! It runs in the release workflow's `verify-macos` job, which holds no secrets. The parsers below are pure
//! functions over the text Apple's tools print, unit-tested on recorded outputs in
//! `xtask/tests/data/macos-signing/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::package::{self, Kind, Os};

/// The signing properties `codesign -dvv` reports for one signed file.
#[derive(Debug, PartialEq, Eq)]
pub struct SignatureDetails {
    pub authority: String,
    pub team: String,
    pub runtime: bool,
    pub timestamp: bool,
}

/// Parse `codesign -dvv` output (printed on standard error).
pub fn signature_details(text: &str) -> SignatureDetails {
    let value = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key))
            .map(str::trim)
            .unwrap_or_default()
            .to_owned()
    };
    SignatureDetails {
        authority: value("Authority="),
        team: value("TeamIdentifier="),
        runtime: text
            .lines()
            .any(|l| l.starts_with("CodeDirectory") && l.contains("(runtime)")),
        // A secure timestamp is `Timestamp=`; a signature without one says `Signed Time=` (or nothing, ad hoc).
        timestamp: text.lines().any(|l| l.trim().starts_with("Timestamp=")),
    }
}

/// A Developer ID signature by `team`: the authority, the team identifier, a secure timestamp and, for native
/// code, the hardened runtime.
pub fn check_signature(label: &str, text: &str, team: &str, native: bool) -> Result<()> {
    let d = signature_details(text);
    let mut problems = Vec::new();
    if !d.authority.starts_with("Developer ID Application: ") {
        problems.push(format!(
            "not signed with a Developer ID Application certificate (authority {:?})",
            d.authority
        ));
    } else if !d.authority.ends_with(&format!("({team})")) {
        problems.push(format!("signed by {:?}, not by team {team}", d.authority));
    }
    if d.team != team {
        problems.push(format!("team identifier {:?}, expected {team}", d.team));
    }
    if !d.timestamp {
        problems.push("no secure timestamp".to_owned());
    }
    if native && !d.runtime {
        problems.push("hardened runtime not enabled".to_owned());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!("{label}: {}", problems.join("; "))
    }
}

/// `codesign --verify --strict --deep -vv` passed.
pub fn check_verify(label: &str, text: &str) -> Result<()> {
    if text.contains("valid on disk") && text.contains("satisfies its Designated Requirement") {
        Ok(())
    } else {
        bail!("{label}: codesign --verify failed: {}", text.trim())
    }
}

/// `spctl --assess -vv` accepted the file as notarized Developer ID software.
pub fn check_gatekeeper(label: &str, text: &str) -> Result<()> {
    let accepted = text.lines().any(|l| l.trim_end().ends_with(": accepted"));
    let notarized = text
        .lines()
        .any(|l| l.trim() == "source=Notarized Developer ID");
    if accepted && notarized {
        Ok(())
    } else {
        bail!(
            "{label}: Gatekeeper does not accept it as notarized Developer ID software: {}",
            text.trim()
        )
    }
}

/// `xcrun stapler validate` found a stapled ticket.
pub fn check_stapled(label: &str, text: &str) -> Result<()> {
    if text.contains("The validate action worked!") {
        Ok(())
    } else {
        bail!("{label}: no stapled notarization ticket: {}", text.trim())
    }
}

/// `photosquirrel --version` printed the app name and the version.
pub fn check_app_version(output: &str, version: &str) -> Result<()> {
    let mut words = output.split_whitespace();
    if words.next() != Some("photosquirrel") || words.next() != Some(version) {
        bail!(
            "signed photosquirrel reported {:?}, expected version {version}",
            output.trim()
        );
    }
    Ok(())
}

/// The relative paths of every file and symlink in a bundle, without the signature files signing adds.
pub fn bundle_files(app: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    walk_bundle(app, app, &mut out)?;
    out.retain(|p| !p.starts_with("Contents/_CodeSignature/") && p != "Contents/CodeResources");
    out.sort();
    Ok(out)
}

fn walk_bundle(dir: &Path, base: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        let rel = path
            .strip_prefix(base)
            .expect("under base")
            .to_string_lossy()
            .replace('\\', "/");
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            walk_bundle(&path, base, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// Rebuild the macOS `.tar.gz` from the signed app's programs and notices, and write its checksum. Returns the
/// archive's file name.
pub fn rebuild_archive(
    root: &Path,
    app: &Path,
    out: &Path,
    staging: &Path,
    version: &str,
) -> Result<String> {
    let name = package::artifact_name(version, Os::Macos, "universal", Kind::TarGz);
    let bins = app.join("Contents/MacOS");
    let licences = app.join("Contents/Resources").join(package::LICENCES_FILE);
    package::archive(
        root,
        Os::Macos,
        &bins,
        &licences,
        &name,
        staging,
        &out.join(&name),
        version,
    )?;
    write_checksum(out, &name)?;
    Ok(name)
}

fn write_checksum(dir: &Path, name: &str) -> Result<()> {
    let bytes = fs::read(dir.join(name))?;
    fs::write(
        dir.join(format!("{name}.sha256")),
        package::checksum_line(&bytes, name),
    )?;
    Ok(())
}

/// Run a tool and return standard output and standard error together (Apple's tools print on either).
/// The output is also printed as a collapsed group in the GitHub Actions log, so the recordings in
/// `xtask/tests/data/macos-signing/` can be refreshed from a real run.
fn output(cmd: &mut Command) -> Result<(bool, String)> {
    let out = cmd.output().with_context(|| format!("running {cmd:?}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let program = cmd.get_program().to_string_lossy().into_owned();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    eprintln!("::group::{program} {}", args.join(" "));
    eprintln!("{}", text.trim_end());
    eprintln!("::endgroup::");
    Ok((out.status.success(), text))
}

/// Run one of Apple's tools that must succeed: a non-zero exit fails the check whatever it printed.
fn checked(label: &str, cmd: &mut Command) -> Result<String> {
    let (ok, text) = output(cmd)?;
    if !ok {
        bail!("{label}: {:?} failed: {}", cmd.get_program(), text.trim());
    }
    Ok(text)
}

/// Whether two bundles hold the same files with the same bytes (symlinks by their targets).
pub fn same_bundle(a: &Path, b: &Path) -> Result<Vec<String>> {
    let (fa, fb) = (bundle_files_all(a)?, bundle_files_all(b)?);
    let mut differ: Vec<String> = fa
        .iter()
        .filter(|p| !fb.contains(p))
        .chain(fb.iter().filter(|p| !fa.contains(p)))
        .cloned()
        .collect();
    for rel in fa.iter().filter(|p| fb.contains(p)) {
        let (pa, pb) = (a.join(rel), b.join(rel));
        let same = match (fs::read_link(&pa), fs::read_link(&pb)) {
            (Ok(ta), Ok(tb)) => ta == tb,
            (Err(_), Err(_)) => fs::read(&pa)? == fs::read(&pb)?,
            _ => false,
        };
        if !same {
            differ.push(rel.clone());
        }
    }
    differ.sort();
    Ok(differ)
}

/// Every file of a bundle, the signature files included.
fn bundle_files_all(app: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    walk_bundle(app, app, &mut out)?;
    out.sort();
    Ok(out)
}

/// Unmounts a disk image when dropped, also after a failed check.
struct Mounted(PathBuf);

impl Drop for Mounted {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .arg("detach")
            .arg(&self.0)
            .arg("-force")
            .output();
    }
}

fn unzip(zip: &Path, dest: &Path) -> Result<PathBuf> {
    fs::remove_dir_all(dest).ok();
    fs::create_dir_all(dest)?;
    crate::run(
        &format!("unpack {}", zip.display()),
        Command::new("ditto").arg("-x").arg("-k").arg(zip).arg(dest),
    )?;
    let app = dest.join("PhotoSquirrel.app");
    if !app.is_dir() {
        bail!("{} does not contain PhotoSquirrel.app", zip.display());
    }
    Ok(app)
}

fn is_macho(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    bytes.len() >= 4
        && [
            0xfeedface_u32,
            0xfeedfacf,
            0xcafebabe,
            0xcafebabf,
            0xcefaedfe,
            0xcffaedfe,
        ]
        .contains(&u32::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
        ]))
}

/// The `macos-signed` command.
pub fn run(root: &Path, signed: &Path, unsigned_app: &Path, team: &str, out: &Path) -> Result<()> {
    if team.trim().is_empty() {
        bail!("--team-id is empty: set the repository variable APPLE_TEAM_ID");
    }
    if !cfg!(target_os = "macos") {
        bail!("macos-signed runs on macOS (codesign, spctl, stapler)");
    }
    let version = package::workspace_version(root)?;
    let work = crate::temp_dir(root)?.join(format!("macos-signed-{}", std::process::id()));
    fs::remove_dir_all(&work).ok();
    let app = unzip(&signed.join("PhotoSquirrel.app.zip"), &work.join("signed"))?;
    let unsigned = unzip(unsigned_app, &work.join("unsigned"))?;
    let dmg_name = package::artifact_name(&version, Os::Macos, "universal", Kind::Dmg);
    let dmg = signed.join(&dmg_name);
    if !dmg.is_file() {
        bail!("{} is missing", dmg.display());
    }

    // The signed bundle holds exactly the unsigned bundle's files, and the same Info.plist.
    let (a, b) = (bundle_files(&app)?, bundle_files(&unsigned)?);
    if a != b {
        let extra: Vec<_> = a.iter().filter(|p| !b.contains(p)).collect();
        let missing: Vec<_> = b.iter().filter(|p| !a.contains(p)).collect();
        bail!(
            "the signed app's files differ from the unsigned app's: extra {extra:?}, missing {missing:?}"
        );
    }
    if fs::read(app.join("Contents/Info.plist"))? != fs::read(unsigned.join("Contents/Info.plist"))?
    {
        bail!("the signed app's Info.plist differs from the unsigned app's");
    }

    // Every Mach-O file and the bundle: Developer ID by the team, hardened runtime, secure timestamp.
    let mut machos = 0;
    for rel in &a {
        let path = app.join(rel);
        if is_macho(&path) {
            let text = checked(rel, Command::new("codesign").arg("-dvv").arg(&path))?;
            check_signature(rel, &text, team, true)?;
            machos += 1;
        }
    }
    if machos == 0 {
        bail!("no Mach-O file in the signed app");
    }
    let text = checked(
        "PhotoSquirrel.app",
        Command::new("codesign").arg("-dvv").arg(&app),
    )?;
    check_signature("PhotoSquirrel.app", &text, team, true)?;
    let text = checked(
        "PhotoSquirrel.app",
        Command::new("codesign")
            .args(["--verify", "--strict", "--deep", "-vv"])
            .arg(&app),
    )?;
    check_verify("PhotoSquirrel.app", &text)?;
    let text = checked(&dmg_name, Command::new("codesign").arg("-dvv").arg(&dmg))?;
    check_signature(&dmg_name, &text, team, false)?;

    // Gatekeeper and the stapled tickets.
    let text = checked(
        "PhotoSquirrel.app",
        Command::new("spctl")
            .args(["--assess", "--type", "execute", "-vv"])
            .arg(&app),
    )?;
    check_gatekeeper("PhotoSquirrel.app", &text)?;
    let text = checked(
        &dmg_name,
        Command::new("spctl")
            .args([
                "--assess",
                "--type",
                "open",
                "--context",
                "context:primary-signature",
                "-vv",
            ])
            .arg(&dmg),
    )?;
    check_gatekeeper(&dmg_name, &text)?;
    for (label, path) in [("PhotoSquirrel.app", &app), (dmg_name.as_str(), &dmg)] {
        let text = checked(
            label,
            Command::new("xcrun")
                .args(["stapler", "validate"])
                .arg(path),
        )?;
        check_stapled(label, &text)?;
    }

    // The app users get is the one inside the DMG: it must be the verified app, byte for byte.
    let mount = work.join("dmg");
    fs::create_dir_all(&mount)?;
    checked(
        &dmg_name,
        Command::new("hdiutil")
            .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
            .arg(&mount)
            .arg(&dmg),
    )?;
    let mounted = Mounted(mount.clone());
    let in_dmg = mount.join("PhotoSquirrel.app");
    if !in_dmg.is_dir() {
        bail!("{dmg_name} holds no PhotoSquirrel.app");
    }
    if !fs::symlink_metadata(mount.join("Applications")).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("{dmg_name} has no Applications link");
    }
    let differ = same_bundle(&in_dmg, &app)?;
    if !differ.is_empty() {
        bail!("the app in {dmg_name} differs from the verified app: {differ:?}");
    }
    drop(mounted);

    // The signed programs start under the hardened runtime (dyld loads every linked framework before main).
    let (ok, text) =
        output(Command::new(app.join("Contents/MacOS/photosquirrel")).arg("--version"))?;
    if !ok {
        bail!("signed photosquirrel --version failed: {text}");
    }
    check_app_version(&text, &version)?;
    let (ok, text) =
        output(Command::new(app.join("Contents/MacOS/photosquirrel-cli")).arg("--version"))?;
    if !ok {
        bail!("signed photosquirrel-cli --version failed: {text}");
    }
    package::check_smoke(&text, &version)?;

    // The packages: the signed DMG, and the archive rebuilt from the signed app.
    fs::create_dir_all(out)?;
    fs::copy(&dmg, out.join(&dmg_name))?;
    write_checksum(out, &dmg_name)?;
    let archive = rebuild_archive(root, &app, out, &work.join("staging"), &version)?;
    eprintln!(
        "macOS packages signed by team {team}, notarized and stapled: {dmg_name}, {archive} ({machos} Mach-O files)"
    );
    fs::remove_dir_all(&work).ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: &str = "MD988WZMG9";

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf()
    }

    fn recorded(name: &str) -> String {
        let path = root().join("xtask/tests/data/macos-signing").join(name);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// One job of a workflow: from its `  <name>:` line up to the next job's.
    fn job<'a>(workflow: &'a str, name: &str) -> &'a str {
        let header = format!("\n  {name}:\n");
        let start = workflow
            .find(&header)
            .unwrap_or_else(|| panic!("no {name} job"))
            + 1;
        let rest = &workflow[start..];
        let mut offset = 0;
        for (i, line) in rest.split_inclusive('\n').enumerate() {
            let is_job = line.starts_with("  ")
                && !line.starts_with("   ")
                && line.trim_end().ends_with(':')
                && !line.trim_start().starts_with('#');
            if i > 0 && is_job {
                return &rest[..offset];
            }
            offset += line.len();
        }
        rest
    }

    fn release_workflow() -> String {
        fs::read_to_string(root().join(".github/workflows/release.yml")).expect("release.yml")
    }

    #[test]
    fn scenario_release_packaging_signed_app_accepted_by_gatekeeper() {
        check_signature("app", &recorded("codesign-dvv-app.txt"), TEAM, true)
            .expect("app signature");
        check_signature("dmg", &recorded("codesign-dvv-dmg.txt"), TEAM, false)
            .expect("dmg signature");
        check_verify("app", &recorded("codesign-verify-app.txt")).expect("verify");
        check_gatekeeper("app", &recorded("spctl-app.txt")).expect("gatekeeper app");
        check_gatekeeper("dmg", &recorded("spctl-dmg.txt")).expect("gatekeeper dmg");
        check_stapled("app", &recorded("stapler-app.txt")).expect("stapled app");
        check_stapled("dmg", &recorded("stapler-dmg.txt")).expect("stapled dmg");
        let d = signature_details(&recorded("codesign-dvv-app.txt"));
        assert_eq!(d.team, TEAM);
        assert!(d.authority.starts_with("Developer ID Application: ") && d.runtime && d.timestamp);
    }

    #[test]
    fn scenario_release_packaging_unsigned_or_wrongly_signed_app_fails_the_release() {
        let adhoc = check_signature(
            "PhotoSquirrel.app",
            &recorded("codesign-dvv-adhoc.txt"),
            TEAM,
            true,
        )
        .expect_err("ad hoc");
        let msg = adhoc.to_string();
        assert!(
            msg.contains("PhotoSquirrel.app") && msg.contains("Developer ID"),
            "{msg}"
        );

        let good = recorded("codesign-dvv-app.txt");
        let no_runtime = good.replace("flags=0x10000(runtime)", "flags=0x0(none)");
        let msg = check_signature("app", &no_runtime, TEAM, true)
            .expect_err("runtime")
            .to_string();
        assert!(msg.contains("hardened runtime"), "{msg}");

        let other_team = good.replace(TEAM, "ZZZZZZZZZZ");
        let msg = check_signature("app", &other_team, TEAM, true)
            .expect_err("team")
            .to_string();
        assert!(msg.contains("expected MD988WZMG9"), "{msg}");

        let no_timestamp = good
            .lines()
            .filter(|l| !l.starts_with("Timestamp="))
            .collect::<Vec<_>>()
            .join("\n");
        let msg = check_signature("app", &no_timestamp, TEAM, true)
            .expect_err("timestamp")
            .to_string();
        assert!(msg.contains("timestamp"), "{msg}");

        let msg = check_gatekeeper("PhotoSquirrel.app", &recorded("spctl-rejected.txt"))
            .expect_err("rejected")
            .to_string();
        assert!(
            msg.contains("PhotoSquirrel.app") && msg.contains("rejected"),
            "{msg}"
        );

        let msg = check_stapled(
            "PhotoSquirrel-0.4.0-macos-universal.dmg",
            &recorded("stapler-missing.txt"),
        )
        .expect_err("not stapled")
        .to_string();
        assert!(msg.contains(".dmg") && msg.contains("stapled"), "{msg}");

        assert!(
            check_verify("app", "PhotoSquirrel.app: code object is not signed at all").is_err()
        );
        assert!(check_app_version("photosquirrel 0.3.0", "0.3.1").is_err());
        check_app_version("photosquirrel 0.3.1\n", "0.3.1").expect("version");
    }

    #[cfg(unix)]
    #[test]
    fn scenario_release_packaging_archive_binaries_come_from_the_signed_app() {
        use std::os::unix::fs::PermissionsExt;
        let version = package::workspace_version(&root()).expect("version");
        let base = crate::temp_dir(&root())
            .expect("tmp")
            .join(format!("macos-archive-{}", std::process::id()));
        fs::remove_dir_all(&base).ok();
        let app = base.join("PhotoSquirrel.app");
        let macos = app.join("Contents/MacOS");
        fs::create_dir_all(&macos).expect("app");
        fs::create_dir_all(app.join("Contents/Resources")).expect("resources");
        fs::write(
            app.join("Contents/Resources").join(package::LICENCES_FILE),
            "notices\n",
        )
        .expect("notices");
        // Stand-ins for the signed programs: each prints its version, with a marker standing in for a signature.
        for (bin, name) in [
            ("photosquirrel", "photosquirrel"),
            ("photosquirrel-cli", "photosquirrel-cli"),
        ] {
            let path = macos.join(bin);
            fs::write(
                &path,
                format!("#!/bin/sh\n# signed\necho '{name} {version}'\n"),
            )
            .expect("program");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let out = base.join("out");
        fs::create_dir_all(&out).expect("out");
        let name =
            rebuild_archive(&root(), &app, &out, &base.join("staging"), &version).expect("archive");
        assert_eq!(
            name,
            format!("PhotoSquirrel-{version}-macos-universal.tar.gz")
        );

        let unpacked = base.join("unpacked");
        fs::create_dir_all(&unpacked).expect("unpacked");
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(out.join(&name))
            .arg("-C")
            .arg(&unpacked)
            .status()
            .expect("tar");
        assert!(status.success());
        let folder = unpacked.join(name.trim_end_matches(".tar.gz"));
        for bin in ["photosquirrel", "photosquirrel-cli"] {
            assert_eq!(
                fs::read(folder.join(bin)).expect("archived"),
                fs::read(macos.join(bin)).expect("signed"),
                "{bin} is not the signed program"
            );
        }
        let sum = fs::read_to_string(out.join(format!("{name}.sha256"))).expect("checksum");
        assert_eq!(
            sum,
            package::checksum_line(&fs::read(out.join(&name)).expect("archive"), &name)
        );
        fs::remove_dir_all(&base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn same_bundle_compares_bytes_and_links() {
        let base = crate::temp_dir(&root())
            .expect("tmp")
            .join(format!("macos-same-{}", std::process::id()));
        fs::remove_dir_all(&base).ok();
        let make = |name: &str, body: &str| {
            let app = base.join(name);
            fs::create_dir_all(app.join("Contents/MacOS")).expect("dirs");
            fs::write(app.join("Contents/MacOS/photosquirrel"), body).expect("file");
            std::os::unix::fs::symlink("MacOS/photosquirrel", app.join("Contents/link"))
                .expect("link");
            app
        };
        let (a, b, c) = (make("a", "one"), make("b", "one"), make("c", "two"));
        assert!(same_bundle(&a, &b).expect("same").is_empty());
        assert_eq!(
            same_bundle(&a, &c).expect("differ"),
            ["Contents/MacOS/photosquirrel"]
        );
        fs::write(b.join("Contents/extra"), "x").expect("extra");
        assert_eq!(same_bundle(&a, &b).expect("extra"), ["Contents/extra"]);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn bundle_files_ignore_signature_files() {
        let base = crate::temp_dir(&root())
            .expect("tmp")
            .join(format!("macos-bundle-{}", std::process::id()));
        fs::remove_dir_all(&base).ok();
        fs::create_dir_all(base.join("Contents/MacOS")).expect("dirs");
        fs::create_dir_all(base.join("Contents/_CodeSignature")).expect("dirs");
        fs::write(base.join("Contents/MacOS/photosquirrel"), "x").expect("file");
        fs::write(base.join("Contents/Info.plist"), "x").expect("file");
        fs::write(base.join("Contents/_CodeSignature/CodeResources"), "x").expect("file");
        assert_eq!(
            bundle_files(&base).expect("files"),
            ["Contents/Info.plist", "Contents/MacOS/photosquirrel"]
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scenario_release_packaging_signing_job_isolated_from_the_build() {
        let text = release_workflow();
        let sign = job(&text, "sign-macos");
        assert!(sign.contains("environment: macos_signing"), "{sign}");
        assert!(sign.contains("contents: read"), "{sign}");
        assert!(!sign.contains("contents: write"));
        assert!(
            !sign.contains("cargo"),
            "the signing job runs cargo:\n{sign}"
        );
        assert!(!sign.contains("rust-toolchain") && !sign.contains("rust-cache"));
        assert!(sign.contains("bash packaging/macos/sign.sh"));
        // Only the signing job references the signing secrets.
        let secrets = [
            "MACOS_CERTIFICATE_P12_BASE64",
            "MACOS_CERTIFICATE_PASSWORD",
            "MACOS_SIGNING_IDENTITY",
            "APPLE_API_KEY_ID",
            "APPLE_API_ISSUER_ID",
            "APPLE_API_KEY_P8",
        ];
        for s in secrets {
            let reference = format!("secrets.{s}");
            assert_eq!(
                text.matches(&reference).count(),
                1,
                "{s} referenced outside sign-macos"
            );
            assert!(sign.contains(&reference), "{s} not given to sign-macos");
        }
        // The script cleans up on every outcome, never traces commands and builds nothing.
        let script = fs::read_to_string(root().join("packaging/macos/sign.sh")).expect("sign.sh");
        assert!(script.contains("trap cleanup EXIT"));
        let cleanup = script
            .split("cleanup() {")
            .nth(1)
            .expect("cleanup")
            .split("\n}")
            .next()
            .expect("body");
        assert!(cleanup.contains("security delete-keychain"));
        assert!(cleanup.contains("rm -rf \"$work\""));
        assert!(
            script.contains("work=$(mktemp -d)")
                && script.contains("\"$work/cert.p12\"")
                && script.contains("\"$work/AuthKey.p8\"")
        );
        assert!(
            !script
                .lines()
                .any(|l| l.trim_start().starts_with("set -x") || l.contains("set -o xtrace"))
        );
        assert!(
            !script
                .lines()
                .any(|l| !l.trim_start().starts_with('#') && l.contains("cargo"))
        );
        assert!(script.contains("--options runtime") && script.contains("--timestamp"));
        // Signing never uses --deep (only the verification does).
        assert!(
            !script.lines().any(|l| {
                let l = l.trim_start();
                (l.starts_with("sign ") || l.starts_with("codesign --force"))
                    && l.contains("--deep")
            }),
            "signing with --deep"
        );
        assert!(script.contains("notarytool submit") && script.contains("stapler staple"));
    }

    #[test]
    fn scenario_release_packaging_release_requires_the_signed_macos_packages() {
        let text = release_workflow();
        let release = job(&text, "release");
        assert!(
            release.contains("needs: [package, runtime-sources, verify-macos]"),
            "{release}"
        );
        assert!(release.contains("name: signed-macos-packages"));
        // The step guard: a tag, or a dry run that asked to sign, fails without the signed packages.
        assert!(release.contains("SIGN_MACOS: ${{ inputs.sign_macos }}"));
        assert!(
            release.contains(
                "if [ \"$GITHUB_EVENT_NAME\" = push ] || [ \"$SIGN_MACOS\" = true ]; then"
            )
        );
        // The job condition: verify-macos may be skipped only on a dry run that did not ask to sign.
        let release_if = release
            .split("    if: >-\n")
            .nth(1)
            .and_then(|r| r.split("    runs-on:").next())
            .expect("release if");
        assert!(
            release_if.contains("needs.verify-macos.result == 'success'"),
            "{release_if}"
        );
        assert!(
            release_if.contains(
                "(needs.verify-macos.result == 'skipped' && github.event_name != 'push' && !inputs.sign_macos)"
            ),
            "{release_if}"
        );
        assert!(release.contains("::error::a release needs the signed macOS packages"));
        assert!(release.contains("rm packages/PhotoSquirrel-*-macos-*"));
        let sign = job(&text, "sign-macos");
        assert!(
            sign.contains("github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')")
        );
        let verify = job(&text, "verify-macos");
        assert!(verify.contains("needs: [package, sign-macos]"));
        assert!(verify.contains("cargo xtask macos-signed"));
        assert!(verify.contains("--team-id \"${{ vars.APPLE_TEAM_ID }}\""));
        assert!(
            !verify.contains("secrets."),
            "verify-macos must hold no secrets"
        );
        assert!(!verify.contains("environment:"));
        // verify-macos runs whenever sign-macos succeeded: it has no condition of its own.
        assert!(
            !verify.lines().any(|l| l.trim_start().starts_with("if:")),
            "verify-macos must not have its own condition"
        );
    }

    #[test]
    fn scenario_release_packaging_dry_run_without_signing_says_so() {
        let text = release_workflow();
        assert!(text.contains("      sign_macos:\n"), "no sign_macos input");
        let input = text.split("      sign_macos:\n").nth(1).expect("input");
        assert!(input.contains("default: false") && input.contains("type: boolean"));
        let sign = job(&text, "sign-macos");
        assert!(sign.contains("(github.event_name == 'workflow_dispatch' && inputs.sign_macos)"));
        let release = job(&text, "release");
        assert!(release.contains("macOS packages are NOT signed"));
        assert!(release.contains("GITHUB_STEP_SUMMARY"));
    }

    #[test]
    fn scenario_release_packaging_no_quarantine_workaround_for_macos() {
        for file in ["README.md", "packaging/release-notes.md"] {
            let text = fs::read_to_string(root().join(file)).expect(file);
            assert!(
                !text.contains("xattr"),
                "{file} still has the xattr workaround"
            );
            assert!(
                !text.contains("is damaged"),
                "{file} still says the app is damaged"
            );
            let first = text
                .split("### First launch")
                .nth(1)
                .unwrap_or_else(|| panic!("{file}: no First launch"));
            let first = first.split("\n#").next().expect("section");
            assert!(
                first.contains("notarized"),
                "{file}: macOS first launch does not name the notarized app"
            );
            assert!(
                first.contains("SmartScreen"),
                "{file}: the Windows note is gone"
            );
        }
    }
}
