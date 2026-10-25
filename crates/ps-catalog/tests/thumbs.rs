//! `thumbs.db`, the content-addressed thumbnail store (media-analysis task 5.4, ADR-0016).

use ps_catalog::{CatalogError, ThumbStore, db};
use ps_model::{
    ContentId, ContentIdKind, Format, MediaKind, PlaceholderInfo, ThumbKind, ThumbnailRecord,
};

/// A stand-in for `ps-media`'s encoded 384 px JPEG: the store only needs bytes with a JPEG signature.
fn jpeg(seed: u8) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
    v.extend((0..30_000u32).map(|i| (i as u8).wrapping_mul(seed)));
    v.extend([0xFF, 0xD9]);
    v
}

fn image_thumb(seed: u8) -> ThumbnailRecord {
    ThumbnailRecord {
        kind: ThumbKind::Image,
        width: 384,
        height: 288,
        dominant_rgb: [seed, 40, 200],
        jpeg: jpeg(seed),
        placeholder: None,
    }
}

fn identity_of(bytes: &[u8]) -> ContentId {
    ContentId::full(*blake3::hash(bytes).as_bytes())
}

#[test]
fn scenario_thumbnails_duplicates_share_one_thumbnail() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let photo: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    // Three byte-identical copies in different folders.
    let mut ids = Vec::new();
    for dir in ["a", "b/c", "backup"] {
        let d = tmp.path().join("source").join(dir);
        std::fs::create_dir_all(&d).expect("mkdir");
        std::fs::write(d.join("IMG_0001.JPG"), &photo).expect("write");
        ids.push(identity_of(
            &std::fs::read(d.join("IMG_0001.JPG")).expect("read"),
        ));
    }
    let store = ThumbStore::open(&tmp.path().join("data")).expect("open");
    for id in &ids {
        // The scanner checks before encoding; storing again is an idempotent replace either way.
        if !store.contains(id).expect("contains") {
            store.put(id, &image_thumb(9)).expect("put");
        }
        store.put(id, &image_thumb(9)).expect("put again");
    }
    assert!(ids.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(
        store.count().expect("count"),
        1,
        "one thumbnail for the content identity"
    );
    assert_eq!(store.get(&ids[2]).expect("get"), Some(image_thumb(9)));
    // A different photo gets its own row.
    let other = identity_of(b"another photo");
    store.put(&other, &image_thumb(3)).expect("put");
    assert_eq!(store.count().expect("count"), 2);
}

#[test]
fn scenario_thumbnails_thumbnail_available_offline() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let source = tmp.path().join("USB-DRIVE");
    std::fs::create_dir_all(source.join("2014")).expect("mkdir");
    let photo: Vec<u8> = (0..100_000u32).map(|i| (i % 241) as u8).collect();
    std::fs::write(source.join("2014/IMG_0042.JPG"), &photo).expect("write");
    let id = identity_of(&std::fs::read(source.join("2014/IMG_0042.JPG")).expect("read"));
    let data = tmp.path().join("data");
    {
        let store = ThumbStore::open(&data).expect("open");
        store.put(&id, &image_thumb(77)).expect("put");
    }
    // The source folder goes away (renamed / unplugged); the store is reopened later.
    std::fs::rename(&source, tmp.path().join("renamed-away")).expect("rename");
    assert!(!source.exists());
    let store = ThumbStore::open(&data).expect("reopen");
    let t = store.get(&id).expect("get").expect("still stored");
    assert_eq!(t.jpeg, jpeg(77));
    assert_eq!(t.dominant_rgb, [77, 40, 200]);
    assert_eq!(
        store
            .dominant_colours(&[id, identity_of(b"never stored")])
            .expect("colours"),
        vec![Some([77, 40, 200]), None]
    );
    // Only thumbs.db (and its WAL files) lives in the data directory.
    for e in std::fs::read_dir(&data).expect("read dir") {
        let name = e.expect("entry").file_name();
        assert!(name.to_string_lossy().starts_with("thumbs.db"), "{name:?}");
    }
}

#[test]
fn placeholders_kinds_and_validation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = ThumbStore::open(tmp.path()).expect("open");
    let id = ContentId::sampled([5; 32]);
    let placeholder = ThumbnailRecord {
        kind: ThumbKind::Placeholder,
        width: 0,
        height: 0,
        dominant_rgb: [10, 10, 10],
        jpeg: Vec::new(),
        placeholder: Some(PlaceholderInfo {
            media_kind: MediaKind::Video,
            format: Some(Format::Mov),
            duration_ms: Some(12_500),
            capture_date: None,
            note: "no poster-frame tool configured".into(),
            reason: None,
        }),
    };
    store.put(&id, &placeholder).expect("put");
    assert_eq!(store.get(&id).expect("get"), Some(placeholder.clone()));
    // Replacing with a poster frame later (tool configured).
    let poster = ThumbnailRecord {
        kind: ThumbKind::PosterFrame,
        ..image_thumb(1)
    };
    store.put(&id, &poster).expect("replace");
    assert_eq!(
        store.get(&id).expect("get").map(|t| t.kind),
        Some(ThumbKind::PosterFrame)
    );
    assert_eq!(store.count().expect("count"), 1);
    assert_eq!(id.kind, ContentIdKind::Sampled);

    let not_jpeg = ThumbnailRecord {
        jpeg: b"PNG...".to_vec(),
        ..image_thumb(1)
    };
    assert!(matches!(
        store.put(&id, &not_jpeg),
        Err(CatalogError::Invalid(_))
    ));
    let pixels_in_placeholder = ThumbnailRecord {
        jpeg: jpeg(1),
        ..placeholder
    };
    assert!(matches!(
        store.put(&id, &pixels_in_placeholder),
        Err(CatalogError::Invalid(_))
    ));
    // Batch insert in one transaction.
    let batch: Vec<(ContentId, ThumbnailRecord)> = (100..150u8)
        .map(|i| (ContentId::full([i; 32]), image_thumb(i)))
        .collect();
    store
        .put_many(batch.iter().map(|(i, t)| (i, t)))
        .expect("batch");
    assert_eq!(store.count().expect("count"), 51);
}

#[test]
fn newer_thumbs_db_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    drop(ThumbStore::open(tmp.path()).expect("create"));
    let path = tmp.path().join(ps_catalog::THUMBS_FILE);
    let newer = db::latest(db::THUMB_MIGRATIONS) + 1;
    rusqlite::Connection::open(&path)
        .expect("raw")
        .pragma_update(None, "user_version", newer)
        .expect("bump");
    assert!(matches!(
        ThumbStore::open(tmp.path()),
        Err(CatalogError::NewerSchema { found, .. }) if found == newer
    ));
}
