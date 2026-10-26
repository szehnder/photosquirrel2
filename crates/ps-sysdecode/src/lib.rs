//! HEIC/HEIF through the operating system's decoder (ADR-0089): Apple ImageIO on macOS, WIC on Windows (HEIF
//! Image Extensions plus an HEVC codec), the system libheif loaded at run time on Linux and other Unix
//! systems (1.12 or newer, with an HEVC decoder). PhotoSquirrel ships no HEVC decoder.
//!
//! [`detect`] builds the platform backend, runs platform checks that explain a failure precisely, and makes the
//! backend available only once it decodes an embedded sample (and its rotated twin, which tells whether the
//! backend applies HEIF transformations). Otherwise the report carries the reason and the fix for this system
//! ([`guidance`]). Detection reads no user file and opens no network connection (ADR-0029).
//!
//! Core layer (ADR-0003): no tokio, no gpui-kit. Every `unsafe` block has a `SAFETY:` comment.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod guidance;
pub mod os;

#[cfg(all(unix, not(target_vendor = "apple")))]
pub mod libheif;

#[cfg(target_os = "macos")]
mod imageio;

#[cfg(windows)]
pub mod wic;

use std::path::PathBuf;
use std::sync::Arc;

use ps_media::decode::{DecodeError, RgbImage};
use ps_media::{Decoders, StillDecoder};
use ps_model::{DecoderEntry, DecoderFormat, DecoderReport};

pub use guidance::{Finding, guidance, reason};
pub use os::{OsFamily, OsInfo};

/// The HEIC self-test sample: 640x480, an `hvc1` primary item and a thumbnail, a vertical gradient from
/// [`SELFTEST_TOP`] to [`SELFTEST_BOTTOM`], made by the project.
pub const SELFTEST_HEIC: &[u8] = include_bytes!("../data/selftest.heic");
/// The same HEVC payload re-boxed with `irot` 1 (90 degrees anti-clockwise) by `ps_fixtures::heif`.
pub const SELFTEST_ROT_HEIC: &[u8] = include_bytes!("../data/selftest-rot.heic");
/// Reference colours of the self-test sample (libheif decode): near the top, the middle and the bottom.
pub const SELFTEST_TOP: [u8; 3] = [233, 103, 85];
pub const SELFTEST_MIDDLE: [u8; 3] = [162, 115, 125];
pub const SELFTEST_BOTTOM: [u8; 3] = [93, 126, 167];
/// Per channel.
pub const SELFTEST_TOLERANCE: i32 = 24;
/// The look sample: a 640x480 photo-like picture (`ps_fixtures::media::jpeg(640, 480, 2026, 95)` encoded with
/// `heif-enc -q 40 --no-thumb`), with detail in both directions. The self-test sample is a smooth vertical
/// gradient, whose difference hash is noise (every row is flat), so the look check uses this one.
pub const LOOK_HEIC: &[u8] = include_bytes!("../data/look.heic");
/// Reference perceptual hashes of [`LOOK_HEIC`] (the Linux libheif decode, ps-hash version 1); every backend
/// must decode within 4 bits of both (spec "System decoders agree on the look").
pub const LOOK_PHASH: u64 = 0xb832_ec92_72ad_52ad;
pub const LOOK_DHASH: u64 = 0x7f7f_7fff_ffff_ffff;

/// The pixel cap of a system decode (the same as other decoders).
pub const MAX_PIXELS: u64 = 768 * 1024 * 1024 / 3;

/// End a real-decoder test that cannot run on this machine, visibly (gate review of `heic-system-decoders`):
/// fails with `PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF=1`; otherwise writes `SKIPPED: <why>` to the process's stderr
/// directly (the test harness captures `println!`/`eprintln!` of passing tests, not the handle), and on GitHub
/// Actions a `::warning::` annotation and a line in the job summary, so a skip never looks like a pass.
#[doc(hidden)]
pub fn skip_test(why: &str) {
    let require = std::env::var(ENV_REQUIRE_SYSTEM_HEIF).is_ok_and(|v| v == "1");
    assert!(
        !require,
        "{ENV_REQUIRE_SYSTEM_HEIF}=1 but this test cannot run: {why}"
    );
    skip_note(why);
}

/// [`skip_test`] without the require flag's failure: for a test that needs something more specific than a HEIC
/// decoder (a particular libheif version), which the require flag does not promise.
#[doc(hidden)]
pub fn skip_note(why: &str) {
    use std::io::Write as _;
    let test = std::thread::current()
        .name()
        .unwrap_or("a real-decoder test")
        .to_owned();
    let _ = writeln!(std::io::stderr(), "SKIPPED: {test}: {why}");
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        let one_line = why.replace(['\n', '\r'], " ");
        let _ = writeln!(
            std::io::stdout(),
            "::warning title=real-decoder test skipped::{test}: {one_line}"
        );
        if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
            // The runner's own summary file, outside every user folder.
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(summary) {
                let _ = writeln!(f, "- SKIPPED `{test}`: {one_line}");
            }
        }
    }
}

/// How `PHOTOSQUIRREL_HEIF_DECODER` chooses the HEIC entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeifChoice {
    /// Behave as if no decoder were found (any build).
    None,
    /// A deterministic test decoder (debug builds only; a release build treats it as `None`).
    Fake,
}

/// Environment overrides of detection (ADR-0089), all reported by `doctor`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// `PHOTOSQUIRREL_HEIF_DECODER=none|fake`.
    pub heif: Option<HeifChoice>,
    /// `PHOTOSQUIRREL_LIBHEIF=<path>`: the libheif to load (Linux), honoured in every build.
    pub libheif: Option<PathBuf>,
    /// The raw `PHOTOSQUIRREL_HEIF_DECODER` value when it is not understood (reported, ignored).
    pub unknown_heif: Option<String>,
}

pub const ENV_HEIF_DECODER: &str = "PHOTOSQUIRREL_HEIF_DECODER";
pub const ENV_LIBHEIF: &str = "PHOTOSQUIRREL_LIBHEIF";
pub const ENV_REQUIRE_SYSTEM_HEIF: &str = "PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF";

impl Overrides {
    pub fn from_env() -> Overrides {
        let raw = std::env::var(ENV_HEIF_DECODER).ok();
        let heif = match raw.as_deref().map(str::trim) {
            Some("none") => Some(HeifChoice::None),
            Some("fake") => Some(HeifChoice::Fake),
            _ => None,
        };
        Overrides {
            unknown_heif: raw
                .filter(|_| heif.is_none())
                .filter(|r| !r.trim().is_empty()),
            heif,
            libheif: std::env::var_os(ENV_LIBHEIF)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        }
    }

    /// The overrides in force, as `NAME=value` lines.
    pub fn list(&self) -> Vec<String> {
        let mut v = Vec::new();
        match self.heif {
            Some(HeifChoice::None) => v.push(format!("{ENV_HEIF_DECODER}=none")),
            Some(HeifChoice::Fake) if cfg!(debug_assertions) => {
                v.push(format!("{ENV_HEIF_DECODER}=fake"));
            }
            Some(HeifChoice::Fake) => v.push(format!(
                "{ENV_HEIF_DECODER}=fake (ignored: release build; behaves as none)"
            )),
            None => {}
        }
        if let Some(u) = &self.unknown_heif {
            v.push(format!(
                "{ENV_HEIF_DECODER}={u} (not understood: use none or fake)"
            ));
        }
        if let Some(p) = &self.libheif {
            v.push(format!("{ENV_LIBHEIF}={}", p.display()));
        }
        v
    }

    /// The effective HEIC choice of this build: `fake` only with `debug_assertions` (a release binary must
    /// never write fake hashes into a real catalog), else `none`.
    pub fn effective_heif(&self) -> Option<HeifChoice> {
        match self.heif {
            Some(HeifChoice::Fake) if !cfg!(debug_assertions) => Some(HeifChoice::None),
            c => c,
        }
    }
}

/// What a platform probe found before the self-test.
pub struct PlatformProbe {
    /// "ImageIO", "WIC", "libheif".
    pub backend_name: String,
    pub version: Option<String>,
    /// The backend, when it could be built.
    pub decoder: Option<Arc<dyn StillDecoder>>,
    /// The most specific finding so far (a decoder may still be present: a self-test failure then reports
    /// this rather than a generic failure).
    pub finding: Option<Finding>,
}

/// The decoders of this machine, from the environment's overrides and this OS.
pub fn detect_from_env() -> Decoders {
    detect(&OsInfo::current(), &Overrides::from_env())
}

/// Detect the HEIC decoder for `os` (D5): the platform backend, its checks, then the self-test. AVIF is always
/// available (the bundled decoder).
pub fn detect(os: &OsInfo, ov: &Overrides) -> Decoders {
    let probe = match ov.effective_heif() {
        Some(HeifChoice::None) => PlatformProbe {
            backend_name: "none".into(),
            version: None,
            decoder: None,
            finding: Some(Finding::Forced),
        },
        #[cfg(debug_assertions)]
        Some(HeifChoice::Fake) => PlatformProbe {
            backend_name: "fake HEIC decoder (PHOTOSQUIRREL_HEIF_DECODER=fake)".into(),
            version: None,
            decoder: Some(Arc::new(ps_media::decoders::fake::FakeHeif::new())),
            finding: None,
        },
        _ => platform_probe(ov),
    };
    finish(os, probe, ov)
}

/// The platform's probe.
#[allow(unused_variables)]
fn platform_probe(ov: &Overrides) -> PlatformProbe {
    #[cfg(target_os = "macos")]
    {
        imageio::probe()
    }
    #[cfg(windows)]
    {
        wic::probe()
    }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    {
        libheif::probe(ov.libheif.as_deref())
    }
    #[cfg(not(any(unix, windows)))]
    {
        PlatformProbe {
            backend_name: "none".into(),
            version: None,
            decoder: None,
            finding: Some(Finding::NoLibrary),
        }
    }
}

/// Run the self-test on a probe's decoder and build the registry with the report.
pub fn finish(os: &OsInfo, probe: PlatformProbe, ov: &Overrides) -> Decoders {
    let fake = probe.backend_name.starts_with("fake");
    let (decoder, finding) = match probe.decoder {
        Some(d) => match self_test(&*d) {
            // The fake decoder does not decode the real sample's pixels: it is used as is.
            _ if fake => (Some(d), None),
            Ok(applies) => {
                let wrapped: Arc<dyn StillDecoder> = Arc::new(Measured { inner: d, applies });
                (Some(wrapped), None)
            }
            Err(detail) => (
                None,
                Some(probe.finding.clone().unwrap_or(Finding::SelfTestFailed {
                    backend: probe.backend_name.clone(),
                    detail,
                })),
            ),
        },
        None => (
            None,
            Some(probe.finding.clone().unwrap_or(Finding::NoLibrary)),
        ),
    };
    let entry = match &finding {
        None => DecoderEntry {
            format: DecoderFormat::Heic,
            available: true,
            backend: Some(probe.backend_name.clone()),
            version: probe.version.clone(),
            reason: None,
            fix: None,
            assembly: None,
        },
        Some(f) => DecoderEntry {
            format: DecoderFormat::Heic,
            available: false,
            backend: (probe.backend_name != "none").then(|| probe.backend_name.clone()),
            version: probe.version.clone(),
            reason: Some(reason(f)),
            fix: Some(guidance(os, f)),
            assembly: None,
        },
    };
    let mut report = DecoderReport::default();
    report.set(entry);
    report.set(ps_media::av1::report_entry());
    report.overrides = ov.list();
    report.os = Some(os.summary());
    Decoders::from_parts(decoder, report)
}

/// A backend with what the self-test learned about transformations.
struct Measured {
    inner: Arc<dyn StillDecoder>,
    applies: bool,
}

impl StillDecoder for Measured {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn decode(&self, bytes: &[u8], min_long: u32) -> Result<RgbImage, DecodeError> {
        self.inner.decode(bytes, min_long)
    }

    fn applies_transforms(&self) -> bool {
        self.applies
    }
}

fn near(got: [u8; 3], want: [u8; 3]) -> bool {
    (0..3).all(|c| (i32::from(got[c]) - i32::from(want[c])).abs() <= SELFTEST_TOLERANCE)
}

/// Check a decode of the self-test sample, upright: landscape 4:3 at least 320 px wide, the reference
/// colours top to bottom.
pub fn check_upright(img: &RgbImage) -> Result<(), String> {
    let (w, h) = (img.width, img.height);
    if w < 320 || (w * 3).abs_diff(h * 4) > 4 {
        return Err(format!("decoded the sample to {w}x{h}, expected 640x480"));
    }
    for (fy, want, what) in [
        (8, SELFTEST_TOP, "top"),
        (2, SELFTEST_MIDDLE, "middle"),
        (8, SELFTEST_BOTTOM, "bottom"),
    ] {
        let y = match what {
            "top" => h / fy,
            "middle" => h / fy,
            _ => h - 1 - h / fy,
        };
        let got = img.pixel(w / 2, y);
        if !near(got, want) {
            return Err(format!(
                "the sample's {what} colour is {got:?}, expected about {want:?}"
            ));
        }
    }
    Ok(())
}

/// Decode the embedded sample and its rotated twin: `Ok(true)` when the backend applies HEIF transformations,
/// `Ok(false)` when it returns the stored orientation, `Err` when it cannot decode the sample correctly.
pub fn self_test(d: &dyn StillDecoder) -> Result<bool, String> {
    let img = ps_media::util::contain(|| d.decode(SELFTEST_HEIC, 512))
        .ok_or_else(|| "the decoder panicked".to_owned())?
        .map_err(|e| e.to_string())?;
    check_upright(&img)?;
    let rot = ps_media::util::contain(|| d.decode(SELFTEST_ROT_HEIC, 512))
        .ok_or_else(|| "the decoder panicked".to_owned())?
        .map_err(|e| format!("rotated sample: {e}"))?;
    if rot.height > rot.width {
        // Turned 90 degrees anti-clockwise: the top (red) is now on the left.
        let left = rot.pixel(rot.width / 8, rot.height / 2);
        let right = rot.pixel(rot.width - 1 - rot.width / 8, rot.height / 2);
        if near(left, SELFTEST_TOP) && near(right, SELFTEST_BOTTOM) {
            Ok(true)
        } else {
            Err(format!(
                "the rotated sample decoded with {left:?} left and {right:?} right"
            ))
        }
    } else {
        check_upright(&rot).map(|_| false)
    }
}

/// Windows build number and whether Media Foundation is missing (an N edition without the Media Feature
/// Pack); `(0, false)` elsewhere.
pub(crate) mod platform {
    #[cfg(windows)]
    pub fn windows_version() -> (u32, bool) {
        crate::wic::windows_version()
    }

    #[cfg(not(windows))]
    pub fn windows_version() -> (u32, bool) {
        (0, false)
    }
}
