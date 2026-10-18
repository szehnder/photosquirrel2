//! The export plan (dry run, ADR-0004, ADR-0022). Planning only reads: the ledger rows, the library folder
//! (existence, sizes, hashes of same-size files for adoption) and the free space. Nothing is written until the
//! user confirms and [`crate::execute`] runs the plan.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use ps_model::{ContentId, FileId, LedgerEntry, LedgerState, RelPath, SourceId};
use serde::Serialize;

use crate::input::{ExportInput, KeeperFile, SourceInfo};
use crate::layout::{MAX_SUFFIX, candidate, collision_key, folder_of, join, split_name};
use crate::library::{Existing, LibraryProbe, normalize, partial_of};
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
}

impl Planner<'_> {
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
            Existing::File { size }
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
    for e in ledger {
        match e.state {
            LedgerState::Done => done.entry(e.content).or_default().push(e),
            LedgerState::InProgress => {
                in_progress.insert(collision_key(&e.library_rel_path));
            }
            LedgerState::Failed => {}
        }
    }
    let mut p = Planner {
        probe,
        reserved: HashSet::new(),
        in_progress,
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

        // Keepers whose content is already in the library (ledger row and the copy still there).
        let mut todo = Vec::new();
        for (u, f) in &all {
            let mut found = None;
            for e in done.get(&f.content).into_iter().flatten() {
                if probe.existing(&e.library_rel_path)? == (Existing::File { size: e.size }) {
                    found = Some(e.library_rel_path.clone());
                    break;
                }
            }
            match found {
                Some(path) => {
                    p.reserved.insert(collision_key(&path));
                    out.already_exported.push(AlreadyExported {
                        unit: *u,
                        file: f.file,
                        library_rel_path: path,
                    });
                }
                None => todo.push((*u, *f)),
            }
        }
        if todo.is_empty() {
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
                source_path: input.source_path(f.source, &f.rel_path).unwrap_or_default(),
                target,
                size: f.size,
                mtime_ns: f.mtime_ns,
                content: f.content,
                action,
            });
        }
        if let Some(s) = &sidecar {
            p.reserved.insert(collision_key(s));
            out.totals.sidecars += 1;
        }
        let kind = if units.len() > 1 {
            BundleKind::LivePhoto
        } else if all.len() > 1 {
            BundleKind::KeeperSet
        } else {
            BundleKind::Single
        };
        out.bundles.push(PlannedBundle {
            units,
            kind,
            files,
            sidecar,
            suffix: n,
        });
    }

    out.totals.unavailable = out.unavailable.len() as u64;
    out.totals.already_exported = out.already_exported.len() as u64;
    out.required_bytes = if out.totals.files + out.totals.sidecars == 0 {
        0
    } else {
        out.totals.bytes + out.totals.sidecars * SIDECAR_RESERVE_BYTES + MANIFEST_RESERVE_BYTES
    };
    out.free_bytes = probe.free_space()?;
    Ok(out)
}
