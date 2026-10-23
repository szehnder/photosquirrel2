//! The rescan re-date pass rebuilds a file's date inputs from its stored metadata (ADR-0071): for every
//! analysed file, resolving from the stored facts must give exactly the date analysis gave.

use std::path::Path;
use std::time::{Duration, SystemTime};

use ps_fixtures::media as fx;
use ps_media::{
    DateContext, EmbeddedDates, FixedZone, LocalZone, RuleZone, extract_metadata, probe_bytes,
    resolve_capture_date,
};
use ps_model::{Metadata, RelPath};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const FOLDERS: &[&str] = &[
    "",
    "DCIM/100CANON/",
    "2019-07-04 Beach/",
    "2019_07_04/",
    "20190704/",
    "2019/07/04/",
    "2019-07 Italy/",
    "2019/07/",
    "2019/",
    "Backup 2019-07-04/",
    "2018/2019-07-04 Beach/raw/",
    "1999-12-31/",
];

fn exif_date(rng: &mut ChaCha8Rng) -> String {
    format!(
        "{:04}:{:02}:{:02} {:02}:{:02}:{:02}",
        rng.random_range(1995..2030),
        rng.random_range(1..=12),
        rng.random_range(1..=28),
        rng.random_range(0..24),
        rng.random_range(0..60),
        rng.random_range(0..60)
    )
}

fn offset(rng: &mut ChaCha8Rng) -> String {
    let m: i32 = rng.random_range(-12 * 4..=14 * 4) * 15;
    format!(
        "{}{:02}:{:02}",
        if m < 0 { '-' } else { '+' },
        m.abs() / 60,
        m.abs() % 60
    )
}

#[test]
fn resolving_from_stored_facts_equals_the_analysis_for_1000_files() {
    let zones: [&dyn LocalZone; 5] = [
        &FixedZone(0),
        &FixedZone(330),
        &RuleZone::LOS_ANGELES,
        &RuleZone::NEW_YORK,
        &RuleZone::BERLIN,
    ];
    let mut rng = ChaCha8Rng::seed_from_u64(71);
    let mut kinds = [0usize; 6];
    for i in 0..1000u64 {
        let kind = rng.random_range(0..6usize);
        kinds[kind] += 1;
        let (bytes, name) = match kind {
            0 => (fx::jpeg(32, 24, i, 80), "IMG.JPG"),
            1 => {
                let spec = fx::ExifSpec {
                    date_time_original: Some(exif_date(&mut rng)),
                    offset_time_original: rng.random_bool(0.5).then(|| offset(&mut rng)),
                    ..Default::default()
                };
                (fx::jpeg_with_exif(32, 24, i, 80, &spec), "IMG.JPG")
            }
            2 => {
                let spec = fx::ExifSpec {
                    create_date: Some(exif_date(&mut rng)),
                    ..Default::default()
                };
                (fx::jpeg_with_exif(32, 24, i, 80, &spec), "IMG.JPG")
            }
            3 => {
                // An offset without any date: it makes the file time local.
                let spec = fx::ExifSpec {
                    offset_time_original: Some(offset(&mut rng)),
                    make: Some("Scanner".into()),
                    ..Default::default()
                };
                (fx::jpeg_with_exif(32, 24, i, 80, &spec), "IMG.JPG")
            }
            4 => {
                let spec = fx::MovieSpec {
                    creation_unix: Some(rng.random_range(800_000_000..1_900_000_000)),
                    ..fx::MovieSpec::iphone()
                };
                (fx::movie(&spec), "IMG.MOV")
            }
            _ => {
                let spec = fx::MovieSpec {
                    creation_unix: None,
                    ..fx::MovieSpec::iphone()
                };
                (fx::movie(&spec), "IMG.MOV")
            }
        };
        let folder = FOLDERS[rng.random_range(0..FOLDERS.len())];
        let rel = RelPath::from_bytes(format!("{folder}{name}").into_bytes()).expect("rel");
        let mtime_secs: i64 = rng.random_range(100_000_000..1_900_000_000);
        let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(mtime_secs as u64);
        let zone = zones[rng.random_range(0..zones.len())];
        let probe = probe_bytes(&bytes, bytes.len() as u64, Some(Path::new(name)));
        let analysed = extract_metadata(
            &bytes,
            &probe,
            Some(mtime),
            &DateContext {
                rel_path: Some(&rel),
                zone,
            },
        );
        // Through the catalog's storage format.
        let json = serde_json::to_string(&analysed).expect("json");
        let stored: Metadata = serde_json::from_str(&json).expect("parse");
        let again = resolve_capture_date(
            &EmbeddedDates::from_stored(&stored),
            Some(&rel),
            Some(mtime_secs),
            zone,
        );
        assert_eq!(again, analysed.capture_date, "file {i} at {rel}");
    }
    assert!(kinds.iter().all(|n| *n > 100), "{kinds:?}");
}
