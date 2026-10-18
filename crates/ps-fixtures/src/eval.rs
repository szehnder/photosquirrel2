//! Scores a grouping result against the truth (ADR-0025 gate): pairwise precision and recall over files the
//! classifier fully analyzes (expected class not `junk`/`graphic`), keeper accuracy over truth groups with at
//! least two such members, and must-not-merge violations.

use std::collections::{BTreeMap, BTreeSet};

use ps_model::PhotoClass;
use serde::{Deserialize, Serialize};

use crate::truth::{MustNotMergeKind, Truth};

/// One group of a grouping result, by corpus-relative path (`/`-separated). Files absent from every result
/// group are treated as singletons.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultGroup {
    pub members: Vec<String>,
    pub keeper: Option<String>,
}

/// A result group that joined shots from one must-not-merge set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub set_id: String,
    pub kind: MustNotMergeKind,
    pub result_group: usize,
    pub truth_groups: Vec<String>,
}

/// ADR-0025 gate thresholds.
pub const GATE_PRECISION: f64 = 0.995;
pub const GATE_RECALL: f64 = 0.97;
pub const GATE_KEEPER_ACCURACY: f64 = 0.99;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub precision: f64,
    pub recall: f64,
    pub keeper_accuracy: f64,
    pub must_not_merge_violations: usize,
    pub true_pairs: u64,
    pub result_pairs: u64,
    pub correct_pairs: u64,
    pub keeper_groups: usize,
    pub keepers_correct: usize,
    /// Result paths not present in the truth (ignored for scoring).
    pub unknown_paths: usize,
    pub violations: Vec<Violation>,
}

fn pairs(n: u64) -> u64 {
    n * n.saturating_sub(1) / 2
}

impl Score {
    /// Score `results` against `truth`.
    pub fn evaluate(truth: &Truth, results: &[ResultGroup]) -> Score {
        let index: BTreeMap<&str, usize> = truth
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.as_str(), i))
            .collect();
        let eligible = |i: usize| {
            !matches!(
                truth.files[i].expected_class,
                PhotoClass::Junk | PhotoClass::Graphic
            )
        };

        // Resolve result groups to file indices (each file in at most one group: first wins).
        let mut unknown = 0;
        let mut assigned: BTreeMap<usize, usize> = BTreeMap::new();
        let mut rgroups: Vec<Vec<usize>> = Vec::with_capacity(results.len());
        for (ri, r) in results.iter().enumerate() {
            let mut m = Vec::new();
            for p in &r.members {
                match index.get(p.as_str()) {
                    Some(&i) => {
                        if let std::collections::btree_map::Entry::Vacant(e) = assigned.entry(i) {
                            e.insert(ri);
                            m.push(i);
                        }
                    }
                    None => unknown += 1,
                }
            }
            rgroups.push(m);
        }

        // Pairwise precision / recall.
        let tgroups = truth.groups();
        let true_pairs: u64 = tgroups
            .values()
            .map(|m| pairs(m.iter().filter(|&&i| eligible(i)).count() as u64))
            .sum();
        let mut result_pairs = 0u64;
        let mut correct = 0u64;
        for m in &rgroups {
            let el: Vec<usize> = m.iter().copied().filter(|&i| eligible(i)).collect();
            result_pairs += pairs(el.len() as u64);
            let mut by_truth: BTreeMap<&str, u64> = BTreeMap::new();
            for i in el {
                *by_truth.entry(truth.files[i].group.as_str()).or_default() += 1;
            }
            correct += by_truth.values().map(|&n| pairs(n)).sum::<u64>();
        }
        let precision = if result_pairs == 0 {
            1.0
        } else {
            correct as f64 / result_pairs as f64
        };
        let recall = if true_pairs == 0 {
            1.0
        } else {
            correct as f64 / true_pairs as f64
        };

        // Keeper accuracy: accepted keepers are the expected keeper / keeper set and byte-identical copies.
        let mut keeper_groups = 0;
        let mut keepers_correct = 0;
        for members in tgroups.values() {
            let el: Vec<usize> = members.iter().copied().filter(|&i| eligible(i)).collect();
            if el.len() < 2 {
                continue;
            }
            let accepted: BTreeSet<&str> = members
                .iter()
                .filter(|&&i| truth.files[i].keeper || truth.files[i].keeper_set)
                .map(|&i| truth.files[i].content.as_str())
                .collect();
            if accepted.is_empty() {
                continue;
            }
            keeper_groups += 1;
            // The result group holding most of this truth group's members.
            let mut votes: BTreeMap<usize, usize> = BTreeMap::new();
            for &i in &el {
                if let Some(&rg) = assigned.get(&i) {
                    *votes.entry(rg).or_default() += 1;
                }
            }
            let chosen = votes
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(&rg, _)| rg);
            let ok = match chosen {
                Some(rg) => results[rg]
                    .keeper
                    .as_deref()
                    .and_then(|k| index.get(k))
                    .is_some_and(|&k| accepted.contains(truth.files[k].content.as_str())),
                // Nothing grouped: the expected keeper is its own singleton keeper.
                None => true,
            };
            if ok {
                keepers_correct += 1;
            }
        }
        let keeper_accuracy = if keeper_groups == 0 {
            1.0
        } else {
            keepers_correct as f64 / keeper_groups as f64
        };

        // Must-not-merge violations: one per (set, result group) joining two or more of the set's groups.
        let mut violations = Vec::new();
        for set in &truth.must_not_merge {
            let set_groups: BTreeSet<&str> = set.groups.iter().map(String::as_str).collect();
            for (ri, m) in rgroups.iter().enumerate() {
                let hit: BTreeSet<&str> = m
                    .iter()
                    .map(|&i| truth.files[i].group.as_str())
                    .filter(|g| set_groups.contains(g))
                    .collect();
                if hit.len() >= 2 {
                    violations.push(Violation {
                        set_id: set.id.clone(),
                        kind: set.kind,
                        result_group: ri,
                        truth_groups: hit.into_iter().map(str::to_string).collect(),
                    });
                }
            }
        }

        Score {
            precision,
            recall,
            keeper_accuracy,
            must_not_merge_violations: violations.len(),
            true_pairs,
            result_pairs,
            correct_pairs: correct,
            keeper_groups,
            keepers_correct,
            unknown_paths: unknown,
            violations,
        }
    }

    /// The ADR-0025 gate.
    pub fn passes_gate(&self) -> bool {
        self.precision >= GATE_PRECISION
            && self.recall >= GATE_RECALL
            && self.keeper_accuracy >= GATE_KEEPER_ACCURACY
            && self.must_not_merge_violations == 0
    }

    /// `measure ...` lines for the record (ADR-0026 style).
    pub fn measure_lines(&self, prefix: &str) -> String {
        format!(
            "measure {prefix}-precision={:.4}\nmeasure {prefix}-recall={:.4}\nmeasure {prefix}-keeper-accuracy={:.4}\nmeasure {prefix}-must-not-merge-violations={}",
            self.precision, self.recall, self.keeper_accuracy, self.must_not_merge_violations
        )
    }
}

/// The truth's own grouping (every truth group with its expected keeper), e.g. as a perfect baseline.
pub fn truth_result(truth: &Truth) -> Vec<ResultGroup> {
    truth
        .groups()
        .into_values()
        .map(|members| ResultGroup {
            keeper: members
                .iter()
                .find(|&&i| truth.files[i].keeper)
                .map(|&i| truth.files[i].path.clone()),
            members: members
                .iter()
                .map(|&i| truth.files[i].path.clone())
                .collect(),
        })
        .collect()
}
