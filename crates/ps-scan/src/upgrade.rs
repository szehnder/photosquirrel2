//! Perceptual-hash upgrades without the sources (ADR-0012, ADR-0016): when the hash algorithm version
//! changes, every thumbed image's hashes are recomputed from its stored thumbnail in `thumbs.db`, so offline
//! sources are upgraded too. No source file is opened.

use ps_catalog::{Catalog, Command, ThumbStore};
use ps_hash::{ImageView, PixelLayout};
use ps_model::{PerceptualHash, ThumbKind};

use crate::error::Result;

/// What an upgrade did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HashUpgradeReport {
    /// Files whose hashes were of another version.
    pub stale: u64,
    pub upgraded: u64,
    /// Files without a decodable stored thumbnail (placeholders, missing rows).
    pub skipped: u64,
}

/// Recompute hashes of every thumbed image whose stored version differs from `version`, using `hasher`
/// on the decoded thumbnail. [`upgrade_hashes_current`] uses the current `ps-hash` algorithm.
pub fn upgrade_hashes(
    catalog: &Catalog,
    thumbs: &ThumbStore,
    version: u8,
    hasher: impl Fn(&ImageView<'_>) -> PerceptualHash,
) -> Result<HashUpgradeReport> {
    let stale = catalog.read().stale_hashes(version)?;
    let mut report = HashUpgradeReport {
        stale: stale.len() as u64,
        ..HashUpgradeReport::default()
    };
    let mut batch = Vec::new();
    let mut cache: Option<(ps_model::ContentId, Option<PerceptualHash>)> = None;
    for (id, content) in stale {
        let h = match &cache {
            Some((c, h)) if *c == content => *h,
            _ => {
                let h = hash_thumbnail(thumbs, &content, &hasher)?;
                cache = Some((content, h));
                h
            }
        };
        match h {
            Some(h) => {
                batch.push((id, h));
                report.upgraded += 1;
            }
            None => report.skipped += 1,
        }
        if batch.len() >= 2_000 {
            catalog
                .writer()
                .submit(Command::SetHashes(std::mem::take(&mut batch)))?;
        }
    }
    if !batch.is_empty() {
        catalog.writer().submit(Command::SetHashes(batch))?;
    }
    catalog.writer().flush()?;
    Ok(report)
}

/// [`upgrade_hashes`] to [`ps_hash::PERCEPTUAL_HASH_VERSION`].
pub fn upgrade_hashes_current(catalog: &Catalog, thumbs: &ThumbStore) -> Result<HashUpgradeReport> {
    upgrade_hashes(
        catalog,
        thumbs,
        ps_hash::PERCEPTUAL_HASH_VERSION,
        ps_hash::perceptual_hash,
    )
}

fn hash_thumbnail(
    thumbs: &ThumbStore,
    content: &ps_model::ContentId,
    hasher: &impl Fn(&ImageView<'_>) -> PerceptualHash,
) -> Result<Option<PerceptualHash>> {
    let Some(t) = thumbs.get(content)? else {
        return Ok(None);
    };
    if t.kind == ThumbKind::Placeholder || t.jpeg.is_empty() {
        return Ok(None);
    }
    // Thumbnails are stored oriented (ADR-0016), so the view is upright.
    let Ok((img, _)) = ps_media::decode::decode_jpeg(&t.jpeg, 0) else {
        return Ok(None);
    };
    Ok(
        ImageView::new(img.width, img.height, PixelLayout::Rgb8, &img.data)
            .ok()
            .map(|v| hasher(&v)),
    )
}
