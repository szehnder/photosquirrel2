//! Edge rules (ADR-0011, ADR-0013, ADR-0035): when two files are the same shot, when they are only a
//! similar-shot suggestion, and why. Every verdict is explainable.

use std::fmt;

use ps_hash::PERCEPTUAL_HASH_VERSION;
use ps_model::{CaptureTime, Dimensions, FileId};
use serde::{Deserialize, Serialize};

use crate::input::DedupInput;

/// Derivative thresholds (ADR-0013), inclusive.
pub const MAX_PHASH_DISTANCE: u32 = 6;
pub const MAX_DHASH_DISTANCE: u32 = 8;
/// Largest relative display-aspect difference for a derivative edge (ADR-0013).
pub const MAX_ASPECT_DIFF: f64 = 0.02;
/// RAW+JPEG keeper-set capture tolerance (ADR-0014).
pub const KEEPER_SET_MAX_MILLIS: i64 = 1000;

/// Why a file is joined to another one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EdgeKind {
    /// Identical content identity (sampled identities also need equal size, duration and dimensions).
    Exact,
    /// The same image at another size or compression.
    Derivative {
        phash_distance: u8,
        dhash_distance: u8,
    },
    /// The user merged the two groups (ADR-0033 "always pair").
    UserMerge,
}

impl EdgeKind {
    /// Larger is stronger: exact, then derivatives by total then pHash distance. User merges are applied
    /// after the automatic rules and never compete.
    pub(crate) fn strength(self) -> (u8, u32, u32) {
        match self {
            EdgeKind::Exact => (3, 0, 0),
            EdgeKind::Derivative {
                phash_distance: p,
                dhash_distance: d,
            } => (
                2,
                u32::MAX - (u32::from(p) + u32::from(d)),
                u32::MAX - u32::from(p),
            ),
            EdgeKind::UserMerge => (1, 0, 0),
        }
    }

    /// The catalog's edge type (user merges have none).
    pub fn to_group_edge(self) -> Option<ps_model::GroupEdge> {
        match self {
            EdgeKind::Exact => Some(ps_model::GroupEdge::Exact),
            EdgeKind::Derivative {
                phash_distance,
                dhash_distance,
            } => Some(ps_model::GroupEdge::Derivative {
                phash_distance,
                dhash_distance,
            }),
            EdgeKind::UserMerge => None,
        }
    }
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EdgeKind::Exact => f.write_str("exact copy (identical content)"),
            EdgeKind::Derivative {
                phash_distance,
                dhash_distance,
            } => write!(
                f,
                "same image at another size or compression (pHash distance {phash_distance}, dHash distance {dhash_distance})"
            ),
            EdgeKind::UserMerge => f.write_str("merged by you"),
        }
    }
}

/// Metadata that contradicts a hash match (ADR-0013 as amended by ADR-0035). Such pairs become similar-shot
/// suggestions.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Contradiction {
    /// Both have a capture time and the seconds differ.
    CaptureSecond { a: CaptureTime, b: CaptureTime },
    /// Same capture second, both have sub-seconds, and they differ.
    SubSecond { a_millis: u16, b_millis: u16 },
    /// Both have a camera serial and they differ.
    CameraSerial { a: String, b: String },
    /// Both have a camera model and they differ.
    CameraModel { a: String, b: String },
    /// Both look like camera originals of one model with identical stored dimensions but different content:
    /// two originals are two shots.
    TwoCameraOriginals { model: String, dims: Dimensions },
    /// Both record which frame they are (`OriginalRawFileName`, maker-note file number) and the frames differ
    /// (ADR-0105).
    FrameId { a: String, b: String },
}

impl fmt::Display for Contradiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Contradiction::CaptureSecond { a, b } => write!(
                f,
                "taken at different times ({} and {})",
                a.display(),
                b.display()
            ),
            Contradiction::SubSecond { a_millis, b_millis } => write!(
                f,
                "taken in the same second but at different sub-seconds ({a_millis} ms and {b_millis} ms): burst frames"
            ),
            Contradiction::CameraSerial { a, b } => {
                write!(f, "taken with different cameras (serial {a} and {b})")
            }
            Contradiction::CameraModel { a, b } => {
                write!(f, "taken with different camera models ({a} and {b})")
            }
            Contradiction::TwoCameraOriginals { model, dims } => write!(
                f,
                "two different camera originals from {model} at {dims}: two shots"
            ),
            Contradiction::FrameId { a, b } => {
                write!(
                    f,
                    "recorded as different frames ({a} and {b}): burst frames"
                )
            }
        }
    }
}

/// Why no edge and no suggestion exists between two files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NoMatch {
    /// One of the files is not a still image of an analyzed class, or has no hash of the current version.
    NotComparable { detail: String },
    /// Hash distances above the thresholds.
    HashesDiffer {
        phash_distance: u8,
        dhash_distance: u8,
    },
    /// Display dimensions unknown for one file.
    UnknownDimensions,
    /// Display aspect ratios differ by more than 2% (a crop).
    AspectDiffers {
        a: Dimensions,
        b: Dimensions,
        diff: f64,
    },
    /// Equal sampled identity but size, duration or dimensions differ (ADR-0011).
    SampledFactsDiffer,
}

impl fmt::Display for NoMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NoMatch::NotComparable { detail } => write!(f, "not comparable: {detail}"),
            NoMatch::HashesDiffer {
                phash_distance,
                dhash_distance,
            } => write!(
                f,
                "images differ (pHash distance {phash_distance} > {MAX_PHASH_DISTANCE} or dHash distance {dhash_distance} > {MAX_DHASH_DISTANCE})"
            ),
            NoMatch::UnknownDimensions => f.write_str("dimensions unknown"),
            NoMatch::AspectDiffers { a, b, diff } => write!(
                f,
                "aspect ratios differ by {:.1}% ({a} vs {b}): a crop or another image",
                diff * 100.0
            ),
            NoMatch::SampledFactsDiffer => f.write_str(
                "equal sampled identity but size, duration or dimensions differ: not the same file",
            ),
        }
    }
}

/// The decision for one pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum PairVerdict {
    Edge {
        edge: EdgeKind,
    },
    /// Passed the hash and aspect tests, failed the metadata test: a similar-shot suggestion.
    Suggestion {
        phash_distance: u8,
        dhash_distance: u8,
        reason: Contradiction,
    },
    NoMatch {
        reason: NoMatch,
    },
}

impl fmt::Display for PairVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairVerdict::Edge { edge } => write!(f, "duplicate: {edge}"),
            PairVerdict::Suggestion {
                phash_distance,
                dhash_distance,
                reason,
            } => write!(
                f,
                "similar shot only (pHash distance {phash_distance}, dHash distance {dhash_distance}): {reason}"
            ),
            PairVerdict::NoMatch { reason } => write!(f, "not a duplicate: {reason}"),
        }
    }
}

/// A similar-shot suggestion: never collapsed into a group (ADR-0013).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Suggestion {
    /// `file_a < file_b`.
    pub file_a: FileId,
    pub file_b: FileId,
    pub phash_distance: u8,
    pub dhash_distance: u8,
    pub reason: Contradiction,
}

fn norm(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
}

/// The comparable camera model: lower-case, whitespace collapsed, and a leading make word stripped, so that
/// "Canon EOS 70D" (EXIF) and "EOS 70D" (rawler's normalized RAW model) compare equal, as do
/// "NIKON CORPORATION" / "NIKON D750" and "Nikon" / "D750". `None` without a model.
pub fn camera_model_key(make: Option<&str>, model: Option<&str>) -> Option<String> {
    let words = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
    };
    let mut m = words(model?);
    if let Some(first) = make.map(words).and_then(|w| w.into_iter().next())
        && m.len() > 1
        && m[0] == first
    {
        m.remove(0);
    }
    (!m.is_empty()).then(|| m.join(" "))
}

fn model_key(m: &crate::input::DedupMeta) -> Option<String> {
    camera_model_key(m.camera_make.as_deref(), m.camera_model.as_deref())
}

/// Exact edge: identical content identity; sampled identities also need equal size, duration and
/// dimensions (ADR-0011).
pub fn exact_verdict(a: &DedupInput, b: &DedupInput) -> Option<Result<EdgeKind, NoMatch>> {
    let (ca, cb) = (a.content?, b.content?);
    if ca != cb {
        return None;
    }
    Some(if a.exact_key() == b.exact_key() {
        Ok(EdgeKind::Exact)
    } else {
        Err(NoMatch::SampledFactsDiffer)
    })
}

/// Relative difference of two display aspect ratios (0.02 = 2%).
pub fn aspect_diff(a: Dimensions, b: Dimensions) -> f64 {
    let ra = f64::from(a.width) / f64::from(a.height.max(1));
    let rb = f64::from(b.width) / f64::from(b.height.max(1));
    ra.max(rb) / ra.min(rb).max(f64::MIN_POSITIVE) - 1.0
}

/// Files that may get derivative edges: analyzed still images (ADR-0015 candidate classes) with a hash of the
/// current algorithm version.
pub fn derivative_eligible(x: &DedupInput) -> Result<(), String> {
    if !x.is_image() {
        return Err(format!("{} is not a still image", x.format.label()));
    }
    if !x.class.is_candidate() {
        return Err(format!("class {} is not analyzed", x.class.as_str()));
    }
    match x.hash {
        None => Err("no perceptual hash".into()),
        Some(h) if h.version != PERCEPTUAL_HASH_VERSION => {
            Err(format!("perceptual hash version {} is outdated", h.version))
        }
        Some(_) => Ok(()),
    }
}

/// A camera RAW and a JPEG/HEIC with the same stem in the same folder from the same camera: the two halves
/// of one capture (ADR-0014). Both capture times must be present and match: within
/// [`KEEPER_SET_MAX_MILLIS`] for keeper sets (`strict == false`), or to the second and sub-second as ADR-0035
/// requires of edges (`strict == true`).
pub fn raw_jpeg_companions(a: &DedupInput, b: &DedupInput, strict: bool) -> bool {
    let (raw, other) = match (a.format.is_raw(), b.format.is_raw()) {
        (true, false) => (a, b),
        (false, true) => (b, a),
        _ => return false,
    };
    if !matches!(
        other.format,
        ps_model::Format::Jpeg | ps_model::Format::Heic
    ) {
        return false;
    }
    if raw.folder() != other.folder() || !raw.stem().eq_ignore_ascii_case(other.stem()) {
        return false;
    }
    let (ma, mb) = (model_key(&raw.meta), model_key(&other.meta));
    if ma.is_none() || ma != mb {
        return false;
    }
    if let (Some(sa), Some(sb)) = (
        norm(&raw.meta.camera_serial),
        norm(&other.meta.camera_serial),
    ) && sa != sb
    {
        return false;
    }
    match (raw.meta.capture_time, other.meta.capture_time) {
        (Some(ta), Some(tb)) if strict => {
            ta.local_seconds == tb.local_seconds
                && (!(raw.meta.subsec_present && other.meta.subsec_present)
                    || ta.millis == tb.millis)
        }
        (Some(ta), Some(tb)) => ta.diff_millis(&tb) <= KEEPER_SET_MAX_MILLIS,
        _ => false,
    }
}

/// Both files positively record one frame (ADR-0107): the same frame identity ([`FrameId::same_frame`]) and the
/// same capture time, to the second and to the sub-second when both record it. Anything missing is not evidence.
///
/// [`FrameId::same_frame`]: ps_model::FrameId::same_frame
fn same_recorded_frame(ma: &crate::input::DedupMeta, mb: &crate::input::DedupMeta) -> bool {
    let (Some(fa), Some(fb)) = (&ma.frame, &mb.frame) else {
        return false;
    };
    let (Some(ta), Some(tb)) = (ma.capture_time, mb.capture_time) else {
        return false;
    };
    fa.same_frame(fb)
        && ta.local_seconds == tb.local_seconds
        && (!(ma.subsec_present && mb.subsec_present) || ta.millis == tb.millis)
}

/// The metadata test of ADR-0013 / ADR-0035 for a hash-matching pair.
pub fn metadata_contradiction(a: &DedupInput, b: &DedupInput) -> Option<Contradiction> {
    let (ma, mb) = (&a.meta, &b.meta);
    if let (Some(ta), Some(tb)) = (ma.capture_time, mb.capture_time) {
        if ta.local_seconds != tb.local_seconds {
            return Some(Contradiction::CaptureSecond { a: ta, b: tb });
        }
        if ma.subsec_present && mb.subsec_present && ta.millis != tb.millis {
            return Some(Contradiction::SubSecond {
                a_millis: ta.millis,
                b_millis: tb.millis,
            });
        }
    }
    if let (Some(sa), Some(sb)) = (norm(&ma.camera_serial), norm(&mb.camera_serial))
        && sa != sb
    {
        return Some(Contradiction::CameraSerial {
            a: ma.camera_serial.clone().unwrap_or_default(),
            b: mb.camera_serial.clone().unwrap_or_default(),
        });
    }
    let (mda, mdb) = (model_key(ma), model_key(mb));
    if let (Some(x), Some(y)) = (&mda, &mdb)
        && x != y
    {
        return Some(Contradiction::CameraModel {
            a: ma.camera_model.clone().unwrap_or_default(),
            b: mb.camera_model.clone().unwrap_or_default(),
        });
    }
    // ADR-0105: two files that record different frames are two shots (a missing identity matches nothing).
    let rules = ps_model::frame::frame_identity_enabled();
    if rules
        && let (Some(fa), Some(fb)) = (&ma.frame, &mb.frame)
        && let Some((x, y)) = fa.differs(fb)
    {
        return Some(Contradiction::FrameId { a: x, b: y });
    }
    // ADR-0035: two camera originals of one model with identical stored dimensions are two shots; a RAW file is
    // a camera original whether or not its maker notes were parsed (ADR-0105). The RAW and JPEG halves of one
    // capture (same stem, same folder, same time) are one shot, not two originals, and so are two files that both
    // record the same frame at the same capture time (ADR-0107: a DNG and the same DNG with XMP written into it).
    let original = |x: &DedupInput| x.meta.has_maker_notes || (rules && x.format.is_raw());
    if original(a)
        && !(rules && same_recorded_frame(ma, mb))
        && original(b)
        && mda.is_some()
        && mda == mdb
        && let (Some(da), Some(db)) = (ma.stored(), mb.stored())
        && da == db
        && (a.content.is_none() || a.content != b.content)
        && !raw_jpeg_companions(a, b, true)
    {
        return Some(Contradiction::TwoCameraOriginals {
            model: ma.camera_model.clone().unwrap_or_default(),
            dims: da,
        });
    }
    None
}

/// The derivative rule for a pair with known hash distances (ADR-0013 + ADR-0035), in order: hash thresholds,
/// display aspect within 2%, metadata test.
pub fn derivative_verdict(
    a: &DedupInput,
    b: &DedupInput,
    phash_distance: u32,
    dhash_distance: u32,
) -> PairVerdict {
    let (p, d) = (phash_distance.min(255) as u8, dhash_distance.min(255) as u8);
    if phash_distance > MAX_PHASH_DISTANCE || dhash_distance > MAX_DHASH_DISTANCE {
        return PairVerdict::NoMatch {
            reason: NoMatch::HashesDiffer {
                phash_distance: p,
                dhash_distance: d,
            },
        };
    }
    let (Some(da), Some(db)) = (a.meta.display(), b.meta.display()) else {
        return PairVerdict::NoMatch {
            reason: NoMatch::UnknownDimensions,
        };
    };
    let diff = aspect_diff(da, db);
    if diff > MAX_ASPECT_DIFF {
        return PairVerdict::NoMatch {
            reason: NoMatch::AspectDiffers { a: da, b: db, diff },
        };
    }
    match metadata_contradiction(a, b) {
        Some(reason) => PairVerdict::Suggestion {
            phash_distance: p,
            dhash_distance: d,
            reason,
        },
        None => PairVerdict::Edge {
            edge: EdgeKind::Derivative {
                phash_distance: p,
                dhash_distance: d,
            },
        },
    }
}

/// The full decision for any two files: exact first, then the derivative rule.
pub fn pair_verdict(a: &DedupInput, b: &DedupInput) -> PairVerdict {
    match exact_verdict(a, b) {
        Some(Ok(edge)) => return PairVerdict::Edge { edge },
        Some(Err(reason)) => return PairVerdict::NoMatch { reason },
        None => {}
    }
    if let Err(detail) = derivative_eligible(a).and_then(|()| derivative_eligible(b)) {
        return PairVerdict::NoMatch {
            reason: NoMatch::NotComparable { detail },
        };
    }
    let (Some(ha), Some(hb)) = (a.hash, b.hash) else {
        unreachable!("eligible files have hashes")
    };
    derivative_verdict(a, b, ha.phash_distance(&hb), ha.dhash_distance(&hb))
}

/// A human-readable explanation of how two files relate, for the "why" panel and the CLI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairExplanation {
    pub file_a: FileId,
    pub file_b: FileId,
    pub verdict: PairVerdict,
    pub phash_distance: Option<u32>,
    pub dhash_distance: Option<u32>,
    pub display_a: Option<Dimensions>,
    pub display_b: Option<Dimensions>,
    /// One line per rule checked.
    pub lines: Vec<String>,
}

/// Explain the decision for `a` and `b`, listing each rule.
pub fn explain_pair(a: &DedupInput, b: &DedupInput) -> PairExplanation {
    let verdict = pair_verdict(a, b);
    let (pd, dd) = match (a.hash, b.hash) {
        (Some(x), Some(y)) if x.version == y.version => {
            (Some(x.phash_distance(&y)), Some(x.dhash_distance(&y)))
        }
        _ => (None, None),
    };
    let (da, db) = (a.meta.display(), b.meta.display());
    let mut lines = Vec::new();
    lines.push(match (a.content, b.content) {
        (Some(x), Some(y)) if x == y => "content identity: identical".to_string(),
        (Some(_), Some(_)) => "content identity: different".to_string(),
        _ => "content identity: unknown for one file".to_string(),
    });
    if let (Some(p), Some(d)) = (pd, dd) {
        lines.push(format!(
            "perceptual hashes: pHash distance {p} (limit {MAX_PHASH_DISTANCE}), dHash distance {d} (limit {MAX_DHASH_DISTANCE})"
        ));
    } else {
        lines.push("perceptual hashes: not available for both files".into());
    }
    if let (Some(x), Some(y)) = (da, db) {
        lines.push(format!(
            "display aspect: {x} vs {y}, {:.2}% apart (limit {:.0}%)",
            aspect_diff(x, y) * 100.0,
            MAX_ASPECT_DIFF * 100.0
        ));
    }
    match metadata_contradiction(a, b) {
        Some(c) => lines.push(format!("metadata: {c}")),
        None => lines.push("metadata: no contradiction".into()),
    }
    lines.push(format!("decision: {verdict}"));
    PairExplanation {
        file_a: a.file,
        file_b: b.file,
        verdict,
        phash_distance: pd,
        dhash_distance: dd,
        display_a: da,
        display_b: db,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::DedupMeta;
    use ps_model::{ContentId, Format, MediaMetadata, PerceptualHash, PhotoClass, RawFormat};

    fn img(id: i64, path: &str) -> DedupInput {
        DedupInput {
            file: FileId(id),
            content: Some(ContentId::full([id as u8; 32])),
            hash: Some(PerceptualHash {
                phash: 0,
                dhash: 0,
                version: PERCEPTUAL_HASH_VERSION,
            }),
            format: Format::Jpeg,
            class: PhotoClass::Photo,
            path: path.into(),
            size: 1,
            mtime_ns: 0,
            meta: DedupMeta {
                width: Some(4000),
                height: Some(3000),
                capture_time: Some(CaptureTime::from_parts(2020, 1, 1, 12, 0, 0)),
                camera_model: Some("X100".into()),
                has_maker_notes: true,
                ..Default::default()
            },
        }
    }

    #[test]
    fn aspect_rule_uses_display_dimensions() {
        assert!(aspect_diff(Dimensions::new(4000, 3000), Dimensions::new(1024, 768)) < 1e-9);
        assert!(aspect_diff(Dimensions::new(4000, 3000), Dimensions::new(4000, 2850)) > 0.02);
        let a = img(1, "a/x.jpg");
        let mut b = img(2, "b/y.jpg");
        b.meta.has_maker_notes = false;
        b.meta.width = Some(768);
        b.meta.height = Some(1024);
        b.meta.orientation = ps_model::Orientation(6);
        assert!(matches!(pair_verdict(&a, &b), PairVerdict::Edge { .. }));
    }

    #[test]
    fn subsecond_only_compared_when_both_present() {
        let mut a = img(1, "a/x.jpg");
        let mut b = img(2, "b/y.jpg");
        b.meta.has_maker_notes = false;
        a.meta.capture_time.as_mut().unwrap().millis = 340;
        a.meta.subsec_present = true;
        assert!(matches!(pair_verdict(&a, &b), PairVerdict::Edge { .. }));
        b.meta.subsec_present = true;
        assert!(matches!(
            pair_verdict(&a, &b),
            PairVerdict::Suggestion {
                reason: Contradiction::SubSecond { .. },
                ..
            }
        ));
    }

    #[test]
    fn two_camera_originals_rule_spares_raw_jpeg_pairs() {
        let a = img(1, "DCIM/IMG_1.JPG");
        let mut b = img(2, "DCIM/IMG_1.DNG");
        b.format = Format::Raw(RawFormat::Dng);
        assert!(matches!(pair_verdict(&a, &b), PairVerdict::Edge { .. }));
        b.path = "Other/IMG_1.DNG".into();
        assert!(matches!(
            pair_verdict(&a, &b),
            PairVerdict::Suggestion {
                reason: Contradiction::TwoCameraOriginals { .. },
                ..
            }
        ));
        let c = img(3, "DCIM/IMG_2.JPG");
        assert!(matches!(
            pair_verdict(&a, &c),
            PairVerdict::Suggestion {
                reason: Contradiction::TwoCameraOriginals { .. },
                ..
            }
        ));
    }

    #[test]
    fn missing_hash_or_junk_class_never_derivative() {
        let a = img(1, "a/x.jpg");
        let mut b = img(2, "b/y.jpg");
        b.class = PhotoClass::Junk;
        assert!(matches!(pair_verdict(&a, &b), PairVerdict::NoMatch { .. }));
        b.class = PhotoClass::Photo;
        b.hash = None;
        assert!(matches!(pair_verdict(&a, &b), PairVerdict::NoMatch { .. }));
    }

    #[test]
    fn subsec_presence_read_from_tags() {
        let mut m = MediaMetadata {
            capture_time: Some(CaptureTime::from_parts(2020, 1, 1, 12, 0, 0)),
            ..Default::default()
        };
        assert!(!DedupMeta::from_media(&m, false).subsec_present);
        m.all_tags
            .push(("Exif".into(), "SubSecTimeOriginal".into(), "0".into()));
        assert!(DedupMeta::from_media(&m, false).subsec_present);
    }

    /// D1: a Canon CR2 read through rawler reports "EOS 70D" while its JPEG says "Canon EOS 70D"; both have
    /// SubSecTimeOriginal 99 (990 ms). They are one capture pair and an edge, not a model contradiction.
    #[test]
    fn raw_jpeg_pair_matches_normalized_camera_model() {
        let mut jpg = img(1, "DCIM/IMG_2180.JPG");
        jpg.meta.camera_make = Some("Canon".into());
        jpg.meta.camera_model = Some("Canon EOS 70D".into());
        jpg.meta.capture_time.as_mut().unwrap().millis = 990;
        jpg.meta.subsec_present = true;
        let mut raw = jpg.clone();
        raw.file = FileId(2);
        raw.content = Some(ContentId::full([2; 32]));
        raw.path = "DCIM/IMG_2180.CR2".into();
        raw.format = Format::Raw(RawFormat::Cr2);
        raw.meta.camera_model = Some("EOS 70D".into());
        raw.meta.camera_make = Some("CANON".into());
        assert!(raw_jpeg_companions(&raw, &jpg, true));
        assert_eq!(metadata_contradiction(&raw, &jpg), None);
        assert!(matches!(pair_verdict(&raw, &jpg), PairVerdict::Edge { .. }));
        // A different model is still a contradiction after normalization.
        raw.meta.camera_model = Some("EOS 80D".into());
        assert!(matches!(
            metadata_contradiction(&raw, &jpg),
            Some(Contradiction::CameraModel { .. })
        ));
    }

    #[test]
    fn camera_model_keys_strip_the_make() {
        let k = |make: &str, model: &str| camera_model_key(Some(make), Some(model));
        assert_eq!(k("Canon", "Canon EOS 70D"), k("Canon", "EOS 70D"));
        assert_eq!(k("NIKON CORPORATION", "NIKON D750"), k("Nikon", "D750"));
        assert_eq!(k("Apple", "iPhone 12"), Some("iphone 12".into()));
        assert_ne!(k("Canon", "EOS 70D"), k("Canon", "EOS 7D"));
        assert_eq!(
            camera_model_key(None, Some("  EOS  70D ")),
            Some("eos 70d".into())
        );
        assert_eq!(camera_model_key(Some("Canon"), None), None);
    }

    /// The sub-second is "present" only when the tag parses and the capture time carries that value.
    #[test]
    fn subsec_presence_is_consistent_with_the_parsed_value() {
        let mut m = MediaMetadata {
            capture_time: Some(CaptureTime::from_parts(2020, 1, 1, 12, 0, 0)),
            ..Default::default()
        };
        m.all_tags
            .push(("Exif".into(), "0x9291".into(), "99".into()));
        // The tag says 990 ms but the capture time was not given it: not a usable sub-second.
        assert!(!DedupMeta::from_media(&m, false).subsec_present);
        m.capture_time.as_mut().unwrap().millis = 990;
        assert!(DedupMeta::from_media(&m, false).subsec_present);
        m.all_tags[0].2 = "  ".into();
        m.capture_time.as_mut().unwrap().millis = 0;
        assert!(!DedupMeta::from_media(&m, false).subsec_present);
    }

    #[test]
    fn explanation_lists_every_rule() {
        let a = img(1, "a/x.jpg");
        let b = img(2, "b/x.jpg");
        let e = explain_pair(&a, &b);
        assert!(e.lines.len() >= 5, "{:?}", e.lines);
        assert!(
            e.lines
                .last()
                .unwrap()
                .starts_with("decision: similar shot only")
        );
    }
}
