//! Frame identity (ADR-0105): which camera frame a file says it is.
//!
//! A converted DNG records the RAW file it came from (`OriginalRawFileName`), and Canon maker notes record the
//! camera's file number (`101-0034`). Two files that record different frames are never one image, whatever their
//! hashes and capture times. A missing identity never matches anything, and a file name is never an identity.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// The version of the metadata rules the stored metadata of a file was read under (catalog `file.meta_version`,
/// migration 23). 1: frame identity and maker notes in `DNGPrivateData` (ADR-0105). A rescan re-reads the header
/// of unchanged RAW-format rows below it.
pub const META_LOGIC_VERSION: u8 = 1;

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

static FRAME_IDENTITY: AtomicBool = AtomicBool::new(true);

/// Whether frame identities, maker notes in `DNGPrivateData` and the RAW-original rule are in force (always, except
/// in tests that group a corpus the way builds before ADR-0105 did).
pub fn frame_identity_enabled() -> bool {
    FRAME_IDENTITY.load(Ordering::Relaxed)
}

/// Test hook: turn frame identity off (metadata reading in `ps-media` and the frame and RAW-original rules in
/// `ps-dedup`) to reproduce the grouping of builds before ADR-0105. Process-wide: only for a test binary of its own.
#[doc(hidden)]
pub fn set_frame_identity_enabled(enabled: bool) {
    FRAME_IDENTITY.store(enabled, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(a.storage(), "raw:IMG_0034 num:101-0034");
        assert_eq!(a.label(), "101-0034 (IMG_0034)");
        let json = serde_json::to_string(&a).expect("json");
        assert_eq!(serde_json::from_str::<FrameId>(&json).expect("back"), a);
    }
}
