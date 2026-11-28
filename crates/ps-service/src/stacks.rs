//! Stacks of similar shots over a catalog (best-of-stacks, ADR-0135): the stored groups and the grouping inputs,
//! read once, and stacks computed from them under any thresholds without reading a source file.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;
use ps_dedup::{DedupInput, GroupMembership, ShotQuality, StackDecisions, StackThresholds, Stacks};
use ps_model::{ContentId, FileId};

use crate::scan::ScanService;

pub use ps_dedup::{PickReason, Stack};

/// What stacks are computed from: the analyzed files' grouping inputs and the stored group membership.
#[derive(Debug, Default)]
pub struct StackInputs {
    pub inputs: Vec<DedupInput>,
    pub groups: GroupMembership,
    /// Files excluded by a user decision (or an accepted AI suggestion).
    pub excluded: HashSet<FileId>,
    /// File -> its index in `inputs`.
    by_file: HashMap<FileId, usize>,
    /// Group keeper -> every member of its group (the keeper included).
    pub members_of: HashMap<FileId, Vec<FileId>>,
}

impl StackInputs {
    /// Read the catalog behind `svc` (catalog reads only; no source file is opened).
    pub fn load(svc: &ScanService) -> Result<StackInputs> {
        let catalog = svc.engine().catalog();
        let inputs = ps_scan::post::load_inputs(catalog, None)?;
        let r = catalog.read();
        let (groups, members_of) = r.with(|c| {
            let mut m = GroupMembership::default();
            let mut members_of: HashMap<FileId, Vec<FileId>> = HashMap::new();
            let mut stmt = c.prepare_cached(
                "SELECT g.keeper_file_id, m.file_id FROM dup_group g JOIN dup_member m ON m.group_id = g.id",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let (k, f) = (FileId(row.get(0)?), FileId(row.get(1)?));
                m.keepers.insert(k);
                m.grouped.insert(f);
                members_of.entry(k).or_default().push(f);
            }
            Ok((m, members_of))
        })?;
        let excluded = r.user_excluded_files()?.into_iter().collect();
        let by_file = inputs
            .iter()
            .enumerate()
            .map(|(i, x)| (x.file, i))
            .collect();
        Ok(StackInputs {
            inputs,
            groups,
            excluded,
            by_file,
            members_of,
        })
    }

    /// The stacks under `thresholds`, with the user's picks and separations and the shot-quality scores.
    pub fn stacks(
        &self,
        thresholds: StackThresholds,
        separations: &BTreeSet<(ContentId, ContentId)>,
        picks: &BTreeSet<ContentId>,
        quality: &HashMap<FileId, ShotQuality>,
    ) -> Stacks {
        let d = StackDecisions {
            separations,
            picks,
            excluded: &self.excluded,
            quality,
        };
        ps_dedup::stacks_of(&self.inputs, &self.groups, thresholds, &d)
    }

    /// The input of file `f`.
    pub fn input(&self, f: FileId) -> Option<&DedupInput> {
        self.by_file.get(&f).map(|&i| &self.inputs[i])
    }
}

/// What [`StackInputs`] and the scores depend on: the stored groups and scan runs (as for the review list), the
/// stored exclusions and how many files are scored. The same key means the same inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct InputsKey {
    groups: crate::review::GroupsKey,
    excluded: crate::review::ExclusionsKey,
    scored: i64,
}

fn inputs_key(svc: &ScanService, store: &crate::review::ReviewStore) -> Result<InputsKey> {
    let scored = svc.engine().catalog().read().with(|c| {
        Ok(c.query_row(
            "SELECT count(*) FROM file WHERE shot_quality_version IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )?)
    })?;
    Ok(InputsKey {
        groups: crate::review::groups_key(svc, store)?,
        excluded: crate::review::exclusions_key(svc)?,
        scored,
    })
}

/// The stacks of one set of thresholds, as the review and the gallery read them.
#[derive(Debug, Default)]
pub struct StacksView {
    pub thresholds: StackThresholds,
    pub stacks: Stacks,
    /// Shot -> its stack's index.
    pub index: HashMap<FileId, usize>,
    /// Each shot's content identity (picks, separations and review state are kept by content).
    pub contents: HashMap<FileId, ContentId>,
    /// Shots whose scores are not there yet ("Scores pending").
    pub unscored: HashSet<FileId>,
    /// Each shot's scores.
    pub quality: HashMap<FileId, ShotQuality>,
    /// Each shot's file name.
    pub names: HashMap<FileId, String>,
    /// Shots that are duplicate groups -> every copy (the shot first).
    pub copies: HashMap<FileId, Vec<FileId>>,
}

impl StacksView {
    /// The stack holding shot `f`.
    pub fn stack_of(&self, f: FileId) -> Option<&ps_dedup::Stack> {
        self.index.get(&f).map(|&i| &self.stacks.stacks[i])
    }

    /// A stack's anchor: the content of its first shot (what its review state is kept under).
    pub fn anchor(&self, s: &ps_dedup::Stack) -> Option<ContentId> {
        s.shots.first().and_then(|f| self.contents.get(f)).copied()
    }

    fn contents_of(&self, s: &ps_dedup::Stack) -> Vec<ContentId> {
        s.shots
            .iter()
            .filter_map(|f| self.contents.get(f))
            .copied()
            .collect()
    }
}

/// The last loaded inputs and scores: a threshold change, a pick or a take-out recomputes stacks from them without
/// reading the catalog again (ADR-0135 decision 3).
#[derive(Debug, Default)]
pub struct StackCache {
    last: std::sync::Mutex<
        Option<(
            InputsKey,
            std::sync::Arc<(StackInputs, HashMap<FileId, ShotQuality>)>,
        )>,
    >,
}

impl StackCache {
    pub fn new() -> StackCache {
        StackCache::default()
    }

    fn inputs(
        &self,
        svc: &ScanService,
        store: &crate::review::ReviewStore,
    ) -> Result<std::sync::Arc<(StackInputs, HashMap<FileId, ShotQuality>)>> {
        let key = inputs_key(svc, store)?;
        if let Some((k, v)) = &*self.last.lock().unwrap_or_else(|e| e.into_inner())
            && *k == key
        {
            return Ok(v.clone());
        }
        let inputs = StackInputs::load(svc)?;
        let quality = svc.engine().catalog().read().shot_qualities()?;
        let v = std::sync::Arc::new((inputs, quality));
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, v.clone()));
        Ok(v)
    }

    /// The stacks under `thresholds`, with the stored picks and separations.
    pub fn view(
        &self,
        svc: &ScanService,
        store: &crate::review::ReviewStore,
        thresholds: StackThresholds,
    ) -> Result<StacksView> {
        let v = self.inputs(svc, store)?;
        let (inputs, quality) = (&v.0, &v.1);
        let d = svc.engine().catalog().read().stack_decisions()?;
        let stacks = inputs.stacks(thresholds, &d.separations, &d.picks, quality);
        let index = stacks.index();
        let contents: HashMap<FileId, ContentId> = index
            .keys()
            .filter_map(|f| inputs.input(*f).and_then(|x| x.content).map(|c| (*f, c)))
            .collect();
        let unscored = index
            .keys()
            .filter(|f| !quality.contains_key(f))
            .copied()
            .collect();
        let quality = index
            .keys()
            .filter_map(|f| quality.get(f).map(|q| (*f, *q)))
            .collect();
        let names = index
            .keys()
            .filter_map(|f| {
                inputs.input(*f).map(|x| {
                    let name = x.path.rsplit('/').next().unwrap_or(&x.path);
                    (*f, name.to_owned())
                })
            })
            .collect();
        let copies = index
            .keys()
            .filter_map(|f| {
                inputs.members_of.get(f).map(|m| {
                    let mut all = vec![*f];
                    all.extend(m.iter().copied().filter(|x| x != f));
                    (*f, all)
                })
            })
            .collect();
        Ok(StacksView {
            copies,
            names,
            thresholds,
            stacks,
            index,
            contents,
            unscored,
            quality,
        })
    }
}

/// Make shot `file` its stack's pick ("Picked by you"; kept by content across restarts and rescans).
pub fn pick(svc: &ScanService, view: &StacksView, file: FileId) -> Result<()> {
    let s = view
        .stack_of(file)
        .ok_or_else(|| anyhow::anyhow!("file {file} is in no stack"))?;
    let c = *view
        .contents
        .get(&file)
        .ok_or_else(|| anyhow::anyhow!("file {file} has no content identity"))?;
    let _edit = svc.edit()?;
    svc.engine()
        .catalog()
        .writer()
        .set_stack_pick(view.contents_of(s), Some(c))?;
    Ok(())
}

/// Back to the automatic pick for the stack holding `file`.
pub fn clear_pick(svc: &ScanService, view: &StacksView, file: FileId) -> Result<()> {
    let s = view
        .stack_of(file)
        .ok_or_else(|| anyhow::anyhow!("file {file} is in no stack"))?;
    let _edit = svc.edit()?;
    svc.engine()
        .catalog()
        .writer()
        .set_stack_pick(view.contents_of(s), None)?;
    Ok(())
}

/// Take shot `file` out of its stack: it is never stacked again with the shots that stack holds now, under any
/// thresholds, until [`put_back`].
pub fn take_out(svc: &ScanService, view: &StacksView, file: FileId) -> Result<()> {
    let s = view
        .stack_of(file)
        .ok_or_else(|| anyhow::anyhow!("file {file} is in no stack"))?;
    let c = *view
        .contents
        .get(&file)
        .ok_or_else(|| anyhow::anyhow!("file {file} has no content identity"))?;
    let pairs: Vec<(ContentId, ContentId)> = view
        .contents_of(s)
        .into_iter()
        .filter(|o| *o != c)
        .map(|o| (c, o))
        .collect();
    let _edit = svc.edit()?;
    svc.engine()
        .catalog()
        .writer()
        .set_stack_separations(pairs, true)?;
    Ok(())
}

/// Put shot `file` back: every "taken out" pair with its content is removed.
pub fn put_back(svc: &ScanService, file: FileId) -> Result<()> {
    let r = svc.engine().catalog().read();
    let c = r
        .file_by_id(file)?
        .and_then(|rec| rec.identity)
        .ok_or_else(|| anyhow::anyhow!("file {file} has no content identity"))?;
    let pairs: Vec<(ContentId, ContentId)> = r
        .stack_decisions()?
        .separations
        .into_iter()
        .filter(|(a, b)| *a == c || *b == c)
        .collect();
    let _edit = svc.edit()?;
    svc.engine()
        .catalog()
        .writer()
        .set_stack_separations(pairs, false)?;
    Ok(())
}

/// Shots taken out of a stack (each content's file ids are not tracked; the view maps them).
pub fn taken_out(svc: &ScanService) -> Result<BTreeSet<ContentId>> {
    let d = svc.engine().catalog().read().stack_decisions()?;
    Ok(d.separations
        .into_iter()
        .flat_map(|(a, b)| [a, b])
        .collect())
}

impl StacksView {
    /// Every file of a stack: each shot and, for a shot that is a duplicate group, all its copies.
    pub fn files_of(&self, s: &ps_dedup::Stack) -> Vec<FileId> {
        let mut out = Vec::new();
        for f in &s.shots {
            match self.copies.get(f) {
                Some(all) => out.extend(all.iter().copied()),
                None => out.push(*f),
            }
        }
        out
    }
}

/// What [`exclude_stack`] replaced, so [`undo_exclude_stack`] can put it back exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct StackExclusion {
    pub prior: Vec<crate::review::PriorDecision>,
}

/// Keep none of a stack: every copy of every shot gets the user decision "exclude" (with `reason`), in one catalog
/// transaction; copies already excluded keep their own record. Excluded shots are never stacked, so the stack
/// leaves the Stacks tab. Nothing on disk changes.
pub fn exclude_stack(
    svc: &ScanService,
    view: &StacksView,
    stack: usize,
    reason: Option<String>,
) -> Result<StackExclusion> {
    let s = view
        .stacks
        .stacks
        .get(stack)
        .ok_or_else(|| anyhow::anyhow!("no stack {stack}"))?;
    let files = view.files_of(s);
    let reason = reason
        .map(|r| r.trim().to_owned())
        .filter(|r| !r.is_empty());
    let _edit = svc.edit()?;
    let r = svc.engine().catalog().read();
    let before = r.user_decisions_of(&files)?;
    let changed: Vec<FileId> = files
        .iter()
        .copied()
        .filter(|f| {
            before
                .get(f)
                .is_none_or(|x| x.decision != ps_catalog::UserDecision::Exclude)
        })
        .collect();
    let prior = changed
        .iter()
        .map(|f| match before.get(f) {
            Some(x) => crate::review::PriorDecision {
                file: *f,
                decision: Some(x.decision),
                reason: x.reason.clone(),
                ai_kind: x.ai_kind,
            },
            None => crate::review::PriorDecision {
                file: *f,
                decision: None,
                reason: None,
                ai_kind: None,
            },
        })
        .collect();
    svc.engine().catalog().writer().set_user_decisions(
        changed
            .iter()
            .map(|f| ps_catalog::DecisionWrite {
                file: *f,
                decision: Some(ps_catalog::UserDecision::Exclude),
                reason: reason.clone(),
                ai_kind: None,
            })
            .collect(),
    )?;
    Ok(StackExclusion { prior })
}

/// Put back what [`exclude_stack`] changed: every copy gets the decision it had.
pub fn undo_exclude_stack(svc: &ScanService, x: &StackExclusion) -> Result<()> {
    let _edit = svc.edit()?;
    svc.engine().catalog().writer().set_user_decisions(
        x.prior
            .iter()
            .map(|p| ps_catalog::DecisionWrite {
                file: p.file,
                decision: p.decision,
                reason: p.reason.clone(),
                ai_kind: p.ai_kind,
            })
            .collect(),
    )?;
    Ok(())
}
