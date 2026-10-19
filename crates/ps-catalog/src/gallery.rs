//! Gallery queries (gallery change; ADR-0015, ADR-0018, ADR-0023): user include/exclude decisions, the
//! filter bar's extra predicates and facets, and exact-duplicate stacks.
//!
//! User decisions live in `user_inclusion` ([`crate::db::GALLERY_MIGRATION`]). Until that migration is
//! registered the table is absent: [`Readers::has_user_decisions`] says so, the class filter falls back to
//! the classifier's verdict and [`crate::Command::SetUserDecision`] reports an error.

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};

use ps_model::{FileId, Filter, PhotoClass};

use crate::error::{CatalogError, Result};
use crate::read::Readers;

/// What the user decided about a file. The classifier's result is kept; this wins in queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UserDecision {
    /// Show it in the Library although the classifier set it aside.
    Include,
    /// Set it aside although the classifier kept it.
    Exclude,
}

impl UserDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            UserDecision::Include => "include",
            UserDecision::Exclude => "exclude",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "include" => Some(UserDecision::Include),
            "exclude" => Some(UserDecision::Exclude),
            _ => None,
        }
    }

    /// The reason shown beside the classifier's reasons ("included by you", "excluded by you: screenshot").
    pub fn reason_text(self, reason: Option<&str>) -> String {
        let base = match self {
            UserDecision::Include => "included by you",
            UserDecision::Exclude => "excluded by you",
        };
        match reason.map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) => format!("{base}: {r}"),
            None => base.to_owned(),
        }
    }
}

/// A stored decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserDecisionRecord {
    pub file: FileId,
    pub decision: UserDecision,
    pub reason: Option<String>,
    pub at_ms: i64,
}

fn table_exists(c: &Connection, name: &str) -> Result<bool> {
    Ok(
        c.prepare_cached("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?
            .query_row([name], |_| Ok(()))
            .optional()?
            .is_some(),
    )
}

impl Readers {
    /// Whether this catalog stores user decisions (migration 7 applied).
    pub fn has_user_decisions(&self) -> Result<bool> {
        self.with(|c| table_exists(c, "user_inclusion"))
    }

    /// The user's decision about a file, if any.
    pub fn user_decision(&self, id: FileId) -> Result<Option<UserDecisionRecord>> {
        self.with(|c| {
            if !table_exists(c, "user_inclusion")? {
                return Ok(None);
            }
            c.prepare_cached(
                "SELECT file_id, decision, reason, at FROM user_inclusion WHERE file_id = ?1",
            )?
            .query_row([id.0], decision_row)
            .optional()?
            .transpose()
        })
    }

    /// Every stored decision, by file id.
    pub fn user_decisions(&self) -> Result<Vec<UserDecisionRecord>> {
        self.with(|c| {
            if !table_exists(c, "user_inclusion")? {
                return Ok(Vec::new());
            }
            let mut stmt = c.prepare_cached(
                "SELECT file_id, decision, reason, at FROM user_inclusion ORDER BY file_id",
            )?;
            let rows = stmt.query_map([], decision_row)?;
            rows.map(|r| r.map_err(CatalogError::from).and_then(|r| r))
                .collect()
        })
    }

    /// Camera models of present files with their counts, most common first (filter bar facet).
    pub fn camera_models(&self) -> Result<Vec<(String, u64)>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT camera_model, count(*) AS n FROM file
                 WHERE camera_model IS NOT NULL AND kflags < 16
                 GROUP BY camera_model ORDER BY n DESC, camera_model",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Stored duplicate groups (ADR-0013) as stacks: every grouped file with its group's keeper and the
    /// group's member count. Empty until a regroup has run.
    pub fn group_stacks(&self) -> Result<Vec<(FileId, FileId, u32)>> {
        self.with(|c| {
            if !table_exists(c, "dup_member")? {
                return Ok(Vec::new());
            }
            let mut stmt = c.prepare_cached(
                "SELECT m.file_id, g.keeper_file_id, g.member_count FROM dup_member m
                 JOIN dup_group g ON g.id = m.group_id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    FileId(r.get(0)?),
                    FileId(r.get(1)?),
                    r.get::<_, i64>(2)? as u32,
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Exact-duplicate sets among present files: files sharing a content identity (ADR-0011), each set
    /// ordered by id. The gallery shows each set as one stack until duplicate groups are stored.
    ///
    /// The inner scan is index-only over `file_by_content` (~0.2 s for 1M files); filtering missing files
    /// there would cost a table lookup per row (~10 s cold), so presence is checked on the few candidates.
    pub fn exact_duplicates(&self) -> Result<Vec<Vec<FileId>>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, content_id FROM file
                 WHERE kflags < 16 AND content_id IN (
                     SELECT content_id FROM file INDEXED BY file_by_content
                     WHERE content_id IS NOT NULL
                     GROUP BY content_id HAVING count(*) > 1)
                 ORDER BY content_id, id",
            )?;
            let mut rows = stmt.query([])?;
            let mut out: Vec<Vec<FileId>> = Vec::new();
            let mut last: Option<Vec<u8>> = None;
            while let Some(r) = rows.next()? {
                let id = FileId(r.get(0)?);
                let digest: Vec<u8> = r.get(1)?;
                if last.as_deref() == Some(&digest[..]) {
                    if let Some(set) = out.last_mut() {
                        set.push(id);
                    }
                } else {
                    out.push(vec![id]);
                    last = Some(digest);
                }
            }
            out.retain(|set| set.len() > 1);
            Ok(out)
        })
    }
}

fn decision_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<UserDecisionRecord>> {
    let decision: String = r.get(1)?;
    Ok(match UserDecision::parse(&decision) {
        Some(d) => Ok(UserDecisionRecord {
            file: FileId(r.get(0)?),
            decision: d,
            reason: r.get(2)?,
            at_ms: r.get(3)?,
        }),
        None => Err(CatalogError::Corrupt(format!("user decision {decision}"))),
    })
}

/// The class filter with user decisions applied, and the gallery's extra predicates, as `AND ...` SQL with
/// bound values. `base` already holds the conditions of every other filter field.
pub(crate) fn extend_filter_sql(
    c: &Connection,
    f: &Filter,
    sql: &mut String,
    args: &mut Vec<Value>,
) -> Result<()> {
    if let Some(classes) = &f.classes {
        let codes = |sql: &mut String, args: &mut Vec<Value>| {
            sql.push_str("(kflags & 7) IN (");
            for (i, cl) in classes.iter().enumerate() {
                sql.push_str(if i == 0 { "?" } else { ",?" });
                args.push(Value::Integer(i64::from(cl.code())));
            }
            sql.push(')');
        };
        let decisions = table_exists(c, "user_inclusion")?;
        if classes.is_empty() {
            sql.push_str(" AND 0");
        } else if !decisions {
            sql.push_str(" AND ");
            codes(sql, args);
        } else {
            // An included file counts as a photo, an excluded one as set aside.
            let mut wanted = Vec::new();
            if classes.iter().any(|c| is_library(*c)) {
                wanted.push(UserDecision::Include);
            }
            if classes.iter().any(|c| !is_library(*c)) {
                wanted.push(UserDecision::Exclude);
            }
            sql.push_str(" AND ((");
            codes(sql, args);
            sql.push_str(" AND id NOT IN (SELECT file_id FROM user_inclusion))");
            if !wanted.is_empty() {
                sql.push_str(" OR id IN (SELECT file_id FROM user_inclusion WHERE decision IN (");
                for (i, d) in wanted.iter().enumerate() {
                    sql.push_str(if i == 0 { "?" } else { ",?" });
                    args.push(Value::Text(d.as_str().to_owned()));
                }
                sql.push_str("))");
            }
            sql.push(')');
        }
    }
    if let Some(model) = &f.camera_model {
        sql.push_str(" AND camera_model = ?");
        args.push(Value::Text(model.clone()));
    }
    if let Some(gps) = f.has_gps {
        sql.push_str(" AND has_gps = ?");
        args.push(Value::Integer(i64::from(gps)));
    }
    if let Some(folder) = f
        .folder
        .as_deref()
        .map(|d| d.trim_matches('/'))
        .filter(|d| !d.is_empty())
    {
        // Paths under `folder/`: byte range [folder + '/', folder + '0') ('0' follows '/').
        let mut lo = folder.as_bytes().to_vec();
        lo.push(b'/');
        let mut hi = folder.as_bytes().to_vec();
        hi.push(b'0');
        sql.push_str(" AND rel_path >= ? AND rel_path < ?");
        args.push(Value::Blob(lo));
        args.push(Value::Blob(hi));
    }
    if let Some((start, end)) = f.captured {
        sql.push_str(" AND capture_ts >= ? AND capture_ts < ?");
        args.push(Value::Integer(start));
        args.push(Value::Integer(end));
    }
    Ok(())
}

/// Classes shown in the Library (the rest are set aside in the Excluded view).
pub fn is_library(c: PhotoClass) -> bool {
    matches!(c, PhotoClass::Photo | PhotoClass::LikelyPhoto)
}

/// `PhotoClass` helper re-exported for callers building Library/Excluded filters.
pub fn excluded_classes() -> Vec<PhotoClass> {
    vec![
        PhotoClass::ThumbnailCache,
        PhotoClass::Graphic,
        PhotoClass::Junk,
    ]
}

/// The Library's classes: photos and likely photos.
pub fn library_classes() -> Vec<PhotoClass> {
    vec![PhotoClass::Photo, PhotoClass::LikelyPhoto]
}
