//! Duplicate grouping and explainable best-copy ranking (ADR-0013, ADR-0014, ADR-0033, ADR-0035).
//!
//! Pure and deterministic: no I/O, no threads. The scanner/catalog builds one [`DedupInput`] per analyzed file
//! and calls [`group`] (or [`Grouper::regroup`] for new/changed files); the result lists each group's members
//! with the reason they joined, the keeper (or RAW+JPEG keeper set) with human-readable reasons, and
//! similar-shot suggestions that are never collapsed.
//!
//! - [`edges`]: exact, sampled and derivative rules, the metadata contradiction test, pair explanations.
//! - [`group`]: union-find with separation constraints, the 60 s chain guard, user merges, incremental regroup.
//! - [`rank`]: the lexicographic best-copy order with upscale detection and keeper reasons.
//! - [`keeper_set`]: RAW+JPEG pairing.

pub mod edges;
pub mod group;
pub mod input;
pub mod keeper_set;
pub mod rank;
pub mod unionfind;

pub use edges::{
    Contradiction, EdgeKind, NoMatch, PairExplanation, PairVerdict, Suggestion, explain_pair,
    pair_verdict,
};
pub use group::{Constraints, Group, Grouper, Grouping, Member, MemberReason, group};
pub use input::{DedupInput, DedupMeta};
pub use rank::{FormatTier, KeeperReason, Ranking, rank};
