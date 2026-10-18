//! Corpus harness: generate a ground-truth corpus, analyze every file the way the scanner will (ps-media probe,
//! classify, metadata, analysis decode; ps-hash identity and perceptual hash) into [`DedupInput`]s, and score
//! groupings with `ps_fixtures::eval`.
#![allow(dead_code, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ps_dedup::{DedupInput, DedupMeta, Grouping};
use ps_fixtures::eval::{ResultGroup, Score};
use ps_fixtures::{DirWriter, Preset, Truth};
use ps_hash::{ImageView, PixelLayout, identity_from_buffer, perceptual_hash};
use ps_model::{ClassifierSettings, FileId, MediaKind};
use rayon::prelude::*;

pub struct Corpus {
    _dir: Option<tempfile::TempDir>,
    pub root: PathBuf,
    pub truth: Truth,
    pub inputs: Vec<DedupInput>,
}

impl Corpus {
    pub fn path_of(&self, f: FileId) -> &str {
        &self.inputs[f.0 as usize - 1].path
    }

    pub fn input(&self, path: &str) -> &DedupInput {
        self.inputs.iter().find(|x| x.path == path).unwrap()
    }

    pub fn results(&self, g: &Grouping) -> Vec<ResultGroup> {
        g.groups
            .iter()
            .map(|grp| ResultGroup {
                members: grp
                    .members
                    .iter()
                    .map(|m| self.path_of(m.file).to_string())
                    .collect(),
                keeper: Some(self.path_of(grp.keeper).to_string()),
            })
            .collect()
    }

    pub fn score(&self, g: &Grouping) -> Score {
        Score::evaluate(&self.truth, &self.results(g))
    }
}

/// Analyze one corpus file into a grouping input. `rel` is corpus-relative with `/` separators.
pub fn analyze(root: &Path, rel: &str, id: FileId) -> DedupInput {
    let abs = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    let bytes = std::fs::read(&abs).unwrap();
    let mtime = std::fs::metadata(&abs).unwrap().modified().ok();
    let probe = ps_media::probe_bytes(&bytes, bytes.len() as u64, Some(&abs));
    let (class, _) = ps_media::classify(&probe, Path::new(rel), &ClassifierSettings::default());
    let meta = ps_media::extract_metadata(&bytes, &probe, mtime);
    let format = probe.format.unwrap_or(ps_model::Format::Jpeg);
    // Junk and graphics are not fully read or analyzed (ADR-0015).
    let analyzed = class.is_candidate() && probe.format.is_some();
    let content = analyzed.then(|| identity_from_buffer(&bytes));
    let hash = (analyzed && format.kind() == MediaKind::Image)
        .then(|| ps_media::decode_for_analysis(&bytes, &probe).ok())
        .flatten()
        .and_then(|d| {
            let img = d.image;
            ImageView::new(img.width, img.height, PixelLayout::Rgb8, &img.data)
                .ok()
                .map(|v| perceptual_hash(&v))
        });
    let mtime_ns = mtime
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64);
    DedupInput {
        file: id,
        content,
        hash,
        format,
        class,
        path: rel.to_string(),
        size: bytes.len() as u64,
        mtime_ns,
        meta: DedupMeta::from_media(&meta.fields, meta.likely_camera_original),
    }
}

/// Generate `preset` with `seed` into `root` (must be absent or empty) and analyze it. File ids are 1-based
/// positions in the truth's (path-sorted) file list.
pub fn load(root: PathBuf, seed: u64, preset: Preset, dir: Option<tempfile::TempDir>) -> Corpus {
    let truth = if root.join(ps_fixtures::TRUTH_FILE).is_file() {
        Truth::read(&root).unwrap()
    } else {
        let w = DirWriter::new(&root).unwrap();
        ps_fixtures::generate(seed, preset, &w).unwrap()
    };
    let inputs: Vec<DedupInput> = truth
        .files
        .par_iter()
        .enumerate()
        .map(|(i, f)| analyze(&root, &f.path, FileId(i as i64 + 1)))
        .collect();
    Corpus {
        _dir: dir,
        root,
        truth,
        inputs,
    }
}

/// The small corpus, seed 42, generated and analyzed once per test binary.
pub fn small() -> &'static Corpus {
    static C: OnceLock<Corpus> = OnceLock::new();
    C.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        load(root, 42, Preset::Small, Some(dir))
    })
}

pub fn print_score(label: &str, s: &Score) {
    println!(
        "{label}: precision={:.4} recall={:.4} keeper={:.4} ({}/{}) must-not-merge={} pairs: true={} result={} correct={}",
        s.precision,
        s.recall,
        s.keeper_accuracy,
        s.keepers_correct,
        s.keeper_groups,
        s.must_not_merge_violations,
        s.true_pairs,
        s.result_pairs,
        s.correct_pairs
    );
}
