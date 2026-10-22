#![allow(dead_code)]
//! Shared helpers: scratch folders, hand-made sources, the ps-fixtures small corpus as an export input,
//! tree snapshots and a durable file ledger (stands in for the catalog ledger in kill tests).

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ps_export::{
    CopyRef, ExportInput, ExportUnit, FileFacts, GroupFacts, KeeperFile, Ledger, SourceInfo,
};
use ps_fixtures::{DirWriter, Preset, Truth};
use ps_model::{
    CaptureTime, ContentId, ContentIdKind, FileId, Format, GroupId, LedgerEntry, PhotoClass,
    RelPath, SourceId,
};

/// A fresh scratch folder under the target dir (never in the repository, `/tmp` or a home photo folder),
/// removed on drop.
pub fn scratch(tag: &str) -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(base).expect("scratch base");
    tempfile::Builder::new()
        .prefix(&format!("ps-export-{tag}-"))
        .tempdir_in(base)
        .expect("scratch")
}

pub fn rel(s: &str) -> RelPath {
    RelPath::from_bytes(s.as_bytes().to_vec()).expect("rel path")
}

pub fn set_mtime(path: &Path, secs: i64) {
    let t = ps_export::copy::system_time(secs * 1_000_000_000);
    File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(t))
        .expect("set mtime");
}

pub fn mtime_ns(path: &Path) -> i64 {
    let m = fs::metadata(path).expect("stat").modified().expect("mtime");
    match m.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    }
}

/// Deterministic pseudo-random bytes.
pub fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

pub fn format_of(name: &str) -> Option<Format> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => Format::Jpeg,
        "heic" => Format::Heic,
        "mov" => Format::Mov,
        "mp4" => Format::Mp4,
        "dng" => Format::Raw(ps_model::RawFormat::Dng),
        "png" => Format::Png,
        _ => return None,
    })
}

/// Hand-made sources in a scratch folder.
pub struct Sources {
    pub dir: tempfile::TempDir,
    pub sources: Vec<SourceInfo>,
    next_file: i64,
}

impl Sources {
    pub fn new(tag: &str, n: usize) -> Self {
        let dir = scratch(tag);
        let mut sources = Vec::new();
        for i in 0..n {
            let root = dir.path().join(format!("drive-{i}"));
            fs::create_dir_all(&root).expect("source root");
            sources.push(SourceInfo {
                id: SourceId(i as i64 + 1),
                root,
                online: true,
                name: format!("drive-{i}"),
            });
        }
        Self {
            dir,
            sources,
            next_file: 1,
        }
    }

    pub fn library(&self) -> PathBuf {
        self.dir.path().join("library")
    }

    /// Write a file into source `s` and describe it.
    pub fn add(
        &mut self,
        s: usize,
        rel_path: &str,
        data: &[u8],
        mtime_secs: i64,
        capture: Option<CaptureTime>,
    ) -> KeeperFile {
        let path = rel(rel_path).under(&self.sources[s].root);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, data).expect("write source");
        set_mtime(&path, mtime_secs);
        let file = FileId(self.next_file);
        self.next_file += 1;
        KeeperFile {
            file,
            source: self.sources[s].id,
            rel_path: rel(rel_path),
            size: data.len() as u64,
            mtime_ns: mtime_ns(&path),
            content: ps_hash::identity_from_buffer(data),
            format: format_of(rel_path),
            undated: capture.is_none(),
            date_origin: if capture.is_none() {
                ps_export::DateOrigin::MTIME
            } else {
                ps_export::DateOrigin::OWN_EXIF
            },
            capture,
            content_identifier: None,
            library_name: None,
        }
    }

    pub fn input(&self, units: Vec<ExportUnit>) -> ExportInput {
        ExportInput {
            sources: self.sources.clone(),
            units,
        }
    }
}

pub fn unit(files: Vec<KeeperFile>) -> ExportUnit {
    let copies = files
        .iter()
        .map(|f| CopyRef {
            file: f.file,
            source: f.source,
            rel_path: f.rel_path.clone(),
        })
        .collect();
    ExportUnit {
        group: None,
        files,
        copies,
        keeper_reasons: vec!["Only copy".into()],
        insight: None,
        user_tags: Vec::new(),
        description: None,
    }
}

pub fn day(y: i32, m: u8, d: u8) -> Option<CaptureTime> {
    Some(CaptureTime::from_parts(y, m, d, 12, 0, 0))
}

/// The ps-fixtures small corpus (seed 42), generated once per test binary. Read-only for tests.
pub struct SmallCorpus {
    pub root: PathBuf,
    pub truth: Truth,
}

pub fn small_corpus() -> &'static SmallCorpus {
    static C: OnceLock<SmallCorpus> = OnceLock::new();
    C.get_or_init(|| {
        let root = ps_fixtures::shared_dir(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "export-corpus-small-42",
            |root| {
                let w = DirWriter::new(root).expect("dir writer");
                ps_fixtures::generate(42, Preset::Small, &w).expect("generate");
            },
        );
        let truth = Truth::read(&root).expect("truth");
        SmallCorpus { root, truth }
    })
}

/// What the small corpus is, as export and disposition inputs.
pub struct CorpusModel {
    pub input: ExportInput,
    pub files: Vec<FileFacts>,
    pub groups: Vec<GroupFacts>,
    /// truth path -> file id
    pub ids: BTreeMap<String, FileId>,
}

/// Build the export input from the truth (keepers and keeper sets of every non-junk group) and the
/// disposition facts of every file. `drives`: which drives to include (all when empty).
pub fn corpus_model(c: &SmallCorpus, drives: &[&str], offline: &[&str]) -> CorpusModel {
    let truth = &c.truth;
    let wanted = |d: &str| drives.is_empty() || drives.contains(&d);
    let sources: Vec<SourceInfo> = truth
        .drives
        .iter()
        .enumerate()
        .filter(|(_, d)| wanted(d))
        .map(|(i, d)| SourceInfo {
            id: SourceId(i as i64 + 1),
            root: c.root.join(d),
            online: !offline.contains(&d.as_str()),
            name: d.clone(),
        })
        .collect();
    let sid =
        |d: &str| SourceId(truth.drives.iter().position(|x| x == d).expect("drive") as i64 + 1);
    let mut ids = BTreeMap::new();
    let mut files = Vec::new();
    let mut by_group: BTreeMap<&str, Vec<&ps_fixtures::TruthFile>> = BTreeMap::new();
    for (i, f) in truth.files.iter().enumerate() {
        if !wanted(&f.drive) {
            continue;
        }
        let id = FileId(i as i64 + 1);
        ids.insert(f.path.clone(), id);
        let rel_in = f
            .path
            .strip_prefix(&format!("{}/", f.drive))
            .expect("drive prefix");
        files.push(FileFacts {
            file: id,
            source: sid(&f.drive),
            rel_path: rel(rel_in),
            size: f.size,
            content: ContentId::from_hex(&f.content, ContentIdKind::Full),
            class: Some(f.expected_class),
            unreadable: false,
            missing: false,
            user_excluded: false,
            quarantined: None,
        });
        by_group.entry(&f.group).or_default().push(f);
    }
    let keeper_file = |f: &ps_fixtures::TruthFile| {
        let path = c.root.join(&f.path);
        KeeperFile {
            file: ids[&f.path],
            source: sid(&f.drive),
            rel_path: rel(f
                .path
                .strip_prefix(&format!("{}/", f.drive))
                .expect("prefix")),
            size: f.size,
            mtime_ns: mtime_ns(&path),
            content: ContentId::from_hex(&f.content, ContentIdKind::Full).expect("hex"),
            format: Format::from_storage(&f.format),
            capture: None,
            undated: true,
            date_origin: ps_export::DateOrigin::MTIME,
            content_identifier: None,
            library_name: None,
        }
    };
    let mut units = Vec::new();
    let mut groups = Vec::new();
    for (gi, (g, members)) in by_group.iter().enumerate() {
        if members.iter().all(|m| m.category.is_junk()) {
            continue;
        }
        let Some(keeper) = members.iter().find(|m| m.keeper) else {
            continue;
        };
        let mut set: Vec<&ps_fixtures::TruthFile> =
            members.iter().copied().filter(|m| m.keeper_set).collect();
        if set.is_empty() {
            set.push(keeper);
        }
        // Keeper first.
        set.sort_by_key(|m| !m.keeper);
        if members.len() > 1 {
            groups.push(GroupFacts {
                members: members.iter().map(|m| ids[&m.path]).collect(),
                keeper_set: set.iter().map(|m| ids[&m.path]).collect(),
            });
        }
        units.push(ExportUnit {
            group: Some(GroupId(gi as i64 + 1)),
            files: set.iter().map(|m| keeper_file(m)).collect(),
            copies: members
                .iter()
                .map(|m| CopyRef {
                    file: ids[&m.path],
                    source: sid(&m.drive),
                    rel_path: rel(m.path.strip_prefix(&format!("{}/", m.drive)).expect("p")),
                })
                .collect(),
            keeper_reasons: vec![format!("truth keeper of {g}")],
            insight: None,
            user_tags: Vec::new(),
            description: None,
        });
    }
    CorpusModel {
        input: ExportInput { sources, units },
        files,
        groups,
        ids,
    }
}

/// Snapshot of a tree: relative path -> (size, mtime ns, BLAKE3).
pub type Snapshot = BTreeMap<PathBuf, (u64, i64, [u8; 32])>;

pub fn snapshot(root: &Path, skip: &[&Path]) -> Snapshot {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).expect("read_dir") {
            let p = e.expect("entry").path();
            if skip.iter().any(|s| p.starts_with(s)) {
                continue;
            }
            let m = fs::symlink_metadata(&p).expect("stat");
            if m.is_dir() {
                stack.push(p.clone());
                out.insert(
                    p.strip_prefix(root).expect("prefix").to_path_buf(),
                    (0, 0, [0; 32]),
                );
            } else {
                let h = *blake3::hash(&fs::read(&p).expect("read")).as_bytes();
                out.insert(
                    p.strip_prefix(root).expect("prefix").to_path_buf(),
                    (m.len(), mtime_ns(&p), h),
                );
            }
        }
    }
    out
}

/// Every file under `root` (relative paths).
pub fn files_under(root: &Path) -> Vec<PathBuf> {
    snapshot(root, &[])
        .into_iter()
        .filter(|(p, v)| v.0 > 0 || root.join(p).is_file())
        .map(|(p, _)| p)
        .filter(|p| root.join(p).is_file())
        .collect()
}

pub fn partials_under(root: &Path) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    files_under(root)
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".ps-partial"))
        .collect()
}

/// A durable ledger in a JSON-lines file (test stand-in for the catalog's `export_ledger`).
pub struct FileLedger {
    path: PathBuf,
}

impl FileLedger {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Ledger for FileLedger {
    fn entries(&self) -> io::Result<Vec<LedgerEntry>> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut rows: BTreeMap<RelPath, LedgerEntry> = BTreeMap::new();
        for l in text.lines().filter(|l| !l.is_empty()) {
            let e: LedgerEntry = serde_json::from_str(l).map_err(io::Error::other)?;
            rows.insert(e.library_rel_path.clone(), e);
        }
        Ok(rows.into_values().collect())
    }

    fn record(&mut self, e: &LedgerEntry) -> io::Result<()> {
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)?;
        let mut line = serde_json::to_vec(e).map_err(io::Error::other)?;
        line.push(b'\n');
        f.write_all(&line)?;
        f.sync_data()
    }
}

/// Library files (excluding `.photosquirrel/`) grouped by content hash.
pub fn library_by_hash(lib: &Path) -> BTreeMap<[u8; 32], Vec<PathBuf>> {
    let mut out: BTreeMap<[u8; 32], Vec<PathBuf>> = BTreeMap::new();
    for p in files_under(lib) {
        if p.starts_with(ps_export::LIBRARY_META_DIR)
            || p.extension()
                .is_some_and(|e| e == "xmp" || e == "ps-partial")
        {
            continue;
        }
        let h = *blake3::hash(&fs::read(lib.join(&p)).expect("read")).as_bytes();
        out.entry(h).or_default().push(p);
    }
    out
}

pub fn photo_class_is_junk(c: PhotoClass) -> bool {
    matches!(c, PhotoClass::Junk | PhotoClass::Graphic)
}

/// A [`ps_export::FsLibrary`] that counts how often the planner reads a library file's contents
/// ([`ps_export::LibraryProbe::identical`]) and stats one.
pub struct CountingProbe {
    pub inner: ps_export::FsLibrary,
    pub reads: std::sync::atomic::AtomicUsize,
    pub stats: std::sync::atomic::AtomicUsize,
}

impl CountingProbe {
    pub fn new(lib: &Path) -> Self {
        Self {
            inner: ps_export::FsLibrary::new(lib),
            reads: Default::default(),
            stats: Default::default(),
        }
    }

    pub fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ps_export::LibraryProbe for CountingProbe {
    fn existing(&self, rel: &RelPath) -> io::Result<ps_export::Existing> {
        self.stats.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.existing(rel)
    }

    fn identical(&self, rel: &RelPath, content: &ContentId, size: u64) -> io::Result<bool> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.identical(rel, content, size)
    }

    fn free_space(&self) -> io::Result<Option<u64>> {
        self.inner.free_space()
    }
}
