//! The catalog-backed AI queue (ai-insights tasks 2.1, 2.3, 2.5): keeper-first order on a scanned corpus, and
//! a queue that survives being killed mid-run.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ps_catalog::Catalog;
use ps_fixtures::{DirWriter, Params, Preset};
use ps_llm::queue::{PreviewLoader, QueueConfig, QueueState};
use ps_llm::testing::{MockServer, Reply, insight_json, openai_reply, preview};
use ps_llm::{PRESETS, PreviewError, PreviewJpeg, ProviderConfig, RetryPolicy};
use ps_model::{ContentId, FileId, PhotoClass};
use ps_service::ai::{AiService, status};
use ps_service::scan::{ScanOptions, ScanService};

fn cfg_for(url: &str) -> ProviderConfig {
    let mut cfg = ProviderConfig::from_preset(&PRESETS[0]);
    cfg.base_url = format!("{url}/v1");
    cfg.model = "recording-vlm".into();
    cfg.retry = RetryPolicy {
        max_retries: 2,
        rate_limit_base: Duration::from_millis(10),
        server_backoff: vec![Duration::from_millis(10), Duration::from_millis(20)],
        max_retry_after: Duration::from_secs(1),
    };
    cfg
}

fn ok_reply() -> Reply {
    Reply::json(
        200,
        &openai_reply(&insight_json("people", 2, 1, &[]).to_string()),
    )
}

fn qc(cfg: &ProviderConfig) -> QueueConfig {
    let mut q = QueueConfig::local(&cfg.name);
    q.concurrency = 1;
    q
}

/// Records the order in which contents are loaded (= requested, with concurrency 1).
struct OrderLoader(Mutex<Vec<ContentId>>);

impl PreviewLoader for OrderLoader {
    fn load(&self, c: &ContentId) -> Result<PreviewJpeg, PreviewError> {
        self.0.lock().unwrap().push(*c);
        Ok(preview(u64::from(c.digest[0])))
    }
}

fn tiny() -> Params {
    Params {
        standalone: 10,
        sets_per_kind: 2,
        burst_sets: 2,
        videos: 1,
        scans: 1,
        icons: 2,
        ui_graphics: 2,
        tiny_web: 2,
        junk_videos: 1,
        raw_share: 0.25,
        dim_scale: (1, 2),
        twelve_mp: 0,
        backups: (1, 2),
    }
}

#[test]
fn scenario_ai_queue_keepers_processed_first() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("corpus");
    let w = DirWriter::new(&root).unwrap();
    ps_fixtures::generate_with(5, Preset::Small, tiny(), &w).unwrap();
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&root.join(d)).unwrap();
    }
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let catalog = svc.engine().catalog().clone();

    // Keeper and non-keeper contents, and the tier of each keeper content.
    let r = catalog.read();
    let mut keeper_files: HashSet<FileId> = HashSet::new();
    let mut grouped: HashSet<FileId> = HashSet::new();
    for (_, members, set) in r.group_memberships().unwrap() {
        grouped.extend(members);
        keeper_files.extend(set);
    }
    let mut photo_keepers: HashSet<ContentId> = HashSet::new();
    let mut keepers: HashSet<ContentId> = HashSet::new();
    r.for_each_file(|f| {
        let Some(c) = f.identity else { return };
        if keeper_files.contains(&f.id) || !grouped.contains(&f.id) {
            keepers.insert(c);
            if matches!(f.class, Some(PhotoClass::Photo | PhotoClass::LikelyPhoto)) {
                photo_keepers.insert(c);
            }
        }
    })
    .unwrap();
    assert!(
        grouped.len() > keeper_files.len(),
        "the corpus has non-keepers"
    );

    let server = MockServer::with_handler(|_| ok_reply());
    let cfg = cfg_for(&server.url());
    let loader = Arc::new(OrderLoader(Mutex::new(Vec::new())));
    let ai = AiService::with_loader(catalog.clone(), cfg.clone(), qc(&cfg), loader.clone())
        .expect("ai service");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(server.request_count(), 0, "nothing sent before start");
    let queued = ai.start(false).unwrap();
    assert!(queued > 0);
    assert!(ai.wait_idle(Duration::from_secs(60)));
    let order = loader.0.lock().unwrap().clone();
    assert_eq!(order.len() as u64, queued);
    assert_eq!(server.request_count() as u64, queued);
    let first_other = order
        .iter()
        .position(|c| !photo_keepers.contains(c))
        .unwrap_or(order.len());
    assert!(
        order[first_other..]
            .iter()
            .all(|c| !photo_keepers.contains(c)),
        "every photo keeper is requested before any other item"
    );
    assert!(
        order.iter().all(|c| keepers.contains(c)),
        "no non-keeper is requested"
    );
    let s = status(&catalog).unwrap();
    assert_eq!((s.done, s.pending), (queued, 0));
    // RAW+JPEG pairs are described once; their other content gets the same result.
    assert!(s.results >= queued, "{} results for {queued}", s.results);
    assert_eq!(ai.state(), QueueState::Finished);
}

/// Keeper sets of several contents (RAW+JPEG pairs) in a scanned corpus, as (JPEG content, other contents).
fn capture_pairs(catalog: &Catalog) -> Vec<(ContentId, Vec<ContentId>)> {
    let r = catalog.read();
    let mut out = Vec::new();
    for (_, _, set) in r.group_memberships().unwrap() {
        let recs: Vec<_> = set
            .iter()
            .filter_map(|f| r.file_by_id(*f).unwrap())
            .collect();
        let jpeg = recs
            .iter()
            .find(|f| f.format == Some(ps_model::Format::Jpeg))
            .and_then(|f| f.identity);
        let mut others: Vec<ContentId> = recs
            .iter()
            .filter(|f| f.format.is_some_and(|x| x.is_raw()))
            .filter_map(|f| f.identity)
            .collect();
        others.dedup();
        if let Some(j) = jpeg
            && !others.is_empty()
            && !others.contains(&j)
        {
            out.push((j, others));
        }
    }
    out
}

#[test]
fn raw_jpeg_pairs_are_described_once_through_the_jpeg() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("corpus");
    let w = DirWriter::new(&root).unwrap();
    let mut params = tiny();
    params.raw_share = 1.0;
    ps_fixtures::generate_with(9, Preset::Small, params, &w).unwrap();
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&root.join(d)).unwrap();
    }
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let catalog = svc.engine().catalog().clone();
    let pairs = capture_pairs(&catalog);
    assert!(pairs.len() >= 2, "the corpus has RAW+JPEG pairs: {pairs:?}");
    // A queue from before pairs were described once: the RAW content was queued on its own.
    let (_, raws) = &pairs[0];
    catalog
        .writer()
        .ai_enqueue(vec![(raws[0], i64::MIN / 2)])
        .unwrap();

    let server = MockServer::with_handler(|_| {
        Reply::json(
            200,
            &openai_reply(&insight_json("pets", 0, 0, &["dog"]).to_string()),
        )
    });
    let cfg = cfg_for(&server.url());
    let loader = Arc::new(OrderLoader(Mutex::new(Vec::new())));
    let ai = AiService::with_loader(catalog.clone(), cfg.clone(), qc(&cfg), loader.clone())
        .expect("ai service");
    ai.start(false).unwrap();
    assert!(ai.wait_idle(Duration::from_secs(60)));
    let order = loader.0.lock().unwrap().clone();
    assert_eq!(server.request_count(), order.len());
    let unique: HashSet<_> = order.iter().collect();
    assert_eq!(unique.len(), order.len(), "no content is described twice");
    let r = catalog.read();
    for (jpeg, raws) in &pairs {
        assert!(order.contains(jpeg), "the pair's JPEG is described");
        for raw in raws {
            assert!(!order.contains(raw), "the pair's RAW is not sent");
            let a = r.ai_result_for(jpeg).unwrap().expect("JPEG result");
            let b = r
                .ai_result_for(raw)
                .unwrap()
                .expect("RAW gets the same result");
            assert_eq!(a.insight, b.insight);
        }
    }
    assert_eq!(status(&catalog).unwrap().pending, 0);

    // AI Review lists each pair once, with both files; a correction applies to both contents.
    let store = ps_service::insights::InsightStore::in_memory();
    let items = ps_service::insights::items(
        &catalog,
        &store,
        &ps_service::insights::InsightFilter::default(),
        10_000,
    )
    .unwrap();
    for (jpeg, raws) in &pairs {
        let listed: Vec<_> = items
            .iter()
            .filter(|i| i.content == jpeg.to_hex() || raws.iter().any(|r| i.content == r.to_hex()))
            .collect();
        assert_eq!(listed.len(), 1, "one AI Review item per pair");
        assert_eq!(listed[0].content, jpeg.to_hex());
        let raw_files = r.files_with_content(&raws[0]).unwrap();
        assert!(raw_files.iter().all(|f| listed[0].files.contains(f)));
    }
    let (jpeg, raws) = &pairs[0];
    ps_service::insights::correct(
        &catalog,
        &store,
        &jpeg.to_hex(),
        ps_service::insights::Correction {
            category: Some(ps_model::SceneCategory::Travel),
            ..Default::default()
        },
    )
    .unwrap();
    let raw_effective = ps_service::insights::effective(&catalog, &store, &raws[0])
        .unwrap()
        .unwrap();
    assert_eq!(raw_effective.category, ps_model::SceneCategory::Travel);
}

const CHILD_ENV: &str = "PS_AI_RESTART_CHILD";

fn synthetic(n: u8) -> ContentId {
    let mut d = [0u8; 32];
    d[0] = n;
    d[31] = 0x5A;
    ContentId::full(d)
}

fn fixed_loader() -> Arc<dyn PreviewLoader> {
    Arc::new(|c: &ContentId| -> Result<PreviewJpeg, PreviewError> {
        Ok(preview(u64::from(c.digest[0])))
    })
}

/// The child process: runs the queue until it is killed.
#[test]
#[ignore = "child process of scenario_ai_queue_queue_survives_restart"]
fn ai_restart_child() {
    let Ok(spec) = std::env::var(CHILD_ENV) else {
        return;
    };
    let (data, url) = spec.split_once('|').unwrap();
    let catalog = Arc::new(Catalog::open(Path::new(data)).unwrap());
    let cfg = cfg_for(url);
    let ai =
        AiService::with_loader(catalog, cfg.clone(), qc(&cfg), fixed_loader()).expect("ai service");
    ai.start(false).unwrap();
    std::thread::sleep(Duration::from_secs(600));
}

#[test]
fn scenario_ai_queue_queue_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    {
        let catalog = Catalog::open(&data).unwrap();
        let items: Vec<(ContentId, i64)> =
            (0..50u8).map(|i| (synthetic(i), i64::from(i))).collect();
        assert_eq!(catalog.writer().ai_enqueue(items).unwrap(), 50);
        catalog.close().unwrap();
    }
    // After the 10th reply every request hangs until the child is killed.
    let seen = Arc::new(AtomicUsize::new(0));
    let slow = Arc::new(AtomicBool::new(true));
    let server = {
        let (seen, slow) = (seen.clone(), slow.clone());
        MockServer::with_handler(move |_| {
            let n = seen.fetch_add(1, Ordering::SeqCst) + 1;
            if n > 10 && slow.load(Ordering::SeqCst) {
                ok_reply().with_delay(Duration::from_secs(30))
            } else {
                ok_reply()
            }
        })
    };
    let exe = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(exe)
        .args(["--exact", "ai_restart_child", "--ignored", "--nocapture"])
        .env(CHILD_ENV, format!("{}|{}", data.display(), server.url()))
        .spawn()
        .unwrap();
    let t = Instant::now();
    while seen.load(Ordering::SeqCst) < 11 {
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "child made no progress"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(200));
    child.kill().unwrap();
    child.wait().unwrap();
    slow.store(false, Ordering::SeqCst);
    let before = seen.load(Ordering::SeqCst);

    // Restart: 10 done, nothing in flight, paused; resuming sends only the 40 remaining items.
    let catalog = Arc::new(Catalog::open(&data).unwrap());
    let cfg = cfg_for(&server.url());
    let ai = AiService::with_loader(catalog.clone(), cfg.clone(), qc(&cfg), fixed_loader())
        .expect("ai service");
    let s = status(&catalog).unwrap();
    assert_eq!((s.done, s.pending, s.in_flight), (10, 40, 0), "{s:?}");
    assert_eq!(s.state, "paused");
    assert_eq!(ai.state(), QueueState::Paused);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        seen.load(Ordering::SeqCst),
        before,
        "nothing sent while paused"
    );
    ai.start(false).unwrap();
    assert!(ai.wait_idle(Duration::from_secs(60)));
    assert_eq!(seen.load(Ordering::SeqCst) - before, 40);
    let s = status(&catalog).unwrap();
    assert_eq!((s.done, s.results), (50, 50));
}

/// Scan the tiny corpus and describe every keeper once with the current prompt; then mark `n` results as
/// coming from prompt v1 (the PROMPT_VERSION before the pets/animals wording change).
fn described_with_v1_results(
    tmp: &Path,
    n: usize,
) -> (
    ScanService,
    Arc<Catalog>,
    MockServer,
    ProviderConfig,
    Vec<ContentId>,
) {
    let root = tmp.join("corpus");
    let w = DirWriter::new(&root).unwrap();
    ps_fixtures::generate_with(5, Preset::Small, tiny(), &w).unwrap();
    let svc = ScanService::open(&tmp.join("data"), ScanOptions::default()).unwrap();
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&root.join(d)).unwrap();
    }
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let catalog = svc.engine().catalog().clone();
    let server = MockServer::with_handler(|_| ok_reply());
    let cfg = cfg_for(&server.url());
    {
        let ai = AiService::with_loader(catalog.clone(), cfg.clone(), qc(&cfg), fixed_loader())
            .expect("ai service");
        ai.start(false).unwrap();
        assert!(ai.wait_idle(Duration::from_secs(60)));
    }
    // Results stored alone (a RAW+JPEG pair's two contents share one request and one time).
    let all = catalog.read().ai_results(10_000).unwrap();
    let rows: Vec<_> = all
        .iter()
        .filter(|r| all.iter().filter(|o| o.at_ms == r.at_ms).count() == 1)
        .take(n)
        .cloned()
        .collect();
    assert_eq!(rows.len(), n);
    let mut old = Vec::new();
    for row in rows {
        assert_eq!(row.prompt_version, ps_llm::PROMPT_VERSION);
        old.push(row.content);
        catalog
            .writer()
            .ai_complete(ps_catalog::AiResultRow {
                prompt_version: 1,
                ..row
            })
            .unwrap();
    }
    (svc, catalog, server, cfg, old)
}

#[test]
fn scenario_ai_queue_older_prompt_results_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let (_svc, catalog, server, cfg, old) = described_with_v1_results(tmp.path(), 3);
    let sent = server.request_count();
    let results = status(&catalog).unwrap().results;
    // A queue with the current prompt starts again: the v1 results are kept, nothing is re-sent.
    let ai = AiService::with_loader(catalog.clone(), cfg.clone(), qc(&cfg), fixed_loader())
        .expect("ai service");
    assert_eq!(ai.start(false).unwrap(), 0);
    assert!(ai.wait_idle(Duration::from_secs(30)));
    assert_eq!(
        server.request_count(),
        sent,
        "older results are not re-queued"
    );
    assert_eq!(status(&catalog).unwrap().results, results);
    assert_eq!(ps_service::ai::older_prompt_contents(&catalog).unwrap(), 3);
    // Shown as described with an older prompt.
    let store = ps_service::insights::InsightStore::for_catalog(&catalog);
    let items = ps_service::insights::items(
        &catalog,
        &store,
        &ps_service::insights::InsightFilter::default(),
        1_000,
    )
    .unwrap();
    for c in &old {
        let item = items
            .iter()
            .find(|i| i.content == c.to_hex())
            .expect("listed");
        assert_eq!(item.prompt_version, 1);
        assert!(item.older_prompt());
    }
    assert!(
        items
            .iter()
            .filter(|i| !old.iter().any(|c| c.to_hex() == i.content))
            .all(|i| !i.older_prompt())
    );
    let r = ps_service::ai::results(&catalog, 1_000).unwrap();
    assert_eq!(r.iter().filter(|x| x.older_prompt).count(), 3);
}

#[test]
fn scenario_ai_queue_re_describe_with_current_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let (_svc, catalog, server, cfg, old) = described_with_v1_results(tmp.path(), 3);
    let sent = server.request_count();
    // The explicit request queues exactly the older results and sends nothing by itself.
    assert_eq!(
        ps_service::ai::redescribe_older_prompts(&catalog).unwrap(),
        3
    );
    let s = status(&catalog).unwrap();
    assert_eq!(s.pending, 3);
    let ai = AiService::with_loader(catalog.clone(), cfg.clone(), qc(&cfg), fixed_loader())
        .expect("ai service");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(server.request_count(), sent, "nothing sent before start");
    ai.start(false).unwrap();
    assert!(ai.wait_idle(Duration::from_secs(60)));
    assert_eq!(server.request_count(), sent + 3, "each once");
    let r = catalog.read();
    for c in &old {
        let row = r.ai_result_for(c).unwrap().expect("result");
        assert_eq!(row.prompt_version, ps_llm::PROMPT_VERSION);
    }
    assert_eq!(ps_service::ai::older_prompt_contents(&catalog).unwrap(), 0);
    assert_eq!(status(&catalog).unwrap().pending, 0);
}

/// ADR-0049: `settings.toml` can change while a queue runs (another process, an editor). A CLI queue re-checks
/// the file before every request and pauses when the destination or the consent changed there; the app's next
/// save keeps a consent the file withdrew withdrawn.
#[test]
fn scenario_ai_queue_consent_withdrawn_in_the_settings_file() {
    use ps_llm::queue::{AiQueue, MemoryStore};
    use ps_model::settings::{ProviderId, Settings};
    use ps_service::settings::SettingsStore;

    let tmp = tempfile::tempdir().unwrap();
    let store = SettingsStore::in_dir(tmp.path().join("config"));

    // 1. A CLI queue: the server address is changed in the file while it runs.
    let server = MockServer::with_handler(|_| {
        std::thread::sleep(Duration::from_millis(30));
        ok_reply()
    });
    let cfg = cfg_for(&server.url());
    let mut settings = Settings::default();
    settings.ai.base_url = cfg.base_url.clone();
    settings.ai.model = cfg.model.clone();
    store.save(&settings).unwrap();
    let items = Arc::new(MemoryStore::new());
    for i in 0..40u8 {
        let mut d = [0u8; 32];
        d[0] = i;
        items.enqueue(ContentId::full(d), i64::from(i));
    }
    let queue = AiQueue::new(
        items,
        Arc::new(|c: &ContentId| -> Result<PreviewJpeg, PreviewError> {
            Ok(preview(u64::from(c.digest[0])))
        }),
        ps_llm::Describer::new(ps_llm::provider_for(cfg.clone()).unwrap()),
        qc(&cfg),
        ps_llm::LlmRuntime::shared().handle(),
    );
    queue.set_send_gate(ps_service::ai::settings_file_gate(store.clone(), &settings));
    queue.start().unwrap();
    let t = Instant::now();
    while server.request_count() < 2 && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
    }
    store
        .update(|s| s.ai.base_url = "http://127.0.0.1:9/v1".into())
        .unwrap();
    assert!(queue.wait_idle(Duration::from_secs(10)));
    assert_eq!(
        queue.state(),
        QueueState::Paused,
        "the file changed: paused"
    );
    let sent = server.request_count();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.request_count(), sent, "nothing more is sent");
    assert!(sent < 40, "{sent} requests: stopped before the end");

    // 2. A hosted provider: consent withdrawn in the file stops the next request.
    let mut hosted = Settings::default();
    hosted.ai.provider = ProviderId::Anthropic;
    hosted.ai.base_url = ProviderId::Anthropic.default_base_url().into();
    hosted.ai.consent = ps_service::ai::consent_for(&hosted.ai);
    store.save(&hosted).unwrap();
    let gate = ps_service::ai::settings_file_gate(store.clone(), &hosted);
    assert!(gate(), "unchanged file: requests may go");
    store.update(|s| s.ai.consent = None).unwrap();
    assert!(!gate(), "consent withdrawn in the file: the queue pauses");
    store
        .update(|s| s.ai.consent = ps_service::ai::consent_for(&s.ai))
        .unwrap();
    assert!(gate());
    store
        .update(|s| s.ai.base_url = "https://proxy.example.com/v1".into())
        .unwrap();
    assert!(!gate(), "another address in the file");

    // 3. The app: a save after the file withdrew consent does not restore it.
    store.save(&hosted).unwrap();
    let (loaded, mut base) = store.load_tracked();
    let mut ours = loaded.settings.clone();
    assert!(ours.ai.consent.is_some());
    store.update(|s| s.ai.consent = None).unwrap(); // another process withdraws
    ours.appearance.ui_font_size += 1.0; // the app saves an unrelated change
    let saved = store.save_merged(&ours, &mut base).unwrap();
    assert!(saved.ai.consent.is_none(), "consent stays withdrawn");
    assert!(store.load().settings.ai.consent.is_none());
    assert_eq!(saved.appearance.ui_font_size, ours.appearance.ui_font_size);
    // Consent the app gives afterwards (the file unchanged since our save) is written.
    ours.ai.consent = ps_service::ai::consent_for(&ours.ai);
    let saved = store.save_merged(&ours, &mut base).unwrap();
    assert!(saved.ai.consent.is_some());
    // An unrelated edit in the file keeps the app's consent.
    store
        .update(|s| s.appearance.theme = ps_model::settings::ThemeSetting::Light)
        .unwrap();
    let saved = store.save_merged(&ours, &mut base).unwrap();
    assert_eq!(saved.ai.consent, ours.ai.consent);
    assert!(store.load().settings.ai.consent.is_some());
}
