//! Colour spaces of stills and their conversion to sRGB for display (ADR-0113).
//!
//! The probe records a still's [`ColourSpace`] from bytes it already reads ([`decide`]: an embedded ICC profile, a
//! DNG's `PreviewColorSpace`, EXIF `InteropIndex` `R03`, the Canon maker note). [`to_srgb`] converts a display image
//! (a 384 px thumbnail, a 2048 px large preview) after it was resized; the analysis decode, and so every perceptual
//! hash, is never converted. sRGB images are returned untouched, so their thumbnails stay byte-identical.
//!
//! Transforms come from `moxcms` (pure Rust): one each for the built-in Adobe RGB (1998) and Display P3 profiles,
//! built once, and the [`PROFILE_CACHE`] most recent embedded profiles, keyed by their bytes.

use std::sync::{Arc, Mutex, OnceLock};

use moxcms::{ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformOptions};
use ps_model::{ByteRange, ColourSpace, ProbeRecord};

use crate::decode::RgbImage;
use crate::jpeg::IccChunk;

type Transform = Arc<moxcms::Transform8BitExecutor>;

/// How many embedded profiles keep their transform.
pub const PROFILE_CACHE: usize = 8;

/// The colour space of an image to display, with the ICC profile bytes when the answer came from one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColourSource {
    pub space: ColourSpace,
    /// The embedded profile (not sRGB); the conversion uses it rather than the named space it matched.
    pub icc: Option<Arc<[u8]>>,
}

impl ColourSource {
    /// sRGB: no conversion.
    pub fn srgb() -> ColourSource {
        ColourSource::default()
    }

    /// The colour of the still in `bytes` (the file, or at least its probe window) as `probe` recorded it.
    pub fn of(bytes: &[u8], probe: &ProbeRecord) -> ColourSource {
        if probe.colour.is_srgb() {
            return ColourSource::srgb();
        }
        let icc = if probe.icc.is_empty() {
            None
        } else {
            let mut v = Vec::new();
            let mut whole = true;
            for r in &probe.icc {
                match r.slice(bytes) {
                    Some(s) => v.extend_from_slice(s),
                    None => whole = false,
                }
            }
            whole.then(|| Arc::from(v))
        };
        ColourSource {
            space: probe.colour.clone(),
            icc,
        }
    }
}

/// What the probe's tags say about the colour space, besides an ICC profile ([`decide`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ColourTags {
    /// EXIF `ColorSpace` (0xA001): 1 sRGB, 0xFFFF uncalibrated.
    pub exif: Option<u32>,
    /// The Interop IFD's `InteropIndex` (0x0001) is `R03` (DCF's Adobe RGB option file).
    pub r03: bool,
    /// The Canon maker note's `ColorSpace` (0x00B4): 1 sRGB, 2 Adobe RGB.
    pub canon: Option<u16>,
    /// A DNG preview IFD's `PreviewColorSpace` (0xC71A): 2 sRGB, 3 Adobe RGB.
    pub dng_preview: Option<u32>,
}

/// D1's order: the stream's own ICC profile, a DNG's `PreviewColorSpace`, EXIF (`R03` Adobe RGB, `ColorSpace` 1
/// sRGB; 0xFFFF alone is no evidence), the Canon maker note, else sRGB.
pub(crate) fn decide(icc: Option<ColourSpace>, tags: &ColourTags) -> ColourSpace {
    if let Some(c) = icc {
        return c;
    }
    match tags.dng_preview {
        Some(2) => return ColourSpace::Srgb,
        Some(3) => return ColourSpace::AdobeRgb,
        _ => {}
    }
    if tags.r03 {
        return ColourSpace::AdobeRgb;
    }
    if tags.exif == Some(1) {
        return ColourSpace::Srgb;
    }
    if tags.canon == Some(2) {
        return ColourSpace::AdobeRgb;
    }
    ColourSpace::Srgb
}

/// The payload ranges of a JPEG's ICC chunks in sequence order (absolute, `base` added), when they form one profile:
/// every chunk 1..=count once, with one count. `None` for no chunks; `Err` for a malformed set.
pub(crate) fn icc_ranges(chunks: &[IccChunk], base: u64) -> Result<Option<Vec<ByteRange>>, String> {
    let Some(first) = chunks.first() else {
        return Ok(None);
    };
    let count = first.count;
    if count == 0 || chunks.len() != usize::from(count) || chunks.iter().any(|c| c.count != count) {
        return Err("ICC profile chunks are incomplete or inconsistent".into());
    }
    let mut out = Vec::with_capacity(chunks.len());
    for seq in 1..=count {
        let c = chunks
            .iter()
            .find(|c| c.seq == seq)
            .ok_or_else(|| format!("ICC profile chunk {seq} of {count} is missing"))?;
        out.push(ByteRange {
            offset: base + c.offset as u64,
            len: c.len as u64,
        });
    }
    Ok(Some(out))
}

/// The text of a profile's description tag.
fn description(p: &ColorProfile) -> String {
    match &p.description {
        Some(moxcms::ProfileText::PlainString(s)) => s.clone(),
        Some(moxcms::ProfileText::Localizable(l)) => l
            .iter()
            .find(|s| s.language == "en")
            .or_else(|| l.first())
            .map(|s| s.value.clone())
            .unwrap_or_default(),
        Some(moxcms::ProfileText::Description(d)) => d.ascii_string.clone(),
        None => String::new(),
    }
    .trim_matches(char::from(0))
    .trim()
    .to_owned()
}

/// A transform from `src` to sRGB: perceptual intent, relative colorimetric when the profile has no perceptual
/// table (moxcms refuses the intent then).
fn transform_from(src: &ColorProfile) -> Option<Transform> {
    let dst = ColorProfile::new_srgb();
    let make = |intent| {
        src.create_transform_8bit(
            Layout::Rgb,
            &dst,
            Layout::Rgb,
            TransformOptions {
                rendering_intent: intent,
                ..TransformOptions::default()
            },
        )
    };
    make(RenderingIntent::Perceptual)
        .or_else(|_| make(RenderingIntent::RelativeColorimetric))
        .ok()
}

fn builtin(space: &ColourSpace) -> Option<Transform> {
    static ADOBE: OnceLock<Option<Transform>> = OnceLock::new();
    static P3: OnceLock<Option<Transform>> = OnceLock::new();
    match space {
        ColourSpace::AdobeRgb => ADOBE
            .get_or_init(|| transform_from(&ColorProfile::new_adobe_rgb()))
            .clone(),
        ColourSpace::DisplayP3 => P3
            .get_or_init(|| transform_from(&ColorProfile::new_display_p3()))
            .clone(),
        _ => None,
    }
}

/// Samples that tell colour spaces apart: every combination of five levels per channel.
fn samples() -> Vec<u8> {
    const LEVELS: [u8; 5] = [0, 64, 128, 192, 255];
    let mut v = Vec::with_capacity(125 * 3);
    for r in LEVELS {
        for g in LEVELS {
            for b in LEVELS {
                v.extend_from_slice(&[r, g, b]);
            }
        }
    }
    v
}

/// Whether two transforms (`None`: the identity) agree within 2 code values on every sample.
fn same_as(t: &Transform, other: Option<&Transform>) -> bool {
    let src = samples();
    let mut a = vec![0u8; src.len()];
    if t.transform(&src, &mut a).is_err() {
        return false;
    }
    let b = match other {
        None => src,
        Some(o) => {
            let mut b = vec![0u8; src.len()];
            if o.transform(&src, &mut b).is_err() {
                return false;
            }
            b
        }
    };
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 2)
}

/// A parsed embedded profile: what it is called and its transform to sRGB (`None` for sRGB itself).
#[derive(Clone)]
struct Classified {
    space: ColourSpace,
    transform: Option<Transform>,
}

/// The most recent embedded profiles (most recent first), keyed by their bytes.
static PROFILES: Mutex<Vec<(Arc<[u8]>, Classified)>> = Mutex::new(Vec::new());

fn cached(icc: &[u8]) -> Option<Classified> {
    let mut list = PROFILES.lock().unwrap_or_else(|e| e.into_inner());
    let i = list.iter().position(|(k, _)| &k[..] == icc)?;
    let hit = list.remove(i);
    let c = hit.1.clone();
    list.insert(0, hit);
    Some(c)
}

fn remember(icc: &[u8], c: &Classified) {
    let mut list = PROFILES.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|(k, _)| &k[..] != icc);
    list.insert(0, (Arc::from(icc), c.clone()));
    list.truncate(PROFILE_CACHE);
}

/// Parse and classify an embedded profile. `Ok(None)` for a profile of another kind than RGB (a grey or CMYK
/// profile: the decoders hand us RGB, so it does not apply); `Err` for a profile moxcms rejects.
fn classify(icc: &[u8]) -> Result<Option<Classified>, String> {
    if let Some(c) = cached(icc) {
        return Ok(Some(c));
    }
    let p = crate::util::contain(|| ColorProfile::new_from_slice(icc))
        .ok_or_else(|| "the ICC profile could not be parsed".to_owned())?
        .map_err(|e| format!("the ICC profile was rejected ({e})"))?;
    if p.color_space != DataColorSpace::Rgb {
        return Ok(None);
    }
    let desc = description(&p);
    let classified = if desc.to_ascii_lowercase().contains("srgb") {
        Classified {
            space: ColourSpace::Srgb,
            transform: None,
        }
    } else {
        let t = crate::util::contain(|| transform_from(&p))
            .flatten()
            .ok_or_else(|| "no transform could be built from the ICC profile".to_owned())?;
        let space = if same_as(&t, None) {
            ColourSpace::Srgb
        } else if builtin(&ColourSpace::AdobeRgb).is_some_and(|b| same_as(&t, Some(&b))) {
            ColourSpace::AdobeRgb
        } else if builtin(&ColourSpace::DisplayP3).is_some_and(|b| same_as(&t, Some(&b))) {
            ColourSpace::DisplayP3
        } else {
            ColourSpace::Profile { description: desc }
        };
        Classified {
            transform: (!space.is_srgb()).then_some(t),
            space,
        }
    };
    remember(icc, &classified);
    Ok(Some(classified))
}

/// The colour space an embedded profile stands for ([`classify`]); `Ok(None)` when it does not apply.
pub(crate) fn profile_space(icc: &[u8]) -> Result<Option<ColourSpace>, String> {
    Ok(classify(icc)?.map(|c| c.space))
}

/// The transform that brings `src` to sRGB; `None` for sRGB (and for a colour space without a transform).
fn transform_for(src: &ColourSource) -> Option<Transform> {
    if src.space.is_srgb() {
        return None;
    }
    if let Some(icc) = &src.icc
        && let Ok(Some(c)) = classify(icc)
    {
        return c.transform;
    }
    builtin(&src.space)
}

/// Convert a display image to sRGB, in place. sRGB (and the test hook off) returns without touching a pixel.
pub fn to_srgb(img: &mut RgbImage, src: &ColourSource) {
    if src.space.is_srgb() || !ps_model::colour_conversion_enabled() {
        return;
    }
    let Some(t) = transform_for(src) else {
        return;
    };
    let mut out = vec![0u8; img.data.len()];
    if crate::util::contain(|| t.transform(&img.data, &mut out)).is_some_and(|r| r.is_ok()) {
        img.data = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(seq: u8, count: u8, offset: usize) -> IccChunk {
        IccChunk {
            seq,
            count,
            offset,
            len: 10,
        }
    }

    #[test]
    fn detection_order() {
        let none = ColourTags::default();
        assert_eq!(decide(None, &none), ColourSpace::Srgb);
        // 0xFFFF alone says only "not sRGB".
        let uncal = ColourTags {
            exif: Some(0xFFFF),
            ..none.clone()
        };
        assert_eq!(decide(None, &uncal), ColourSpace::Srgb);
        let r03 = ColourTags {
            r03: true,
            ..uncal.clone()
        };
        assert_eq!(decide(None, &r03), ColourSpace::AdobeRgb);
        // The profile comes first.
        assert_eq!(
            decide(Some(ColourSpace::DisplayP3), &r03),
            ColourSpace::DisplayP3
        );
        // EXIF sRGB wins over the maker note; the maker note answers when EXIF does not.
        let canon = ColourTags {
            canon: Some(2),
            ..uncal.clone()
        };
        assert_eq!(decide(None, &canon), ColourSpace::AdobeRgb);
        let canon_srgb_exif = ColourTags {
            exif: Some(1),
            ..canon.clone()
        };
        assert_eq!(decide(None, &canon_srgb_exif), ColourSpace::Srgb);
        // A DNG's preview colour space comes before EXIF.
        let dng = ColourTags {
            dng_preview: Some(2),
            ..r03.clone()
        };
        assert_eq!(decide(None, &dng), ColourSpace::Srgb);
        let dng3 = ColourTags {
            dng_preview: Some(3),
            exif: Some(1),
            ..none
        };
        assert_eq!(decide(None, &dng3), ColourSpace::AdobeRgb);
    }

    #[test]
    fn icc_chunks_in_sequence_order() {
        let r = icc_ranges(&[chunk(2, 2, 100), chunk(1, 2, 20)], 5)
            .unwrap()
            .unwrap();
        assert_eq!(r[0].offset, 25);
        assert_eq!(r[1].offset, 105);
        assert!(icc_ranges(&[chunk(1, 2, 20)], 0).is_err());
        assert!(icc_ranges(&[chunk(1, 2, 20), chunk(1, 2, 40)], 0).is_err());
        assert_eq!(icc_ranges(&[], 0), Ok(None));
    }

    #[test]
    fn profiles_are_classified_by_what_they_do() {
        let enc = |p: ColorProfile| p.encode().unwrap();
        assert_eq!(
            profile_space(&enc(ColorProfile::new_srgb())),
            Ok(Some(ColourSpace::Srgb))
        );
        assert_eq!(
            profile_space(&enc(ColorProfile::new_adobe_rgb())),
            Ok(Some(ColourSpace::AdobeRgb))
        );
        assert_eq!(
            profile_space(&enc(ColorProfile::new_display_p3())),
            Ok(Some(ColourSpace::DisplayP3))
        );
        // An sRGB profile under another name is still sRGB.
        let mut renamed = ColorProfile::new_srgb();
        renamed.description = Some(moxcms::ProfileText::PlainString("Camera RGB".into()));
        assert_eq!(profile_space(&enc(renamed)), Ok(Some(ColourSpace::Srgb)));
        let mut pro = ColorProfile::new_pro_photo_rgb();
        pro.description = Some(moxcms::ProfileText::PlainString("Wide".into()));
        assert_eq!(
            profile_space(&enc(pro)),
            Ok(Some(ColourSpace::Profile {
                description: "Wide".into()
            }))
        );
        assert!(profile_space(b"not a profile at all, just bytes").is_err());
    }

    #[test]
    fn srgb_is_never_touched() {
        let mut img = RgbImage {
            width: 1,
            height: 1,
            data: vec![40, 170, 60],
        };
        to_srgb(&mut img, &ColourSource::srgb());
        assert_eq!(img.data, vec![40, 170, 60]);
        to_srgb(
            &mut img,
            &ColourSource {
                space: ColourSpace::AdobeRgb,
                icc: None,
            },
        );
        assert_ne!(img.data, vec![40, 170, 60]);
    }
}
