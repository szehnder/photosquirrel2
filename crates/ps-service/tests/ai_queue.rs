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
    let ai = AiService::with_loader(catalog.clone(), cfg.clone(), qc(&cfg), loader.clone());
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
    assert_eq!((s.done, s.pending, s.results), (queued, 0, queued));
    assert_eq!(ai.state(), QueueState::Finished);
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
    let ai = AiService::with_loader(catalog, cfg.clone(), qc(&cfg), fixed_loader());
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
    let ai = AiService::with_loader(catalog.clone(), cfg.clone(), qc(&cfg), fixed_loader());
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
