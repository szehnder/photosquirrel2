//! The colour space a still's pixels are recorded in (ADR-0113): read by the probe, stored in `file.colour`
//! (catalog migration 26), shown by the inspector. Previews are converted to sRGB for display; hashes are not.

use serde::{Deserialize, Serialize};

/// The colour space of a still image's pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "space")]
pub enum ColourSpace {
    /// sRGB, and the answer when a file records nothing.
    #[default]
    Srgb,
    /// Adobe RGB (1998): an ICC profile, a DNG preview colour space 3, EXIF `InteropIndex` `R03` or the Canon maker
    /// note's colour space 2.
    AdobeRgb,
    /// Display P3 (an embedded profile, as phones write).
    DisplayP3,
    /// Another embedded ICC profile, by its description (empty when it has none).
    Profile { description: String },
}

/// `file.colour` of a row analysed before colour spaces were read and not re-checked (migration 26).
pub const COLOUR_UNCHECKED: &str = "unchecked";

/// The prefix of `file.colour` for a row whose colour space is known but whose stored thumbnail was not converted
/// (the one-time re-check could not make it again: a file over the in-memory limit, a sampled identity, a RAW found
/// cut, a decode that failed), e.g. `unconverted:adobe_rgb`. A later change can select these rows to try again.
pub const COLOUR_UNCONVERTED_PREFIX: &str = "unconverted:";

/// What `file.colour` records (catalog migration 26, ADR-0113).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StoredColour {
    /// The colour space; the stored previews were made from it (converted to sRGB when it is not sRGB).
    Space(ColourSpace),
    /// The colour space is known, but the stored thumbnail was kept as it was, not converted.
    Unconverted(ColourSpace),
    /// Analysed before colour spaces were read, and not re-checked ([`COLOUR_UNCHECKED`]).
    Unchecked,
}

impl StoredColour {
    /// The `file.colour` text.
    pub fn storage(&self) -> String {
        match self {
            StoredColour::Space(c) => c.storage(),
            StoredColour::Unconverted(c) => format!("{COLOUR_UNCONVERTED_PREFIX}{}", c.storage()),
            StoredColour::Unchecked => COLOUR_UNCHECKED.to_owned(),
        }
    }

    /// The state a stored `file.colour` text names; `None` for unknown text.
    pub fn from_storage(s: &str) -> Option<StoredColour> {
        if s == COLOUR_UNCHECKED {
            return Some(StoredColour::Unchecked);
        }
        match s.strip_prefix(COLOUR_UNCONVERTED_PREFIX) {
            Some(rest) => ColourSpace::from_storage(rest).map(StoredColour::Unconverted),
            None => ColourSpace::from_storage(s).map(StoredColour::Space),
        }
    }

    /// The stored previews were converted from a colour space other than sRGB, so they are not the decode's own
    /// pixels (perceptual hashes must not be taken from them; ADR-0113).
    pub fn preview_converted(&self) -> bool {
        matches!(self, StoredColour::Space(c) if !c.is_srgb())
    }

    /// What the inspector shows: [`ColourSpace::label`], and for a thumbnail that was not converted, that it was
    /// not ("Adobe RGB (1998), preview not converted"). `None` for sRGB and unchecked.
    pub fn label(&self) -> Option<String> {
        match self {
            StoredColour::Space(c) => c.label(),
            StoredColour::Unconverted(c) => {
                c.label().map(|l| format!("{l}, preview not converted"))
            }
            StoredColour::Unchecked => None,
        }
    }
}

impl ColourSpace {
    /// Pixels need no conversion for display.
    pub fn is_srgb(&self) -> bool {
        matches!(self, ColourSpace::Srgb)
    }

    /// The catalog's `file.colour` text: `srgb`, `adobe_rgb`, `display_p3` or `profile:<description>`.
    pub fn storage(&self) -> String {
        match self {
            ColourSpace::Srgb => "srgb".into(),
            ColourSpace::AdobeRgb => "adobe_rgb".into(),
            ColourSpace::DisplayP3 => "display_p3".into(),
            ColourSpace::Profile { description } => format!("profile:{description}"),
        }
    }

    /// The colour space a stored `file.colour` text names; `None` for [`COLOUR_UNCHECKED`] and unknown text.
    pub fn from_storage(s: &str) -> Option<ColourSpace> {
        match s {
            "srgb" => Some(ColourSpace::Srgb),
            "adobe_rgb" => Some(ColourSpace::AdobeRgb),
            "display_p3" => Some(ColourSpace::DisplayP3),
            _ => s.strip_prefix("profile:").map(|d| ColourSpace::Profile {
                description: d.to_owned(),
            }),
        }
    }

    /// What the inspector shows ("Adobe RGB (1998)", "Display P3", "Embedded profile: <description>"); `None` for
    /// sRGB, which shows nothing.
    pub fn label(&self) -> Option<String> {
        match self {
            ColourSpace::Srgb => None,
            ColourSpace::AdobeRgb => Some("Adobe RGB (1998)".into()),
            ColourSpace::DisplayP3 => Some("Display P3".into()),
            ColourSpace::Profile { description } if description.trim().is_empty() => {
                Some("Embedded profile".into())
            }
            ColourSpace::Profile { description } => {
                Some(format!("Embedded profile: {}", description.trim()))
            }
        }
    }
}

#[cfg(feature = "test-hooks")]
static CONVERSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether previews are converted to sRGB (always, except in a test that compares an analysis with the conversion
/// off, with the `test-hooks` feature).
#[cfg(feature = "test-hooks")]
pub fn colour_conversion_enabled() -> bool {
    CONVERSION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether previews are converted to sRGB: always in a build without the `test-hooks` feature.
#[cfg(not(feature = "test-hooks"))]
pub const fn colour_conversion_enabled() -> bool {
    true
}

/// Test hook (feature `test-hooks`, enabled only from dev-dependencies): turn the conversion of previews to sRGB off,
/// to compare with the build before ADR-0113. Process-wide: only for a test binary of its own.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn set_colour_conversion_enabled(enabled: bool) {
    CONVERSION.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_round_trips() {
        for c in [
            ColourSpace::Srgb,
            ColourSpace::AdobeRgb,
            ColourSpace::DisplayP3,
            ColourSpace::Profile {
                description: "ProPhoto: wide".into(),
            },
        ] {
            assert_eq!(ColourSpace::from_storage(&c.storage()), Some(c));
        }
        assert_eq!(ColourSpace::from_storage(COLOUR_UNCHECKED), None);
        assert_eq!(ColourSpace::from_storage("nonsense"), None);
        for s in [
            StoredColour::Space(ColourSpace::DisplayP3),
            StoredColour::Unconverted(ColourSpace::AdobeRgb),
            StoredColour::Unconverted(ColourSpace::Profile {
                description: "Wide".into(),
            }),
            StoredColour::Unchecked,
        ] {
            assert_eq!(StoredColour::from_storage(&s.storage()), Some(s));
        }
        assert_eq!(
            StoredColour::Unconverted(ColourSpace::AdobeRgb).storage(),
            "unconverted:adobe_rgb"
        );
        assert_eq!(StoredColour::from_storage("unconverted:nonsense"), None);
    }

    #[test]
    fn an_unconverted_preview_says_so() {
        assert_eq!(
            StoredColour::Unconverted(ColourSpace::AdobeRgb)
                .label()
                .as_deref(),
            Some("Adobe RGB (1998), preview not converted")
        );
        assert_eq!(
            StoredColour::Space(ColourSpace::AdobeRgb)
                .label()
                .as_deref(),
            Some("Adobe RGB (1998)")
        );
        assert_eq!(StoredColour::Unconverted(ColourSpace::Srgb).label(), None);
        assert_eq!(StoredColour::Unchecked.label(), None);
        assert!(StoredColour::Space(ColourSpace::DisplayP3).preview_converted());
        for s in [
            StoredColour::Space(ColourSpace::Srgb),
            StoredColour::Unconverted(ColourSpace::AdobeRgb),
            StoredColour::Unchecked,
        ] {
            assert!(!s.preview_converted(), "{s:?}");
        }
    }

    #[test]
    fn only_wide_spaces_have_a_label() {
        assert_eq!(ColourSpace::Srgb.label(), None);
        assert_eq!(
            ColourSpace::AdobeRgb.label().as_deref(),
            Some("Adobe RGB (1998)")
        );
        assert_eq!(
            ColourSpace::Profile {
                description: " ".into()
            }
            .label()
            .as_deref(),
            Some("Embedded profile")
        );
    }
}
