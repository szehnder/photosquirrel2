//! Placeholder rows (`preview-placeholders`, ADR-0109, catalog migration 24): the waiting rule that says which
//! placeholders can become previews now, the counts by format and reason, and the rows a Fill in previews job
//! reads.
//!
//! A placeholder row can be filled in now when any holds:
//! - it waits for a decoder (`decoder_missing = k`) and this machine can decode `k` (ADR-0091);
//! - it has no decoder marker and no recorded decode failure (those follow ADR-0091 and ADR-0098), and its
//!   `preview_gen` is below the running version's generation of its format's family, unless it is a video
//!   placeholder (`no_poster_tool`, `poster_failed`) and no poster-frame tool is available;
//! - it is a video waiting for a poster-frame tool (`no_poster_tool`) and a tool is available now, whatever its
//!   generation (the tool clause, ADR-0114).
//!
//! Current-generation placeholders whose variant is not supported, whose data could not be read or whose poster
//! frame failed wait for a change of the file (the normal diff) or of the generation; a failed poster is also
//! retried when the user asks (`retry_failed` of [`Readers::fillable_files`]). Missing files never count.

use rusqlite::params;
use rusqlite::types::Value;

use ps_model::{
    DecoderKey, FillScope, Format, Generations, PreviewReason, PreviewStatusRow, SourceId,
};

use crate::FileRecord;
use crate::error::Result;
use crate::read::{FILE_COLUMNS, Readers, file_record};

/// What this run of the app can decode: the inputs of the waiting rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitingRule {
    /// Decoders this machine has (files waiting for them can be filled in).
    pub decodable: Vec<DecoderKey>,
    /// The running version's decoder generations.
    pub generations: Generations,
    /// A poster-frame tool is available now (resolved when each run starts, ADR-0114).
    pub poster_tool: bool,
}

impl WaitingRule {
    /// The rule as an SQL condition over a `file` row (no parameters: every value is the build's own constant).
    pub fn sql(&self) -> String {
        let decodable: Vec<String> = self
            .decodable
            .iter()
            .map(|k| format!("'{}'", k.as_str()))
            .collect();
        let mut gen_case = String::from("(CASE coalesce(format, '')");
        for (f, g) in self.generations.formats_above_zero() {
            gen_case.push_str(&format!(" WHEN '{}' THEN {g}", f.storage_key()));
        }
        gen_case.push_str(" ELSE 0 END)");
        let missing = if decodable.is_empty() {
            "0".to_owned()
        } else {
            format!("decoder_missing IN ({})", decodable.join(", "))
        };
        let (no_tool, tool) = if self.poster_tool {
            ("0", "1")
        } else {
            ("1", "0")
        };
        format!(
            "(stage = 3 AND missing_since IS NULL AND ({missing}
              OR (decoder_missing IS NULL AND decode_failure IS NULL AND preview_gen IS NOT NULL
                  AND preview_reason IS NOT NULL AND preview_gen < {gen_case}
                  AND NOT (coalesce(preview_reason, '') IN ('no_poster_tool', 'poster_failed') AND {no_tool}))
              OR (decoder_missing IS NULL AND decode_failure IS NULL AND preview_gen IS NOT NULL
                  AND coalesce(preview_reason, '') = 'no_poster_tool' AND {tool})))"
        )
    }

    /// [`WaitingRule::sql`] with the explicit retry of failed posters (ADR-0114): `poster_failed` rows too, when a
    /// tool is available.
    pub fn sql_retrying_failed_posters(&self) -> String {
        if !self.poster_tool {
            return self.sql();
        }
        format!(
            "({} OR (stage = 3 AND missing_since IS NULL AND decoder_missing IS NULL AND decode_failure IS NULL
                     AND preview_gen IS NOT NULL AND coalesce(preview_reason, '') = 'poster_failed'))",
            self.sql()
        )
    }

    /// Whether `r` can be filled in now (the same rule in Rust, for one row).
    pub fn fillable(&self, r: &FileRecord) -> bool {
        if r.stage != ps_model::Stage::Thumbed || r.missing_since_ms.is_some() {
            return false;
        }
        if let Some(k) = r.decoder_missing {
            return self.decodable.contains(&k);
        }
        if r.decode_failure.is_some() {
            return false;
        }
        r.preview
            .is_some_and(|p| self.placeholder_waits(r.format, p))
    }

    /// The placeholder part of the rule, for a row without a decoder marker or a recorded decode failure: its
    /// generation is below this run's for `format` (a video placeholder only with a tool), or it is a video waiting
    /// for a poster-frame tool and one is available (the tool clause, ADR-0114). The rescan's diff uses it too.
    pub fn placeholder_waits(&self, format: Option<Format>, p: ps_model::PreviewStamp) -> bool {
        let video = matches!(
            p.reason,
            PreviewReason::NoPosterTool | PreviewReason::PosterFailed
        );
        (p.generation < self.generations.of_format(format) && !(video && !self.poster_tool))
            || (p.reason == PreviewReason::NoPosterTool && self.poster_tool)
    }

    /// [`WaitingRule::fillable`] with the explicit retry of failed posters ([`WaitingRule::sql_retrying_failed_posters`]).
    pub fn fillable_retrying_failed_posters(&self, r: &FileRecord) -> bool {
        self.fillable(r)
            || (self.poster_tool
                && r.stage == ps_model::Stage::Thumbed
                && r.missing_since_ms.is_none()
                && r.decoder_missing.is_none()
                && r.decode_failure.is_none()
                && r.preview
                    .is_some_and(|p| p.reason == PreviewReason::PosterFailed))
    }
}

fn scope_sql(scope: &FillScope, sql: &mut String, args: &mut Vec<Value>) {
    if let Some(f) = scope.format {
        sql.push_str(" AND format = ?");
        args.push(Value::Text(f.storage_key()));
    }
    if let Some(r) = scope.reason {
        sql.push_str(" AND preview_reason = ?");
        args.push(Value::Text(r.as_str().to_owned()));
    }
}

/// The condition of a present file catalogued as damaged ([`Readers::damaged_count`]), answered from the partial
/// index `file_preview`.
pub const DAMAGED_WHERE: &str = "preview_reason = 'unreadable' AND missing_since IS NULL";

/// Source ids inline (integers only).
fn sources_sql(sources: &[SourceId], sql: &mut String) {
    if !sources.is_empty() {
        let ids: Vec<String> = sources.iter().map(|s| s.0.to_string()).collect();
        sql.push_str(&format!(" AND source_id IN ({})", ids.join(", ")));
    }
}

/// Migration 25's one-time task (ADR-0111): stamp video placeholder rows of older catalogs with the reason their
/// stored thumbnail records.
pub const VIDEO_PLACEHOLDER_TASK: &str = "video_placeholder_reasons";

impl Readers {
    /// Whether the one-time catalog task `name` is still to do (`catalog_task`, migration 25). `false` on older
    /// catalogs (opened read-only).
    pub fn catalog_task_pending(&self, name: &str) -> Result<bool> {
        if self.schema_version()? < crate::db::VIDEO_PLACEHOLDER_MIGRATION.version {
            return Ok(false);
        }
        self.with(|c| {
            Ok(
                c.prepare_cached("SELECT EXISTS (SELECT 1 FROM catalog_task WHERE name = ?1)")?
                    .query_row([name], |r| r.get::<_, bool>(0))?,
            )
        })
    }

    /// Thumbed video rows without a placeholder reason (the work list of [`VIDEO_PLACEHOLDER_TASK`]): those with a
    /// poster frame and those catalogued with a placeholder before migration 24.
    pub fn unmarked_video_rows(&self) -> Result<Vec<FileRecord>> {
        if self.schema_version()? < crate::db::PREVIEW_GEN_MIGRATION.version {
            return Ok(Vec::new());
        }
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {FILE_COLUMNS} FROM file
                 WHERE stage = 3 AND media_kind = 1 AND preview_reason IS NULL AND content_id IS NOT NULL"
            ))?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(file_record(r)?);
            }
            Ok(out)
        })
    }

    /// Placeholders of present files by format and reason, with how many can be filled in now (`previews
    /// status`, the Library notice): one aggregate over the partial index `file_preview`. `sources` empty: every
    /// source. Empty on a catalog older than migration 24 (opened read-only).
    pub fn preview_status(
        &self,
        rule: &WaitingRule,
        sources: &[SourceId],
    ) -> Result<Vec<PreviewStatusRow>> {
        if self.schema_version()? < crate::db::PREVIEW_GEN_MIGRATION.version {
            return Ok(Vec::new());
        }
        let mut sql = format!(
            "SELECT coalesce(format, ''), preview_reason, count(*), sum(CASE WHEN {} THEN 1 ELSE 0 END)
             FROM file WHERE preview_reason IS NOT NULL AND missing_since IS NULL",
            rule.sql()
        );
        sources_sql(sources, &mut sql);
        sql.push_str(" GROUP BY 1, 2 ORDER BY 1, 2");
        self.with(|c| {
            let mut stmt = c.prepare_cached(&sql)?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let reason: String = r.get(1)?;
                out.push(PreviewStatusRow {
                    format: r.get(0)?,
                    reason: PreviewReason::from_key(&reason)
                        .unwrap_or(PreviewReason::NeedsNewerVersion),
                    count: r.get::<_, i64>(2)? as u64,
                    fillable: r.get::<_, i64>(3)? as u64,
                });
            }
            Ok(out)
        })
    }

    /// Present files that can be filled in now within `scope` and `sources` (all when empty), by source and path
    /// (each lane reads in path order). The Fill in previews job's work list. `retry_failed`: also the videos whose
    /// poster frame failed, when a tool is available (the explicit retry, ADR-0114).
    pub fn fillable_files(
        &self,
        rule: &WaitingRule,
        scope: &FillScope,
        sources: &[SourceId],
        retry_failed: bool,
    ) -> Result<Vec<FileRecord>> {
        if self.schema_version()? < crate::db::PREVIEW_GEN_MIGRATION.version {
            return Ok(Vec::new());
        }
        let mut sql = format!(
            "SELECT {FILE_COLUMNS} FROM file WHERE preview_reason IS NOT NULL AND {}",
            if retry_failed {
                rule.sql_retrying_failed_posters()
            } else {
                rule.sql()
            }
        );
        let mut args = Vec::new();
        scope_sql(scope, &mut sql, &mut args);
        sources_sql(sources, &mut sql);
        sql.push_str(" ORDER BY source_id, rel_path");
        self.with(|c| {
            let mut stmt = c.prepare_cached(&sql)?;
            let mut rows = stmt.query(rusqlite::params_from_iter(args))?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(file_record(r)?);
            }
            Ok(out)
        })
    }

    /// Present files of `sources` (every source when empty) catalogued as could not be read (damaged:
    /// `preview_reason = 'unreadable'`, ADR-0115), whatever their format: one aggregate over the partial index
    /// `file_preview`. 0 on a catalog older than migration 24 (opened read-only).
    pub fn damaged_count(&self, sources: &[SourceId]) -> Result<u64> {
        if self.schema_version()? < crate::db::PREVIEW_GEN_MIGRATION.version {
            return Ok(0);
        }
        let mut sql = format!("SELECT count(*) FROM file WHERE {DAMAGED_WHERE}");
        sources_sql(sources, &mut sql);
        self.with(|c| {
            Ok(c.prepare_cached(&sql)?
                .query_row([], |r| r.get::<_, i64>(0))? as u64)
        })
    }

    /// How many present files have a placeholder of `reason`, of `format` when given (the dialog's "N other
    /// files").
    pub fn preview_count(&self, reason: PreviewReason, format: Option<Format>) -> Result<u64> {
        if self.schema_version()? < crate::db::PREVIEW_GEN_MIGRATION.version {
            return Ok(0);
        }
        self.with(|c| {
            Ok(c.prepare_cached(
                "SELECT count(*) FROM file
                 WHERE preview_reason = ?1 AND (?2 IS NULL OR format = ?2) AND missing_since IS NULL",
            )?
            .query_row(
                params![reason.as_str(), format.map(Format::storage_key)],
                |r| r.get::<_, i64>(0),
            )? as u64)
        })
    }
}
