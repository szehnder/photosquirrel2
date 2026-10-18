//! HEIC/HEIF/AVIF stills through libheif, behind the `heic` cargo feature (ADR-0017). Without the feature
//! every call returns [`DecodeError::HeicUnavailable`] and callers fall back to "preview unavailable".

use crate::decode::{DecodeError, RgbImage};

/// Decode the primary image, or (with `thumb_min_long`) the largest embedded thumbnail whose long edge is
/// at least that many pixels when one exists. libheif applies the HEIF transformations (irot/imir).
#[cfg(feature = "heic")]
pub fn decode(bytes: &[u8], thumb_min_long: Option<u32>) -> Result<RgbImage, DecodeError> {
    crate::util::contain(|| decode_inner(bytes, thumb_min_long))
        .unwrap_or_else(|| Err(DecodeError::Failed("libheif failure".into())))
}

#[cfg(feature = "heic")]
fn decode_inner(bytes: &[u8], thumb_min_long: Option<u32>) -> Result<RgbImage, DecodeError> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
    let err = |e: libheif_rs::HeifError| DecodeError::Failed(e.to_string());
    let lib = LibHeif::new();
    let ctx = HeifContext::read_from_bytes(bytes).map_err(err)?;
    let mut handle = ctx.primary_image_handle().map_err(err)?;
    if let Some(min) = thumb_min_long {
        let mut ids = vec![0; handle.number_of_thumbnails().min(16)];
        let n = handle.thumbnail_ids(&mut ids);
        let best = ids[..n]
            .iter()
            .filter_map(|id| handle.thumbnail(*id).ok())
            .filter(|t| t.width().max(t.height()) >= min)
            .min_by_key(|t| t.width().max(t.height()));
        if let Some(t) = best {
            handle = t;
        }
    }
    let img = lib
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(err)?;
    let planes = img.planes();
    let p = planes
        .interleaved
        .ok_or_else(|| DecodeError::Failed("no interleaved RGB plane".into()))?;
    let (w, h, stride) = (p.width, p.height, p.stride);
    let mut data = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
        let row = p
            .data
            .get(y * stride..y * stride + w as usize * 3)
            .ok_or_else(|| DecodeError::Failed("short HEIF plane".into()))?;
        data.extend_from_slice(row);
    }
    Ok(RgbImage {
        width: w,
        height: h,
        data,
    })
}

#[cfg(not(feature = "heic"))]
pub fn decode(_bytes: &[u8], _thumb_min_long: Option<u32>) -> Result<RgbImage, DecodeError> {
    Err(DecodeError::HeicUnavailable)
}
