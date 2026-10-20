//! Migration 5 (ai-insights): a version-4 catalog (before the AI tables) upgrades to the latest schema and the
//! AI tables enforce their keys and codes.
#![allow(clippy::unwrap_used)]

#[test]
fn ai_insights_migration_applies_on_latest_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    assert_eq!(ps_catalog::db::AI_MIGRATION.version, 5);
    ps_catalog::db::create_catalog_at_version(&path, 4).unwrap();
    ps_catalog::Catalog::open_file(&path, Default::default())
        .unwrap()
        .close()
        .unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let v: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, ps_catalog::latest_schema_version());

    for t in [
        "ai_queue",
        "ai_result",
        "ai_correction",
        "ai_suggestion",
        "ai_run",
    ] {
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{t}");
    }
    let cid = vec![7u8; 32];
    conn.execute(
        "INSERT INTO ai_queue (content_id, priority, enqueued_at, updated_at) VALUES (?1, 0, 1, 1)",
        [&cid],
    )
    .unwrap();
    // One queue entry per content identity.
    assert!(conn
        .execute("INSERT INTO ai_queue (content_id, priority, enqueued_at, updated_at) VALUES (?1, 5, 2, 2)", [&cid])
        .is_err());
    let ins = "INSERT INTO ai_result (content_id, provider, model, schema_version, prompt_version, insight, latency_ms, at) \
               VALUES (?1, 'LM Studio (local)', 'm', 1, 1, '{}', 10, 1)";
    conn.execute(ins, [&cid]).unwrap();
    // One result per content, provider, model and schema version.
    assert!(conn.execute(ins, [&cid]).is_err());
    assert!(
        conn.execute("UPDATE ai_queue SET state = 9", []).is_err(),
        "state codes are checked"
    );
    assert!(
        conn.execute(
            "INSERT INTO ai_run (id, state, updated_at) VALUES (2, 0, 1)",
            []
        )
        .is_err(),
        "single row"
    );
}

/// Migration 11: older results keep seq 0; every write (also a re-description in place with an older time)
/// takes the next number, so an incremental reader finds it.
#[test]
fn ai_result_seq_finds_rewrites_whatever_their_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    ps_catalog::db::create_catalog_at_version(
        &path,
        ps_catalog::db::AI_RESULT_SEQ_MIGRATION.version - 1,
    )
    .unwrap();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO ai_result (content_id, provider, model, schema_version, prompt_version, insight,
                 latency_ms, requests, at) VALUES (?1, 'p', 'm', 1, 1, '{}', 1, 1, 500)",
            [vec![1u8; 32]],
        )
        .unwrap();
    }
    let cat = ps_catalog::Catalog::open_file(&path, Default::default()).unwrap();
    assert!(
        cat.read()
            .ai_result_contents_after_seq(0)
            .unwrap()
            .is_empty()
    );
    let row = |d: u8, at_ms: i64| ps_catalog::AiResultRow {
        content: ps_model::ContentId::full([d; 32]),
        provider: "p".into(),
        model: "m".into(),
        schema_version: 1,
        prompt_version: 2,
        insight: "{}".into(),
        latency_ms: 1,
        requests: 1,
        at_ms,
    };
    cat.writer().ai_complete(row(2, 1_000)).unwrap();
    assert_eq!(
        cat.read().ai_result_contents_after_seq(0).unwrap(),
        vec![[2u8; 32]]
    );
    // Content 1 is described again in place, with a time older than every row.
    cat.writer().ai_complete(row(1, 10)).unwrap();
    assert_eq!(
        cat.read().ai_result_contents_after_seq(1).unwrap(),
        vec![[1u8; 32]]
    );
    cat.close().unwrap();
}
