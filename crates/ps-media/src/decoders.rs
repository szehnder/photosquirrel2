//! The decoder registry (ADR-0089): decoders that are not built into ps-media are passed in, so whether a
//! HEIC decodes is a property of the machine (detected at run time by `ps-sysdecode`), not of the build. AVIF
//! is always decoded by the bundled AV1 decoder ([`crate::av1`], ADR-0090) and never goes through the registry.

use std::sync::Arc;

pub use ps_model::{
    DecoderEntry, DecoderFormat, DecoderKey, DecoderReport, Fix, FixStep, OsSummary,
    PlaceholderReason,
};

use crate::decode::{DecodeError, RgbImage};

/// A decoder for one format family that is not built into ps-media (HEIC/HEIF today).
pub trait StillDecoder: Send + Sync {
    /// Backend name for reports ("ImageIO", "WIC", "libheif 1.17.6").
    fn name(&self) -> &str;
    /// Decode the primary image to at least `min_long` px on the long edge when the source is that large
    /// (a backend may return the full size), upright (HEIF irot/imir applied) if [`Self::applies_transforms`].
    fn decode(&self, bytes: &[u8], min_long: u32) -> Result<RgbImage, DecodeError>;
    /// The backend applies the container's rotation and mirroring itself.
    fn applies_transforms(&self) -> bool;
}

/// The decoders of this machine and what detection found.
#[derive(Clone, Default)]
pub struct Decoders {
    heif: Option<Arc<dyn StillDecoder>>,
    report: DecoderReport,
}

impl std::fmt::Debug for Decoders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoders")
            .field("heif", &self.heif.as_ref().map(|d| d.name().to_owned()))
            .field("report", &self.report)
            .finish()
    }
}

impl Decoders {
    /// No HEIC decoder (core tests, and machines without one). The report says nothing about why; detection
    /// fills it in.
    pub fn none() -> Decoders {
        let mut report = DecoderReport::default();
        report.set(DecoderEntry {
            format: DecoderFormat::Heic,
            available: false,
            backend: None,
            version: None,
            reason: Some("no HEIC decoder on this system".into()),
            fix: None,
            assembly: None,
        });
        report.set(crate::av1::report_entry());
        Decoders { heif: None, report }
    }

    /// A registry whose HEIC entry is `heif`.
    pub fn with_heif(heif: Arc<dyn StillDecoder>) -> Decoders {
        let mut d = Decoders::none();
        d.report.set(DecoderEntry {
            format: DecoderFormat::Heic,
            available: true,
            backend: Some(heif.name().to_owned()),
            version: None,
            reason: None,
            fix: None,
            assembly: None,
        });
        d.heif = Some(heif);
        d
    }

    /// A registry from detection: the decoder (if any) and the full report.
    pub fn from_parts(heif: Option<Arc<dyn StillDecoder>>, report: DecoderReport) -> Decoders {
        Decoders { heif, report }
    }

    pub fn heif(&self) -> Option<&Arc<dyn StillDecoder>> {
        self.heif.as_ref()
    }

    pub fn report(&self) -> &DecoderReport {
        &self.report
    }

    pub fn report_mut(&mut self) -> &mut DecoderReport {
        &mut self.report
    }

    /// The backend that decodes `key` on this machine, as a decode-failure record names it ("libheif 1.23.3",
    /// "ImageIO", "rav1d 1.1"); `None` when there is none (ADR-0098).
    pub fn backend(&self, key: DecoderKey) -> Option<String> {
        match key {
            // The backend's name, with the version detection found when the name does not carry it (an
            // ImageIO or WIC update is a new backend too).
            DecoderKey::Heif => self.heif.as_ref().map(|d| {
                let name = d.name();
                match self
                    .report
                    .entry(DecoderFormat::Heic)
                    .and_then(|e| e.version.as_deref())
                {
                    Some(v) if !name.contains(v) => format!("{name} {v}"),
                    _ => name.to_owned(),
                }
            }),
            DecoderKey::Avif => Some(crate::av1::BACKEND.to_owned()),
        }
    }

    /// Files waiting for `key` can be decoded now.
    pub fn can_decode(&self, key: DecoderKey) -> bool {
        match key {
            DecoderKey::Heif => self.heif.is_some(),
            DecoderKey::Avif => true,
        }
    }

    /// Decode a HEIC/HEIF through the registry, or [`DecodeError::DecoderMissing`]. Panics in the backend are
    /// contained.
    pub fn decode_heif(&self, bytes: &[u8], min_long: u32) -> Result<RgbImage, DecodeError> {
        let d = self
            .heif
            .as_ref()
            .ok_or(DecodeError::DecoderMissing(DecoderKey::Heif))?;
        crate::util::contain(|| d.decode(bytes, min_long))
            .unwrap_or_else(|| Err(DecodeError::Failed(format!("{} failure", d.name()))))
    }

    /// The HEIC backend applies HEIF transformations itself (orientation from the probe is then not applied).
    pub fn heif_applies_transforms(&self) -> bool {
        self.heif.as_ref().is_some_and(|d| d.applies_transforms())
    }
}

/// The placeholder note for a missing decoder.
pub fn missing_note(key: DecoderKey) -> String {
    format!("No {} decoder on this system", key.label())
}

/// A deterministic test decoder for HEIC (feature `test-support`): an image derived from the input bytes, or a
/// given image per input; it can fail, and may or may not apply the container's rotation.
// In tests and test builds (`test-support`, enabled by dev-dependencies only), and in debug builds for
// `PHOTOSQUIRREL_HEIF_DECODER=fake`: never in a release binary (ADR-0089).
#[cfg(any(test, feature = "test-support", debug_assertions))]
pub mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// A fake HEIC decoder.
    pub struct FakeHeif {
        /// Images to return for exact inputs (keyed by the input's BLAKE3-free FNV hash of the bytes).
        given: Mutex<HashMap<u64, RgbImage>>,
        /// Fail every decode with this message.
        pub fail: Option<String>,
        /// Apply the container's `irot` (read from the bytes) itself.
        pub transforms: bool,
        /// Size of the derived image (long edge).
        pub size: u32,
        /// The backend name it reports (a decoder upgrade is a new name, ADR-0098).
        pub name: String,
    }

    impl Default for FakeHeif {
        fn default() -> Self {
            FakeHeif {
                given: Mutex::new(HashMap::new()),
                fail: None,
                transforms: true,
                size: 640,
                name: "fake HEIC decoder".into(),
            }
        }
    }

    /// FNV-1a over the bytes (identity of an input for [`FakeHeif::give`]).
    pub fn key_of(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    impl FakeHeif {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn failing(msg: &str) -> Self {
            FakeHeif {
                fail: Some(msg.into()),
                ..Self::default()
            }
        }

        /// This decoder under another backend name (a decoder upgrade, ADR-0098).
        pub fn named(self, name: &str) -> Self {
            FakeHeif {
                name: name.into(),
                ..self
            }
        }

        pub fn without_transforms() -> Self {
            FakeHeif {
                transforms: false,
                ..Self::default()
            }
        }

        /// Return `img` (as stored, upright) for these exact bytes.
        pub fn give(&self, bytes: &[u8], img: RgbImage) {
            self.given
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key_of(bytes), img);
        }

        /// The image as stored in the file (before the container's rotation), derived from its bytes: a smooth
        /// gradient pattern with a seed from the bytes, `size` x `size * 3 / 4`, or the probe's stored size.
        fn derived(&self, bytes: &[u8]) -> RgbImage {
            let seed = key_of(bytes);
            let (w, h) = crate::probe::probe_bytes(bytes, bytes.len() as u64, None)
                .stored
                .map(|d| {
                    let d = d.fit_long_edge(self.size);
                    (d.width.max(1), d.height.max(1))
                })
                .unwrap_or((self.size, self.size * 3 / 4));
            let mut data = Vec::with_capacity(w as usize * h as usize * 3);
            let (a, b, c) = (
                (seed & 0xff) as u32,
                ((seed >> 8) & 0xff) as u32,
                ((seed >> 16) & 0xff) as u32,
            );
            for y in 0..h {
                for x in 0..w {
                    let r = ((x * 255 / w.max(1)) + a) % 256;
                    let g = ((y * 255 / h.max(1)) + b) % 256;
                    let bl = (((x + y) * 255 / (w + h).max(1)) + c) % 256;
                    data.extend_from_slice(&[r as u8, g as u8, bl as u8]);
                }
            }
            RgbImage {
                width: w,
                height: h,
                data,
            }
        }
    }

    impl StillDecoder for FakeHeif {
        fn name(&self) -> &str {
            &self.name
        }

        fn decode(&self, bytes: &[u8], _min_long: u32) -> Result<RgbImage, DecodeError> {
            if let Some(m) = &self.fail {
                return Err(DecodeError::Failed(m.clone()));
            }
            if crate::sniff(bytes) != Ok(ps_model::Format::Heic) {
                return Err(DecodeError::Failed("not a HEIF file".into()));
            }
            let given = self
                .given
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key_of(bytes))
                .cloned();
            match given {
                // A given image is already upright.
                Some(img) => Ok(img),
                None => {
                    let img = self.derived(bytes);
                    if self.transforms {
                        let o =
                            crate::probe::probe_bytes(bytes, bytes.len() as u64, None).orientation;
                        Ok(img.oriented(o))
                    } else {
                        Ok(img)
                    }
                }
            }
        }

        fn applies_transforms(&self) -> bool {
            self.transforms
        }
    }
}
