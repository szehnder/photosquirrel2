//! `duplicate-detection` scenarios of burst-frame-identity (ADR-0105) on hand-built inputs: recorded frame
//! identities keep frames apart, and RAW files are camera originals.
#![allow(clippy::unwrap_used)]

use ps_dedup::{
    Constraints, Contradiction, DedupInput, DedupMeta, Grouping, PairVerdict, group, pair_verdict,
};
use ps_hash::PERCEPTUAL_HASH_VERSION;
use ps_model::{
    CaptureTime, ContentId, FileId, Format, FrameId, Orientation, PerceptualHash, PhotoClass,
    RawFormat,
};

const BASE_P: u64 = 0x9e37_79b9_7f4a_7c15;
const BASE_D: u64 = 0x3c6e_f372_fe94_f82b;

fn content(n: u64) -> ContentId {
    let mut d = [0u8; 32];
    d[..8].copy_from_slice(&n.to_le_bytes());
    ContentId::full(d)
}

/// `p` / `d` low bits flipped from the base hashes: the distance between `hash(a, b)` and `hash(c, d)` is
/// `|a - c|` and `|b - d|`.
fn hash(p: u32, d: u32) -> PerceptualHash {
    let mask = |n: u32| if n == 0 { 0 } else { u64::MAX >> (64 - n) };
    PerceptualHash {
        phash: BASE_P ^ mask(p),
        dhash: BASE_D ^ mask(d),
        version: PERCEPTUAL_HASH_VERSION,
    }
}

/// 2020-04-20 02:00:23 and `millis` ms.
fn at(millis: u16) -> CaptureTime {
    CaptureTime {
        millis,
        ..CaptureTime::from_parts(2020, 4, 20, 2, 0, 23)
    }
}

struct File {
    id: i64,
    path: &'static str,
    format: Format,
    dims: (u32, u32),
    /// `None`: no sub-second recorded.
    millis: Option<u16>,
    maker_notes: bool,
    frame: Option<FrameId>,
    hash: (u32, u32),
}

fn input(f: File) -> DedupInput {
    DedupInput {
        file: FileId(f.id),
        content: Some(content(f.id as u64)),
        hash: Some(hash(f.hash.0, f.hash.1)),
        format: f.format,
        class: PhotoClass::Photo,
        path: f.path.into(),
        size: 20_000_000 + f.id as u64,
        mtime_ns: 1_587_340_823_000_000_000,
        meta: DedupMeta {
            width: Some(f.dims.0),
            height: Some(f.dims.1),
            orientation: Orientation::UPRIGHT,
            capture_time: Some(at(f.millis.unwrap_or(0))),
            subsec_present: f.millis.is_some(),
            camera_make: Some("Canon".into()),
            camera_model: Some("Canon EOS 70D".into()),
            camera_serial: Some("0123456789".into()),
            has_maker_notes: f.maker_notes,
            camera_original: f.maker_notes,
            has_gps: false,
            has_exposure: true,
            jpeg_quality: (f.format == Format::Jpeg).then_some(96),
            duration_ms: None,
            frame: f.frame,
        },
    }
}

const CR2: Format = Format::Raw(RawFormat::Cr2);
const DNG: Format = Format::Raw(RawFormat::Dng);
const JPEG_DIMS: (u32, u32) = (5472, 3648);
const DNG_DIMS: (u32, u32) = (5568, 3708);

fn num(n: u32) -> Option<FrameId> {
    FrameId::new(None, Some(1_010_000 + n))
}

fn dng_id(n: u32) -> Option<FrameId> {
    FrameId::new(Some(format!("IMG_{n:04}")), Some(1_010_000 + n))
}

/// Frames 34 (sub-second 00) and 35 (130 ms) of a 70D burst as on the owner's card: per frame a CR2 and a JPEG
/// in DCIM, a JPEG copy and the converted DNG in a dated folder. Frame 34's DNG has no sub-second time and hashes
/// closer to frame 35 (distance 1) than to its own frame (distance 3). The JPEG copies are byte-identical.
fn card() -> Vec<DedupInput> {
    let mut v = card_files();
    v[2].content = v[1].content;
    v[6].content = v[5].content;
    v
}

fn card_files() -> Vec<DedupInput> {
    vec![
        input(File {
            id: 1,
            path: "1/DCIM/101CANON/IMG_0034.CR2",
            format: CR2,
            dims: JPEG_DIMS,
            millis: Some(0),
            maker_notes: true,
            frame: num(34),
            hash: (0, 0),
        }),
        input(File {
            id: 2,
            path: "1/DCIM/101CANON/IMG_0034.JPG",
            format: Format::Jpeg,
            dims: JPEG_DIMS,
            millis: Some(0),
            maker_notes: true,
            frame: num(34),
            hash: (0, 0),
        }),
        input(File {
            id: 3,
            path: "1/2020/2020-04-20/IMG_0034.JPG",
            format: Format::Jpeg,
            dims: JPEG_DIMS,
            millis: Some(0),
            maker_notes: true,
            frame: num(34),
            hash: (0, 0),
        }),
        input(File {
            id: 4,
            path: "1/2020/2020-04-20/IMG_0034.DNG",
            format: DNG,
            dims: DNG_DIMS,
            millis: None,
            maker_notes: true,
            frame: dng_id(34),
            hash: (2, 1),
        }),
        input(File {
            id: 5,
            path: "1/DCIM/101CANON/IMG_0035.CR2",
            format: CR2,
            dims: JPEG_DIMS,
            millis: Some(130),
            maker_notes: true,
            frame: num(35),
            hash: (2, 2),
        }),
        input(File {
            id: 6,
            path: "1/DCIM/101CANON/IMG_0035.JPG",
            format: Format::Jpeg,
            dims: JPEG_DIMS,
            millis: Some(130),
            maker_notes: true,
            frame: num(35),
            hash: (2, 2),
        }),
        input(File {
            id: 7,
            path: "1/2020/2020-04-20/IMG_0035.JPG",
            format: Format::Jpeg,
            dims: JPEG_DIMS,
            millis: Some(130),
            maker_notes: true,
            frame: num(35),
            hash: (2, 2),
        }),
        input(File {
            id: 8,
            path: "1/2020/2020-04-20/IMG_0035.DNG",
            format: DNG,
            dims: DNG_DIMS,
            millis: Some(130),
            maker_notes: true,
            frame: dng_id(35),
            hash: (2, 2),
        }),
    ]
}

fn members(g: &Grouping, f: i64) -> Vec<i64> {
    let mut v: Vec<i64> = g
        .group_of(FileId(f))
        .map(|x| x.members.iter().map(|m| m.file.0).collect())
        .unwrap_or_default();
    v.sort_unstable();
    v
}

#[test]
fn scenario_duplicate_detection_converted_burst_frame_without_sub_second_not_merged() {
    let inputs = card();
    let g = group(&inputs, &Constraints::default());
    assert_eq!(
        members(&g, 4),
        vec![1, 2, 3, 4],
        "frame 34's DNG with frame 34"
    );
    assert_eq!(members(&g, 5), vec![5, 6, 7, 8], "frame 35 on its own");
    let k34 = g.group_of(FileId(1)).unwrap().keeper;
    let k35 = g.group_of(FileId(5)).unwrap().keeper;
    assert_eq!(k34, FileId(4), "each frame keeps its own DNG");
    assert_eq!(k35, FileId(8));
    // The frames are a similar-shot suggestion naming both frames.
    let s = g
        .suggestions
        .iter()
        .find(|s| s.file_a == FileId(4) && s.file_b == FileId(5))
        .expect("frame 34's DNG and frame 35's CR2 are a suggestion");
    assert_eq!(
        s.reason,
        Contradiction::FrameId {
            a: "101-0034".into(),
            b: "101-0035".into()
        }
    );
    assert_eq!(
        s.reason.to_string(),
        "recorded as different frames (101-0034 and 101-0035): burst frames"
    );
    // Two DNGs compare their RAW names when nothing else tells them apart.
    let mut a = inputs[3].clone();
    let mut b = inputs[7].clone();
    for x in [&mut a, &mut b] {
        x.meta.frame.as_mut().unwrap().file_number = None;
        x.meta.subsec_present = false;
        x.meta.capture_time = Some(at(0));
    }
    b.meta.width = Some(5570);
    assert!(matches!(
        pair_verdict(&a, &b),
        PairVerdict::Suggestion {
            reason: Contradiction::FrameId { .. },
            ..
        }
    ));
    // A missing identity never matches: frame 34's CR2 without its file number still joins nothing of frame 35.
    let mut no_id = inputs.clone();
    no_id[3].meta.frame = None;
    let g = group(&no_id, &Constraints::default());
    assert!(!members(&g, 4).contains(&5));
}

#[test]
fn scenario_duplicate_detection_resized_copy_still_joins_its_original() {
    let jpeg = input(File {
        id: 1,
        path: "1/DCIM/101CANON/IMG_0034.JPG",
        format: Format::Jpeg,
        dims: JPEG_DIMS,
        millis: Some(0),
        maker_notes: true,
        frame: num(34),
        hash: (0, 0),
    });
    let mut resized = input(File {
        id: 2,
        path: "1/Exports/IMG_0034_1024.jpg",
        format: Format::Jpeg,
        dims: (1024, 683),
        millis: None,
        maker_notes: false,
        frame: None,
        hash: (1, 2),
    });
    resized.meta.camera_original = false;
    let g = group(&[jpeg.clone(), resized.clone()], &Constraints::default());
    assert_eq!(members(&g, 2), vec![1, 2]);
    // Also when the copy kept no metadata at all.
    resized.meta = DedupMeta {
        width: Some(1024),
        height: Some(683),
        jpeg_quality: Some(85),
        ..Default::default()
    };
    let g = group(&[jpeg, resized], &Constraints::default());
    assert_eq!(members(&g, 2), vec![1, 2]);
}

#[test]
fn scenario_duplicate_detection_two_converted_dngs_of_two_frames_not_merged() {
    // No maker notes parsed, no frame identity, no sub-seconds: only the RAW-original rule tells them apart.
    let dng = |id: i64, path: &'static str, hash: (u32, u32)| {
        input(File {
            id,
            path,
            format: DNG,
            dims: DNG_DIMS,
            millis: None,
            maker_notes: false,
            frame: None,
            hash,
        })
    };
    let a = dng(1, "1/2020/IMG_0034.DNG", (0, 0));
    let b = dng(2, "1/2020/IMG_0035.DNG", (1, 1));
    let g = group(&[a.clone(), b.clone()], &Constraints::default());
    assert!(g.groups.is_empty(), "{:?}", g.groups);
    assert_eq!(g.suggestions.len(), 1);
    assert!(matches!(
        g.suggestions[0].reason,
        Contradiction::TwoCameraOriginals { .. }
    ));
    // A CR2 and its own DNG (other dimensions) still join.
    let cr2 = input(File {
        id: 3,
        path: "1/DCIM/IMG_0034.CR2",
        format: CR2,
        dims: JPEG_DIMS,
        millis: Some(0),
        maker_notes: true,
        frame: None,
        hash: (0, 1),
    });
    let g = group(&[a, cr2], &Constraints::default());
    assert_eq!(members(&g, 1), vec![1, 3]);
}

#[test]
fn scenario_duplicate_detection_re_saved_dng_of_one_frame_still_joins() {
    // A converted DNG and the same DNG after Lightroom wrote XMP into it: same dimensions, other bytes, both
    // record frame 101-0034 and the same capture time (ADR-0107).
    let dng = |id: i64, path: &'static str, frame: Option<FrameId>, hash: (u32, u32)| {
        input(File {
            id,
            path,
            format: DNG,
            dims: DNG_DIMS,
            millis: None,
            maker_notes: true,
            frame,
            hash,
        })
    };
    let a = dng(1, "1/2020/IMG_0034.DNG", dng_id(34), (0, 0));
    let b = dng(2, "2/Lightroom/IMG_0034.DNG", dng_id(34), (0, 0));
    assert!(
        matches!(pair_verdict(&a, &b), PairVerdict::Edge { .. }),
        "{:?}",
        pair_verdict(&a, &b)
    );
    let g = group(&[a.clone(), b.clone()], &Constraints::default());
    assert_eq!(members(&g, 1), vec![1, 2]);
    // Only the RAW name recorded (no file number on either side): the stem and the capture time agree.
    let stem_only = |x: &DedupInput| {
        let mut x = x.clone();
        x.meta.frame.as_mut().unwrap().file_number = None;
        x
    };
    let (sa, sb) = (stem_only(&a), stem_only(&b));
    assert!(matches!(pair_verdict(&sa, &sb), PairVerdict::Edge { .. }));
    // A stem can repeat after the counter rolls over: without a capture time on one side it is not evidence.
    let mut undated = sb.clone();
    undated.meta.capture_time = None;
    assert!(matches!(
        pair_verdict(&sa, &undated),
        PairVerdict::Suggestion {
            reason: Contradiction::TwoCameraOriginals { .. },
            ..
        }
    ));
    // A stem on one side and a file number on the other is no agreement either.
    assert!(matches!(
        pair_verdict(&a, &sb),
        PairVerdict::Suggestion {
            reason: Contradiction::TwoCameraOriginals { .. },
            ..
        }
    ));
    // Two DNGs of two frames still split, as before.
    let c = dng(3, "1/2020/IMG_0035.DNG", dng_id(35), (1, 1));
    let g = group(&[a, c], &Constraints::default());
    assert!(g.groups.is_empty(), "{:?}", g.groups);
}

#[test]
fn nef_and_its_converted_dng_of_equal_dimensions_stay_apart_without_a_recorded_frame() {
    // ADR-0107: a non-Canon RAW and a converted DNG with no agreeing identity (no file number, the NEF records no
    // OriginalRawFileName) keep the conservative split: an extra copy is exported, nothing is left out.
    let nikon = |mut x: DedupInput| {
        x.meta.camera_make = Some("NIKON CORPORATION".into());
        x.meta.camera_model = Some("NIKON D750".into());
        x
    };
    let nef = nikon(input(File {
        id: 1,
        path: "1/DCIM/100NCD750/DSC_0034.NEF",
        format: Format::Raw(RawFormat::Nef),
        dims: (6032, 4032),
        millis: Some(300),
        maker_notes: true,
        frame: None,
        hash: (0, 0),
    }));
    let dng = nikon(input(File {
        id: 2,
        path: "1/2020/DSC_0034.dng",
        format: DNG,
        dims: (6032, 4032),
        millis: Some(300),
        maker_notes: true,
        frame: FrameId::new(Some("DSC_0034".into()), None),
        hash: (0, 0),
    }));
    let g = group(&[nef, dng], &Constraints::default());
    assert!(g.groups.is_empty(), "{:?}", g.groups);
    assert!(matches!(
        g.suggestions[0].reason,
        Contradiction::TwoCameraOriginals { .. }
    ));
}
