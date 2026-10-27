//! `media-metadata` scenarios of burst-frame-identity (ADR-0105): frame identity and maker notes kept in
//! `DNGPrivateData`.

use ps_fixtures::dng::converted_burst_frame;
use ps_model::{FrameId, Metadata};

fn metadata(bytes: &[u8]) -> Metadata {
    let probe = ps_media::probe_bytes(bytes, bytes.len() as u64, None);
    ps_media::extract_metadata(
        bytes,
        &probe,
        None,
        &ps_media::DateContext {
            rel_path: None,
            zone: &ps_media::FixedZone(0),
        },
    )
}

fn subsec(m: &Metadata) -> Option<&str> {
    m.fields
        .all_tags
        .iter()
        .find(|(_, n, _)| n == "SubSecTimeOriginal")
        .map(|(_, _, v)| v.as_str())
}

#[test]
fn scenario_media_metadata_converted_dng_records_its_frame() {
    let f = converted_burst_frame(7, 0, 3, 960);
    assert_eq!(f.file_number, 1_010_034);
    let dng = metadata(&f.dng);
    let id = dng.fields.frame_id.clone().expect("frame identity");
    assert_eq!(id.raw_name.as_deref(), Some("IMG_0034"));
    assert_eq!(id.file_number, Some(1_010_034));
    assert_eq!(FrameId::file_number_label(1_010_034), "101-0034");
    assert!(dng.fields.has_maker_notes, "maker notes in DNGPrivateData");
    // The converter dropped the 00 sub-second.
    assert_eq!(subsec(&dng), None);
    // The camera's own files record the same file number from the Exif maker note, and no RAW name.
    for bytes in [&f.cr2, &f.jpeg] {
        let m = metadata(bytes);
        assert_eq!(
            m.fields.frame_id,
            Some(FrameId {
                raw_name: None,
                file_number: Some(1_010_034)
            })
        );
        assert!(m.fields.has_maker_notes);
        assert_eq!(subsec(&m), Some("00"));
    }
    // The EOS 70D path: the file number in CameraInfo instead of tag 0x0008, in all three files.
    let ci = ps_fixtures::dng::converted_burst_frame_camera_info(7, 0, 3, 960);
    for bytes in [&ci.cr2, &ci.jpeg, &ci.dng] {
        let m = metadata(bytes);
        assert_eq!(
            m.fields.frame_id.and_then(|f| f.file_number),
            Some(1_010_034)
        );
    }
    // A JPEG without maker notes (an editor's copy) records no frame.
    let plain = ps_fixtures::media::jpeg(800, 533, 3, 85);
    assert_eq!(metadata(&plain).fields.frame_id, None);
}

#[test]
fn frame_facts_of_truncated_and_damaged_dngs_never_panic() {
    let f = converted_burst_frame(9, 1, 150, 480);
    let full = metadata(&f.dng);
    assert!(full.fields.frame_id.is_some());
    // Every prefix (as a probe window or a damaged file would give).
    let step = (f.dng.len() / 400).max(1);
    for n in (0..f.dng.len()).step_by(step) {
        let b = &f.dng[..n];
        let probe = ps_media::probe_bytes(b, b.len() as u64, None);
        let _ = ps_media::read_frame_facts(b, &probe);
    }
    // Damaged DNGPrivateData: flip bytes through the IFD0 values region.
    let probe = ps_media::probe_bytes(&f.dng, f.dng.len() as u64, None);
    let pos = f
        .dng
        .windows(10)
        .position(|w| w == b"Adobe\0MakN")
        .expect("private data");
    for i in 0..64 {
        let mut d = f.dng.clone();
        d[pos + 6 + i] ^= 0xA5;
        let _ = ps_media::read_frame_facts(&d, &probe);
        let mut d = f.dng.clone();
        d.truncate(pos + 6 + i);
        let p = ps_media::probe_bytes(&d, d.len() as u64, None);
        let _ = ps_media::read_frame_facts(&d, &p);
    }
}

/// The re-read (ADR-0108): whatever the probe window holds, the frame values beyond it are fetched (bounded) and
/// the facts equal those of a full read; a failed fetch leaves them unknown, never "none".
#[test]
fn re_read_fetches_frame_values_beyond_the_window() {
    for f in [
        converted_burst_frame(11, 0, 3, 960),
        ps_fixtures::dng::converted_burst_frame_camera_info(11, 1, 140, 960),
    ] {
        for bytes in [&f.dng, &f.cr2] {
            let full_probe = ps_media::probe_bytes(bytes, bytes.len() as u64, None);
            let full = ps_media::read_frame_facts(bytes, &full_probe);
            assert!(full.frame_id.is_some());
            for cut in (16..bytes.len()).step_by(997) {
                let head = &bytes[..cut];
                let probe = ps_media::probe_bytes(head, bytes.len() as u64, None);
                if probe.kind() != Some(ps_model::MediaKind::Image) {
                    // Too short to tell the format (a real probe window is 256 KB).
                    continue;
                }
                let mut fetched = 0usize;
                let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
                    let off = off as usize;
                    let end = (off + len).min(bytes.len());
                    fetched += end.saturating_sub(off);
                    Ok(bytes.get(off..end).unwrap_or_default().to_vec())
                };
                let got =
                    ps_media::reread_frame_facts(head, bytes.len() as u64, &probe, &mut fetch)
                        .unwrap();
                if let Some(got) = got {
                    assert_eq!(got.frame_id, full.frame_id, "cut at {cut}");
                    assert_eq!(got.maker_notes, full.maker_notes, "cut at {cut}");
                }
                assert!(fetched as u64 <= 3 * 1024 * 1024, "{fetched} bytes fetched");
                // A fetch that fails is an error; one that comes back short leaves the facts unknown.
                let mut short = |_: u64, _: usize| -> Result<Vec<u8>, ()> { Ok(Vec::new()) };
                let s = ps_media::reread_frame_facts(head, bytes.len() as u64, &probe, &mut short)
                    .unwrap();
                if let Some(s) = s {
                    assert!(
                        s.frame_id.is_none() || s.frame_id == full.frame_id,
                        "cut at {cut}: {s:?}"
                    );
                }
            }
            // The whole probe window of a small file is the file: the same facts.
            let probe = ps_media::probe_bytes(bytes, bytes.len() as u64, None);
            let mut none = |_: u64, _: usize| -> Result<Vec<u8>, ()> { Err(()) };
            assert_eq!(
                ps_media::reread_frame_facts(bytes, bytes.len() as u64, &probe, &mut none)
                    .unwrap()
                    .map(|x| x.frame_id),
                Some(full.frame_id.clone())
            );
        }
    }
}
