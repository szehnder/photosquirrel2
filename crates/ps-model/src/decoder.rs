//! Decoders that are not built into PhotoSquirrel (ADR-0089, ADR-0090, ADR-0091): which one a file waited for,
//! and the detection report with the reason and the fix when one is missing.

use serde::{Deserialize, Serialize};

/// A decoder a file's format needs. Stored in `file.decoder_missing` (catalog migration 19) by its
/// [`DecoderKey::as_str`] key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoderKey {
    /// HEIC/HEIF stills: the operating system's decoder (ImageIO, WIC, the system libheif).
    Heif,
    /// AVIF stills: the bundled AV1 decoder. Only rows of catalogs from before it was bundled wait for it.
    Avif,
}

impl DecoderKey {
    pub const ALL: [DecoderKey; 2] = [DecoderKey::Heif, DecoderKey::Avif];

    /// The storage key.
    pub fn as_str(self) -> &'static str {
        match self {
            DecoderKey::Heif => "heif",
            DecoderKey::Avif => "avif",
        }
    }

    pub fn from_storage(s: &str) -> Option<DecoderKey> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// "HEIC", "AVIF": how the format is named in messages.
    pub fn label(self) -> &'static str {
        match self {
            DecoderKey::Heif => "HEIC",
            DecoderKey::Avif => "AVIF",
        }
    }
}

/// Why a placeholder stands in for pixels (stored with the placeholder; absent in rows written before it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PlaceholderReason {
    /// No decoder for this format on this machine.
    DecoderMissing { key: DecoderKey },
    /// A video without a poster-frame tool.
    NoPosterTool,
    /// An AVIF image sequence without a still item.
    Sequence,
}

/// One step of a fix: text, optionally a command to run or a link to open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixStep {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// A plain web URL, always shown (`https://apps.microsoft.com/detail/<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// A link to open only on an explicit click (`ms-windows-store://pdp/?ProductId=<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
}

/// How to make a missing decoder available on this system.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    pub summary: String,
    pub steps: Vec<FixStep>,
    /// The app must be restarted (and sources rescanned) afterwards.
    pub restart: bool,
}

impl Fix {
    /// Every step as plain text lines (commands and URLs on their own line), for CLI and doctor output.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.steps {
            out.push(s.text.clone());
            if let Some(c) = &s.command {
                out.push(format!("    {c}"));
            }
            if let Some(u) = &s.url {
                out.push(format!("    {u}"));
            }
        }
        out
    }
}

/// A format family the report covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoderFormat {
    Heic,
    Avif,
    PosterFrames,
}

impl DecoderFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            DecoderFormat::Heic => "heic",
            DecoderFormat::Avif => "avif",
            DecoderFormat::PosterFrames => "poster_frames",
        }
    }
}

/// What detection found for one format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecoderEntry {
    pub format: DecoderFormat,
    pub available: bool,
    /// "ImageIO", "WIC", "libheif", "bundled AV1 decoder (rav1d)", "ffmpeg".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Why it is unavailable (or a remark when available, such as slower AVIF without assembly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    /// AVIF: built with rav1d's assembly (`avif-asm`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<bool>,
}

impl DecoderEntry {
    /// One line for reports: the backend when available, else the reason.
    pub fn detail(&self) -> String {
        match (self.available, &self.backend, &self.reason) {
            (true, Some(b), Some(r)) => format!("{b} ({r})"),
            (true, Some(b), None) => match &self.version {
                Some(v) if !b.contains(v.as_str()) => format!("{b} {v}"),
                _ => b.clone(),
            },
            (_, _, Some(r)) => r.clone(),
            (true, None, None) => "available".into(),
            (false, _, None) => "unavailable".into(),
        }
    }
}

/// The operating system as detection saw it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsSummary {
    /// `linux`, `macos`, `windows`, ...
    pub family: String,
    /// `/etc/os-release` ID on Linux, the edition on Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// What decoder detection found (doctor, the scan summary, the inspector).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecoderReport {
    pub entries: Vec<DecoderEntry>,
    /// `PHOTOSQUIRREL_HEIF_DECODER` / `PHOTOSQUIRREL_LIBHEIF` in force ("PHOTOSQUIRREL_HEIF_DECODER=none").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsSummary>,
}

impl DecoderReport {
    pub fn entry(&self, f: DecoderFormat) -> Option<&DecoderEntry> {
        self.entries.iter().find(|e| e.format == f)
    }

    /// The entry of the format a decoder key decodes.
    pub fn for_key(&self, k: DecoderKey) -> Option<&DecoderEntry> {
        self.entry(match k {
            DecoderKey::Heif => DecoderFormat::Heic,
            DecoderKey::Avif => DecoderFormat::Avif,
        })
    }

    /// Insert or replace the entry of `e.format`.
    pub fn set(&mut self, e: DecoderEntry) {
        match self.entries.iter_mut().find(|x| x.format == e.format) {
            Some(x) => *x = e,
            None => self.entries.push(e),
        }
    }
}

/// Files of one format still waiting for a decoder at the end of a run (ADR-0091).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Undecoded {
    /// `heic`, `avif` (the catalog format key).
    pub format: String,
    pub key: DecoderKey,
    pub count: u64,
    /// The decoder is available now (a rescan fills the previews in).
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    /// Sources holding such files (for "rescan to fill in").
    #[serde(default)]
    pub sources: Vec<crate::SourceId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_keys_round_trip_through_storage() {
        for k in DecoderKey::ALL {
            assert_eq!(DecoderKey::from_storage(k.as_str()), Some(k));
        }
        assert_eq!(DecoderKey::from_storage("heif"), Some(DecoderKey::Heif));
        assert_eq!(DecoderKey::from_storage("avif"), Some(DecoderKey::Avif));
        assert_eq!(DecoderKey::from_storage("jxl"), None);
        assert_eq!(
            serde_json::to_string(&DecoderKey::Heif).expect("json"),
            "\"heif\""
        );
    }

    /// Placeholder and diff JSON stored before this change still deserialises.
    #[test]
    fn json_written_before_decoder_keys_deserialises() {
        let old = r#"{"media_kind":"image","format":"heic","duration_ms":null,"capture_date":null,
            "note":"preview unavailable (HEIC decoder not built in)"}"#;
        let p: crate::PlaceholderInfo = serde_json::from_str(old).expect("old placeholder");
        assert_eq!(p.reason, None);
        let old = r#"{"new":1,"changed":2,"unchanged":3,"missing":0,"returning":0,"moved":0}"#;
        let d: crate::DiffCounts = serde_json::from_str(old).expect("old diff");
        assert_eq!((d.awaiting_decoder, d.redecode, d.redecoded), (0, 0, 0));
        assert_eq!(d.unchanged, 3);
    }

    #[test]
    fn placeholder_reason_json() {
        let r = PlaceholderReason::DecoderMissing {
            key: DecoderKey::Heif,
        };
        let s = serde_json::to_string(&r).expect("json");
        assert_eq!(s, r#"{"kind":"decoder_missing","key":"heif"}"#);
        assert_eq!(
            serde_json::from_str::<PlaceholderReason>(&s).expect("back"),
            r
        );
    }
}
