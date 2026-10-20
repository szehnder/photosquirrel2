//! Bundles of units exported together with one stem: RAW+JPEG keeper sets (one unit with two files) and
//! Live Photo still/video pairs (two units).
//!
//! Live Photo detection (export design): a still (HEIC/JPEG) and a video (MOV) in the same source folder with
//! the same stem (case-insensitive) whose Apple ContentIdentifiers match; when either lacks one, the same
//! stem with capture times within 2 s.

use std::collections::BTreeMap;

use ps_model::{Format, MediaKind};

use crate::input::{ExportInput, ExportUnit, KeeperFile};
use crate::layout::split_name;

fn single(u: &ExportUnit) -> Option<&KeeperFile> {
    (u.files.len() == 1).then(|| &u.files[0])
}

/// Capture-time tolerance for Live Photo pairs without ContentIdentifiers.
pub const LIVE_PAIR_MAX_DIFF_MS: i64 = 2_000;

fn is_live_still(f: &KeeperFile) -> bool {
    matches!(f.format, Some(Format::Heic | Format::Jpeg))
}

fn is_live_video(f: &KeeperFile) -> bool {
    matches!(f.format, Some(Format::Mov))
}

/// A still and a video form a Live Photo pair.
pub fn is_live_pair(still: &KeeperFile, video: &KeeperFile) -> bool {
    if !is_live_still(still) || !is_live_video(video) || still.source != video.source {
        return false;
    }
    if still.rel_path.parent() != video.rel_path.parent() {
        return false;
    }
    let (a, _) = split_name(still.name());
    let (b, _) = split_name(video.name());
    if !a.eq_ignore_ascii_case(b) {
        return false;
    }
    match (&still.content_identifier, &video.content_identifier) {
        (Some(x), Some(y)) => x == y,
        _ => match (&still.capture, &video.capture) {
            (Some(x), Some(y)) => x.diff_millis(y) <= LIVE_PAIR_MAX_DIFF_MS,
            _ => false,
        },
    }
}

/// Group unit indices into bundles: each bundle is one unit, or a Live Photo still unit followed by its video
/// unit when `live_pairs` is on. Every unit appears in exactly one bundle.
pub fn bundles(input: &ExportInput, live_pairs: bool) -> Vec<Vec<usize>> {
    let mut paired_video = vec![false; input.units.len()];
    let mut partner: Vec<Option<usize>> = vec![None; input.units.len()];
    if live_pairs {
        // Index videos by (source, folder, lower-case stem).
        let mut videos: BTreeMap<(i64, Vec<u8>, Vec<u8>), Vec<usize>> = BTreeMap::new();
        for (i, u) in input.units.iter().enumerate() {
            if let Some(f) = single(u)
                && f.kind() == Some(MediaKind::Video)
                && is_live_video(f)
            {
                videos.entry(key(f)).or_default().push(i);
            }
        }
        for (i, u) in input.units.iter().enumerate() {
            let Some(still) = single(u).filter(|f| is_live_still(f)) else {
                continue;
            };
            let Some(cands) = videos.get(&key(still)) else {
                continue;
            };
            if let Some(&v) = cands
                .iter()
                .find(|&&v| !paired_video[v] && is_live_pair(still, &input.units[v].files[0]))
            {
                paired_video[v] = true;
                partner[i] = Some(v);
            }
        }
    }
    let mut out = Vec::with_capacity(input.units.len());
    for i in 0..input.units.len() {
        if paired_video[i] {
            continue;
        }
        match partner[i] {
            Some(v) => out.push(vec![i, v]),
            None => out.push(vec![i]),
        }
    }
    out
}

fn key(f: &KeeperFile) -> (i64, Vec<u8>, Vec<u8>) {
    let folder = f
        .rel_path
        .parent()
        .map(|p| p.into_bytes())
        .unwrap_or_default();
    // Pairing is about the source files: their own names, not a cleaner library name.
    let (stem, _) = split_name(f.rel_path.file_name());
    (f.source.0, folder, stem.to_ascii_lowercase())
}
