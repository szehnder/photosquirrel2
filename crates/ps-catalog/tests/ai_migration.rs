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
