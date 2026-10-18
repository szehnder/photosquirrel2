//! Every media fixture generator produces non-empty output that is identical for the same seed.

use ps_fixtures::media::*;

fn all(seed: u64) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = media_set(seed)
        .into_iter()
        .map(|f| (f.path, f.bytes))
        .collect();
    let (q60, q92) = quality_pair(seed);
    v.push(("q60".into(), q60));
    v.push(("q92".into(), q92));
    let (orig, edited) = camera_original_and_edited(seed);
    v.push(("orig".into(), orig));
    v.push(("edited".into(), edited));
    v.push(("40tags".into(), jpeg_with_40_tags(seed).0));
    v.push((
        "smallthumb".into(),
        jpeg_with_small_exif_thumbnail(640, 480, seed),
    ));
    v.push(("dng1616".into(), dng_with_preview(1616, 1080, 64, 48, seed)));
    v.push((
        "gps".into(),
        jpeg_with_exif(
            64,
            48,
            seed,
            80,
            &ExifSpec {
                gps: Some(GpsSpec::eiffel()),
                ..Default::default()
            },
        ),
    ));
    v
}

#[test]
fn media_generators_are_non_empty_and_deterministic() {
    let a = all(7);
    let b = all(7);
    assert_eq!(a.len(), b.len());
    for ((name, x), (_, y)) in a.iter().zip(&b) {
        assert!(!x.is_empty(), "{name} is empty");
        assert_eq!(x, y, "{name} differs between runs with the same seed");
    }
    let c = all(8);
    let differs = a.iter().zip(&c).filter(|((_, x), (_, y))| x != y).count();
    assert!(differs > 5, "seed should change most fixtures");
    assert_eq!(jpeg_with_40_tags(1).1, 40);
}
