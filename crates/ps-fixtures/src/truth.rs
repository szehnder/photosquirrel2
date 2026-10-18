//! `truth.json` (truth_version 1): every generated file with its category, expected class, true group and
//! expected keeper, plus the must-not-merge sets (ADR-0013, ADR-0014, ADR-0015, ADR-0025, ADR-0035).

use std::collections::BTreeMap;

use ps_model::PhotoClass;
use serde::{Deserialize, Serialize};

pub const TRUTH_VERSION: u32 = 1;
pub const TRUTH_FILE: &str = "truth.json";

/// What a file is, as generated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Camera JPEG straight from the camera (includes burst frames, look-alikes, same-scene shots).
    Original,
    /// The DNG half of a RAW+JPEG pair.
    RawOriginal,
    /// An edited crop of an original (its own group; must not merge with the original).
    Crop,
    /// A scanned print (no camera EXIF).
    Scan,
    /// Phone/camera video.
    Video,
    /// Byte-identical copy under the same file name (backup trees).
    ExactCopy,
    /// Byte-identical copy under another name (`IMG_1234 (1).JPG`, `Copy of ...`).
    RenamedCopy,
    /// Downscaled to 1024, 384 or 160 px long edge.
    Resized,
    /// Re-encoded at quality 60-85.
    Recompressed,
    /// The original with its EXIF (APP1) segment removed.
    ExifStripped,
    /// Lossless PNG conversion.
    PngConversion,
    /// A cache thumbnail/preview in a cache folder (`.thumbnails`, Lightroom previews, Photos proxies).
    CacheThumbnail,
    /// Application/website icon (`.ico`).
    Icon,
    /// UI graphic: palette PNG/GIF with transparency.
    UiGraphic,
    /// Tiny web image.
    TinyWeb,
    /// Very short or tiny video.
    JunkVideo,
}

impl Category {
    /// Derivative kinds (members of an original's group that are not the original).
    pub const DERIVATIVES: [Category; 7] = [
        Category::ExactCopy,
        Category::RenamedCopy,
        Category::Resized,
        Category::Recompressed,
        Category::ExifStripped,
        Category::PngConversion,
        Category::CacheThumbnail,
    ];

    /// Junk/graphic files that belong to no photo group and never have a keeper.
    pub fn is_junk(self) -> bool {
        matches!(
            self,
            Category::Icon | Category::UiGraphic | Category::TinyWeb | Category::JunkVideo
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Original => "original",
            Category::RawOriginal => "raw_original",
            Category::Crop => "crop",
            Category::Scan => "scan",
            Category::Video => "video",
            Category::ExactCopy => "exact_copy",
            Category::RenamedCopy => "renamed_copy",
            Category::Resized => "resized",
            Category::Recompressed => "recompressed",
            Category::ExifStripped => "exif_stripped",
            Category::PngConversion => "png_conversion",
            Category::CacheThumbnail => "cache_thumbnail",
            Category::Icon => "icon",
            Category::UiGraphic => "ui_graphic",
            Category::TinyWeb => "tiny_web",
            Category::JunkVideo => "junk_video",
        }
    }
}

/// One generated file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruthFile {
    /// Relative to the corpus root, `/`-separated (`drive-b/Backup 2015/Old Laptop/...`).
    pub path: String,
    /// The simulated drive root (`drive-a`, `drive-b`, `drive-c`).
    pub drive: String,
    pub category: Category,
    /// What the ADR-0015 classifier should say with default settings.
    pub expected_class: PhotoClass,
    /// True group id (`g00042`; junk files get singleton groups `j00007`).
    pub group: String,
    /// The expected keeper of its group (ADR-0014). Exactly one per non-junk group.
    pub keeper: bool,
    /// Member of the group's RAW+JPEG keeper set (both the DNG and its JPEG).
    pub keeper_set: bool,
    /// `ps_model::Format::storage_key` (`jpeg`, `png`, `raw:dng`, `mov`, ...).
    pub format: String,
    /// Display (orientation-applied) dimensions.
    pub width: u32,
    pub height: u32,
    pub size: u64,
    /// Modification time, Unix seconds.
    pub mtime: i64,
    /// BLAKE3 of the file bytes (hex) - the ADR-0011 content identity.
    pub content: String,
    /// Path of the file this one was made from, for derivatives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
}

/// Kinds of must-not-merge sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MustNotMergeKind {
    /// An original and its crop (aspect change >= 5%).
    Crop,
    /// Burst-like near shots of one camera (see [`BurstVariant`]).
    Burst,
    /// Same scene, same camera, minutes apart.
    MinutesApart,
    /// Near-identical shots from different cameras (other model, or same model with another serial).
    DifferentCamera,
}

impl MustNotMergeKind {
    pub const ALL: [MustNotMergeKind; 4] = [
        MustNotMergeKind::Crop,
        MustNotMergeKind::Burst,
        MustNotMergeKind::MinutesApart,
        MustNotMergeKind::DifferentCamera,
    ];
}

/// ADR-0035 burst flavours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BurstVariant {
    /// Frames 0.3-5 s apart.
    Spaced,
    /// Same capture second, different SubSecTimeOriginal.
    SameSecondSubsec,
    /// Same capture second, no sub-seconds, maker notes present, equal dimensions.
    SameSecondNoSubsec,
}

/// Distinct shots whose groups must stay apart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MustNotMerge {
    pub id: String,
    pub kind: MustNotMergeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<BurstVariant>,
    /// The truth groups (one per distinct shot) that must not be merged with each other.
    pub groups: Vec<String>,
    /// The originals' paths, in the same order as `groups`.
    pub originals: Vec<String>,
}

/// The whole truth file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truth {
    pub truth_version: u32,
    pub generator: String,
    pub seed: u64,
    pub preset: String,
    pub drives: Vec<String>,
    /// Sorted by path.
    pub files: Vec<TruthFile>,
    /// Sorted by id.
    pub must_not_merge: Vec<MustNotMerge>,
}

impl Truth {
    pub fn to_json(&self) -> Vec<u8> {
        let mut v = serde_json::to_vec_pretty(self).expect("truth serialises");
        v.push(b'\n');
        v
    }

    pub fn from_json(bytes: &[u8]) -> Result<Truth, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Read `truth.json` from a corpus folder.
    pub fn read(dir: &std::path::Path) -> std::io::Result<Truth> {
        let bytes = std::fs::read(dir.join(TRUTH_FILE))?;
        Truth::from_json(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Group id -> member indices into `files` (sorted by group id).
    pub fn groups(&self) -> BTreeMap<&str, Vec<usize>> {
        let mut g: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, f) in self.files.iter().enumerate() {
            g.entry(f.group.as_str()).or_default().push(i);
        }
        g
    }

    pub fn file(&self, path: &str) -> Option<&TruthFile> {
        self.files
            .binary_search_by(|f| f.path.as_str().cmp(path))
            .ok()
            .map(|i| &self.files[i])
    }

    /// Count of files per category.
    pub fn category_counts(&self) -> BTreeMap<Category, usize> {
        let mut m = BTreeMap::new();
        for f in &self.files {
            *m.entry(f.category).or_insert(0) += 1;
        }
        m
    }

    /// Count of files per expected class.
    pub fn class_counts(&self) -> BTreeMap<PhotoClass, usize> {
        let mut m = BTreeMap::new();
        for f in &self.files {
            *m.entry(f.expected_class).or_insert(0) += 1;
        }
        m
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}
