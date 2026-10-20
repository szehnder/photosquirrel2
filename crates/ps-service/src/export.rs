//! Export and completion reports (change `export`; ADR-0004, ADR-0022, ADR-0023).
//!
//! [`Exports`] builds the pure `ps-export` input from the catalog (one unit per duplicate group's keeper set
//! plus every ungrouped photo), plans and runs exports with the catalog-backed ledger ([`CatalogLedger`],
//! migration 6), and derives every file's disposition from the catalog, the groups and the ledger with
//! [`ps_export::derive_dispositions`] (the single implementation of the ADR-0023 rules).
//!
//! Only `ps-export` writes under user paths, and only inside the chosen library folder. Sources are read.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use ps_catalog::Catalog;
use ps_export::{
    CompletionReport, CopyRef, DateFacts, DispositionFacts, ExecConfig, ExportInput, ExportReport,
    ExportUnit, FileDisposition, FileFacts, FsLibrary, GroupFacts, KeeperFile, Ledger, NoHooks,
    Plan, ProfileSettings, SourceInfo,
};
use ps_model::{
    CaptureDate, CaptureTime, DateProvenance, Disposition, ExportProfile, FileId, GroupId,
    ImageInsight, LedgerEntry, PhotoClass, SourceStatus, Stage, path_from_bytes, path_to_bytes,
};

use crate::scan::ScanService;

/// Errors of the export facade.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Catalog(#[from] ps_catalog::CatalogError),
    #[error(transparent)]
    Plan(#[from] ps_export::PlanError),
    #[error(transparent)]
    Exec(#[from] ps_export::ExecError),
    #[error("library {}: {source}", path.display())]
    Library {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("no export library is registered yet: export first or name the library")]
    NoLibrary,
}

pub type Result<T, E = ExportError> = std::result::Result<T, E>;

/// [`Ledger`] over the catalog's `export_ledger` rows of one library (migration 6). `record` is committed
/// before it returns.
#[derive(Debug, Clone)]
pub struct CatalogLedger {
    catalog: Arc<Catalog>,
    library: i64,
}

impl CatalogLedger {
    pub fn new(catalog: Arc<Catalog>, library: i64) -> Self {
        Self { catalog, library }
    }
}

fn io_err(e: ps_catalog::CatalogError) -> io::Error {
    io::Error::other(e.to_string())
}

impl Ledger for CatalogLedger {
    fn entries(&self) -> io::Result<Vec<LedgerEntry>> {
        self.catalog.read().ledger(self.library).map_err(io_err)
    }

    fn record(&mut self, entry: &LedgerEntry) -> io::Result<()> {
        self.catalog
            .writer()
            .ledger_record(self.library, entry.clone())
            .map_err(io_err)
    }
}

/// Dispositions and their aggregation for one library.
#[derive(Clone, Debug, Serialize)]
pub struct Completion {
    pub library: PathBuf,
    pub verified: bool,
    pub report: CompletionReport,
    /// Totals over every source: files and bytes per disposition.
    pub totals: ps_model::CompletionCounts,
}

/// One file's disposition with its path (inspector, drill-down).
#[derive(Clone, Debug, Serialize)]
pub struct FileDispositionInfo {
    pub path: PathBuf,
    #[serde(flatten)]
    pub disposition: FileDisposition,
}

/// The export facade of one [`ScanService`].
#[derive(Clone, Copy, Debug)]
pub struct Exports<'a> {
    svc: &'a ScanService,
}

impl ScanService {
    /// Export planning, runs and completion reports.
    pub fn exports(&self) -> Exports<'_> {
        Exports { svc: self }
    }
}

/// A file's own resolved capture date (with provenance); `undated` when it is only the file time.
fn embedded_of(
    rec: &ps_catalog::FileRecord,
    meta: Option<&ps_model::Metadata>,
) -> Option<CaptureDate> {
    let m = meta?;
    let d = m.capture_date.or_else(|| {
        m.fields.capture_time.map(|value| CaptureDate {
            value,
            provenance: DateProvenance::DateTimeOriginal,
            undated: false,
        })
    })?;
    Some(CaptureDate {
        undated: d.undated || rec.capture_undated,
        ..d
    })
}

/// Largest sidecar read when looking for a date.
const SIDECAR_MAX_BYTES: u64 = 1 << 20;

/// The date in an XMP sidecar next to `path` (`IMG_0042.JPG.xmp`, `IMG_0042.xmp`, either case). Opened
/// read-only; a missing or unreadable sidecar is no date.
fn sidecar_date(path: &Path) -> Option<CaptureTime> {
    use std::io::Read as _;
    let mut candidates = Vec::with_capacity(4);
    for ext in ["xmp", "XMP"] {
        let mut s = path.as_os_str().to_os_string();
        s.push(".");
        s.push(ext);
        candidates.push(PathBuf::from(s));
        candidates.push(path.with_extension(ext));
    }
    candidates.into_iter().find_map(|p| {
        let f = std::fs::File::open(&p).ok()?;
        let mut buf = Vec::new();
        f.take(SIDECAR_MAX_BYTES).read_to_end(&mut buf).ok()?;
        ps_export::date::xmp_sidecar_date(&buf)
    })
}

impl<'a> Exports<'a> {
    fn catalog(&self) -> &'a Arc<Catalog> {
        self.svc.engine().catalog()
    }

    /// The export input: one unit per duplicate group (its keeper set, keeper first) and one per ungrouped,
    /// present photo or likely photo, minus user exclusions. Insights are advisory sidecar content only.
    pub fn input(&self) -> Result<ExportInput> {
        let r = self.catalog().read();
        let sources: Vec<SourceInfo> = r
            .sources()?
            .into_iter()
            .filter(|s| s.folded_into.is_none())
            .map(|s| {
                let root = path_from_bytes(&s.root);
                let online = s.status == SourceStatus::Online && root.is_dir();
                SourceInfo {
                    id: s.id,
                    name: root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.display().to_string()),
                    root,
                    online,
                }
            })
            .collect();
        let excluded: HashSet<FileId> = r.user_excluded_files()?.into_iter().collect();
        let mut recs: HashMap<FileId, (ps_catalog::FileRecord, Option<ps_model::Metadata>)> =
            HashMap::new();
        r.analyzed_files(None, |rec, meta| {
            recs.insert(rec.id, (rec, meta));
        })?;
        let online_roots: HashMap<ps_model::SourceId, &Path> = sources
            .iter()
            .filter(|s| s.online)
            .map(|s| (s.id, s.root.as_path()))
            .collect();
        let facts = |id: FileId| -> Option<DateFacts<'_>> {
            let (rec, meta) = recs.get(&id)?;
            Some(DateFacts {
                file: id,
                name: rec.key.path.file_name(),
                embedded: embedded_of(rec, meta.as_ref()),
            })
        };
        let sidecar = |f: &DateFacts<'_>| -> Option<CaptureTime> {
            let (rec, _) = recs.get(&f.file)?;
            let root = online_roots.get(&rec.key.source)?;
            sidecar_date(&rec.key.path.under(root))
        };
        // The filing date (ADR-0043): the file's own embedded date, else the best its group knows.
        let keeper_file = |id: FileId, group: &[FileId]| -> Option<KeeperFile> {
            let (rec, meta) = recs.get(&id)?;
            let others: Vec<DateFacts<'_>> = group
                .iter()
                .filter(|f| **f != id)
                .filter_map(|f| facts(*f))
                .collect();
            let date = ps_export::date::resolve(&facts(id)?, &others, sidecar);
            Some(KeeperFile {
                file: rec.id,
                source: rec.key.source,
                rel_path: rec.key.path.clone(),
                size: rec.stat.size,
                mtime_ns: rec.stat.mtime_ns,
                content: rec.identity?,
                format: rec.format,
                capture: date.capture,
                undated: date.undated,
                date_origin: date.origin,
                // Live Photo pairing (`ps_export::pairs`): the Apple ContentIdentifier from the metadata.
                content_identifier: meta
                    .as_ref()
                    .and_then(|m| m.fields.content_identifier.clone()),
            })
        };
        let copy_ref = |id: FileId| -> Option<CopyRef> {
            let (rec, _) = recs.get(&id)?;
            Some(CopyRef {
                file: rec.id,
                source: rec.key.source,
                rel_path: rec.key.path.clone(),
            })
        };
        let insight = |id: FileId| -> Option<ImageInsight> {
            let content = recs.get(&id)?.0.identity?;
            let row = r.ai_result_for(&content).ok()??;
            serde_json::from_str(&row.insight).ok()
        };
        let mut units = Vec::new();
        let mut grouped: HashSet<FileId> = HashSet::new();
        for (gid, members, set) in r.group_memberships()? {
            grouped.extend(members.iter().copied());
            let Some(g) = r.group(gid)? else { continue };
            if excluded.contains(&g.keeper) {
                continue;
            }
            let mut ids: Vec<FileId> = vec![g.keeper];
            ids.extend(set.iter().copied().filter(|f| *f != g.keeper));
            let files: Vec<KeeperFile> = ids
                .iter()
                .filter_map(|f| keeper_file(*f, &members))
                .collect();
            if files.is_empty() {
                continue;
            }
            units.push(ExportUnit {
                group: Some(GroupId(gid.0)),
                files,
                copies: members.iter().filter_map(|f| copy_ref(*f)).collect(),
                keeper_reasons: g.reason_texts(),
                insight: insight(g.keeper),
                user_tags: Vec::new(),
                description: None,
            });
        }
        let mut singles: Vec<FileId> = recs
            .iter()
            .filter(|(id, (rec, _))| {
                !grouped.contains(id)
                    && !excluded.contains(id)
                    && matches!(rec.class, Some(PhotoClass::Photo | PhotoClass::LikelyPhoto))
            })
            .map(|(id, _)| *id)
            .collect();
        singles.sort();
        for id in singles {
            let Some(k) = keeper_file(id, &[]) else {
                continue;
            };
            units.push(ExportUnit {
                group: None,
                files: vec![k],
                copies: copy_ref(id).into_iter().collect(),
                keeper_reasons: vec!["the only copy".into()],
                insight: insight(id),
                user_tags: Vec::new(),
                description: None,
            });
        }
        Ok(ExportInput { sources, units })
    }

    fn normalized(root: &Path) -> Result<PathBuf> {
        ps_export::library::normalize(root).map_err(|source| ExportError::Library {
            path: root.to_path_buf(),
            source,
        })
    }

    /// Register a library folder in the catalog (no file is written) and return its id.
    pub fn library_id(&self, root: &Path) -> Result<i64> {
        let root = Self::normalized(root)?;
        Ok(self
            .catalog()
            .writer()
            .export_library(path_to_bytes(&root), None)?)
    }

    /// The ledger rows of the library at `root` for read-only uses (dry runs, reports): the registered
    /// library's rows; an unregistered library is registered when this process owns the catalog, and has no
    /// rows when it only observes it (another process owns it, ADR-0041).
    fn ledger_for_read(&self, root: &Path) -> Result<Vec<LedgerEntry>> {
        let root = Self::normalized(root)?;
        let key = path_to_bytes(&root);
        let known = self
            .catalog()
            .read()
            .export_libraries()?
            .into_iter()
            .find(|l| l.root == key)
            .map(|l| l.id);
        let id = match known {
            Some(id) => id,
            None if self.catalog().is_observer() => return Ok(Vec::new()),
            None => self.library_id(&root)?,
        };
        Ok(self.catalog().read().ledger(id)?)
    }

    /// The most recently registered library, if any.
    pub fn last_library(&self) -> Result<Option<PathBuf>> {
        Ok(self
            .catalog()
            .read()
            .export_libraries()?
            .last()
            .map(|l| path_from_bytes(&l.root)))
    }

    /// The dry run: what an export to `library` would copy. Reads only (the library folder need not exist).
    pub fn plan(&self, library: &Path, settings: ProfileSettings) -> Result<(Plan, ExportInput)> {
        let input = self.input()?;
        let ledger = self.ledger_for_read(library)?;
        let root = Self::normalized(library)?;
        let plan = ps_export::plan(&input, &ledger, &root, &FsLibrary::new(&root), settings)?;
        Ok((plan, input))
    }

    /// Plan and run an export (copy-verify-rename into `library`; sources untouched). `cancel` stops after the
    /// current files; re-running resumes.
    pub fn run(
        &self,
        library: &Path,
        settings: ProfileSettings,
        cfg: &ExecConfig,
        cancel: &AtomicBool,
    ) -> Result<(Plan, ExportReport)> {
        let (plan, input) = self.plan(library, settings)?;
        let id = self.library_id(library)?;
        let mut ledger = CatalogLedger::new(self.catalog().clone(), id);
        let report = ps_export::execute(&plan, &input, &mut ledger, cfg, &NoHooks, cancel)?;
        Ok((plan, report))
    }

    /// Every catalogued file's disposition with respect to `library` (`verify`: re-hash every library copy).
    pub fn dispositions(
        &self,
        library: &Path,
        verify: bool,
    ) -> Result<(
        Vec<FileFacts>,
        Vec<FileDisposition>,
        HashMap<ps_model::RelPath, ps_export::CopyCheck>,
    )> {
        let root = Self::normalized(library)?;
        let ledger = self.ledger_for_read(&root)?;
        let checks = ps_export::check_library(&root, &ledger, verify);
        let (files, d) = self.derive(&ledger, &checks)?;
        Ok((files, d, checks))
    }

    /// Dispositions before any export (no library yet): nothing is exported or covered, so every keeper and
    /// candidate is pending. The gallery's disposition filter uses this until a library exists.
    pub fn dispositions_without_library(&self) -> Result<(Vec<FileFacts>, Vec<FileDisposition>)> {
        self.derive(&[], &HashMap::new())
    }

    fn derive(
        &self,
        ledger: &[LedgerEntry],
        checks: &HashMap<ps_model::RelPath, ps_export::CopyCheck>,
    ) -> Result<(Vec<FileFacts>, Vec<FileDisposition>)> {
        let r = self.catalog().read();
        let excluded: HashSet<FileId> = r.user_excluded_files()?.into_iter().collect();
        let mut files = Vec::new();
        r.for_each_file(|f| {
            files.push(FileFacts {
                file: f.id,
                source: f.key.source,
                rel_path: f.key.path.clone(),
                size: f.stat.size,
                content: f.identity,
                class: f.class,
                unreadable: f.stage == Stage::Failed && f.attempts >= 3,
                missing: f.missing_since_ms.is_some(),
                user_excluded: excluded.contains(&f.id),
            });
        })?;
        let groups: Vec<GroupFacts> = r
            .group_memberships()?
            .into_iter()
            .map(|(_, members, keeper_set)| GroupFacts {
                members,
                keeper_set,
            })
            .collect();
        let d = ps_export::derive_dispositions(&DispositionFacts {
            files: &files,
            groups: &groups,
            ledger,
            checks,
        });
        Ok((files, d))
    }

    /// The completion report (per source and folder) for `library`, or the last library used.
    pub fn completion(&self, library: Option<&Path>, verify: bool) -> Result<Completion> {
        let lib = match library {
            Some(l) => Self::normalized(l)?,
            None => self.last_library()?.ok_or(ExportError::NoLibrary)?,
        };
        let (files, d, checks) = self.dispositions(&lib, verify)?;
        let report = ps_export::report(&files, &d, &checks);
        let mut totals = ps_model::CompletionCounts::default();
        for (f, x) in files.iter().zip(&d) {
            totals.add(x.disposition, f.size);
        }
        Ok(Completion {
            library: lib,
            verified: verify,
            report,
            totals,
        })
    }

    /// Files with a given disposition (drill-down), with absolute paths.
    pub fn files_with(
        &self,
        library: &Path,
        disposition: Disposition,
        limit: usize,
    ) -> Result<Vec<FileDispositionInfo>> {
        let (files, d, _) = self.dispositions(library, false)?;
        let roots: HashMap<_, _> = self
            .catalog()
            .read()
            .sources()?
            .into_iter()
            .map(|s| (s.id, path_from_bytes(&s.root)))
            .collect();
        Ok(files
            .iter()
            .zip(d)
            .filter(|(_, x)| x.disposition == disposition)
            .take(limit)
            .map(|(f, x)| FileDispositionInfo {
                path: f
                    .rel_path
                    .under(roots.get(&f.source).map_or(Path::new(""), PathBuf::as_path)),
                disposition: x,
            })
            .collect())
    }
}

/// Profile defaults for a profile name (`apple`, `google`, `lightroom`).
pub fn profile_settings(name: &str) -> Option<ProfileSettings> {
    ExportProfile::parse(name).map(ProfileSettings::defaults)
}
