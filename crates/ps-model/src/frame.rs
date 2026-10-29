//! Frame identity (ADR-0105): which camera frame a file says it is.
//!
//! A converted DNG records the RAW file it came from (`OriginalRawFileName`), and Canon maker notes record the
//! camera's file number (`101-0034`). Two files that record different frames are never one image, whatever their
//! hashes and capture times. A missing identity never matches anything, and a file name is never an identity.

#[cfg(feature = "test-hooks")]
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// The version of the metadata rules the stored metadata of a file was read under (catalog `file.meta_version`,
/// migration 23). 1: frame identity and maker notes in `DNGPrivateData` (ADR-0105). 2: the structure of a RAW file
/// was checked for a cut (ADR-0117): a RAW-format row with real preview pixels stored by an earlier version may hide
/// a file cut after its intact embedded preview. A rescan re-reads the header of unchanged rows below the version
/// they need ([`meta_reread_due`]).
pub const META_LOGIC_VERSION: u8 = 2;

/// The first metadata version, frame identity (ADR-0105): what every row needs.
pub const FRAME_META_VERSION: u8 = 1;

/// Whether an analysed row of `format` stored at metadata version `meta_version` is due a one-time header re-read:
/// below [`FRAME_META_VERSION`] (frame identity, ADR-0105), or a RAW-format row with real preview pixels
/// (`placeholder` false) below [`META_LOGIC_VERSION`] (the structure check, ADR-0117). Version 2 changes nothing
/// for other rows, so they stay current at 1; a RAW placeholder was either judged by this version's decode or is
/// re-read in full by the RAW decoder generation bump (ADR-0115).
pub fn meta_reread_due(format: Option<crate::Format>, placeholder: bool, meta_version: u8) -> bool {
    meta_version < FRAME_META_VERSION
        || (meta_version < META_LOGIC_VERSION
            && !placeholder
            && format.is_some_and(crate::Format::is_raw))
}

/// What a file records about its frame. Each part compares only with the same part of another file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameId {
    /// A DNG's `OriginalRawFileName`, its stem upper-cased (`IMG_0034` for `IMG_0034.CR2`).
    #[serde(rename = "raw", skip_serializing_if = "Option::is_none")]
    pub raw_name: Option<String>,
    /// The maker-note file number, `directory * 10000 + index` (Canon `101-0034` = 1010034).
    #[serde(rename = "num", skip_serializing_if = "Option::is_none")]
    pub file_number: Option<u32>,
}

impl FrameId {
    /// `None` when neither part is known.
    pub fn new(raw_name: Option<String>, file_number: Option<u32>) -> Option<FrameId> {
        (raw_name.is_some() || file_number.is_some()).then_some(FrameId {
            raw_name,
            file_number,
        })
    }

    /// The stem of an `OriginalRawFileName` value, upper-cased: `IMG_0034.CR2` -> `IMG_0034`. Folders (some
    /// writers keep them) and surrounding whitespace are dropped; `None` when nothing is left.
    pub fn raw_stem(value: &str) -> Option<String> {
        let name = value
            .trim()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .trim();
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).trim();
        (!stem.is_empty()).then(|| stem.to_uppercase())
    }

    /// `101-0034` for 1010034.
    pub fn file_number_label(n: u32) -> String {
        format!("{:03}-{:04}", n / 10_000, n % 10_000)
    }

    /// The part both record with different values, as display labels (`(a, b)`), or `None`. The camera's file
    /// number decides when both record it: two DNGs of one frame whose RAW was renamed before one conversion keep
    /// the same file number, so their RAW names are not compared.
    pub fn differs(&self, other: &FrameId) -> Option<(String, String)> {
        if let (Some(a), Some(b)) = (self.file_number, other.file_number) {
            return (a != b).then(|| (Self::file_number_label(a), Self::file_number_label(b)));
        }
        if let (Some(a), Some(b)) = (&self.raw_name, &other.raw_name)
            && a != b
        {
            return Some((a.clone(), b.clone()));
        }
        None
    }

    /// Whether both positively record the same frame (ADR-0107): equal camera file numbers, or, when neither
    /// records a file number, equal `OriginalRawFileName` stems. A stem can repeat after the camera's counter rolls
    /// over, so callers also require the same capture time.
    pub fn same_frame(&self, other: &FrameId) -> bool {
        match (self.file_number, other.file_number) {
            (Some(a), Some(b)) => a == b,
            (None, None) => {
                matches!((&self.raw_name, &other.raw_name), (Some(a), Some(b)) if a == b)
            }
            _ => false,
        }
    }

    /// The catalog's `file.frame_id` text: `raw:IMG_0034 num:101-0034` (the parts present).
    pub fn storage(&self) -> String {
        let mut parts = Vec::new();
        if let Some(r) = &self.raw_name {
            parts.push(format!("raw:{r}"));
        }
        if let Some(n) = self.file_number {
            parts.push(format!("num:{}", Self::file_number_label(n)));
        }
        parts.join(" ")
    }

    /// For people: `101-0034 (IMG_0034)`.
    pub fn label(&self) -> String {
        match (&self.raw_name, self.file_number) {
            (Some(r), Some(n)) => format!("{} ({r})", Self::file_number_label(n)),
            (Some(r), None) => r.clone(),
            (None, Some(n)) => Self::file_number_label(n),
            (None, None) => String::new(),
        }
    }
}

#[cfg(feature = "test-hooks")]
static FRAME_IDENTITY: AtomicBool = AtomicBool::new(true);

/// Whether frame identities, maker notes in `DNGPrivateData` and the RAW-original rule are in force (always, except
/// in tests that group a corpus the way builds before ADR-0105 did, with the `test-hooks` feature).
#[cfg(feature = "test-hooks")]
pub fn frame_identity_enabled() -> bool {
    FRAME_IDENTITY.load(Ordering::Relaxed)
}

/// Whether frame identities, maker notes in `DNGPrivateData` and the RAW-original rule are in force: always in a
/// build without the `test-hooks` feature.
#[cfg(not(feature = "test-hooks"))]
pub const fn frame_identity_enabled() -> bool {
    true
}

/// Test hook (feature `test-hooks`, enabled only from dev-dependencies): turn frame identity off (metadata reading
/// in `ps-media` and the frame and RAW-original rules in `ps-dedup`) to reproduce the grouping of builds before
/// ADR-0105. Process-wide: only for a test binary of its own.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn set_frame_identity_enabled(enabled: bool) {
    FRAME_IDENTITY.store(enabled, Ordering::Relaxed);
}

#[cfg(feature = "test-hooks")]
static STRUCTURE_CHECK: AtomicBool = AtomicBool::new(true);

/// Whether a RAW file's structure is checked for a cut (ADR-0115): always, except in tests that catalog a cut file
/// the way builds before ADR-0115 did, with the `test-hooks` feature.
#[cfg(feature = "test-hooks")]
pub fn structure_check_enabled() -> bool {
    STRUCTURE_CHECK.load(Ordering::Relaxed)
}

/// Whether a RAW file's structure is checked for a cut (ADR-0115): always in a build without the `test-hooks`
/// feature.
#[cfg(not(feature = "test-hooks"))]
pub const fn structure_check_enabled() -> bool {
    true
}

/// Test hook (feature `test-hooks`, enabled only from dev-dependencies): turn the RAW structure check off
/// (`ps_media::raw_check`) to write a catalog as 0.3.0 did, where a RAW cut after its intact preview had preview
/// pixels and hashes. Process-wide: only for a test binary of its own.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn set_structure_check_enabled(enabled: bool) {
    STRUCTURE_CHECK.store(enabled, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_rows_are_due_a_header_re_read() {
        use crate::{Format, RawFormat};
        let cr2 = Some(Format::Raw(RawFormat::Cr2));
        let jpeg = Some(Format::Jpeg);
        // Version 0: everything (frame identity).
        assert!(meta_reread_due(jpeg, false, 0));
        assert!(meta_reread_due(cr2, true, 0));
        // Version 1: only a RAW with preview pixels (the structure check).
        assert!(meta_reread_due(cr2, false, 1));
        assert!(!meta_reread_due(cr2, true, 1), "a RAW placeholder");
        assert!(!meta_reread_due(jpeg, false, 1));
        assert!(!meta_reread_due(None, false, 1));
        // Current.
        assert!(!meta_reread_due(cr2, false, META_LOGIC_VERSION));
    }

    #[test]
    fn stems_labels_and_differences() {
        assert_eq!(
            FrameId::raw_stem("IMG_0034.CR2").as_deref(),
            Some("IMG_0034")
        );
        assert_eq!(
            FrameId::raw_stem(" img_0034.cr2 ").as_deref(),
            Some("IMG_0034")
        );
        assert_eq!(
            FrameId::raw_stem("C:\\DCIM\\_MG_0001.CR2").as_deref(),
            Some("_MG_0001")
        );
        assert_eq!(FrameId::raw_stem(" "), None);
        assert_eq!(FrameId::file_number_label(1_010_034), "101-0034");
        let a = FrameId::new(Some("IMG_0034".into()), Some(1_010_034)).expect("a");
        let b = FrameId::new(None, Some(1_010_035)).expect("b");
        let c = FrameId::new(Some("IMG_0035".into()), None).expect("c");
        assert_eq!(a.differs(&b), Some(("101-0034".into(), "101-0035".into())));
        assert_eq!(a.differs(&c), Some(("IMG_0034".into(), "IMG_0035".into())));
        // Different kinds never compare.
        assert_eq!(b.differs(&c), None);
        assert_eq!(a.differs(&a.clone()), None);
        // Equal file numbers decide: a renamed RAW's DNG is the same frame.
        let renamed = FrameId::new(Some("2020-04-20_0001".into()), Some(1_010_034)).expect("r");
        assert_eq!(a.differs(&renamed), None);
        assert_eq!(FrameId::new(None, None), None);
        // Same frame only on positive agreement.
        assert!(a.same_frame(&renamed));
        assert!(!a.same_frame(&b));
        assert!(!b.same_frame(&c), "a file number and a stem never agree");
        assert!(c.same_frame(&c.clone()));
        assert!(
            !a.same_frame(&c),
            "one file number missing: the stems alone do not decide"
        );
        assert_eq!(a.storage(), "raw:IMG_0034 num:101-0034");
        assert_eq!(a.label(), "101-0034 (IMG_0034)");
        let json = serde_json::to_string(&a).expect("json");
        assert_eq!(serde_json::from_str::<FrameId>(&json).expect("back"), a);
    }
}
