//! The image sent to a model: a JPEG of at most 1024 px on the long edge, quality 85, downscaled in memory from
//! the 2048 px preview (generated from the source when not cached), ADR-0036. Never the original file's bytes,
//! never its path (spec "Previews only, never originals").

use std::sync::Arc;

use ps_model::ProbeRecord;

/// Long edge of the LLM preview.
pub const LLM_PREVIEW_LONG_EDGE: u32 = 1024;
/// JPEG quality of the LLM preview.
pub const LLM_PREVIEW_QUALITY: u8 = 85;

/// A JPEG preview ready to upload. `Debug` prints dimensions and size only.
#[derive(Clone, PartialEq, Eq)]
pub struct PreviewJpeg {
    bytes: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl std::fmt::Debug for PreviewJpeg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PreviewJpeg({}x{}, {} bytes)",
            self.width,
            self.height,
            self.bytes.len()
        )
    }
}

impl PreviewJpeg {
    /// Wrap JPEG bytes that are already a small preview (tests, cached LLM previews). Fails when the bytes are
    /// not a JPEG or the long edge exceeds [`LLM_PREVIEW_LONG_EDGE`].
    pub fn from_jpeg(bytes: Vec<u8>) -> Result<Self, PreviewError> {
        let sof = ps_media::jpeg::sof_of(&bytes)
            .filter(|_| bytes.starts_with(&[0xFF, 0xD8]))
            .ok_or_else(|| PreviewError::Failed("not a JPEG".into()))?;
        let (width, height) = (u32::from(sof.width), u32::from(sof.height));
        if width.max(height) > LLM_PREVIEW_LONG_EDGE {
            return Err(PreviewError::Failed(format!(
                "{width}x{height} exceeds the {LLM_PREVIEW_LONG_EDGE} px LLM preview"
            )));
        }
        Ok(Self {
            bytes: bytes.into(),
            width,
            height,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Standard base64 of the JPEG, as every wire format embeds it.
    pub fn base64(&self) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }
}

/// What a preview can be made from.
#[derive(Clone, Copy, Debug)]
pub enum PreviewInput<'a> {
    /// The cached 2048 px preview JPEG (already oriented).
    Cached(&'a [u8]),
    /// The source file's bytes, read read-only by the caller, and its probe.
    Source {
        bytes: &'a [u8],
        probe: &'a ProbeRecord,
    },
    /// Neither a cached preview nor the source is available (source volume offline).
    Offline,
}

/// Why no preview was produced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreviewError {
    /// Try again later: the item is deferred, not failed (ADR-0036).
    #[error("deferred: {0}")]
    Deferred(String),
    #[error("no preview: {0}")]
    Failed(String),
}

/// The LLM preview and, when it had to be generated from the source, the new 2048 px preview for the cache.
#[derive(Clone, Debug)]
pub struct Derived {
    pub llm: PreviewJpeg,
    pub generated_large: Option<Vec<u8>>,
}

/// Produce the 1024 px LLM preview.
pub fn derive(input: PreviewInput<'_>) -> Result<Derived, PreviewError> {
    match input {
        PreviewInput::Offline => Err(PreviewError::Deferred("source offline".into())),
        PreviewInput::Cached(jpeg) => Ok(Derived {
            llm: downscale(jpeg)?,
            generated_large: None,
        }),
        PreviewInput::Source { bytes, probe } => {
            let (large, _, _) = ps_media::thumb::make_large_preview(
                bytes,
                probe,
                ps_media::thumb::PREVIEW_LONG_EDGE,
            )
            .map_err(|e| PreviewError::Failed(e.to_string()))?;
            Ok(Derived {
                llm: downscale(&large)?,
                generated_large: Some(large),
            })
        }
    }
}

/// Downscale a (2048 px) preview JPEG to the LLM preview.
fn downscale(jpeg: &[u8]) -> Result<PreviewJpeg, PreviewError> {
    let (img, _) = ps_media::decode::decode_jpeg(jpeg, LLM_PREVIEW_LONG_EDGE)
        .map_err(|e| PreviewError::Failed(e.to_string()))?;
    let small = ps_media::decode::resize_to_long_edge(&img, LLM_PREVIEW_LONG_EDGE);
    let bytes = ps_media::thumb::encode_jpeg(&small, LLM_PREVIEW_QUALITY)
        .map_err(|e| PreviewError::Failed(e.to_string()))?;
    Ok(PreviewJpeg {
        bytes: bytes.into(),
        width: small.width,
        height: small.height,
    })
}
