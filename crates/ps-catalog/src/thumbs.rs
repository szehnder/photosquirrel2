//! `thumbs.db`: the content-addressed thumbnail store (ADR-0016).
//!
//! One row per content identity, so byte-identical files share one thumbnail, and rows never reference a
//! source path, so thumbnails stay readable while sources are unplugged. The store takes already-encoded
//! JPEG bytes (384 px, q82, from `ps-media`) and the dominant colour; it never decodes or encodes images.
//!
//! Writes go through one mutex-guarded connection, each call one transaction (WAL, `synchronous=NORMAL`):
//! when [`ThumbStore::put`] returns, the row is committed, so the scanner can then commit the `thumbed`
//! stage in the catalog (ADR-0008). Reads use a small pool of read-only connections.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};

use ps_model::{ContentId, PlaceholderInfo, ThumbKind, ThumbnailRecord};

use crate::db::{self, THUMB_MIGRATIONS};
use crate::error::{CatalogError, Result};
use crate::model::{codes, now_ms};

/// File name of the thumbnail store inside the data directory (beside the catalog).
pub const THUMBS_FILE: &str = "thumbs.db";

const READERS: usize = 2;

/// The thumbnail store.
pub struct ThumbStore {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl std::fmt::Debug for ThumbStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbStore")
            .field("path", &self.path)
            .finish()
    }
}

impl ThumbStore {
    /// Open (creating and migrating) `thumbs.db` in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<ThumbStore> {
        std::fs::create_dir_all(data_dir).map_err(db::io_err(data_dir))?;
        ThumbStore::open_file(&data_dir.join(THUMBS_FILE))
    }

    /// Open a store at an explicit path. A store with a newer schema is refused unmodified.
    pub fn open_file(path: &Path) -> Result<ThumbStore> {
        db::refuse_newer(path, THUMB_MIGRATIONS)?;
        let mut writer = db::open_writer(path)?;
        db::migrate(&mut writer, path, THUMB_MIGRATIONS, None)?;
        let readers = (0..READERS)
            .map(|_| db::open_reader(path).map(Mutex::new))
            .collect::<Result<Vec<_>>>()?;
        Ok(ThumbStore {
            path: path.to_path_buf(),
            writer: Mutex::new(writer),
            readers,
            next: AtomicUsize::new(0),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        for c in &self.readers {
            if let Some(g) = c.try_lock() {
                return f(&g);
            }
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        f(&self.readers[i].lock())
    }

    /// Store (or replace) the thumbnail of a content identity; committed when this returns.
    pub fn put(&self, id: &ContentId, thumb: &ThumbnailRecord) -> Result<()> {
        self.put_many([(id, thumb)])
    }

    /// Store several thumbnails in one transaction.
    pub fn put_many<'a>(
        &self,
        items: impl IntoIterator<Item = (&'a ContentId, &'a ThumbnailRecord)>,
    ) -> Result<()> {
        self.upsert(items, false).map(|_| ())
    }

    /// Store several thumbnails in one transaction under the replace rule of placeholders (ADR-0109): a thumbnail
    /// is written where none is stored or a placeholder is, never over stored pixels. The check and the write are
    /// one statement, so a concurrent writer (a copy of the same content on another lane) cannot slip a placeholder
    /// over pixels. Returns, per item in order, whether it was written.
    pub fn put_many_over_placeholders<'a>(
        &self,
        items: impl IntoIterator<Item = (&'a ContentId, &'a ThumbnailRecord)>,
    ) -> Result<Vec<bool>> {
        self.upsert(items, true)
    }

    fn upsert<'a>(
        &self,
        items: impl IntoIterator<Item = (&'a ContentId, &'a ThumbnailRecord)>,
        over_placeholders_only: bool,
    ) -> Result<Vec<bool>> {
        let items: Vec<_> = items.into_iter().collect();
        for (_, t) in &items {
            validate(t)?;
        }
        let mut written = Vec::with_capacity(items.len());
        let mut conn = self.writer.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare_cached(if over_placeholders_only {
                "INSERT INTO thumb (content_id, identity_kind, kind, width, height, dominant_rgb, jpeg,
                                    placeholder, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (content_id) DO UPDATE SET
                    identity_kind = excluded.identity_kind, kind = excluded.kind, width = excluded.width,
                    height = excluded.height, dominant_rgb = excluded.dominant_rgb, jpeg = excluded.jpeg,
                    placeholder = excluded.placeholder, created_at = excluded.created_at
                 WHERE thumb.kind = 'placeholder'"
            } else {
                "INSERT INTO thumb (content_id, identity_kind, kind, width, height, dominant_rgb, jpeg,
                                    placeholder, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (content_id) DO UPDATE SET
                    identity_kind = excluded.identity_kind, kind = excluded.kind, width = excluded.width,
                    height = excluded.height, dominant_rgb = excluded.dominant_rgb, jpeg = excluded.jpeg,
                    placeholder = excluded.placeholder, created_at = excluded.created_at"
            })?;
            let now = now_ms();
            for (id, t) in &items {
                let placeholder = t
                    .placeholder
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| CatalogError::Invalid(e.to_string()))?;
                let jpeg: Option<&[u8]> = (!t.jpeg.is_empty()).then_some(&t.jpeg[..]);
                let n = stmt.execute(params![
                    &id.digest[..],
                    codes::identity_kind(id.kind),
                    t.kind.as_str(),
                    t.width,
                    t.height,
                    rgb_to_int(t.dominant_rgb),
                    jpeg,
                    placeholder,
                    now
                ])?;
                written.push(n > 0);
            }
        }
        tx.commit()?;
        Ok(written)
    }

    /// A thumbnail is stored for this identity (duplicates need no second encode).
    pub fn contains(&self, id: &ContentId) -> Result<bool> {
        self.read(|c| {
            Ok(
                c.prepare_cached("SELECT 1 FROM thumb WHERE content_id = ?1")?
                    .query_row([&id.digest[..]], |_| Ok(()))
                    .optional()?
                    .is_some(),
            )
        })
    }

    /// The kind of the stored thumbnail of a content identity, without reading its blob (`None`: none stored).
    pub fn stored_kind(&self, id: &ContentId) -> Result<Option<ThumbKind>> {
        self.read(|c| {
            let kind: Option<String> = c
                .prepare_cached("SELECT kind FROM thumb WHERE content_id = ?1")?
                .query_row([&id.digest[..]], |r| r.get(0))
                .optional()?;
            Ok(kind.as_deref().and_then(ThumbKind::parse))
        })
    }

    /// The stored thumbnail of a content identity.
    pub fn get(&self, id: &ContentId) -> Result<Option<ThumbnailRecord>> {
        self.read(|c| {
            let row = c
                .prepare_cached(
                    "SELECT kind, width, height, dominant_rgb, jpeg, placeholder FROM thumb
                     WHERE content_id = ?1",
                )?
                .query_row([&id.digest[..]], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u32>(1)?,
                        r.get::<_, u32>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Option<Vec<u8>>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })
                .optional()?;
            let Some((kind, width, height, rgb, jpeg, placeholder)) = row else {
                return Ok(None);
            };
            let kind = ThumbKind::parse(&kind)
                .ok_or_else(|| CatalogError::Corrupt(format!("thumb kind {kind}")))?;
            let placeholder: Option<PlaceholderInfo> = placeholder
                .map(|p| serde_json::from_str(&p))
                .transpose()
                .map_err(|e| CatalogError::Corrupt(e.to_string()))?;
            Ok(Some(ThumbnailRecord {
                kind,
                width,
                height,
                dominant_rgb: int_to_rgb(rgb),
                jpeg: jpeg.unwrap_or_default(),
                placeholder,
            }))
        })
    }

    /// Dominant colours for a batch of identities (grid placeholders before textures load, ADR-0018), in
    /// input order; `None` where nothing is stored.
    pub fn dominant_colours(&self, ids: &[ContentId]) -> Result<Vec<Option<[u8; 3]>>> {
        let keys: Rc<Vec<Value>> =
            Rc::new(ids.iter().map(|i| Value::Blob(i.digest.to_vec())).collect());
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT content_id, dominant_rgb FROM thumb WHERE content_id IN rarray(?1)",
            )?;
            let mut found = std::collections::HashMap::with_capacity(ids.len());
            let mut rows = stmt.query([keys])?;
            while let Some(r) = rows.next()? {
                let d: Vec<u8> = r.get(0)?;
                found.insert(d, int_to_rgb(r.get(1)?));
            }
            Ok(ids
                .iter()
                .map(|i| found.get(&i.digest[..]).copied())
                .collect())
        })
    }

    /// Number of stored thumbnails.
    pub fn count(&self) -> Result<u64> {
        self.read(|c| {
            Ok(c.query_row("SELECT count(*) FROM thumb", [], |r| r.get::<_, i64>(0))? as u64)
        })
    }
}

fn validate(t: &ThumbnailRecord) -> Result<()> {
    match t.kind {
        ThumbKind::Image | ThumbKind::PosterFrame => {
            if !t.jpeg.starts_with(&[0xFF, 0xD8, 0xFF]) {
                return Err(CatalogError::Invalid(
                    "thumbnail bytes are not a JPEG (missing SOI marker)".into(),
                ));
            }
        }
        ThumbKind::Placeholder => {
            if !t.jpeg.is_empty() {
                return Err(CatalogError::Invalid(
                    "placeholder thumbnails carry no pixels".into(),
                ));
            }
        }
    }
    Ok(())
}

fn rgb_to_int(c: [u8; 3]) -> i64 {
    (i64::from(c[0]) << 16) | (i64::from(c[1]) << 8) | i64::from(c[2])
}

fn int_to_rgb(v: i64) -> [u8; 3] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}
