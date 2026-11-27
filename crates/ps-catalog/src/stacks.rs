//! What stacks of similar shots store (ADR-0135, migration 27): the user's stack decisions and each photo's
//! shot-quality scores. The stacks themselves are computed, never stored.

use std::collections::{BTreeSet, HashMap};

use rusqlite::params;

use ps_model::{ContentId, FileId, SHOT_QUALITY_VERSION, ShotQuality};

use crate::error::Result;
use crate::groups::{content_from, kind_code, ordered, pair_rows};
use crate::model::now_ms;
use crate::read::Readers;
use crate::writer::Writer;

/// The user's stack decisions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackDecisionRows {
    /// "Taken out of the stack", each pair ordered (smaller first).
    pub separations: BTreeSet<(ContentId, ContentId)>,
    /// "Pick this one".
    pub picks: BTreeSet<ContentId>,
}

impl Writer {
    /// Add (`add == true`) or remove "never stack together" pairs.
    pub fn set_stack_separations(
        &self,
        pairs: Vec<(ContentId, ContentId)>,
        add: bool,
    ) -> Result<()> {
        self.exec_constraints(move |c| pair_rows(c, "stack_separation", &pairs, add))
    }

    /// Make `pick` the user's pick among `stack` (the contents of one stack): every other pick of those contents
    /// is removed in the same transaction. `None` clears them all (back to the automatic pick).
    pub fn set_stack_pick(&self, stack: Vec<ContentId>, pick: Option<ContentId>) -> Result<()> {
        self.exec_constraints(move |c| {
            for x in &stack {
                c.prepare_cached(
                    "DELETE FROM stack_pick WHERE content_id = ?1 AND content_kind = ?2",
                )?
                .execute(params![&x.digest[..], kind_code(x)])?;
            }
            if let Some(p) = pick {
                c.prepare_cached(
                    "INSERT INTO stack_pick (content_id, content_kind, at) VALUES (?1, ?2, ?3)
                     ON CONFLICT DO UPDATE SET at = excluded.at",
                )?
                .execute(params![&p.digest[..], kind_code(&p), now_ms()])?;
            }
            Ok(())
        })
    }

    /// Store shot-quality scores (version [`SHOT_QUALITY_VERSION`]) in one transaction. `None` records that the
    /// file was tried and has no score (its thumbnail could not be decoded), so it is not tried again.
    pub fn set_shot_quality(&self, rows: Vec<(FileId, Option<ShotQuality>)>) -> Result<()> {
        self.exec(move |c| {
            let mut stmt = c.prepare_cached(
                "UPDATE file SET shot_quality_version = ?2, sharpness = ?3, clipped_high = ?4, clipped_low = ?5
                 WHERE id = ?1",
            )?;
            for (f, q) in &rows {
                stmt.execute(params![
                    f.0,
                    SHOT_QUALITY_VERSION,
                    q.map(|q| f64::from(q.sharpness)),
                    q.map(|q| f64::from(q.clipped_high)),
                    q.map(|q| f64::from(q.clipped_low))
                ])?;
            }
            Ok(())
        })
    }
}

impl Readers {
    /// The user's stack decisions.
    pub fn stack_decisions(&self) -> Result<StackDecisionRows> {
        self.with(|c| {
            let mut out = StackDecisionRows::default();
            let mut stmt = c.prepare_cached(
                "SELECT content_a, kind_a, content_b, kind_b FROM stack_separation",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let a = content_from(r.get(0)?, r.get(1)?)?;
                let b = content_from(r.get(2)?, r.get(3)?)?;
                out.separations.insert(ordered(a, b));
            }
            let mut stmt = c.prepare_cached("SELECT content_id, content_kind FROM stack_pick")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                out.picks.insert(content_from(r.get(0)?, r.get(1)?)?);
            }
            Ok(out)
        })
    }

    /// Every stored shot-quality score of the current version.
    pub fn shot_qualities(&self) -> Result<HashMap<FileId, ShotQuality>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, sharpness, clipped_high, clipped_low FROM file
                 WHERE shot_quality_version = ?1 AND sharpness IS NOT NULL",
            )?;
            let mut rows = stmt.query([SHOT_QUALITY_VERSION])?;
            let mut out = HashMap::new();
            while let Some(r) = rows.next()? {
                out.insert(
                    FileId(r.get(0)?),
                    ShotQuality {
                        sharpness: r.get::<_, f64>(1)? as f32,
                        clipped_high: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0) as f32,
                        clipped_low: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0) as f32,
                    },
                );
            }
            Ok(out)
        })
    }

    /// Up to `limit` files with a stored thumbnail and no shot-quality score of the current version, with their
    /// content identity (the thumbnail key), smallest id first after `after`.
    pub fn unscored_shots(&self, after: FileId, limit: u32) -> Result<Vec<(FileId, ContentId)>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, content_id, identity_kind FROM file
                 WHERE id > ?1 AND stage = 3 AND preview_reason IS NULL AND content_id IS NOT NULL
                   AND (shot_quality_version IS NULL OR shot_quality_version <> ?2)
                 ORDER BY id LIMIT ?3",
            )?;
            let mut rows = stmt.query(params![after.0, SHOT_QUALITY_VERSION, limit])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let kind: Option<i64> = r.get(2)?;
                out.push((
                    FileId(r.get(0)?),
                    content_from(r.get(1)?, kind.unwrap_or(0))?,
                ));
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ps_model::ContentIdKind;

    fn content(b: u8) -> ContentId {
        ContentId {
            digest: [b; 32],
            kind: ContentIdKind::Full,
        }
    }

    #[test]
    fn stack_decisions_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cat = crate::Catalog::open(dir.path()).unwrap();
        let w = cat.writer();
        w.set_stack_separations(
            vec![(content(2), content(1)), (content(1), content(3))],
            true,
        )
        .unwrap();
        w.set_stack_pick(vec![content(1), content(2)], Some(content(2)))
            .unwrap();
        w.set_stack_pick(vec![content(1), content(2)], Some(content(1)))
            .unwrap();
        let d = cat.read().stack_decisions().unwrap();
        assert_eq!(
            d.separations,
            [(content(1), content(2)), (content(1), content(3))]
                .into_iter()
                .collect()
        );
        assert_eq!(
            d.picks,
            [content(1)].into_iter().collect(),
            "one pick per stack"
        );
        w.set_stack_separations(vec![(content(1), content(2))], false)
            .unwrap();
        w.set_stack_pick(vec![content(1), content(2)], None)
            .unwrap();
        let d = cat.read().stack_decisions().unwrap();
        assert_eq!(d.separations.len(), 1);
        assert!(d.picks.is_empty());
    }
}
