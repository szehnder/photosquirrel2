//! tiff-support task 7.1: the 2048 px preview of TIFFs whose directories follow their pixel data is upright, also
//! above 64 MB, where it is decoded from the file in bounded memory (ADR-0086).

#![allow(clippy::unwrap_used)]

use ps_fixtures::media::{card_colour, oriented_card_rgb};
use ps_fixtures::tiff_image::{Sample, TiffSpec, tiff_image};
use ps_model::library::Preview;
use ps_service::library::Library;

#[test]
fn tiff_previews_are_upright_with_their_aspect_ratio() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    std::fs::create_dir_all(&root).unwrap();
    // Display 1600x2400 (orientation 6) in 8 bits, and 2800x4200 in 16 bits (above 64 MB).
    for (name, dw, dh, sample) in [
        ("small.tif", 1600u32, 2400u32, Sample::U8),
        ("large.tif", 2800, 4200, Sample::U16),
    ] {
        let (rgb, sw, sh) = oriented_card_rgb(dw, dh, 6);
        let t = tiff_image(&TiffSpec {
            pixels: Some(rgb),
            sample,
            orientation: Some(6),
            ..TiffSpec::new(sw, sh, 0)
        });
        std::fs::write(root.join(name), t).unwrap();
    }
    assert!(std::fs::metadata(root.join("large.tif")).unwrap().len() > 64 * 1024 * 1024);
    let data = tmp.path().join("data");
    let lib = Library::new(&data);
    let svc = lib.scan().unwrap();
    let id = svc.add_source(&root).unwrap().source.id;
    svc.start(&[id]).unwrap();
    svc.wait().unwrap();
    let rows = svc
        .engine()
        .catalog()
        .read()
        .source_files(id, None, 100)
        .unwrap();
    assert_eq!(rows.len(), 2);
    for r in rows {
        let name = r.key.path.display().into_owned();
        let (dw, dh) = if name == "small.tif" {
            (1600, 2400)
        } else {
            (2800, 4200)
        };
        let Preview::Jpeg(jpeg) = lib
            .preview(ps_model::ItemId::try_from(r.id).unwrap())
            .unwrap()
        else {
            panic!("{name}: no preview");
        };
        let (img, _) = ps_media::decode::decode_jpeg(&jpeg, 4096).unwrap();
        assert_eq!((img.width, img.height), (1365, 2048), "{name}");
        for (x, y) in [(20, 20), (1345, 20), (20, 2028), (1345, 2028)] {
            let want = card_colour(x * dw / 1365, y * dh / 2048, dw, dh);
            let got = img.pixel(x, y);
            let diff = (0..3).map(|c| got[c].abs_diff(want[c])).max().unwrap();
            assert!(diff <= 12, "{name} at ({x},{y}): {got:?} vs {want:?}");
        }
    }
}
