//! The signed update manifest (ADR-0125, spec "Signed manifest").
//!
//! `update-manifest.json` is an envelope `{"format": 1, "manifest": "<exact JSON text>", "signature": "<minisign
//! signature text>"}`. The signature covers the UTF-8 bytes of `manifest` exactly, and its trusted comment must
//! be `photosquirrel-update <version>`. Only after the signature verifies is the manifest parsed.

use std::collections::BTreeMap;

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};

use crate::UpdateError;
use crate::build::{DOWNLOAD_PREFIX, PRODUCT};

/// The envelope format this build reads.
pub const FORMAT: u32 = 1;

/// The largest manifest envelope read (spec "Restricted transport").
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// The longest release notes a manifest carries.
pub const MAX_NOTES_CHARS: usize = 4_000;

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub format: u32,
    pub manifest: String,
    pub signature: String,
}

/// What a release offers. Unknown fields are ignored, so later fields (a channel, a rollout) need no new format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub product: String,
    pub version: String,
    /// RFC 3339 release time.
    pub released: String,
    #[serde(default)]
    pub urgent: bool,
    /// Short release notes (Markdown), at most [`MAX_NOTES_CHARS`].
    #[serde(default)]
    pub notes: String,
    pub notes_url: String,
    /// `platform` (e.g. `linux-x86_64`) -> package kind (e.g. `appimage`) -> package.
    pub packages: BTreeMap<String, BTreeMap<String, Package>>,
}

/// One downloadable package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub size: u64,
    /// Lower-case hex SHA-256.
    pub sha256: String,
    pub url: String,
}

impl Manifest {
    pub fn semver(&self) -> semver::Version {
        semver::Version::parse(&self.version).expect("checked when the manifest was verified")
    }

    /// The package of `kind` for `platform`, if the release has one.
    pub fn package(&self, platform: &str, kind: &str) -> Option<&Package> {
        self.packages.get(platform)?.get(kind)
    }

    /// The trusted comment a signature of this version must carry.
    pub fn trusted_comment(version: &str) -> String {
        format!("photosquirrel-update {version}")
    }

    fn check(&self) -> Result<(), UpdateError> {
        let bad = |m: String| Err(UpdateError::BadManifest(m));
        if self.product != PRODUCT {
            return bad(format!("it is for {:?}", self.product));
        }
        if semver::Version::parse(&self.version).is_err() {
            return bad(format!(
                "version {:?} is not a semantic version",
                self.version
            ));
        }
        if self.notes.chars().count() > MAX_NOTES_CHARS {
            return bad("its notes are too long".into());
        }
        let prefix = format!("{DOWNLOAD_PREFIX}v{}/", self.version);
        for (platform, kinds) in &self.packages {
            for (kind, p) in kinds {
                let ok_name = !p.name.is_empty()
                    && !p.name.contains(['/', '\\'])
                    && p.name != "."
                    && p.name != "..";
                if !ok_name {
                    return bad(format!("{platform} {kind}: bad file name {:?}", p.name));
                }
                if p.url != format!("{prefix}{}", p.name) {
                    return bad(format!(
                        "{platform} {kind}: {} is not under this release's downloads",
                        p.url
                    ));
                }
                if p.sha256.len() != 64 || !p.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return bad(format!("{platform} {kind}: bad SHA-256"));
                }
                if p.size == 0 {
                    return bad(format!("{platform} {kind}: empty package"));
                }
            }
        }
        Ok(())
    }
}

/// Verify an envelope against `keys` and return its manifest. Nothing in an envelope that fails is trusted.
pub fn verify(envelope: &[u8], keys: &[PublicKey]) -> Result<Manifest, UpdateError> {
    let env: Envelope = serde_json::from_slice(envelope)
        .map_err(|e| UpdateError::BadManifest(format!("not an update manifest envelope ({e})")))?;
    if env.format != FORMAT {
        return Err(UpdateError::BadManifest(format!(
            "format {} is not supported by this version",
            env.format
        )));
    }
    let sig = Signature::decode(&env.signature).map_err(|_| UpdateError::BadSignature)?;
    if !keys
        .iter()
        .any(|k| k.verify(env.manifest.as_bytes(), &sig, false).is_ok())
    {
        return Err(UpdateError::BadSignature);
    }
    let manifest: Manifest =
        serde_json::from_str(&env.manifest).map_err(|e| UpdateError::BadManifest(e.to_string()))?;
    // The signed comment binds the signature to this version: a signature of another release cannot be
    // re-used with this text.
    if sig.trusted_comment() != Manifest::trusted_comment(&manifest.version) {
        return Err(UpdateError::BadSignature);
    }
    manifest.check()?;
    Ok(manifest)
}

/// A short plain-text summary of release notes for a one-line notice. The notes are Markdown bullets that open
/// with a bold headline (`- **Updates install themselves.** Details...`, hard-wrapped): the summary is the
/// headlines in order, as far as they fit in `max_chars`. A bullet or paragraph without one stands for itself.
/// Markdown markers are dropped and wrapped lines joined, so nothing raw reaches the notice. Text cut short ends at
/// a word, with an ellipsis.
pub fn notes_summary(notes: &str, max_chars: usize) -> String {
    let mut items: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in notes.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            items.extend(current.take());
            continue;
        }
        if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            items.extend(current.take());
            current = Some(rest.trim().to_owned());
        } else if let Some(c) = current.as_mut() {
            c.push(' ');
            c.push_str(t);
        } else {
            current = Some(t.to_owned());
        }
    }
    items.extend(current);
    let mut summary = String::new();
    for item in &items {
        let headline = item
            .strip_prefix("**")
            .and_then(|r| r.split_once("**"))
            .map_or(item.as_str(), |(bold, _)| bold);
        let mut text = plain_text(headline);
        if text.is_empty() {
            continue;
        }
        if !text.ends_with(['.', '!', '?', '…']) {
            text.push('.');
        }
        let sep = usize::from(!summary.is_empty());
        if summary.chars().count() + sep + text.chars().count() > max_chars {
            if summary.is_empty() {
                summary = cut_at_word(&text, max_chars);
            }
            break;
        }
        if sep == 1 {
            summary.push(' ');
        }
        summary.push_str(&text);
    }
    summary
}

/// Markdown inline markers removed: `**`, `__`, backticks, and `[text](url)` links reduced to their text.
fn plain_text(s: &str) -> String {
    let s = s.replace("**", "").replace("__", "").replace('`', "");
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        match after.find("](").and_then(|close| {
            after[close + 2..]
                .find(')')
                .map(|end| (close, close + 2 + end))
        }) {
            Some((close, end)) => {
                out.push_str(&rest[..open]);
                out.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `s` cut to at most `max_chars` characters at a word boundary, ending in an ellipsis.
fn cut_at_word(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let head: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    let head = match head.rfind(' ') {
        Some(i) if i > 0 => &head[..i],
        _ => head.as_str(),
    };
    format!("{}…", head.trim_end_matches([',', ';', ':', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, sample_manifest};

    fn keys() -> Vec<PublicKey> {
        crate::keys::trusted()
    }

    #[test]
    fn a_signed_manifest_verifies() {
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", b"payload")]);
        let env = testing::envelope(&m);
        let got = verify(env.as_bytes(), &keys()).expect("verifies");
        assert_eq!(got, m);
        assert_eq!(
            got.package("linux-x86_64", "appimage").map(|p| p.size),
            Some(7)
        );
    }

    #[test]
    fn scenario_app_updates_tampered_manifest_rejected() {
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", b"payload")]);
        let env = testing::envelope(&m);
        // A changed SHA-256 after signing.
        let mut e: Envelope = serde_json::from_str(&env).expect("envelope");
        let sum = &m.packages["linux-x86_64"]["appimage"].sha256;
        e.manifest = e.manifest.replace(sum.as_str(), &"0".repeat(64));
        let tampered = serde_json::to_vec(&e).expect("json");
        assert!(matches!(
            verify(&tampered, &keys()),
            Err(UpdateError::BadSignature)
        ));
        assert_eq!(
            verify(&tampered, &keys()).unwrap_err().to_string(),
            "update manifest signature is not valid"
        );
        // Signed by a key that is not built in.
        let other = testing::TestSigner::generate().envelope(&m);
        assert!(matches!(
            verify(other.as_bytes(), &keys()),
            Err(UpdateError::BadSignature)
        ));
        // Unsigned, and a signature for another version's comment.
        let mut e: Envelope = serde_json::from_str(&env).expect("envelope");
        e.signature = String::new();
        assert!(verify(&serde_json::to_vec(&e).expect("json"), &keys()).is_err());
        let wrong_comment =
            testing::TestSigner::test_key().envelope_with_comment(&m, "photosquirrel-update 9.9.9");
        assert!(matches!(
            verify(wrong_comment.as_bytes(), &keys()),
            Err(UpdateError::BadSignature)
        ));
    }

    #[test]
    fn scenario_app_updates_either_built_in_key_accepted() {
        let m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", b"payload")]);
        let (first, second, stranger) = (
            testing::TestSigner::generate(),
            testing::TestSigner::generate(),
            testing::TestSigner::generate(),
        );
        let slots = [first.public_key(), second.public_key()];
        verify(first.envelope(&m).as_bytes(), &slots).expect("first slot");
        verify(second.envelope(&m).as_bytes(), &slots).expect("second slot");
        assert!(matches!(
            verify(stranger.envelope(&m).as_bytes(), &slots),
            Err(UpdateError::BadSignature)
        ));
    }

    #[test]
    fn urls_outside_the_release_are_rejected() {
        let mut m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", b"payload")]);
        m.packages
            .get_mut("linux-x86_64")
            .and_then(|k| k.get_mut("appimage"))
            .expect("package")
            .url = "https://example.com/evil.AppImage".into();
        let env = testing::envelope(&m);
        assert!(matches!(
            verify(env.as_bytes(), &keys()),
            Err(UpdateError::BadManifest(_))
        ));
        let mut m = sample_manifest("0.4.1", &[("linux-x86_64", "appimage", b"payload")]);
        m.packages
            .get_mut("linux-x86_64")
            .and_then(|k| k.get_mut("appimage"))
            .expect("package")
            .name = "../x".into();
        assert!(verify(testing::envelope(&m).as_bytes(), &keys()).is_err());
    }

    /// The notice summary of the real 0.4.0 notes: the bullets' bold headlines, without Markdown or wrapping gaps.
    #[test]
    fn notes_summary_takes_the_bold_headlines() {
        let notes = "- **PhotoSquirrel keeps itself up to date, if you want it to.** It asks once whether to check for updates;
  nothing is sent before you answer. See \"Updates\" below.
- **The macOS app is signed and notarized.** It opens without the \"damaged\" warning or any Terminal command.
";
        assert_eq!(
            notes_summary(notes, 240),
            "PhotoSquirrel keeps itself up to date, if you want it to. The macOS app is signed and notarized."
        );
        // Only the headlines that fit, whole.
        assert_eq!(
            notes_summary(notes, 80),
            "PhotoSquirrel keeps itself up to date, if you want it to."
        );
    }

    #[test]
    fn notes_summary_of_plain_notes() {
        // A bullet without a bold headline stands for itself; markers, links and wrapping are dropped.
        assert_eq!(
            notes_summary(
                "- Faster `scan` of\n  [big folders](https://example.com/x)\n- Fewer bugs",
                240
            ),
            "Faster scan of big folders. Fewer bugs."
        );
        // A paragraph, the test manifests' notes, and nothing.
        assert_eq!(
            notes_summary("What's new in 0.4.1.", 240),
            "What's new in 0.4.1."
        );
        assert_eq!(notes_summary("", 240), "");
        assert_eq!(notes_summary("### Heading only\n\n", 240), "");
        // A first item longer than the limit is cut at a word, with an ellipsis.
        let long = format!("- **{}**", "word ".repeat(100));
        let s = notes_summary(&long, 40);
        assert!(s.ends_with("word…") && s.chars().count() <= 40, "{s}");
        // An unclosed link bracket is kept as text.
        assert_eq!(notes_summary("- a [b c", 240), "a [b c.");
    }
}
