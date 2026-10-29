//! adobe-rgb-color (ADR-0113) in the service: large previews are converted to sRGB and cached under a new key, so a
//! preview cached before the change is never served again; the item detail names a colour space that is not sRGB.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ps_fixtures::colour as cfx;
use ps_media::preview_cache::PreviewCache;
use ps_model::ItemId;
use ps_model::library::Preview;
use ps_service::library::{Library, preview_cache_key};

const ADOBE: &str = "_MG_0001.JPG";
const UNTAGGED: &str = "IMG_0002.JPG";

fn centre(jpeg: &[u8]) -> [u8; 3] {
    let (img, _) = ps_media::decode::decode_jpeg(jpeg, 1).unwrap();
    img.pixel(img.width / 2, img.height / 2)
}

#[test]
fn a_large_preview_cached_under_the_old_key_is_not_returned() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("card");
    std::fs::create_dir_all(&root).unwrap();
    let adobe = cfx::adobe_rgb_jpeg(cfx::GREEN);
    std::fs::write(root.join(ADOBE), &adobe).unwrap();
    std::fs::write(root.join(UNTAGGED), cfx::untagged_jpeg([200, 120, 40])).unwrap();
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
        .source_files(id, None, 10)
        .unwrap();
    let row = |name: &str| {
        rows.iter()
            .find(|r| r.key.path.display() == name)
            .unwrap()
            .clone()
    };
    let r = row(ADOBE);
    let identity = r.identity.unwrap();

    // What the build before the change cached for it: the unconverted preview, under the identity's hex.
    let stale = cfx::untagged_jpeg(cfx::GREEN);
    {
        let mut c = PreviewCache::open(&data.join("previews"), 1 << 30).unwrap();
        c.insert(&identity.to_hex(), &stale).unwrap();
    }
    let item = ItemId::try_from(r.id).unwrap();
    let Preview::Jpeg(jpeg) = lib.preview(item).unwrap() else {
        panic!("no preview");
    };
    assert_ne!(jpeg, stale, "the preview cached before is not served");
    let probe = ps_media::probe_buffer(&adobe, None);
    let (fresh, _, _) = ps_media::thumb::make_large_preview(
        &adobe,
        &probe,
        ps_service::library::PREVIEW_LONG_EDGE,
        &ps_media::Decoders::none(),
    )
    .unwrap();
    assert_eq!(jpeg, fresh, "made again, converted");
    let [r0, g0, b0] = centre(&stale);
    let [r1, g1, b1] = centre(&jpeg);
    assert!(
        r1 + 20 < r0 && g1 >= g0 && b1 + 8 < b0,
        "{:?} -> {:?}",
        (r0, g0, b0),
        (r1, g1, b1)
    );
    // Cached under the new key: 64 hex characters.
    let key = preview_cache_key(&identity);
    assert_eq!(key.len(), 64);
    let mut c = PreviewCache::open(&data.join("previews"), 1 << 30).unwrap();
    assert_eq!(c.get(&key), Some(jpeg));

    // The inspector's detail names the colour space only when it is not sRGB.
    let d = lib.detail(item).unwrap().unwrap();
    assert_eq!(d.colour_space.as_deref(), Some("Adobe RGB (1998)"));
    let u = ItemId::try_from(row(UNTAGGED).id).unwrap();
    assert_eq!(lib.detail(u).unwrap().unwrap().colour_space, None);
}
