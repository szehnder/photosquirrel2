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

/// What kind of block of the release notes a [`NotesBlock`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesBlockKind {
    /// A `#` heading line.
    Heading,
    /// A `- ` or `* ` bullet with its wrapped continuation lines.
    Bullet,
    /// Any other run of lines up to a blank line.
    Paragraph,
}

/// One block of release notes as plain text, for showing in the app: Markdown markers are gone, hard-wrapped
/// lines are joined, and `bold` holds the byte ranges of `text` that were `**bold**` (the bullets' headlines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesBlock {
    pub kind: NotesBlockKind,
    pub text: String,
    pub bold: Vec<std::ops::Range<usize>>,
}

/// The release notes (the manifest's Markdown, from the "What's new in <version>" section) as blocks of plain
/// text with their bold runs, so What's new can show them formatted instead of as raw Markdown. Links keep their
/// text, code spans lose their backticks, and blocks with no text are left out.
pub fn notes_blocks(notes: &str) -> Vec<NotesBlock> {
    let mut raw: Vec<(NotesBlockKind, String)> = Vec::new();
    let mut current: Option<(NotesBlockKind, String)> = None;
    for line in notes.lines() {
        let t = line.trim();
        if t.is_empty() {
            raw.extend(current.take());
        } else if t.starts_with('#') {
            raw.extend(current.take());
            raw.push((
                NotesBlockKind::Heading,
                t.trim_start_matches('#').to_owned(),
            ));
        } else if let Some(rest) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| ["-", "*"].contains(&t).then_some(""))
        {
            raw.extend(current.take());
            current = Some((NotesBlockKind::Bullet, rest.to_owned()));
        } else if let Some((_, text)) = current.as_mut() {
            text.push(' ');
            text.push_str(t);
        } else {
            current = Some((NotesBlockKind::Paragraph, t.to_owned()));
        }
    }
    raw.extend(current);
    raw.into_iter()
        .filter_map(|(kind, markdown)| {
            let (text, bold) = inline(&markdown);
            (!text.is_empty()).then_some(NotesBlock { kind, text, bold })
        })
        .collect()
}

/// One block's inline Markdown as plain text and the byte ranges that were bold (`**` or `__`).
fn inline(markdown: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let plain = links_as_text(&markdown.replace('`', ""));
    let words = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut text = String::with_capacity(words.len());
    let mut bold = Vec::new();
    let mut open: Option<usize> = None;
    let mut rest = words.as_str();
    while let Some(at) = [rest.find("**"), rest.find("__")]
        .into_iter()
        .flatten()
        .min()
    {
        text.push_str(&rest[..at]);
        rest = &rest[at + 2..];
        match open.take() {
            Some(from) => {
                // A space just inside the markers is not part of the bold run.
                let trimmed = text.trim_end().len();
                if trimmed > from {
                    bold.push(from..trimmed);
                }
            }
            None => open = Some(text.len()),
        }
    }
    text.push_str(rest);
    if let Some(from) = open
        && text.len() > from
    {
        bold.push(from..text.len());
    }
    // Markers next to spaces can leave a doubled or edge space; the bold ranges point into the text as built.
    if text.contains("  ") || text.starts_with(' ') || text.ends_with(' ') {
        return squeeze(&text, &bold);
    }
    (text, bold)
}

/// `text` with runs of spaces made single and the ends trimmed, and `bold` moved to match.
fn squeeze(text: &str, bold: &[std::ops::Range<usize>]) -> (String, Vec<std::ops::Range<usize>>) {
    let mut map = vec![0usize; text.len() + 1];
    let mut out = String::with_capacity(text.len());
    let mut prev_space = true;
    for (i, ch) in text.char_indices() {
        map[i] = out.len();
        if ch == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    map[text.len()] = out.len();
    while out.ends_with(' ') {
        out.pop();
    }
    let len = out.len();
    let bold = bold
        .iter()
        .map(|r| map[r.start].min(len)..map[r.end].min(len))
        .filter(|r| !r.is_empty())
        .collect();
    (out, bold)
}

/// `[text](url)` links reduced to their text; an unclosed bracket stays as it is.
fn links_as_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
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
    out
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

    /// The real 0.4.0 notes: three bullets whose bold headlines stay bold, with no Markdown left and the
    /// hard-wrapped lines joined.
    #[test]
    fn notes_blocks_of_the_real_notes() {
        let notes = "- **PhotoSquirrel keeps itself up to date, if you want it to.** It asks once whether to check for updates;
  nothing is sent before you answer. The Debian package gets the verified `.deb` and the `apt` command.
- **The macOS app is signed and notarized.** It opens without the \"damaged\" warning or any Terminal command.
- **Pause and cancel answer faster.** A scan stops within a few hundredths of a second.
";
        let blocks = notes_blocks(notes);
        assert_eq!(blocks.len(), 3);
        assert!(blocks.iter().all(|b| b.kind == NotesBlockKind::Bullet));
        let first = &blocks[0];
        assert_eq!(
            first.text,
            "PhotoSquirrel keeps itself up to date, if you want it to. It asks once whether to check for \
             updates; nothing is sent before you answer. The Debian package gets the verified .deb and the apt \
             command."
        );
        assert_eq!(first.bold.len(), 1);
        assert_eq!(
            &first.text[first.bold[0].clone()],
            "PhotoSquirrel keeps itself up to date, if you want it to."
        );
        assert_eq!(
            &blocks[2].text[blocks[2].bold[0].clone()],
            "Pause and cancel answer faster."
        );
        for b in &blocks {
            assert!(
                !b.text.contains("**") && !b.text.contains("  "),
                "{:?}",
                b.text
            );
        }
    }

    #[test]
    fn notes_blocks_of_other_markdown() {
        let blocks = notes_blocks(
            "## Highlights\n\nWhat's new in\n0.4.1.\n\n* Faster [big folders](https://example.com/x) and __more__\n- a [b c\n- ** spaced ** bold\n- **open bold",
        );
        let kinds: Vec<_> = blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [
                NotesBlockKind::Heading,
                NotesBlockKind::Paragraph,
                NotesBlockKind::Bullet,
                NotesBlockKind::Bullet,
                NotesBlockKind::Bullet,
                NotesBlockKind::Bullet,
            ]
        );
        assert_eq!(blocks[0].text, "Highlights");
        assert_eq!(blocks[1].text, "What's new in 0.4.1.");
        assert_eq!(blocks[2].text, "Faster big folders and more");
        assert_eq!(&blocks[2].text[blocks[2].bold[0].clone()], "more");
        assert_eq!(blocks[3].text, "a [b c");
        assert!(blocks[3].bold.is_empty());
        assert_eq!(blocks[4].text, "spaced bold");
        assert_eq!(&blocks[4].text[blocks[4].bold[0].clone()], "spaced");
        // An unclosed bold runs to the end of its block.
        assert_eq!(blocks[5].text, "open bold");
        assert_eq!(&blocks[5].text[blocks[5].bold[0].clone()], "open bold");
        assert!(notes_blocks("").is_empty());
        assert!(notes_blocks("###\n\n- \n").is_empty());
    }
}
