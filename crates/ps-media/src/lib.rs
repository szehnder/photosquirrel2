//! Format sniffing, metadata (EXIF, QuickTime), decoding, thumbnails and the photo classifier (ADR-0010, 0015, 0017).
//!
//! Everything here is a pure function over bytes (plus read-only file helpers in [`fs`]); the caller (the
//! scanner) owns I/O scheduling. No function panics on malformed input: problems become error records or
//! warnings.

pub mod apple;
pub mod bmff;
pub mod classify;
pub mod decode;
pub mod fs;
pub mod heic;
pub mod jpeg;
pub mod metadata;
pub mod poster;
pub mod preview_cache;
pub mod probe;
pub mod quality;
pub mod thumb;
pub mod tiff;
pub mod util;
pub mod video;

pub use classify::{FolderHints, class_facts, classify, classify_facts, classify_with};
pub use decode::{DecodeError, Decoded, RgbImage, decode_for_analysis};
pub use fs::{CountingReader, open_read_only, read_all};
pub use metadata::extract_metadata;
pub use probe::{PROBE_LIMIT, probe_bytes, probe_path, probe_reader, sniff};
pub use thumb::{analyze_still, make_thumbnail};

/// This build can decode HEIC/HEIF/AVIF stills (cargo feature `heic`, ADR-0017).
pub const HEIC_SUPPORTED: bool = cfg!(feature = "heic");

/// One optional decoder and how to enable it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecoderStatus {
    pub name: &'static str,
    pub available: bool,
    /// What it is, and how to enable it when unavailable.
    pub detail: String,
}

/// Which optional decoders this build and configuration offer (ADR-0017), for `doctor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub heic: bool,
    /// The configured poster-frame tool, when it exists.
    pub poster_frames: Option<std::path::PathBuf>,
    pub decoders: Vec<DecoderStatus>,
}

/// Report decoder availability. `poster_tool` is the configured ffmpeg path (callers wanting the PATH
/// default resolve it with [`poster::ffmpeg_on_path`] first).
pub fn capabilities(poster_tool: Option<&std::path::Path>) -> Capabilities {
    let poster = poster_tool
        .filter(|p| p.is_file())
        .map(std::path::Path::to_path_buf);
    let decoders = vec![
        DecoderStatus {
            name: "heic",
            available: HEIC_SUPPORTED,
            detail: if HEIC_SUPPORTED {
                "HEIC/HEIF/AVIF stills via libheif".into()
            } else {
                "HEIC/HEIF/AVIF previews unavailable: this build has no HEIC decoding (the prebuilt packages do \
                 not include it yet). Build from source with libheif 1.17+ and `--features heic` to enable it"
                    .into()
            },
        },
        DecoderStatus {
            name: "poster_frames",
            available: poster.is_some(),
            detail: match &poster {
                Some(p) => format!("video poster frames via {}", p.display()),
                None => "video poster frames unavailable: install ffmpeg and set its path in settings (or put it on \
                         PATH)"
                    .into(),
            },
        },
    ];
    Capabilities {
        heic: HEIC_SUPPORTED,
        poster_frames: poster,
        decoders,
    }
}
