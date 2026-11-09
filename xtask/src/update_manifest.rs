//! `cargo xtask update-manifest`: write and sign a release's `update-manifest.json`, or check one (ADR-0125, spec
//! `release-packaging` "Signed update manifest").
//!
//! The manifest lists the eight packages of a release (name, size, SHA-256, download URL under the tag), the
//! version, the release time, the notes ("What's new in <version>" from `packaging/release-notes.md`) and the
//! release page. It is signed with the update key from the environment of the `sign` job, or, for a dry run, with
//! a throwaway key whose public half is written out for the check.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::package::{self, Kind, Os};
use ps_update::build::{DOWNLOAD_PREFIX, RELEASE_PAGE_PREFIX};
use ps_update::manifest::{Envelope, FORMAT, MAX_NOTES_CHARS, Manifest, Package};

pub const FILE: &str = "update-manifest.json";

/// `(platform, kind, artifact name)` of every package of `version`.
pub fn packages(version: &str) -> Vec<(&'static str, &'static str, String)> {
    let a = |os, arch, kind| package::artifact_name(version, os, arch, kind);
    vec![
        (
            "linux-x86_64",
            "appimage",
            a(Os::Linux, "x86_64", Kind::AppImage),
        ),
        ("linux-x86_64", "deb", a(Os::Linux, "x86_64", Kind::Deb)),
        (
            "linux-x86_64",
            "tar.gz",
            a(Os::Linux, "x86_64", Kind::TarGz),
        ),
        (
            "macos-universal",
            "dmg",
            a(Os::Macos, "universal", Kind::Dmg),
        ),
        (
            "macos-universal",
            "tar.gz",
            a(Os::Macos, "universal", Kind::TarGz),
        ),
        (
            "macos-universal",
            "app.tar.gz",
            package::app_archive_name(version),
        ),
        (
            "windows-x86_64",
            "setup",
            a(Os::Windows, "x86_64", Kind::Setup),
        ),
        ("windows-x86_64", "zip", a(Os::Windows, "x86_64", Kind::Zip)),
    ]
}

/// The "What's new in <version>" section of the release notes, without its heading.
pub fn notes(release_notes: &str, version: &str) -> Result<String> {
    let heading = format!("### What's new in {version}");
    let start = release_notes
        .lines()
        .position(|l| l.trim() == heading)
        .with_context(|| format!("the release notes have no \"{heading}\" section"))?;
    let body: Vec<&str> = release_notes
        .lines()
        .skip(start + 1)
        .take_while(|l| !l.starts_with("## ") && !l.starts_with("### "))
        .collect();
    let text = body.join("\n").trim().to_owned();
    if text.is_empty() {
        bail!("the \"{heading}\" section is empty");
    }
    Ok(fit(&text))
}

/// The notes as they are when they fit in [`MAX_NOTES_CHARS`]; otherwise cut before the first top-level bullet
/// that does not fit, with a pointer to the release page (which has them whole).
pub fn fit(text: &str) -> String {
    const MORE: &str = "\n\n…and more: see the release page.";
    if text.chars().count() <= MAX_NOTES_CHARS {
        return text.to_owned();
    }
    let budget = MAX_NOTES_CHARS - MORE.chars().count();
    let mut kept = String::new();
    let mut item = String::new();
    for line in text.lines() {
        if line.starts_with("- ") && !item.is_empty() {
            if kept.chars().count() + item.chars().count() > budget {
                item.clear();
                break;
            }
            kept.push_str(&item);
            item.clear();
        }
        item.push_str(line);
        item.push('\n');
    }
    if kept.chars().count() + item.chars().count() <= budget {
        kept.push_str(&item);
    }
    if kept.trim().is_empty() {
        // One bullet longer than the budget: cut it at a character boundary.
        kept = text.chars().take(budget).collect();
    }
    format!("{}{MORE}", kept.trim_end())
}

fn sha256_hex(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The manifest of the packages in `dir`; every one of them must be there.
pub fn manifest(dir: &Path, version: &str, notes: String, released: String) -> Result<Manifest> {
    let mut table: BTreeMap<String, BTreeMap<String, Package>> = BTreeMap::new();
    let mut missing = Vec::new();
    for (platform, kind, name) in packages(version) {
        let path = dir.join(&name);
        if !path.is_file() {
            missing.push(name);
            continue;
        }
        table.entry(platform.to_owned()).or_default().insert(
            kind.to_owned(),
            Package {
                url: format!("{DOWNLOAD_PREFIX}v{version}/{name}"),
                size: fs::metadata(&path)?.len(),
                sha256: sha256_hex(&path)?,
                name,
            },
        );
    }
    if !missing.is_empty() {
        bail!(
            "packages missing from {}: {}",
            dir.display(),
            missing.join(", ")
        );
    }
    Ok(Manifest {
        product: "PhotoSquirrel".into(),
        version: version.to_owned(),
        released,
        urgent: false,
        notes,
        notes_url: format!("{RELEASE_PAGE_PREFIX}v{version}"),
        packages: table,
    })
}

/// The envelope of `manifest`, signed with `sk`.
pub fn sign(manifest: &Manifest, sk: &minisign::SecretKey) -> Result<String> {
    let text = serde_json::to_string_pretty(manifest)?;
    let comment = Manifest::trusted_comment(&manifest.version);
    let signature = minisign::sign(
        None,
        sk,
        text.as_bytes(),
        Some(&comment),
        Some("PhotoSquirrel update manifest"),
    )
    .context("signing the update manifest")?
    .into_string();
    Ok(serde_json::to_string_pretty(&Envelope {
        format: FORMAT,
        manifest: text,
        signature,
    })?)
}

/// Check an envelope against public key lines and the packages beside it.
pub fn check(
    dir: &Path,
    version: &str,
    envelope: &[u8],
    public_keys: &[String],
) -> Result<Manifest> {
    let keys = public_keys
        .iter()
        .map(|k| {
            minisign_verify::PublicKey::from_base64(k)
                .map_err(|e| anyhow::anyhow!("public key {k}: {e}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let m = ps_update::manifest::verify(envelope, &keys).context("the update manifest")?;
    if m.version != version {
        bail!("the manifest is for {}, not {version}", m.version);
    }
    for (platform, kind, name) in packages(version) {
        let p = m
            .package(platform, kind)
            .with_context(|| format!("the manifest lists no {platform} {kind}"))?;
        if p.name != name {
            bail!("{platform} {kind} is {}, expected {name}", p.name);
        }
        let path = dir.join(&name);
        let (size, sum) = (fs::metadata(&path)?.len(), sha256_hex(&path)?);
        if p.size != size || p.sha256 != sum {
            bail!("{name} does not match the manifest (size or SHA-256 differs)");
        }
    }
    Ok(m)
}

/// The secret key named by two environment variables (its minisign file text and its password).
pub fn secret_key_from_env(key_env: &str, password_env: &str) -> Result<minisign::SecretKey> {
    let text = std::env::var(key_env).with_context(|| format!("{key_env} is not set"))?;
    let password = std::env::var(password_env).ok().filter(|p| !p.is_empty());
    minisign::SecretKeyBox::from_string(&text)
        .and_then(|b| b.into_secret_key(password))
        .map_err(|e| anyhow::anyhow!("the update signing key in {key_env}: {e}"))
}

/// Now, as RFC 3339 UTC (`2026-11-09T12:00:00Z`).
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// How the manifest is signed.
pub enum Signer {
    /// The release key, from these environment variables (the `sign` job).
    Env { key: String, password: String },
    /// A throwaway key (dry runs); its public line is written to this file.
    Throwaway(PathBuf),
}

/// The `update-manifest` command: write and sign, or with `check_keys` check.
pub fn run(
    root: &Path,
    packages_dir: &Path,
    tag: &str,
    signer: Option<Signer>,
    check_keys: &[PathBuf],
) -> Result<()> {
    let version = tag
        .strip_prefix('v')
        .with_context(|| format!("{tag} is not a v<version> tag"))?;
    package::check_tag(tag, &package::workspace_version(root)?)?;
    let path = packages_dir.join(FILE);
    if let Some(signer) = signer {
        let notes = notes(
            &fs::read_to_string(root.join("packaging/release-notes.md"))?,
            version,
        )?;
        let m = manifest(packages_dir, version, notes, now_rfc3339())?;
        let sk = match signer {
            Signer::Env { key, password } => secret_key_from_env(&key, &password)?,
            Signer::Throwaway(public_out) => {
                let kp = minisign::KeyPair::generate_unencrypted_keypair()
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                fs::write(&public_out, format!("{}\n", kp.pk.to_base64()))?;
                eprintln!(
                    "dry run: signed with a throwaway key ({})",
                    public_out.display()
                );
                kp.sk
            }
        };
        fs::write(&path, sign(&m, &sk)?)?;
        eprintln!(
            "wrote {} for {version} ({} packages)",
            path.display(),
            packages(version).len()
        );
    }
    if !check_keys.is_empty() {
        let mut lines = Vec::new();
        for f in check_keys {
            let text = fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
            lines.extend(ps_update::keys::key_line(&text).map(str::to_owned));
        }
        if lines.is_empty() {
            bail!("no public key to check the manifest with");
        }
        check(packages_dir, version, &fs::read(&path)?, &lines)?;
        eprintln!("{} verifies and matches the packages", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_packages(dir: &Path, version: &str) {
        for (_, _, name) in packages(version) {
            fs::write(dir.join(&name), format!("contents of {name}")).unwrap();
        }
    }

    #[test]
    fn scenario_release_packaging_manifest_matches_the_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fake_packages(dir, "0.4.0");
        let m = manifest(dir, "0.4.0", "What's new.".into(), now_rfc3339()).unwrap();
        assert_eq!(m.packages.values().map(BTreeMap::len).sum::<usize>(), 8);
        for kinds in m.packages.values() {
            for p in kinds.values() {
                assert!(
                    p.url.starts_with(&format!("{DOWNLOAD_PREFIX}v0.4.0/")),
                    "{}",
                    p.url
                );
            }
        }
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let env = sign(&m, &kp.sk).unwrap();
        let key = vec![kp.pk.to_base64()];
        check(dir, "0.4.0", env.as_bytes(), &key).expect("verifies with the public key");
        // Another key does not verify it.
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(check(dir, "0.4.0", env.as_bytes(), &[other.pk.to_base64()]).is_err());
        // A changed package fails, naming it.
        let changed = package::artifact_name("0.4.0", Os::Windows, "x86_64", Kind::Setup);
        fs::write(dir.join(&changed), "tampered").unwrap();
        let e = check(dir, "0.4.0", env.as_bytes(), &key)
            .unwrap_err()
            .to_string();
        assert!(e.contains(&changed), "{e}");
        // A changed address fails the signature.
        let tampered = env.replace("/releases/download/v0.4.0/", "/releases/download/v0.4.9/");
        assert!(check(dir, "0.4.0", tampered.as_bytes(), &key).is_err());
        // A missing package fails generation.
        fs::remove_file(dir.join(&changed)).unwrap();
        let e = manifest(dir, "0.4.0", "x".into(), now_rfc3339())
            .unwrap_err()
            .to_string();
        assert!(e.contains(&changed), "{e}");
    }

    #[test]
    fn scenario_release_packaging_notes_taken_from_the_release_notes() {
        let text = "intro\n\n### What's new in 0.4.0\n\n- **Updates.** They install themselves.\n\n### Requirements\n\n- x\n";
        assert_eq!(
            notes(text, "0.4.0").unwrap(),
            "- **Updates.** They install themselves."
        );
        assert!(
            notes(text, "0.4.1")
                .unwrap_err()
                .to_string()
                .contains("What's new in 0.4.1")
        );
        let bullet = format!("- {}\n  continued\n", "x".repeat(1_500));
        let long = format!("### What's new in 0.4.0\n\n{}", bullet.repeat(4));
        let cut = notes(&long, "0.4.0").unwrap();
        assert!(cut.chars().count() <= MAX_NOTES_CHARS, "{}", cut.len());
        assert_eq!(cut.matches("- x").count(), 2, "two whole bullets");
        assert!(cut.ends_with("…and more: see the release page."));
        let huge = format!("### What's new in 0.4.0\n\n- {}\n", "y".repeat(9_000));
        assert!(notes(&huge, "0.4.0").unwrap().chars().count() <= MAX_NOTES_CHARS);
        // The repository's template has the section of the workspace version.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let version = package::workspace_version(root).unwrap();
        let template = fs::read_to_string(root.join("packaging/release-notes.md")).unwrap();
        notes(&template, &version).expect("the release notes have this version's section");
    }

    #[test]
    fn release_times_are_rfc3339() {
        let t = now_rfc3339();
        assert_eq!(t.len(), 20, "{t}");
        assert!(
            t.starts_with("20") && t.ends_with('Z') && t.as_bytes()[10] == b'T',
            "{t}"
        );
    }
}
