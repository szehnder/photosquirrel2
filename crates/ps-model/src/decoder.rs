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

/// Decodes of a file that failed for a transient reason (a watchdog timeout, AV1 decoding paused, a thread that
/// could not start) after which rescans stop retrying it with the same decoder (ADR-0098).
pub const MAX_DECODE_ATTEMPTS: u8 = 3;

/// A HEIC or AVIF decode that failed during analysis while its decoder was present (`file.decode_failure`, catalog
/// migration 20, ADR-0098): the file keeps a placeholder (ADR-0095) and this record says whether and when a
/// rescan decodes it again. Retried when the decoder backend changed (`backend`: "libheif 1.23.3", "ImageIO",
/// "rav1d 1.1"), or while a transient failure has fewer than [`MAX_DECODE_ATTEMPTS`] attempts. A permanent failure
/// (the data) waits for the file to change or the backend to change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeFailure {
    pub key: DecoderKey,
    pub backend: String,
    pub transient: bool,
    pub attempts: u8,
}

impl DecodeFailure {
    /// The record after one more failed decode: attempts counted on from `prior` only while the decoder, its
    /// backend and the transience are the same.
    pub fn next(
        prior: Option<&DecodeFailure>,
        key: DecoderKey,
        backend: &str,
        transient: bool,
    ) -> DecodeFailure {
        let attempts = prior
            .filter(|p| p.key == key && p.backend == backend && p.transient && transient)
            .map_or(1, |p| p.attempts.saturating_add(1));
        DecodeFailure {
            key,
            backend: backend.to_owned(),
            transient,
            attempts,
        }
    }

    /// The record after a decode that was refused before it started (ADR-0100: AV1 decoding paused, too many
    /// decode threads): a refusal says nothing about the file, so it uses up no attempt. The prior record with
    /// the same decoder and backend is kept; otherwise a transient record with no attempts, so the next rescan
    /// decodes the file again.
    pub fn refused(prior: Option<&DecodeFailure>, key: DecoderKey, backend: &str) -> DecodeFailure {
        match prior.filter(|p| p.key == key && p.backend == backend) {
            Some(p) => p.clone(),
            None => DecodeFailure {
                key,
                backend: backend.to_owned(),
                transient: true,
                attempts: 0,
            },
        }
    }

    /// Whether a rescan whose decoder for this key is `backend_now` (`None`: none available) decodes the
    /// unchanged file again.
    pub fn retry(&self, backend_now: Option<&str>) -> bool {
        match backend_now {
            None => false,
            Some(b) => b != self.backend || (self.transient && self.attempts < MAX_DECODE_ATTEMPTS),
        }
    }

    /// The stored form: `<key>|<t or p>|<attempts>|<backend>` ("heif|p|1|libheif 1.23.3").
    pub fn to_storage(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.key.as_str(),
            if self.transient { "t" } else { "p" },
            self.attempts,
            self.backend
        )
    }

    pub fn from_storage(s: &str) -> Option<DecodeFailure> {
        let mut it = s.splitn(4, '|');
        let key = DecoderKey::from_storage(it.next()?)?;
        let transient = match it.next()? {
            "t" => true,
            "p" => false,
            _ => return None,
        };
        let attempts = it.next()?.parse().ok()?;
        let backend = it.next()?.to_owned();
        Some(DecodeFailure {
            key,
            backend,
            transient,
            attempts,
        })
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

    /// Retry rule of decode failures (ADR-0098): transient failures up to [`MAX_DECODE_ATTEMPTS`] with the same
    /// backend, permanent ones never with it, either once more after a backend change; attempts count on only
    /// for the same backend and transient failures.
    /// A refused decode uses up no attempt (ADR-0100): the prior record is kept as it was, or a transient one
    /// with no attempts is made, which a rescan retries.
    #[test]
    fn refused_decodes_use_up_no_attempt() {
        let r = DecodeFailure::refused(None, DecoderKey::Avif, "rav1d 1.1");
        assert_eq!((r.transient, r.attempts), (true, 0));
        assert!(r.retry(Some("rav1d 1.1")));
        let t2 = DecodeFailure::next(
            Some(&DecodeFailure::next(
                None,
                DecoderKey::Avif,
                "rav1d 1.1",
                true,
            )),
            DecoderKey::Avif,
            "rav1d 1.1",
            true,
        );
        let mut f = t2.clone();
        for _ in 0..5 {
            f = DecodeFailure::refused(Some(&f), DecoderKey::Avif, "rav1d 1.1");
        }
        assert_eq!(f, t2, "five refusals later: still attempt 2");
        assert!(f.retry(Some("rav1d 1.1")));
        assert_eq!(DecodeFailure::from_storage(&r.to_storage()), Some(r));
        // A record of an older backend is replaced by a fresh one.
        let old = DecodeFailure::next(None, DecoderKey::Avif, "rav1d 1.0", false);
        assert_eq!(
            DecodeFailure::refused(Some(&old), DecoderKey::Avif, "rav1d 1.1").attempts,
            0
        );
    }

    #[test]
    fn decode_failures_retry_while_transient_or_after_a_backend_change() {
        let t1 = DecodeFailure::next(None, DecoderKey::Avif, "rav1d 1.1", true);
        assert_eq!(t1.attempts, 1);
        assert!(t1.retry(Some("rav1d 1.1")));
        let t2 = DecodeFailure::next(Some(&t1), DecoderKey::Avif, "rav1d 1.1", true);
        let t3 = DecodeFailure::next(Some(&t2), DecoderKey::Avif, "rav1d 1.1", true);
        assert_eq!((t2.attempts, t3.attempts), (2, 3));
        assert!(t2.retry(Some("rav1d 1.1")));
        assert!(!t3.retry(Some("rav1d 1.1")), "attempts exhausted");
        assert!(t3.retry(Some("rav1d 1.2")), "a new backend tries again");
        let p = DecodeFailure::next(None, DecoderKey::Heif, "libheif 1.17.6", false);
        assert!(!p.retry(Some("libheif 1.17.6")), "the data does not change");
        assert!(p.retry(Some("libheif 1.18.2")), "an upgrade tries again");
        assert!(!p.retry(None), "no decoder now");
        // A new backend or a permanent failure starts the count again.
        let after = DecodeFailure::next(Some(&t3), DecoderKey::Avif, "rav1d 1.2", true);
        assert_eq!(after.attempts, 1);
        assert_eq!(
            DecodeFailure::next(Some(&t2), DecoderKey::Avif, "rav1d 1.1", false).attempts,
            1
        );
    }

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
