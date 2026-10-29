//! Format sniffing, metadata (EXIF, QuickTime), decoding, thumbnails and the photo classifier (ADR-0010, 0015, 0017).
//!
//! Everything here is a pure function over bytes (plus read-only file helpers in [`fs`]); the caller (the
//! scanner) owns I/O scheduling. No function panics on malformed input: problems become error records or
//! warnings.

pub mod apple;
pub mod av1;
pub mod bmff;
pub mod classify;
pub mod date;
pub mod decode;
pub mod decoders;
pub mod frame;
pub mod fs;
pub mod jpeg;
pub mod metadata;
pub mod poster;
pub mod preview_cache;
pub mod probe;
pub mod quality;
pub mod raw_check;
pub mod thumb;
pub mod tiff;
pub mod tiff_decode;
pub mod util;
pub mod video;

pub use classify::{FolderHints, class_facts, classify, classify_facts, classify_with};
pub use date::{
    DATE_LOGIC_VERSION, DateContext, EmbeddedDates, FixedZone, LocalZone, RuleZone,
    resolve_capture_date,
};
pub use decode::{DecodeError, Decoded, RgbImage, decode_for_analysis};
pub use decoders::{Decoders, StillDecoder};
pub use frame::{FrameFacts, read_frame_facts, reread_frame_facts};
pub use fs::{CountingReader, open_read_only, read_all};
pub use metadata::extract_metadata;
pub use probe::{
    PROBE_LIMIT, needs_directory_reads, probe_buffer, probe_bytes, probe_path, probe_reader,
    probe_seekable, probe_with_directory_reads, sniff, tiff_metadata_block,
};
pub use ps_model::META_LOGIC_VERSION;
pub use thumb::{analyze_still, analyze_still_held, analyze_still_with, make_thumbnail};

/// The `env_logger` filter both binaries start with when `RUST_LOG` is unset (`quiet-decoder-logs`); a set
/// `RUST_LOG` replaces it whole (`RUST_LOG=rawler=warn` brings rawler's diagnostics back for a bug report).
/// - `warn`: PhotoSquirrel's own crates (and every other dependency) log warnings and errors.
/// - `nom_exif=error`: nom-exif warns about every odd but harmless EXIF tag it skips.
/// - `rawler=error`: rawler warns per file about tags it skipped and lenses missing from its table ("No lens
///   definition found in database"), addressed to its maintainers. What such a file means for the user (no RAW
///   metadata, no preview) is reported by our own results, never only through the log.
pub const LOG_DEFAULT_FILTER: &str = "warn,nom_exif=error,rawler=error";

/// The decoder report of this machine with the poster-frame entry for `poster_tool` (the configured ffmpeg
/// path; callers wanting the PATH default resolve it with [`poster::ffmpeg_on_path`] first), for `doctor`, the
/// scan summary and the inspector (ADR-0089).
pub fn decoder_report(
    decoders: &Decoders,
    poster_tool: Option<&std::path::Path>,
) -> ps_model::DecoderReport {
    let mut r = decoders.report().clone();
    if r.entry(ps_model::DecoderFormat::Avif).is_none() {
        r.set(av1::report_entry());
    }
    let poster = poster_tool.filter(|p| p.is_file());
    r.set(ps_model::DecoderEntry {
        format: ps_model::DecoderFormat::PosterFrames,
        available: poster.is_some(),
        backend: poster.map(|p| p.display().to_string()),
        version: None,
        reason: poster
            .is_none()
            .then(|| "video poster frames unavailable".to_owned()),
        fix: poster.is_none().then(|| ps_model::Fix {
            summary: "Install ffmpeg".into(),
            steps: vec![ps_model::FixStep {
                text: "Install ffmpeg and set its path in Settings (or put it on PATH)".into(),
                ..Default::default()
            }],
            restart: false,
        }),
        assembly: None,
    });
    r
}
