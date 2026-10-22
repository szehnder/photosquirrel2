//! The export plan (dry run, ADR-0004, ADR-0022). Planning only reads: the ledger rows, the library folder
//! (existence, sizes and times; hashes only of same-size files for adoption and of recorded copies whose time
//! changed, ADR-0044) and the free space. Nothing is written until the user confirms and [`crate::execute`]
//! runs the plan.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use ps_model::{ContentId, FileId, LedgerEntry, LedgerKind, LedgerState, RelPath, SourceId};
use serde::Serialize;

use crate::date::{DateOrigin, DateSource};
use crate::disposition::CopyCheck;
use crate::input::{ExportInput, KeeperFile, SourceInfo};
use crate::layout::{MAX_SUFFIX, candidate, collision_key, folder_of, join, split_name};
use crate::library::{Existing, LibraryProbe, check_copy, normalize, partial_of};
use crate::pairs::bundles;
use crate::profile::ProfileSettings;

/// Planned bytes reserved per sidecar and for the manifest when checking free space.
pub const SIDECAR_RESERVE_BYTES: u64 = 16 << 10;
pub const MANIFEST_RESERVE_BYTES: u64 = 1 << 20;

/// What happens to a planned file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannedAction {
    /// Copy with the verified protocol.
    Copy,
    /// An identical file is already at the target: record it as exported, copy nothing.
    Adopt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlannedFile {
    /// Index into [`ExportInput::units`].
    pub unit: usize,
    pub file: FileId,
    pub source: SourceId,
    pub source_path: PathBuf,
    /// Library-relative target.
    pub target: RelPath,
    pub size: u64,
    pub mtime_ns: i64,
    pub content: ContentId,
    pub action: PlannedAction,
    /// Where the date of the target folder came from (ADR-0043).
    pub date_origin: DateOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleKind {
    Single,
    /// A RAW+JPEG keeper set (ADR-0014, ADR-0037).
    KeeperSet,
    /// A Live Photo still and its video.
    LivePhoto,
}

/// Files placed together with one stem and one collision suffix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlannedBundle {
    pub units: Vec<usize>,
    pub kind: BundleKind,
    pub files: Vec<PlannedFile>,
    /// `<stem>.xmp` to write; `None` when sidecars are off or one is already there.
    pub sidecar: Option<RelPath>,
    /// 1 = original name, n >= 2 = ` (n)`.
    pub suffix: u32,
    /// Files of the bundle already in the library. A bundle with no `files` and a `sidecar` only writes the
    /// sidecar of exported files that have none (a crashed sidecar write, or sidecars turned on, ADR-0045).
    pub already: Vec<AlreadyExported>,
}

/// A keeper on an offline source: listed, skipped, left pending.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Unavailable {
    pub unit: usize,
    pub file: FileId,
    pub source: SourceId,
    pub source_name: String,
}

/// A keeper whose content the ledger records as exported to a library copy that is still there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AlreadyExported {
    pub unit: usize,
    pub file: FileId,
    pub library_rel_path: RelPath,
    #[serde(skip)]
    pub content: ContentId,
    /// The copy's full BLAKE3 from the ledger (for a sidecar that lists it).
    #[serde(skip)]
    pub full_hash: Option<[u8; 32]>,
}

/// A keeper whose library copy is still at its recorded path but was changed since it was exported (another
/// application wrote into it, for example a photo app saving metadata). Reported; not copied again unless the
/// user asks (ADR-0046).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModifiedInLibrary {
    pub unit: usize,
    pub file: FileId,
    pub library_rel_path: RelPath,
    pub check: CopyCheck,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PlanTotals {
    /// Files to copy.
    pub files: u64,
    /// Bytes to copy.
    pub bytes: u64,
    /// Identical files already in place, to adopt.
    pub adopted: u64,
    /// Planned files that got a ` (n)` suffix.
    pub collisions: u64,
    pub sidecars: u64,
    pub unavailable: u64,
    pub already_exported: u64,
    /// Keepers whose library copy was changed by another application (not copied again, ADR-0046).
    pub modified: u64,
    /// Planned files by where their filing date came from.
    pub dates: DateTotals,
}

/// Planned files by filing-date origin (ADR-0043).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DateTotals {
    /// The file's own embedded date.
    pub own: u64,
    /// The embedded date of another copy in the duplicate group (the keeper lost its EXIF).
    pub other_copy: u64,
    pub file_name: u64,
    pub sidecar: u64,
    /// Only the modification time: under `Undated/`.
    pub undated: u64,
}

impl DateTotals {
    pub fn add(&mut self, o: DateOrigin) {
        let slot = match (o.source, o.copy) {
            (DateSource::FileMtime, _) => &mut self.undated,
            (DateSource::Embedded { .. }, None) => &mut self.own,
            (DateSource::Embedded { .. }, Some(_)) => &mut self.other_copy,
            (DateSource::FileName, _) => &mut self.file_name,
            (DateSource::Sidecar, _) => &mut self.sidecar,
        };
        *slot += 1;
    }

    /// "Dates: 3 filed by another copy's date, 1 by its file name, 2 undated (file time only)", or `None` when
    /// every planned file is dated by its own metadata.
    pub fn note(&self) -> Option<String> {
        let parts: Vec<String> = [
            (
                self.other_copy,
                "filed by the date of another copy (no EXIF of their own)",
            ),
            (self.file_name, "dated by their file name"),
            (self.sidecar, "dated by an XMP sidecar"),
            (self.undated, "undated (file time only, under Undated/)"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
        (!parts.is_empty()).then(|| format!("Dates: {}.", parts.join(", ")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PlanBlocker {
    InsufficientSpace {
        required: u64,
        free: u64,
        shortfall: u64,
    },
}

impl std::fmt::Display for PlanBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanBlocker::InsufficientSpace {
                required,
                free,
                shortfall,
            } => write!(
                f,
                "not enough free space in the library: {required} bytes needed, {free} free ({shortfall} short)"
            ),
        }
    }
}

/// The dry-run result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// Absolute, symlink-resolved library folder.
    pub library_root: PathBuf,
    pub settings: ProfileSettings,
    pub bundles: Vec<PlannedBundle>,
    pub unavailable: Vec<Unavailable>,
    pub already_exported: Vec<AlreadyExported>,
    /// Library copies changed by another application since they were exported (ADR-0046).
    pub modified: Vec<ModifiedInLibrary>,
    pub totals: PlanTotals,
    pub free_bytes: Option<u64>,
    pub required_bytes: u64,
}

impl Plan {
    /// Why the export cannot start, if it cannot.
    pub fn blocker(&self) -> Option<PlanBlocker> {
        let free = self.free_bytes?;
        (free < self.required_bytes).then(|| PlanBlocker::InsufficientSpace {
            required: self.required_bytes,
            free,
            shortfall: self.required_bytes - free,
        })
    }

    pub fn files(&self) -> impl Iterator<Item = &PlannedFile> {
        self.bundles.iter().flat_map(|b| b.files.iter())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("the library {} is inside the source \"{source_name}\" ({}); choose a folder outside every source", library.display(), source_root.display())]
    LibraryInsideSource {
        source_name: String,
        source_root: PathBuf,
        library: PathBuf,
    },
    #[error("the source \"{source_name}\" ({}) is inside the library {}; choose another library folder", source_root.display(), library.display())]
    SourceInsideLibrary {
        source_name: String,
        source_root: PathBuf,
        library: PathBuf,
    },
    #[error("no free name for {name} in {folder} (tried {MAX_SUFFIX} suffixes)")]
    NoFreeName { name: String, folder: String },
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Refuse a library inside a source or a source inside the library (export spec). Paths are normalized
/// (absolute, symlinks of the existing part resolved).
pub fn check_overlap(library: &Path, sources: &[SourceInfo]) -> Result<PathBuf, PlanError> {
    let lib = normalize(library)?;
    for s in sources {
        let root = normalize(&s.root)?;
        if lib.starts_with(&root) {
            return Err(PlanError::LibraryInsideSource {
                source_name: s.name.clone(),
                source_root: root,
                library: lib,
            });
        }
        if root.starts_with(&lib) {
            return Err(PlanError::SourceInsideLibrary {
                source_name: s.name.clone(),
                source_root: root,
                library: lib,
            });
        }
    }
    Ok(lib)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Free,
    Identical,
    Taken,
}

struct Planner<'a> {
    probe: &'a dyn LibraryProbe,
    reserved: HashSet<Vec<u8>>,
    /// Paths of `in_progress` ledger rows: a partial file there is ours, stale, removed before copying.
    in_progress: HashSet<Vec<u8>>,
    /// Sidecars the ledger records as written (`done` sidecar rows): collision key -> the content described.
    sidecars: HashMap<Vec<u8>, ContentId>,
    /// The same rows by content.
    sidecars_of: HashMap<ContentId, Vec<RelPath>>,
}

impl Planner<'_> {
    /// A sidecar we wrote for one of `contents` is at `rel` (ADR-0045): the name is ours to keep.
    fn our_sidecar(&self, rel: &RelPath, contents: &[ContentId]) -> io::Result<bool> {
        let key = collision_key(rel);
        Ok(!self.reserved.contains(&key)
            && self
                .sidecars
                .get(&key)
                .is_some_and(|c| contents.contains(c))
            && matches!(self.probe.existing(rel)?, Existing::File { .. }))
    }

    /// Name free apart from a possible stale partial of ours.
    fn free_name(&self, rel: &RelPath) -> io::Result<bool> {
        let key = collision_key(rel);
        if self.reserved.contains(&key) {
            return Ok(false);
        }
        if self.probe.existing(rel)? != Existing::Absent {
            return Ok(false);
        }
        Ok(self.probe.existing(&partial_of(rel))? == Existing::Absent
            || self.in_progress.contains(&key))
    }

    fn slot(&self, rel: &RelPath, f: &KeeperFile) -> io::Result<Slot> {
        if self.reserved.contains(&collision_key(rel)) {
            return Ok(Slot::Taken);
        }
        match self.probe.existing(rel)? {
            Existing::Absent => Ok(if self.free_name(rel)? {
                Slot::Free
            } else {
                Slot::Taken
            }),
            Existing::File { size, .. }
                if size == f.size && self.probe.identical(rel, &f.content, size)? =>
            {
                Ok(Slot::Identical)
            }
            _ => Ok(Slot::Taken),
        }
    }
}

/// Build the plan. `ledger` holds this library's rows; `probe` reads the library.
pub fn plan(
    input: &ExportInput,
    ledger: &[LedgerEntry],
    library_root: &Path,
    probe: &dyn LibraryProbe,
    settings: ProfileSettings,
) -> Result<Plan, PlanError> {
    let library_root = check_overlap(library_root, &input.sources)?;

    let mut done: HashMap<ContentId, Vec<&LedgerEntry>> = HashMap::new();
    let mut in_progress = HashSet::new();
    let mut sidecars = HashMap::new();
    let mut sidecars_of: HashMap<ContentId, Vec<RelPath>> = HashMap::new();
    for e in ledger {
        match e.state {
            LedgerState::Done if e.kind == LedgerKind::Sidecar => {
                sidecars.insert(collision_key(&e.library_rel_path), e.content);
                sidecars_of
                    .entry(e.content)
                    .or_default()
                    .push(e.library_rel_path.clone());
            }
            LedgerState::Done => done.entry(e.content).or_default().push(e),
            LedgerState::InProgress => {
                in_progress.insert(collision_key(&e.library_rel_path));
            }
            // A suspect file stays where it is; its name is taken (the probe sees it), its content is not
            // exported there.
            LedgerState::Failed | LedgerState::Suspect => {}
        }
    }
    let mut p = Planner {
        probe,
        reserved: HashSet::new(),
        in_progress,
        sidecars,
        sidecars_of,
    };

    let mut groups = bundles(input, settings.live_pairs_together);
    // Deterministic order: folder, then primary name, then file id.
    groups.retain(|b| !input.units[b[0]].files.is_empty());
    groups.sort_by_cached_key(|b| {
        let f = &input.units[b[0]].files[0];
        (folder_of(f), f.name().to_ascii_lowercase(), f.file)
    });

    let mut out = Plan {
        library_root,
        settings,
        bundles: Vec::new(),
        unavailable: Vec::new(),
        already_exported: Vec::new(),
        modified: Vec::new(),
        totals: PlanTotals::default(),
        free_bytes: None,
        required_bytes: 0,
    };

    for units in groups {
        let all: Vec<(usize, &KeeperFile)> = units
            .iter()
            .flat_map(|&u| input.units[u].files.iter().map(move |f| (u, f)))
            .collect();
        let offline = all.iter().find_map(|(_, f)| match input.source(f.source) {
            Some(s) if s.online => None,
            Some(s) => Some(s.name.clone()),
            None => Some(format!("source {}", f.source)),
        });
        if let Some(name) = offline {
            for (u, f) in &all {
                out.unavailable.push(Unavailable {
                    unit: *u,
                    file: f.file,
                    source: f.source,
                    source_name: input
                        .source(f.source)
                        .map_or_else(|| name.clone(), |s| s.name.clone()),
                });
            }
            continue;
        }

        // Keepers whose content is already in the library: a `done` ledger row whose copy is still there. The
        // row is trusted on the copy's size and recorded modification time; only a copy whose time changed
        // (or every copy, with `verify_library`) is re-hashed (ADR-0044). A copy still at its recorded path
        // whose bytes changed (another app wrote into it) is "changed in the library": reported, never copied
        // again unless the user asks (`recopy_modified`, ADR-0046).
        let mut todo = Vec::new();
        let mut already = Vec::new();
        for (u, f) in &all {
            let mut found = None;
            let mut changed = None;
            for e in done.get(&f.content).into_iter().flatten() {
                match check_copy(probe, e, settings.verify_library)? {
                    CopyCheck::Ok => {
                        found = Some(*e);
                        break;
                    }
                    CopyCheck::Missing => {}
                    c @ (CopyCheck::SizeMismatch { .. } | CopyCheck::HashMismatch) => {
                        changed.get_or_insert((*e, c));
                    }
                }
            }
            if found.is_none()
                && !settings.recopy_modified
                && let Some((e, check)) = changed
            {
                p.reserved.insert(collision_key(&e.library_rel_path));
                out.modified.push(ModifiedInLibrary {
                    unit: *u,
                    file: f.file,
                    library_rel_path: e.library_rel_path.clone(),
                    check,
                });
                continue;
            }
            match found {
                Some(e) => {
                    p.reserved.insert(collision_key(&e.library_rel_path));
                    let a = AlreadyExported {
                        unit: *u,
                        file: f.file,
                        library_rel_path: e.library_rel_path.clone(),
                        content: f.content,
                        full_hash: e.full_hash,
                    };
                    out.already_exported.push(a.clone());
                    already.push(a);
                }
                None => todo.push((*u, *f)),
            }
        }
        let contents: Vec<ContentId> = all.iter().map(|(_, f)| f.content).collect();
        let kind = if units.len() > 1 {
            BundleKind::LivePhoto
        } else if all.len() > 1 {
            BundleKind::KeeperSet
        } else {
            BundleKind::Single
        };
        if todo.is_empty() {
            if already.is_empty() {
                // Every file is a changed library copy: nothing to write.
                continue;
            }
            // Every file is in the library. With sidecars on and none of ours present, write the missing
            // sidecar next to the primary copy if that name is free (ADR-0045); a name taken by anything else
            // (a sidecar from before ledger rows, the user's own file) is left alone.
            if settings.sidecars {
                let mut ours = false;
                for rel in contents
                    .iter()
                    .filter_map(|c| p.sidecars_of.get(c))
                    .flatten()
                {
                    if matches!(probe.existing(rel)?, Existing::File { .. }) {
                        ours = true;
                        break;
                    }
                }
                let primary = &already[0].library_rel_path;
                let stem = split_name(primary.file_name()).0;
                let folder = primary
                    .parent()
                    .map(|f| f.display().into_owned())
                    .unwrap_or_default();
                if !ours
                    && let Some(rel) = join(&folder, &candidate(stem, b".xmp", 1))
                    && p.free_name(&rel)?
                {
                    p.reserved.insert(collision_key(&rel));
                    out.totals.sidecars += 1;
                    out.bundles.push(PlannedBundle {
                        units,
                        kind,
                        files: Vec::new(),
                        sidecar: Some(rel),
                        suffix: 1,
                        already,
                    });
                }
            }
            continue;
        }

        let folder = folder_of(all[0].1);
        let sidecar_stem = split_name(todo[0].1.name()).0.to_vec();
        let mut chosen = None;
        'suffix: for n in 1..=MAX_SUFFIX {
            let mut keys = HashSet::new();
            let mut slots = Vec::with_capacity(todo.len());
            for (_, f) in &todo {
                let (stem, ext) = split_name(f.name());
                let Some(rel) = join(&folder, &candidate(stem, ext, n)) else {
                    continue 'suffix;
                };
                if !keys.insert(collision_key(&rel)) {
                    continue 'suffix;
                }
                let s = p.slot(&rel, f)?;
                if s == Slot::Taken {
                    continue 'suffix;
                }
                slots.push((rel, s));
            }
            let mut sidecar = None;
            if settings.sidecars {
                let Some(rel) = join(&folder, &candidate(&sidecar_stem, b".xmp", n)) else {
                    continue 'suffix;
                };
                if keys.contains(&collision_key(&rel)) {
                    continue 'suffix;
                }
                if p.free_name(&rel)? {
                    sidecar = Some(rel);
                } else if p.our_sidecar(&rel, &contents)? {
                    // Our own sidecar of this content (its copy was deleted and is copied again): keep the
                    // name and the sidecar as it is.
                } else {
                    // An existing sidecar is only acceptable next to our own identical files.
                    let ours = slots.iter().all(|(_, s)| *s == Slot::Identical)
                        && !p.reserved.contains(&collision_key(&rel))
                        && matches!(probe.existing(&rel)?, Existing::File { .. });
                    if !ours {
                        continue 'suffix;
                    }
                }
            }
            chosen = Some((n, slots, sidecar));
            break;
        }
        let Some((n, slots, sidecar)) = chosen else {
            return Err(PlanError::NoFreeName {
                name: String::from_utf8_lossy(todo[0].1.name()).into_owned(),
                folder,
            });
        };

        let mut files = Vec::with_capacity(todo.len());
        for ((u, f), (target, slot)) in todo.iter().zip(slots) {
            p.reserved.insert(collision_key(&target));
            let action = if slot == Slot::Identical {
                out.totals.adopted += 1;
                PlannedAction::Adopt
            } else {
                out.totals.files += 1;
                out.totals.bytes += f.size;
                PlannedAction::Copy
            };
            if n > 1 {
                out.totals.collisions += 1;
            }
            files.push(PlannedFile {
                unit: *u,
                file: f.file,
                source: f.source,
                source_path: input
                    .source_path(f.source, f.read_path())
                    .unwrap_or_default(),
                target,
                size: f.size,
                mtime_ns: f.mtime_ns,
                content: f.content,
                action,
                date_origin: f.date_origin,
            });
            out.totals.dates.add(f.date_origin);
        }
        if let Some(s) = &sidecar {
            p.reserved.insert(collision_key(s));
            out.totals.sidecars += 1;
        }
        out.bundles.push(PlannedBundle {
            units,
            kind,
            files,
            sidecar,
            suffix: n,
            already,
        });
    }

    out.totals.unavailable = out.unavailable.len() as u64;
    out.totals.already_exported = out.already_exported.len() as u64;
    out.totals.modified = out.modified.len() as u64;
    out.required_bytes = if out.totals.files + out.totals.sidecars == 0 {
        0
    } else {
        out.totals.bytes + out.totals.sidecars * SIDECAR_RESERVE_BYTES + MANIFEST_RESERVE_BYTES
    };
    out.free_bytes = probe.free_space()?;
    Ok(out)
}
