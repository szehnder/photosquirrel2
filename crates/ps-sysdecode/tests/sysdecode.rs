//! `decoder-support` scenarios of the system decoders (ADR-0089): guidance per system, the libheif checks with
//! fake symbol tables, the self-test, the environment overrides, and the real decoder of the machine.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ps_media::decode::{DecodeError, RgbImage};
use ps_media::{Decoders, StillDecoder};
use ps_model::{DecoderFormat, Fix};
use ps_sysdecode::{Finding, OsInfo, Overrides, guidance};

fn commands(f: &Fix) -> Vec<String> {
    f.steps.iter().filter_map(|s| s.command.clone()).collect()
}

fn ends_with_restart(f: &Fix) {
    assert!(f.restart, "{f:?}");
    let last = f.steps.last().expect("steps");
    assert!(
        last.text.contains("restart PhotoSquirrel") && last.text.contains("rescan"),
        "{last:?}"
    );
}

fn ubuntu(v: &str) -> OsInfo {
    OsInfo::linux(&format!("ID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"{v}\"\n"))
}

#[test]
fn scenario_decoder_support_ubuntu_guidance() {
    let noble = guidance(&ubuntu("24.04"), &Finding::NoLibrary);
    assert_eq!(
        commands(&noble),
        vec!["sudo apt install libheif1 libheif-plugin-libde265"]
    );
    ends_with_restart(&noble);
    let jammy = guidance(&ubuntu("22.04"), &Finding::NoLibrary);
    assert_eq!(commands(&jammy), vec!["sudo apt install libheif1"]);
    ends_with_restart(&jammy);
    // Debian follows the same split (12: built in, 13: plugin).
    let bookworm = OsInfo::linux("ID=debian\nVERSION_ID=\"12\"\n");
    assert_eq!(
        commands(&guidance(&bookworm, &Finding::NoLibrary)),
        vec!["sudo apt install libheif1"]
    );
    let trixie = OsInfo::linux("ID=debian\nVERSION_ID=\"13\"\n");
    assert_eq!(
        commands(&guidance(&trixie, &Finding::NoLibrary)),
        vec!["sudo apt install libheif1 libheif-plugin-libde265"]
    );
}

#[test]
fn scenario_decoder_support_fedora_guidance_names_rpm_fusion() {
    let fedora = OsInfo::linux("ID=fedora\nVERSION_ID=40\n");
    let f = guidance(
        &fedora,
        &Finding::NoHevc {
            version: "1.18.2".into(),
        },
    );
    let c = commands(&f);
    assert_eq!(c.len(), 2, "{c:?}");
    assert!(c[0].starts_with("sudo dnf install https://mirrors.rpmfusion.org/free/fedora/"));
    assert!(c[0].contains("rpmfusion-free-release"));
    assert_eq!(c[1], "sudo dnf install libheif-freeworld");
    assert!(f.steps.iter().any(|s| s.text.contains("RPM Fusion")));
    ends_with_restart(&f);
    // Without any libheif, libheif itself first.
    let none = commands(&guidance(&fedora, &Finding::NoLibrary));
    assert_eq!(none[0], "sudo dnf install libheif");
}

#[test]
fn scenario_decoder_support_arch_and_opensuse_guidance() {
    for os in [
        OsInfo::linux("ID=arch\n"),
        OsInfo::linux("ID=manjaro\nID_LIKE=arch\n"),
        OsInfo::linux("ID=somethingelse\nID_LIKE=arch\n"),
    ] {
        let f = guidance(&os, &Finding::NoLibrary);
        assert_eq!(
            commands(&f),
            vec!["sudo pacman -S --needed libheif libde265"],
            "{os:?}"
        );
        ends_with_restart(&f);
    }
    let tw = OsInfo::linux(
        "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\nVERSION_ID=\"20261020\"\n",
    );
    let f = guidance(
        &tw,
        &Finding::NoHevc {
            version: "1.19.5".into(),
        },
    );
    assert!(f.steps.iter().any(|s| s.text.contains("Packman")), "{f:?}");
    assert!(commands(&f)[0].contains("packman/suse/openSUSE_Tumbleweed/"));
    ends_with_restart(&f);
}

#[test]
fn scenario_decoder_support_windows_guidance_names_the_missing_extensions() {
    let win11 = OsInfo::windows(22631, false);
    let hevc_only = guidance(
        &win11,
        &Finding::WindowsMissing {
            heif: false,
            hevc: true,
        },
    );
    let texts: Vec<&str> = hevc_only.steps.iter().map(|s| s.text.as_str()).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("HEVC Video Extensions ($0.99)"))
    );
    assert!(texts.iter().any(|t| t.contains("Device Manufacturer")));
    assert!(
        !texts.iter().any(|t| t.contains("HEIF Image Extensions")),
        "{texts:?}"
    );
    let urls: Vec<&str> = hevc_only
        .steps
        .iter()
        .filter_map(|s| s.url.as_deref())
        .collect();
    assert_eq!(
        urls,
        vec![
            "https://apps.microsoft.com/detail/9nmzlz57r3t7",
            "https://apps.microsoft.com/detail/9n4wgh0z6vhq"
        ]
    );
    ends_with_restart(&hevc_only);
    let both = guidance(
        &win11,
        &Finding::WindowsMissing {
            heif: true,
            hevc: true,
        },
    );
    let heif = both
        .steps
        .iter()
        .find(|s| s.text.contains("HEIF Image Extensions"))
        .expect("HEIF step");
    assert_eq!(
        heif.url.as_deref(),
        Some("https://apps.microsoft.com/detail/9pmmsr1cgpwg")
    );
    assert_eq!(
        heif.open.as_deref(),
        Some("ms-windows-store://pdp/?ProductId=9PMMSR1CGPWG")
    );
    assert!(
        both.steps
            .iter()
            .any(|s| s.text.contains("HEVC Video Extensions"))
    );
    // Every Store step carries both the web URL (always shown) and the Store link (opened on a click).
    assert!(both.steps.iter().filter(|s| s.open.is_some()).all(|s| {
        s.url
            .as_deref()
            .is_some_and(|u| u.starts_with("https://apps.microsoft.com/detail/"))
    }));
    ends_with_restart(&both);
    // N edition and too old builds.
    let n = guidance(
        &OsInfo::windows(19045, true),
        &Finding::WindowsNoMediaFoundation,
    );
    assert!(n.steps[0].text.contains("Media Feature Pack"));
    let old = guidance(&OsInfo::windows(17134, false), &Finding::WindowsTooOld);
    assert!(old.steps[0].text.contains("1809"));
}

/// The Windows reason mapping from injected enumeration results (task 5.8), on every runner.
#[test]
fn windows_findings_from_enumeration_results() {
    use ps_sysdecode::guidance::windows_finding;
    assert_eq!(
        windows_finding(17134, false, true, Some(1)),
        Some(Finding::WindowsTooOld)
    );
    assert_eq!(
        windows_finding(22631, true, false, None),
        Some(Finding::WindowsNoMediaFoundation)
    );
    assert_eq!(
        windows_finding(22631, false, false, Some(0)),
        Some(Finding::WindowsMissing {
            heif: true,
            hevc: true
        })
    );
    assert_eq!(
        windows_finding(22631, false, true, Some(0)),
        Some(Finding::WindowsMissing {
            heif: false,
            hevc: true
        })
    );
    assert_eq!(
        windows_finding(22631, false, false, Some(2)),
        Some(Finding::WindowsMissing {
            heif: true,
            hevc: false
        })
    );
    assert_eq!(windows_finding(22631, false, true, Some(1)), None);
    // Each finding's reason names the missing piece.
    assert!(
        ps_sysdecode::reason(&Finding::WindowsMissing {
            heif: true,
            hevc: true
        })
        .contains("HEIF Image Extensions and an HEVC decoder")
    );
}

#[test]
fn scenario_decoder_support_macos_needs_nothing() {
    let f = guidance(&OsInfo::macos(), &Finding::NoLibrary);
    assert!(f.summary.contains("Nothing to install"), "{f:?}");
    assert!(!f.restart);
    assert!(commands(&f).is_empty());
}

#[test]
fn scenario_decoder_support_unknown_system_gets_generic_guidance() {
    let f = guidance(&OsInfo::linux("ID=gentoo\n"), &Finding::NoLibrary);
    assert_eq!(f.steps.len(), 2);
    assert_eq!(
        f.steps[0].text,
        "Install libheif 1.12 or newer with an HEVC (libde265) decoder from your distribution"
    );
    ends_with_restart(&f);
}

// ---- libheif checks with fake symbol tables ----

#[cfg(all(unix, not(target_vendor = "apple")))]
mod fake_tables {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use ps_sysdecode::libheif::{HeifLib, probe_lib};

    struct Table {
        version: (u32, u32, u32),
        hevc: bool,
        decodes: AtomicU32,
    }

    impl HeifLib for Table {
        fn version(&self) -> (u32, u32, u32) {
            self.version
        }

        fn has_hevc(&self) -> bool {
            self.hevc
        }

        fn decode(&self, _bytes: &[u8]) -> Result<RgbImage, String> {
            self.decodes.fetch_add(1, Ordering::SeqCst);
            Err("fake table".into())
        }
    }

    fn table(version: (u32, u32, u32), hevc: bool) -> Arc<Table> {
        Arc::new(Table {
            version,
            hevc,
            decodes: AtomicU32::new(0),
        })
    }

    fn heic_entry(d: &Decoders) -> ps_model::DecoderEntry {
        d.report().entry(DecoderFormat::Heic).unwrap().clone()
    }

    #[test]
    fn scenario_decoder_support_library_without_hevc_is_reported() {
        let t = table((1, 17, 6), false);
        let probe = probe_lib(Ok(t.clone() as Arc<dyn HeifLib>));
        let d = ps_sysdecode::finish(&ubuntu("24.04"), probe, &Overrides::default());
        let e = heic_entry(&d);
        assert!(!e.available);
        assert!(
            e.reason
                .as_deref()
                .unwrap()
                .contains("installed but has no HEVC decoder"),
            "{e:?}"
        );
        assert_eq!(e.version.as_deref(), Some("1.17.6"));
        assert!(d.heif().is_none(), "not used for decoding");
        assert_eq!(t.decodes.load(Ordering::SeqCst), 0);
        assert!(e.fix.is_some());
    }

    #[test]
    fn scenario_decoder_support_too_old_library_refused() {
        let t = table((1, 11, 0), true);
        let probe = probe_lib(Ok(t.clone() as Arc<dyn HeifLib>));
        let d = ps_sysdecode::finish(&ubuntu("22.04"), probe, &Overrides::default());
        let e = heic_entry(&d);
        assert!(!e.available);
        let r = e.reason.unwrap();
        assert!(r.contains("1.11") && r.contains("1.12"), "{r}");
        assert_eq!(t.decodes.load(Ordering::SeqCst), 0, "no decode attempted");
        assert!(d.heif().is_none());
    }

    #[test]
    fn missing_library_is_reported_with_the_distribution_fix() {
        let probe = probe_lib(Err("libheif.so.1: not found".into()));
        let d = ps_sysdecode::finish(&ubuntu("24.04"), probe, &Overrides::default());
        let e = heic_entry(&d);
        assert!(!e.available);
        assert!(e.reason.unwrap().contains("not installed"));
        assert_eq!(
            commands(e.fix.as_ref().unwrap()),
            vec!["sudo apt install libheif1 libheif-plugin-libde265"]
        );
    }
}

/// A backend that loads but cannot decode, on a system where the platform checks found the HEIF component
/// but no HEVC decoder: the reason is the missing HEVC decoder, and nothing is decoded with it.
#[test]
fn scenario_decoder_support_decoder_that_loads_but_cannot_decode_is_unavailable() {
    struct Broken;
    impl StillDecoder for Broken {
        fn name(&self) -> &str {
            "WIC"
        }
        fn decode(&self, _: &[u8], _: u32) -> Result<RgbImage, DecodeError> {
            Err(DecodeError::Failed("0x88982F50 component not found".into()))
        }
        fn applies_transforms(&self) -> bool {
            true
        }
    }
    let probe = ps_sysdecode::PlatformProbe {
        backend_name: "WIC".into(),
        version: None,
        decoder: Some(Arc::new(Broken)),
        finding: Some(Finding::WindowsMissing {
            heif: false,
            hevc: true,
        }),
    };
    let d = ps_sysdecode::finish(&OsInfo::windows(22631, false), probe, &Overrides::default());
    let e = d.report().entry(DecoderFormat::Heic).unwrap();
    assert!(!e.available);
    assert!(
        e.reason.as_deref().unwrap().contains("no HEVC decoder"),
        "{e:?}"
    );
    assert!(d.heif().is_none(), "no file is decoded with it");
    // Without a more specific finding, the self-test failure itself.
    let probe = ps_sysdecode::PlatformProbe {
        backend_name: "WIC".into(),
        version: None,
        decoder: Some(Arc::new(Broken)),
        finding: None,
    };
    let d = ps_sysdecode::finish(&OsInfo::windows(22631, false), probe, &Overrides::default());
    let r = d
        .report()
        .entry(DecoderFormat::Heic)
        .unwrap()
        .reason
        .clone()
        .unwrap();
    assert!(r.contains("cannot decode the HEIC sample"), "{r}");
}

/// A backend that decodes the sample correctly passes; whether it applies transformations is learned from the
/// rotated twin.
#[test]
fn self_test_learns_whether_a_backend_applies_transformations() {
    struct Reference {
        transforms: bool,
    }
    impl StillDecoder for Reference {
        fn name(&self) -> &str {
            "reference"
        }
        fn decode(&self, bytes: &[u8], _: u32) -> Result<RgbImage, DecodeError> {
            // The sample's look: a vertical gradient from the top to the bottom colour, 640x480 as stored.
            let (w, h) = (640u32, 480u32);
            let mut data = Vec::with_capacity((w * h * 3) as usize);
            for y in 0..h {
                let t = y as f32 / (h - 1) as f32;
                let c: Vec<u8> = (0..3)
                    .map(|i| {
                        let a = f32::from(ps_sysdecode::SELFTEST_TOP[i]);
                        let b = f32::from(ps_sysdecode::SELFTEST_BOTTOM[i]);
                        (a + (b - a) * t).round() as u8
                    })
                    .collect();
                for _ in 0..w {
                    data.extend_from_slice(&c);
                }
            }
            let img = RgbImage {
                width: w,
                height: h,
                data,
            };
            let p = ps_media::probe_bytes(bytes, bytes.len() as u64, None);
            Ok(if self.transforms {
                img.oriented(p.orientation)
            } else {
                img
            })
        }
        fn applies_transforms(&self) -> bool {
            self.transforms
        }
    }
    for transforms in [true, false] {
        assert_eq!(
            ps_sysdecode::self_test(&Reference { transforms }),
            Ok(transforms)
        );
    }
}

#[test]
fn overrides_are_reported_and_fake_is_debug_only() {
    let ov = Overrides {
        heif: Some(ps_sysdecode::HeifChoice::Fake),
        ..Overrides::default()
    };
    let d = ps_sysdecode::detect(&ubuntu("24.04"), &ov);
    let e = d.report().entry(DecoderFormat::Heic).unwrap();
    if cfg!(debug_assertions) {
        assert!(e.available, "{e:?}");
        assert!(d.heif().is_some());
        assert_eq!(
            d.report().overrides,
            vec!["PHOTOSQUIRREL_HEIF_DECODER=fake"]
        );
    } else {
        // A release-profile build ignores `fake` and reports `none`.
        assert!(!e.available);
        assert!(d.heif().is_none());
        assert!(d.report().overrides[0].contains("ignored"));
    }
    let none = ps_sysdecode::detect(
        &ubuntu("24.04"),
        &Overrides {
            heif: Some(ps_sysdecode::HeifChoice::None),
            ..Overrides::default()
        },
    );
    let e = none.report().entry(DecoderFormat::Heic).unwrap();
    assert!(!e.available);
    assert!(
        e.reason
            .as_deref()
            .unwrap()
            .contains("PHOTOSQUIRREL_HEIF_DECODER=none")
    );
    assert_eq!(
        none.report().overrides,
        vec!["PHOTOSQUIRREL_HEIF_DECODER=none"]
    );
    assert!(
        none.report()
            .entry(DecoderFormat::Avif)
            .is_some_and(|a| a.available)
    );
}

/// The release-profile half of task 5.10, decided at compile time: `fake` is honoured only with
/// `debug_assertions`.
#[test]
fn fake_choice_follows_the_build_profile() {
    let ov = Overrides {
        heif: Some(ps_sysdecode::HeifChoice::Fake),
        ..Overrides::default()
    };
    let want = if cfg!(debug_assertions) {
        ps_sysdecode::HeifChoice::Fake
    } else {
        ps_sysdecode::HeifChoice::None
    };
    assert_eq!(ov.effective_heif(), Some(want));
}

// ---- the real decoder of this machine ----

fn require_system_heif() -> bool {
    std::env::var(ps_sysdecode::ENV_REQUIRE_SYSTEM_HEIF).is_ok_and(|v| v == "1")
}

/// This machine's decoder, or `None` after printing why the test is skipped (a failure with
/// `PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF=1`).
fn system_decoders() -> Option<Decoders> {
    let d = ps_sysdecode::detect(&OsInfo::current(), &Overrides::default());
    if d.heif().is_some() {
        return Some(d);
    }
    let e = d.report().entry(DecoderFormat::Heic).unwrap();
    let why = e.reason.clone().unwrap_or_default();
    assert!(
        !require_system_heif(),
        "PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF=1 but no HEIC decoder: {why}"
    );
    println!("SKIPPED: no HEIC decoder on this runner: {why}");
    None
}

#[test]
fn scenario_decoder_support_system_decoders_agree_with_the_reference_look() {
    let Some(d) = system_decoders() else {
        return;
    };
    // The sample's full size (640x480) from every backend, so the hashes compare decoders, not resize paths
    // (ImageIO otherwise returns a 512 px thumbnail for an analysis decode).
    let img = d
        .heif()
        .unwrap()
        .decode(ps_sysdecode::SELFTEST_HEIC, 640)
        .expect("decode");
    assert_eq!((img.width, img.height), (640, 480));
    let view =
        ps_hash::ImageView::new(img.width, img.height, ps_hash::PixelLayout::Rgb8, &img.data)
            .unwrap();
    let h = ps_hash::perceptual_hash(&view);
    let backend = d.heif().unwrap().name().to_owned();
    println!("{backend}: phash {:#018x} dhash {:#018x}", h.phash, h.dhash);
    let pd = (h.phash ^ ps_sysdecode::SELFTEST_PHASH).count_ones();
    let dd = (h.dhash ^ ps_sysdecode::SELFTEST_DHASH).count_ones();
    assert!(
        pd <= 4 && dd <= 4,
        "{backend}: pHash {pd} bits and dHash {dd} bits from the reference"
    );
}

#[cfg(all(unix, not(target_vendor = "apple")))]
#[test]
fn scenario_decoder_support_linux_loads_the_system_heif_library_at_run_time() {
    // Installed: the sample decodes and the library version is reported.
    if let Some(d) = system_decoders() {
        let e = d.report().entry(DecoderFormat::Heic).unwrap();
        assert!(e.available);
        let v = e.version.clone().expect("libheif version");
        assert!(v.starts_with("1."), "{v}");
        let img = d
            .heif()
            .unwrap()
            .decode(ps_sysdecode::SELFTEST_HEIC, 512)
            .unwrap();
        ps_sysdecode::check_upright(&img).unwrap();
    }
    // Pointed at a library that does not exist: unavailable because it is not installed, and nothing panics.
    let ov = Overrides {
        libheif: Some("/nonexistent/libheif.so.1".into()),
        ..Overrides::default()
    };
    let d = ps_sysdecode::detect(&OsInfo::current(), &ov);
    let e = d.report().entry(DecoderFormat::Heic).unwrap();
    assert!(!e.available);
    assert!(
        e.reason.as_deref().unwrap().contains("not installed"),
        "{e:?}"
    );
    assert_eq!(
        d.report().overrides,
        vec!["PHOTOSQUIRREL_LIBHEIF=/nonexistent/libheif.so.1"]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn scenario_decoder_support_macos_decodes_heic_with_the_built_in_framework() {
    let d = system_decoders().expect("ImageIO is always present on macOS");
    let dec = d.heif().unwrap();
    assert_eq!(dec.name(), "ImageIO");
    let img = dec.decode(ps_sysdecode::SELFTEST_HEIC, 512).unwrap();
    assert!(img.width.max(img.height) >= 512);
    ps_sysdecode::check_upright(&img).unwrap();
    // Upright after the container's rotation, through the registry.
    let p = ps_media::probe_bytes(
        ps_sysdecode::SELFTEST_ROT_HEIC,
        ps_sysdecode::SELFTEST_ROT_HEIC.len() as u64,
        None,
    );
    let rot = ps_media::decode_for_analysis(ps_sysdecode::SELFTEST_ROT_HEIC, &p, &d)
        .unwrap()
        .image;
    assert!(rot.height > rot.width, "{}x{}", rot.width, rot.height);
}

#[cfg(windows)]
#[test]
fn scenario_decoder_support_windows_decodes_heic_when_the_extensions_are_installed() {
    let d = ps_sysdecode::detect(&OsInfo::current(), &Overrides::default());
    let e = d.report().entry(DecoderFormat::Heic).unwrap().clone();
    match d.heif() {
        Some(dec) => {
            let img = dec.decode(ps_sysdecode::SELFTEST_HEIC, 512).unwrap();
            ps_sysdecode::check_upright(&img).unwrap();
        }
        None => {
            let why = e.reason.clone().unwrap_or_default();
            // The reason names the missing piece(s), and the fix the Store extension(s).
            assert!(
                why.contains("HEIF Image Extensions")
                    || why.contains("HEVC")
                    || why.contains("Media Foundation")
                    || why.contains("1809"),
                "{why}"
            );
            let fix = e.fix.expect("fix");
            assert!(
                fix.steps.iter().any(|s| s
                    .url
                    .as_deref()
                    .is_some_and(|u| u.starts_with("https://apps.microsoft.com/detail/")))
                    || why.contains("1809"),
                "{fix:?}"
            );
            assert!(
                !require_system_heif(),
                "PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF=1 but no HEIC decoder: {why}"
            );
            println!("SKIPPED: no HEIC decoder on this runner: {why}");
        }
    }
}

#[test]
fn scenario_media_probe_availability_listed() {
    let d = ps_sysdecode::detect(
        &ubuntu("24.04"),
        &Overrides {
            heif: Some(ps_sysdecode::HeifChoice::None),
            ..Overrides::default()
        },
    );
    let r = ps_media::decoder_report(&d, None);
    let heic = r.entry(DecoderFormat::Heic).unwrap();
    assert!(!heic.available);
    assert!(heic.reason.is_some());
    assert!(
        heic.fix.as_ref().is_some_and(|f| !f.steps.is_empty()),
        "{heic:?}"
    );
    let avif = r.entry(DecoderFormat::Avif).unwrap();
    assert!(avif.available);
    assert!(avif.backend.as_deref().unwrap().contains("bundled"));
    let poster = r.entry(DecoderFormat::PosterFrames).unwrap();
    assert!(!poster.available);
    assert!(
        poster.fix.as_ref().unwrap().steps[0]
            .text
            .contains("ffmpeg")
    );
    // A missing system library is explained for this distribution too.
    let gone = ps_sysdecode::finish(
        &ubuntu("24.04"),
        ps_sysdecode::PlatformProbe {
            backend_name: "libheif".into(),
            version: None,
            decoder: None,
            finding: Some(Finding::NoLibrary),
        },
        &Overrides::default(),
    );
    let h = gone.report().entry(DecoderFormat::Heic).unwrap();
    assert_eq!(
        commands(h.fix.as_ref().unwrap()),
        vec!["sudo apt install libheif1 libheif-plugin-libde265"]
    );
}
