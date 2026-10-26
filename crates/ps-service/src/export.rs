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
    #[error(transparent)]
    Busy(#[from] crate::ops::Busy),
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
    /// When this library's copies were last re-hashed (this report, if `verified`); `None`: never.
    pub verified_at_ms: Option<i64>,
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

/// A file's own resolved capture date (with provenance and precision); `undated` when it is only the file time.
/// The catalog columns are the date (ADR-0071: a re-dated row's columns change, its metadata JSON does not); the
/// JSON's date is used only for a row without a provenance column.
fn embedded_of(
    rec: &ps_catalog::FileRecord,
    meta: Option<&ps_model::Metadata>,
) -> Option<CaptureDate> {
    if let Some(d) = rec.capture_date() {
        return Some(d);
    }
    if rec.capture_provenance.is_some() {
        return None;
    }
    let m = meta?;
    let d = m.capture_date.or_else(|| {
        m.fields.capture_time.map(|value| CaptureDate {
            value,
            provenance: DateProvenance::DateTimeOriginal,
            undated: false,
            precision: ps_model::DatePrecision::Second,
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
fn sidecar_date(path: &Path) -> Option<(CaptureTime, ps_model::DatePrecision)> {
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
        // Only a regular file (not followed through a symlink): opening a FIFO or a device would block or
        // read forever on a source tree we do not control.
        let m = std::fs::symlink_metadata(&p).ok()?;
        if !m.file_type().is_file() || m.len() > SIDECAR_MAX_BYTES {
            return None;
        }
        let f = std::fs::File::open(&p).ok()?;
        let mut buf = Vec::new();
        f.take(SIDECAR_MAX_BYTES).read_to_end(&mut buf).ok()?;
        ps_export::date::xmp_sidecar_date_precise(&buf)
    })
}

/// Files a cleanup run moved into a quarantine folder that are still there, by (source, original path), with
/// the stat fingerprint they were moved with and their quarantine path (ADR-0034, ADR-0063).
pub(crate) struct QuarantineIndex(
    HashMap<(ps_model::SourceId, ps_model::RelPath), (u64, i64, ps_model::RelPath)>,
);

impl QuarantineIndex {
    pub(crate) fn read(r: &ps_catalog::Readers) -> Result<Self> {
        Ok(QuarantineIndex(
            r.quarantined_files()?
                .into_iter()
                .map(|q| {
                    (
                        (q.source, q.rel_path),
                        (q.size, q.mtime_ns, q.quarantine_rel_path),
                    )
                })
                .collect(),
        ))
    }

    /// The quarantine path of the catalogued file `key` with fingerprint `stat`, if a cleanup run moved this
    /// very file (same source, path and fingerprint; a row reused for another file at that path does not
    /// match).
    pub(crate) fn of(
        &self,
        key: &ps_catalog::FileKey,
        stat: &ps_model::StatFingerprint,
    ) -> Option<ps_model::RelPath> {
        self.0
            .get(&(key.source, key.path.clone()))
            .filter(|(size, mtime, _)| *size == stat.size && *mtime == stat.mtime_ns)
            .map(|(_, _, q)| q.clone())
    }
}

impl<'a> Exports<'a> {
    fn catalog(&self) -> &'a Arc<Catalog> {
        self.svc.engine().catalog()
    }

    /// The export input: one unit per duplicate group (its keeper set, keeper first) and one per ungrouped,
    /// present photo or likely photo, minus user exclusions. Insights are advisory sidecar content only.
    pub fn input(&self) -> Result<ExportInput> {
        let r = self.catalog().read();
        // The zone for files without any catalog date, read once per plan (ADR-0070).
        let zone = self.svc.engine().zone();
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
        // A keeper a cleanup run moved into the quarantine folder is read from there (ADR-0066).
        let quarantine = QuarantineIndex::read(r)?;
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
        let sidecar = |f: &DateFacts<'_>| -> Option<(CaptureTime, ps_model::DatePrecision)> {
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
            // No catalog date at all: the local day of the file time in the plan's zone (ADR-0070).
            let capture = date
                .capture
                .or_else(|| Some(ps_export::layout::local_mtime(rec.stat.mtime_ns, &*zone)));
            Some(KeeperFile {
                file: rec.id,
                source: rec.key.source,
                rel_path: rec.key.path.clone(),
                size: rec.stat.size,
                mtime_ns: rec.stat.mtime_ns,
                content: rec.identity?,
                format: rec.format,
                capture,
                undated: date.undated,
                date_origin: date.origin,
                precision: date.precision,
                // Live Photo pairing (`ps_export::pairs`): the Apple ContentIdentifier from the metadata.
                content_identifier: meta
                    .as_ref()
                    .and_then(|m| m.fields.content_identifier.clone()),
                library_name: None,
                read_from: quarantine.of(&rec.key, &rec.stat),
                width: rec.width,
                height: rec.height,
            })
        };
        let copy_ref = |id: FileId| -> Option<CopyRef> {
            let (rec, _) = recs.get(&id)?;
            Some(CopyRef {
                file: rec.id,
                source: rec.key.source,
                rel_path: rec.key.path.clone(),
                // The catalog identity: whether an earlier member's library copy covers the group (ADR-0092).
                content: rec.identity,
            })
        };
        // Sidecar content (ADR-0021, advisory): the effective insight, the AI result with the user's
        // corrections from the AI review store, never a demo result. Only the catalog's owner opens the store
        // (ADR-0042); an observer's dry run writes no sidecar and plans without it.
        let store = (!self.catalog().is_observer())
            .then(|| crate::insights::InsightStore::for_catalog(self.catalog()));
        let corrections = store
            .as_ref()
            .map(crate::insights::InsightStore::corrections)
            .unwrap_or_default();
        let insight = |id: FileId| -> (Option<ImageInsight>, Vec<String>, Option<String>) {
            let Some(content) = recs.get(&id).and_then(|(rec, _)| rec.identity) else {
                return (None, Vec::new(), None);
            };
            let ai = r
                .ai_result_for_except(&content, crate::insights::DEMO_PROVIDER)
                .ok()
                .flatten()
                .and_then(|row| serde_json::from_str::<ImageInsight>(&row.insight).ok());
            let correction = corrections.get(&content.to_hex());
            match (ai, correction) {
                (Some(ai), Some(c)) => (
                    Some(c.apply(&ai)),
                    c.tags.clone().unwrap_or_default(),
                    c.subject.clone(),
                ),
                (ai, _) => (ai, Vec::new(), None),
            }
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
            let mut files: Vec<KeeperFile> = ids
                .iter()
                .filter_map(|f| keeper_file(*f, &members))
                .collect();
            if files.is_empty() {
                continue;
            }
            // A lone still image keeper named like a copy ("Copy of IMG_5437.JPG") is filed under the cleaner
            // name another copy in its group has. Live Photo candidates and RAW+JPEG sets keep their names,
            // whose stems pair them.
            if let [k] = files.as_mut_slice()
                && k.content_identifier.is_none()
                && k.kind() == Some(ps_model::MediaKind::Image)
                && k.format != Some(ps_model::Format::Heic)
            {
                let names: Vec<&[u8]> = members
                    .iter()
                    .filter_map(|m| recs.get(m))
                    .map(|(rec, _)| rec.key.path.file_name())
                    .collect();
                k.library_name = ps_export::layout::cleaner_name(k.rel_path.file_name(), &names);
            }
            let (insight, user_tags, description) = insight(g.keeper);
            units.push(ExportUnit {
                group: Some(GroupId(gid.0)),
                files,
                copies: members.iter().filter_map(|f| copy_ref(*f)).collect(),
                keeper_reasons: g.reason_texts(),
                insight,
                user_tags,
                description,
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
            let (insight, user_tags, description) = insight(id);
            units.push(ExportUnit {
                group: None,
                files: vec![k],
                copies: copy_ref(id).into_iter().collect(),
                keeper_reasons: vec!["the only copy".into()],
                insight,
                user_tags,
                description,
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
        // Not while a scan or a cleanup runs (ADR-0065).
        let _op = self.svc.begin(crate::ops::Operation::Export)?;
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
        // Files moved by a cleanup run (ADR-0034): quarantined, not missing. Matched by source, path and
        // fingerprint, not by a file id SQLite may reuse (ADR-0063).
        let quarantined = QuarantineIndex::read(r)?;
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
                quarantined: quarantined.of(&f.key, &f.stat),
            });
        })?;
        let mut groups: Vec<GroupFacts> = r
            .group_memberships()?
            .into_iter()
            .map(|(_, members, keeper_set)| GroupFacts {
                members,
                keeper_set,
                ..GroupFacts::default()
            })
            .collect();
        better_than_chosen_keepers(r, &files, &mut groups)?;
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
        let key = path_to_bytes(&lib);
        let record = || -> Result<Option<ps_catalog::LibraryRecord>> {
            Ok(self
                .catalog()
                .read()
                .export_libraries()?
                .into_iter()
                .find(|l| l.root == key))
        };
        if verify
            && !self.catalog().is_observer()
            && let Some(l) = record()?
        {
            self.catalog()
                .writer()
                .export_library_verified(l.id, ps_export::now_ms())?;
        }
        let verified_at_ms = record()?.and_then(|l| l.verified_at_ms);
        let report = ps_export::report(&files, &d, &checks);
        let mut totals = ps_model::CompletionCounts::default();
        for (f, x) in files.iter().zip(&d) {
            ps_export::count_disposition(&mut totals, x, f.size);
        }
        Ok(Completion {
            library: lib,
            verified: verify,
            verified_at_ms,
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

/// Fill [`GroupFacts::better_than_keeper`] (ADR-0096): in each group whose keeper the user chose (a keeper-set
/// file whose content has a keeper override), the members the automatic ranking (ADR-0014, the same ranking
/// without the override) puts above every keeper-set file, except copies of the keeper set's content. Reads
/// the grouping inputs of those groups' members from the catalog only (no file is opened); groups without an
/// override cost nothing.
fn better_than_chosen_keepers(
    r: &ps_catalog::Readers,
    files: &[FileFacts],
    groups: &mut [GroupFacts],
) -> Result<()> {
    let overrides: HashSet<ps_model::ContentId> = r
        .group_constraints()?
        .keeper_overrides
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    if overrides.is_empty() {
        return Ok(());
    }
    let content: HashMap<FileId, ps_model::ContentId> = files
        .iter()
        .filter_map(|f| f.content.map(|c| (f.file, c)))
        .collect();
    let chosen: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| {
            g.keeper_set
                .iter()
                .any(|k| content.get(k).is_some_and(|c| overrides.contains(c)))
        })
        .map(|(i, _)| i)
        .collect();
    if chosen.is_empty() {
        return Ok(());
    }
    let ids: Vec<FileId> = chosen
        .iter()
        .flat_map(|&i| groups[i].members.iter().copied())
        .collect();
    let mut inputs: HashMap<FileId, ps_dedup::DedupInput> = HashMap::new();
    r.analyzed_files(Some(&ids), |rec, meta| {
        if let Some(x) = ps_scan::post::dedup_input(&rec, meta.as_ref()) {
            inputs.insert(x.file, x);
        }
    })?;
    let none = std::collections::BTreeSet::new();
    for i in chosen {
        let g = &mut groups[i];
        let refs: Vec<&ps_dedup::DedupInput> =
            g.members.iter().filter_map(|m| inputs.get(m)).collect();
        if refs.len() != g.members.len() {
            // A member without its grouping input (not analysed any more): keep every member out of cleanup
            // rather than guess its rank.
            g.better_than_keeper = g
                .members
                .iter()
                .filter(|m| !g.keeper_set.contains(m))
                .copied()
                .collect();
            continue;
        }
        let auto = ps_dedup::rank(&refs, &none).order;
        let Some(first_keeper) = auto.iter().position(|m| g.keeper_set.contains(m)) else {
            continue;
        };
        let kept: HashSet<ps_model::ContentId> = g
            .keeper_set
            .iter()
            .filter_map(|k| content.get(k).copied())
            .collect();
        g.better_than_keeper = auto[..first_keeper]
            .iter()
            .filter(|m| content.get(m).is_none_or(|c| !kept.contains(c)))
            .copied()
            .collect();
    }
    Ok(())
}

/// Profile defaults for a profile name (`apple`, `google`, `lightroom`).
pub fn profile_settings(name: &str) -> Option<ProfileSettings> {
    ExportProfile::parse(name).map(ProfileSettings::defaults)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A FIFO (or a symlink) named like a sidecar is never opened: the date lookup returns at once.
    #[cfg(unix)]
    #[test]
    fn sidecar_date_skips_fifos_and_symlinks() {
        let tmp = tempfile::tempdir().expect("tmp");
        let photo = tmp.path().join("IMG_0042.JPG");
        std::fs::write(&photo, b"jpeg").expect("photo");
        let fifo = tmp.path().join("IMG_0042.JPG.xmp");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(made.success());
        let (tx, rx) = std::sync::mpsc::channel();
        let p = photo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(sidecar_date(&p));
        });
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("returned without blocking on the FIFO");
        assert_eq!(got, None);

        let real = tmp.path().join("elsewhere.xmp");
        std::fs::write(
            &real,
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:exif="http://ns.adobe.com/exif/1.0/" exif:DateTimeOriginal="2019-05-04T10:00:00"/></rdf:RDF></x:xmpmeta>"#,
        )
        .expect("xmp");
        let other = tmp.path().join("IMG_0043.JPG");
        std::os::unix::fs::symlink(&real, tmp.path().join("IMG_0043.xmp")).expect("symlink");
        assert_eq!(
            sidecar_date(&other),
            None,
            "a symlinked sidecar is not followed"
        );
        std::fs::copy(&real, tmp.path().join("IMG_0044.xmp")).expect("copy");
        assert!(
            sidecar_date(&tmp.path().join("IMG_0044.JPG")).is_some(),
            "a regular sidecar is read"
        );
    }
}
