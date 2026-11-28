//! Shot-quality scores from stored thumbnails (best-of-stacks, ADR-0135 decision 5).
//!
//! The scores are computed from each photo's stored 384 px thumbnail, never from the source. The same pass runs at
//! the end of every completed scan (for the files it analysed) and in the background for catalogs scored before
//! (or by an older version of the scores). A file whose thumbnail cannot be decoded gets no score and ranks after
//! scored shots.

use ps_catalog::{Catalog, ThumbStore};
use ps_model::{FileId, ShotQuality};

/// Files scored per catalog write.
pub const BATCH: u32 = 1000;

/// What a fill did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillReport {
    pub scored: u64,
    /// Thumbnails that were missing or could not be decoded (recorded as tried, not tried again).
    pub skipped: u64,
    /// `stop` asked to end it before every file was scored.
    pub stopped: bool,
}

/// The score of a stored thumbnail's JPEG bytes.
pub fn score_thumbnail(jpeg: &[u8]) -> Option<ShotQuality> {
    if jpeg.is_empty() {
        return None;
    }
    // Full size: the smallest DCT scale (`min_long` 0) would be a 48 px image with no detail left.
    let (img, _) = ps_media::decode::decode_jpeg(jpeg, u32::MAX).ok()?;
    ps_media::shot_quality::shot_quality(&img)
}

/// Score every unscored file with a stored thumbnail, in batches of [`BATCH`]. `stop` is checked between files.
pub fn fill(
    catalog: &Catalog,
    thumbs: &ThumbStore,
    stop: &dyn Fn() -> bool,
) -> ps_catalog::Result<FillReport> {
    let mut report = FillReport::default();
    let mut after = FileId(0);
    loop {
        let page = catalog.read().unscored_shots(after, BATCH)?;
        let Some(&(last, _)) = page.last() else {
            break;
        };
        let mut rows = Vec::with_capacity(page.len());
        for (f, content) in page {
            if stop() {
                report.stopped = true;
                break;
            }
            let q = thumbs.get(&content)?.and_then(|t| score_thumbnail(&t.jpeg));
            if q.is_some() {
                report.scored += 1;
            } else {
                report.skipped += 1;
            }
            rows.push((f, q));
        }
        if !rows.is_empty() {
            catalog.writer().set_shot_quality(rows)?;
        }
        if report.stopped {
            break;
        }
        after = last;
    }
    Ok(report)
}

/// A scan run is recorded as running in the catalog (any process).
fn scan_running(catalog: &Catalog) -> bool {
    catalog
        .read()
        .with(|c| {
            Ok(
                c.prepare_cached("SELECT 1 FROM scan_run WHERE status = 'running' LIMIT 1")?
                    .exists([])?,
            )
        })
        .unwrap_or(false)
}

/// Fill in the scores of a catalog scored before (or by an older version) on a background thread: after `delay`,
/// so opening the app is not slowed; paused while a scan runs (the scan scores its own files at its end); ended
/// when `shutdown` is set. Reads the thumbnail store only.
pub fn spawn_background(
    catalog: std::sync::Arc<Catalog>,
    thumbs: std::sync::Arc<ThumbStore>,
    delay: std::time::Duration,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    use std::sync::atomic::Ordering;
    let down = move || shutdown.load(Ordering::Acquire);
    std::thread::Builder::new()
        .name("ps-shot-quality".into())
        .spawn(move || {
            let step = std::time::Duration::from_millis(50);
            let mut waited = std::time::Duration::ZERO;
            while waited < delay {
                if down() {
                    return;
                }
                std::thread::sleep(step);
                waited += step;
            }
            loop {
                if down() {
                    return;
                }
                if scan_running(&catalog) {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
                match fill(&catalog, &thumbs, &|| down() || scan_running(&catalog)) {
                    Ok(r) if !r.stopped => {
                        if r.scored + r.skipped > 0 {
                            log::info!(
                                "shot quality: {} scored, {} without a usable thumbnail",
                                r.scored,
                                r.skipped
                            );
                        }
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("shot quality: {e}");
                        return;
                    }
                }
            }
        })
        .ok()
}
