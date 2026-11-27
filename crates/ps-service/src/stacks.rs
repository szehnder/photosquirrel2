//! Stacks of similar shots over a catalog (best-of-stacks, ADR-0135): the stored groups and the grouping inputs,
//! read once, and stacks computed from them under any thresholds without reading a source file.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;
use ps_dedup::{DedupInput, GroupMembership, ShotQuality, StackDecisions, StackThresholds, Stacks};
use ps_model::{ContentId, FileId};

use crate::scan::ScanService;

/// What stacks are computed from: the analyzed files' grouping inputs and the stored group membership.
#[derive(Debug, Default)]
pub struct StackInputs {
    pub inputs: Vec<DedupInput>,
    pub groups: GroupMembership,
    /// Files excluded by a user decision (or an accepted AI suggestion).
    pub excluded: HashSet<FileId>,
}

impl StackInputs {
    /// Read the catalog behind `svc` (catalog reads only; no source file is opened).
    pub fn load(svc: &ScanService) -> Result<StackInputs> {
        let catalog = svc.engine().catalog();
        let inputs = ps_scan::post::load_inputs(catalog, None)?;
        let r = catalog.read();
        let groups = r.with(|c| {
            let mut m = GroupMembership::default();
            let mut stmt = c.prepare_cached(
                "SELECT g.keeper_file_id, m.file_id FROM dup_group g JOIN dup_member m ON m.group_id = g.id",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                m.keepers.insert(FileId(row.get(0)?));
                m.grouped.insert(FileId(row.get(1)?));
            }
            Ok(m)
        })?;
        let excluded = r.user_excluded_files()?.into_iter().collect();
        Ok(StackInputs {
            inputs,
            groups,
            excluded,
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
        self.inputs.iter().find(|x| x.file == f)
    }
}
