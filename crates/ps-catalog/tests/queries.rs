//! ID lists, keyset pages and concurrent reads (catalog tasks 3.1-3.3).

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use common::*;
use ps_catalog::{Catalog, FileRecord};
use ps_model::{Filter, IdList, ItemId, MediaKind, PhotoClass, Sort, SortField, SortKey, SourceId};

fn all_records(cat: &Catalog, src: SourceId) -> Vec<FileRecord> {
    let mut out = Vec::new();
    loop {
        let page = cat
            .read()
            .source_files(src, out.last().map(|r: &FileRecord| &r.key.path), 4096)
            .expect("page");
        if page.is_empty() {
            return out;
        }
        out.extend(page);
    }
}

fn assert_keys_consistent(l: &IdList) {
    assert_eq!(l.ids.len(), l.keys.len());
    for (id, k) in l.ids.iter().zip(&l.keys) {
        assert_eq!(k.item(), *id);
    }
    assert!(
        l.keys.windows(2).all(|w| w[0].primary() <= w[1].primary()),
        "primaries non-decreasing"
    );
}

#[test]
fn scenario_catalog_full_id_list_for_filter() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    write_rows(&cat, src, 0..10_000, 2, full_row);
    let recs = all_records(&cat, src);
    assert_eq!(recs.len(), 10_000);

    let list = cat
        .read()
        .id_list(
            &Filter::class(PhotoClass::Photo),
            &Sort::by(SortField::CaptureDate),
        )
        .expect("id list");
    let mut expected: Vec<&FileRecord> = recs
        .iter()
        .filter(|r| r.class == Some(PhotoClass::Photo))
        .collect();
    expected.sort_by_key(|r| (r.capture_ms.is_none(), r.capture_ms, r.id));
    let expected_ids: Vec<ItemId> = expected.iter().map(|r| ItemId(r.id.0 as u32)).collect();
    assert!(expected_ids.len() > 5_000);
    assert!(
        expected.iter().any(|r| r.capture_ms.is_none()),
        "fixture has undated photos"
    );
    assert_eq!(
        list.ids, expected_ids,
        "photo-class files in capture order, ties by id"
    );
    assert_keys_consistent(&list);
    for (k, r) in list.keys.iter().zip(&expected) {
        assert_eq!(k.class(), Some(PhotoClass::Photo));
        match r.capture_ms {
            Some(ms) => assert_eq!(SortKey::primary_signed(k.primary()), ms),
            None => assert_eq!(k.primary(), SortKey::UNDATED),
        }
        assert_eq!(
            k.flags() & SortKey::FLAG_VIDEO != 0,
            r.media_kind() == Some(MediaKind::Video)
        );
    }

    // Other sorts and filters against the same Rust-side oracle.
    let check = |filter: Filter, sort: Sort, keep: &dyn Fn(&FileRecord) -> bool| {
        let got = cat.read().id_list(&filter, &sort).expect("list");
        let mut exp: Vec<&FileRecord> = recs.iter().filter(|r| keep(r)).collect();
        match sort.field {
            SortField::FileSize => exp.sort_by_key(|r| (r.stat.size, r.id)),
            SortField::FileName | SortField::Folder | SortField::Category => exp.sort_by(|a, b| {
                (a.key.path.file_name(), a.id).cmp(&(b.key.path.file_name(), b.id))
            }),
            SortField::CaptureDate => {
                exp.sort_by_key(|r| (r.capture_ms.is_none(), r.capture_ms, r.id))
            }
        }
        if sort.descending {
            let (dated, undated): (Vec<_>, Vec<_>) = exp
                .into_iter()
                .partition(|r| sort.field != SortField::CaptureDate || r.capture_ms.is_some());
            exp = dated
                .into_iter()
                .rev()
                .chain(undated.into_iter().rev())
                .collect();
        }
        let exp: Vec<ItemId> = exp.iter().map(|r| ItemId(r.id.0 as u32)).collect();
        assert_eq!(got.ids, exp, "{filter:?} {sort:?}");
        assert_keys_consistent(&got);
        assert_eq!(cat.read().count(&filter).expect("count"), exp.len() as u64);
    };
    check(Filter::default(), Sort::by(SortField::FileSize), &|_| true);
    check(Filter::default(), Sort::by(SortField::FileName), &|_| true);
    check(
        Filter::default(),
        Sort {
            field: SortField::CaptureDate,
            descending: true,
        },
        &|_| true,
    );
    check(
        Filter {
            classes: Some(vec![PhotoClass::Photo, PhotoClass::LikelyPhoto]),
            kind: Some(MediaKind::Video),
            ..Filter::default()
        },
        Sort {
            field: SortField::FileSize,
            descending: true,
        },
        &|r| {
            matches!(r.class, Some(PhotoClass::Photo | PhotoClass::LikelyPhoto))
                && r.media_kind() == Some(MediaKind::Video)
        },
    );
    check(
        Filter {
            sources: Some(vec![SourceId(99)]),
            ..Filter::default()
        },
        Sort::default(),
        &|_| false,
    );
}

#[test]
fn id_list_queries_use_covering_indexes_without_sorting() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let plans = cat
        .read()
        .with(|c| {
            let mut out = Vec::new();
            for (idx, col) in [
                ("file_by_capture", "capture_ts"),
                ("file_by_size", "size"),
                ("file_by_name", "name"),
            ] {
                let sql = format!(
                    "EXPLAIN QUERY PLAN SELECT id, {col}, kflags
                     FROM file INDEXED BY {idx} WHERE {col} IS NOT NULL AND (kflags & 7) IN (0, 1)
                     AND source_id IN (1) AND kflags < 16 ORDER BY {col}, id"
                );
                let mut s = c.prepare(&sql)?;
                let rows: Vec<String> = s
                    .query_map([], |r| r.get::<_, String>(3))?
                    .collect::<rusqlite::Result<_>>()?;
                out.push(rows.join(" | "));
            }
            Ok(out)
        })
        .expect("plans");
    for p in plans {
        assert!(p.contains("COVERING INDEX"), "{p}");
        assert!(!p.contains("TEMP B-TREE"), "{p}");
    }
}

#[test]
fn scenario_catalog_keyset_page_of_512() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let rows = scale_rows(1_000_000);
    write_rows(&cat, src, 0..rows, 2, full_row);
    let list = cat
        .read()
        .id_list(&Filter::default(), &Sort::default())
        .expect("list");
    assert_eq!(list.len(), rows as usize);
    let mid = list.len() / 2;
    let window = &list.ids[mid..mid + 512];
    let page = cat.read().details(window).expect("page");
    assert_eq!(page.len(), 512);
    let got: Vec<ItemId> = page.iter().map(|r| ItemId(r.id.0 as u32)).collect();
    assert_eq!(got, window, "exactly those items in ID-list order");
    for (r, k) in page.iter().zip(&list.keys[mid..mid + 512]) {
        assert_eq!(
            r.capture_ms
                .map(SortKey::signed_primary)
                .unwrap_or(SortKey::UNDATED),
            k.primary()
        );
    }
    assert!(
        cat.read().details(&list.ids[..513]).is_err(),
        "pages are capped at 512"
    );
}

#[test]
fn scenario_catalog_reads_during_writes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let done = AtomicBool::new(false);
    let queries = AtomicU64::new(0);
    let (elapsed, ()) = std::thread::scope(|s| {
        for _ in 0..2 {
            let readers = cat.readers();
            let (done, queries) = (&done, &queries);
            s.spawn(move || {
                let mut last = 0;
                while !done.load(Ordering::Relaxed) {
                    let l = readers
                        .id_list(&Filter::default(), &Sort::default())
                        .expect("query during writes");
                    assert!(l.len() >= last, "committed rows never disappear");
                    last = l.len();
                    queries.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        let t = Instant::now();
        write_rows(&cat, src, 0..200_000, 2, discovered_row);
        let e = t.elapsed();
        done.store(true, Ordering::Relaxed);
        (e, ())
    });
    let rate = 200_000.0 / elapsed.as_secs_f64();
    let n = queries.load(Ordering::Relaxed);
    println!("reads during writes: {rate:.0} rows/s with {n} concurrent ID-list queries");
    assert!(n >= 2, "queries ran concurrently");
    // Throughput claims hold for optimized builds (`cargo test --release`, `cargo xtask bench`); debug
    // builds report the rate only.
    if !cfg!(debug_assertions) {
        assert!(rate >= 50_000.0, "write rate {rate:.0} rows/s below 50,000");
    }
    assert_eq!(
        cat.read().count(&Filter::default()).expect("count"),
        200_000
    );
}
