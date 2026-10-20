//! Shared helpers for the scan integration tests: a cached generated corpus, tree snapshots, catalog dumps.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use ps_catalog::{FileRecord, Readers};
use ps_fixtures::{DirWriter, Preset, Truth};
use ps_model::{ProgressSnapshot, RunStatus, SourceId};
use ps_scan::{Instrumentation, RunSummary, ScanOptions, Scanner, SourceFs};

pub const SEED: u64 = 42;

/// Scans compete for every core. Latency measurements take this exclusively; other heavy tests share it.
pub static CORES: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Shared access for ordinary scanning tests.
pub fn shared() -> std::sync::RwLockReadGuard<'static, ()> {
    CORES
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Exclusive access for latency measurements.
pub fn exclusive() -> std::sync::RwLockWriteGuard<'static, ()> {
    CORES
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The corpus preset for the gate tests: `small` by default, `PS_SCAN_CORPUS=medium` for the medium gate.
pub fn preset() -> Preset {
    std::env::var("PS_SCAN_CORPUS")
        .ok()
        .and_then(|p| Preset::parse(&p))
        .unwrap_or(Preset::Small)
}

pub struct Corpus {
    pub root: PathBuf,
    pub truth: Truth,
}

/// The generated corpus for [`preset`], seed 42, generated once into the target directory and shared by
/// every test (the scanner never writes to it; tests that modify files work on copies).
pub fn corpus() -> &'static Corpus {
    static C: OnceLock<Corpus> = OnceLock::new();
    C.get_or_init(|| {
        let p = preset();
        let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let root = ps_fixtures::shared_dir(
            &base,
            &format!("ps-scan-corpus-{}-{SEED}", p.as_str()),
            |root| {
                let w = DirWriter::new(root).expect("corpus writer");
                let t = Instant::now();
                ps_fixtures::generate(SEED, p, &w).expect("generate corpus");
                eprintln!("generated the {p} corpus in {:?}", t.elapsed());
            },
        );
        let truth = Truth::read(&root).expect("truth.json");
        Corpus { root, truth }
    })
}

/// Copy `src` to `dst` recursively, keeping modification times (rescan fingerprints depend on them).
pub fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for e in std::fs::read_dir(src).expect("read_dir") {
        let e = e.expect("entry");
        let ft = e.file_type().expect("type");
        let to = dst.join(e.file_name());
        if ft.is_dir() {
            copy_tree(&e.path(), &to);
        } else if ft.is_file() {
            std::fs::copy(e.path(), &to).expect("copy");
            let m = e.metadata().expect("meta").modified().expect("mtime");
            let f = std::fs::File::options()
                .write(true)
                .open(&to)
                .expect("open copy");
            f.set_modified(m).expect("set mtime");
        }
    }
}

/// One entry of a tree snapshot: kind, size, mtime, permissions and content hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snap {
    pub dir: bool,
    pub size: u64,
    pub mtime_ns: i128,
    pub mode: u32,
    pub hash: Option<String>,
}

/// Every path under `root` with its metadata and content hash (files readable by the test only).
pub fn snapshot_tree(root: &Path) -> BTreeMap<String, Snap> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let m = std::fs::symlink_metadata(&p).expect("lstat");
            let rel = p
                .strip_prefix(root)
                .expect("prefix")
                .to_string_lossy()
                .into_owned();
            let mtime_ns = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as i128);
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode()
            };
            #[cfg(not(unix))]
            let mode = u32::from(m.permissions().readonly());
            let hash = if m.is_file() {
                std::fs::read(&p)
                    .ok()
                    .map(|b| blake3::hash(&b).to_hex().to_string())
            } else {
                None
            };
            if m.is_dir() {
                stack.push(p);
            }
            out.insert(
                rel,
                Snap {
                    dir: m.is_dir(),
                    size: if m.is_dir() { 0 } else { m.len() },
                    mtime_ns,
                    mode,
                    hash,
                },
            );
        }
    }
    out
}

/// A scanner over a fresh data directory with an instrumented file layer.
pub struct Env {
    _data: Option<tempfile::TempDir>,
    pub dir: PathBuf,
    scanner: Option<Scanner>,
    pub instr: Arc<Instrumentation>,
}

pub fn env() -> Env {
    env_with(ScanOptions::default())
}

pub fn env_with(opts: ScanOptions) -> Env {
    let data = tempfile::tempdir().expect("data dir");
    let mut e = env_at_with(data.path(), opts);
    e._data = Some(data);
    e
}

/// A scanner over an existing data directory (kept on drop).
pub fn env_at(dir: &Path) -> Env {
    env_at_with(dir, ScanOptions::default())
}

pub fn env_at_with(dir: &Path, mut opts: ScanOptions) -> Env {
    let (fs, instr) = SourceFs::instrumented();
    opts.fs = fs;
    let scanner = Scanner::open(dir, opts).expect("open scanner");
    Env {
        _data: None,
        dir: dir.to_path_buf(),
        scanner: Some(scanner),
        instr,
    }
}

impl Env {
    pub fn s(&self) -> &Scanner {
        self.scanner.as_ref().expect("scanner")
    }

    /// Re-open the scanner on the same data directory (the old one is closed first).
    pub fn reopen(&mut self, mut opts: ScanOptions) {
        drop(self.scanner.take());
        let (fs, instr) = SourceFs::instrumented();
        opts.fs = fs;
        self.scanner = Some(Scanner::open(&self.dir, opts).expect("reopen"));
        self.instr = instr;
    }

    pub fn add(&self, root: &Path) -> SourceId {
        self.s().add_source(root).expect("add source").source.id
    }

    /// A source's root as the scanner stores and opens it (canonical: macOS `/private/var`, Windows long
    /// names). Absolute paths the instrumented layer records are under this, not under the path given to
    /// [`Env::add`].
    pub fn root(&self, id: SourceId) -> PathBuf {
        self.s()
            .sources()
            .expect("sources")
            .into_iter()
            .find(|s| s.id == id)
            .expect("source")
            .root
    }

    /// Scan `sources` to the end.
    pub fn scan(&self, sources: &[SourceId]) -> RunSummary {
        self.s().start_scan(sources).expect("start scan");
        self.s().wait().expect("summary")
    }

    pub fn readers(&self) -> &Readers {
        self.s().catalog().read()
    }

    /// Every catalogued file of a source, by path.
    pub fn files(&self, source: SourceId) -> BTreeMap<String, FileRecord> {
        let mut out = BTreeMap::new();
        let mut after = None;
        loop {
            let page = self
                .readers()
                .source_files(source, after.as_ref(), 4096)
                .expect("files");
            for f in &page {
                out.insert(f.key.path.display().into_owned(), f.clone());
            }
            if page.len() < 4096 {
                break;
            }
            after = page.last().map(|f| f.key.path.clone());
        }
        out
    }
}

/// Wait until `f` holds (polling), or panic after `timeout`.
pub fn wait_until(timeout: Duration, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Poll snapshots until `f` holds.
pub fn wait_snapshot(
    s: &Scanner,
    timeout: Duration,
    what: &str,
    mut f: impl FnMut(&ProgressSnapshot) -> bool,
) -> ProgressSnapshot {
    let t = Instant::now();
    loop {
        if let Some(snap) = s.snapshot()
            && f(&snap)
        {
            return snap;
        }
        assert!(t.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The comparable part of a file record (ids, times and attempts normalised away).
pub fn normalized(f: &FileRecord) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}x{:?}|{:?}|{:?}|{:?}|{:?}|{}|{:?}|{}|{}",
        f.stage,
        f.class,
        f.reasons,
        f.format,
        f.width,
        f.height,
        f.identity,
        f.hashes,
        f.capture_ms,
        f.capture_undated,
        f.has_gps,
        f.camera_model,
        f.stat.size,
        f.missing_since_ms.is_some()
    )
}

/// Expect a completed run.
pub fn completed(s: &RunSummary) {
    assert_eq!(s.status, RunStatus::Completed, "{s:#?}");
}

/// Truth files under `prefix/` with their path relative to it.
pub fn truth_under<'a>(
    c: &'a Corpus,
    prefix: &str,
) -> impl Iterator<Item = (String, &'a ps_fixtures::TruthFile)> + 'a {
    let p = format!("{prefix}/");
    c.truth
        .files
        .iter()
        .filter_map(move |f| f.path.strip_prefix(&p).map(|r| (r.to_string(), f)))
}
