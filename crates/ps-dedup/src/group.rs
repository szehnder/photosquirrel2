//! Grouping (ADR-0013, ADR-0033, ADR-0035): exact and derivative edges, union-find with cannot-link
//! separation constraints, the 60 s chain-merge guard, user merges, per-member explanations, similar-shot
//! suggestions, and incremental regrouping that equals a full regroup.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

use ps_hash::{NearDupIndex, PERCEPTUAL_HASH_VERSION, Thresholds};
use ps_model::{ContentId, FileId};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::edges::{EdgeKind, PairVerdict, Suggestion, derivative_eligible, derivative_verdict};
use crate::input::{DedupInput, ExactKey};
use crate::rank::{KeeperReason, rank};
use crate::unionfind::UnionFind;

/// Largest capture-time span of one group (chain-merge guard, ADR-0013).
pub const MAX_GROUP_SPAN_MS: i64 = 60_000;

/// User constraints (ADR-0033, ADR-0014), keyed by content identity so they survive rescans and moves.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraints {
    /// "Never pair" (stored with the smaller content first).
    pub separations: BTreeSet<(ContentId, ContentId)>,
    /// "Always pair" (stored with the smaller content first). Separations win over merges.
    pub merges: BTreeSet<(ContentId, ContentId)>,
    /// Contents the user chose as keeper of their group.
    pub keeper_overrides: BTreeSet<ContentId>,
}

fn ordered(a: ContentId, b: ContentId) -> (ContentId, ContentId) {
    if a <= b { (a, b) } else { (b, a) }
}

impl Constraints {
    pub fn separate(&mut self, a: ContentId, b: ContentId) {
        if a != b {
            self.separations.insert(ordered(a, b));
        }
    }

    pub fn merge(&mut self, a: ContentId, b: ContentId) {
        if a != b {
            self.merges.insert(ordered(a, b));
        }
    }

    pub fn override_keeper(&mut self, c: ContentId) {
        self.keeper_overrides.insert(c);
    }

    pub fn separated(&self, a: ContentId, b: ContentId) -> bool {
        a != b && self.separations.contains(&ordered(a, b))
    }
}

/// Why a member is in its group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MemberReason {
    /// The keeper: the root of the explanation tree.
    Keeper,
    /// Joined through `edge` to member `via` (the strongest path from the keeper).
    Joined { edge: EdgeKind, via: FileId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub file: FileId,
    pub reason: MemberReason,
}

/// One duplicate group (two or more members).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// Sorted by file id.
    pub members: Vec<Member>,
    pub keeper: FileId,
    /// The keeper and its RAW+JPEG companion, sorted.
    pub keeper_set: Vec<FileId>,
    pub display: FileId,
    pub keeper_reasons: Vec<KeeperReason>,
    /// Members best first.
    pub ranking: Vec<FileId>,
    /// Bytes of members outside the keeper set.
    pub reclaimable_bytes: u64,
}

impl Group {
    pub fn member(&self, f: FileId) -> Option<&Member> {
        self.members
            .binary_search_by_key(&f, |m| m.file)
            .ok()
            .map(|i| &self.members[i])
    }

    pub fn contains(&self, f: FileId) -> bool {
        self.member(f).is_some()
    }
}

/// The result of grouping a set of files. Files in no group are singletons (their own keepers).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grouping {
    /// Sorted by smallest member file id.
    pub groups: Vec<Group>,
    /// Similar-shot suggestions, sorted by (file_a, file_b).
    pub suggestions: Vec<Suggestion>,
    /// Separation constraints whose two contents are both present, sorted.
    pub separations: Vec<(ContentId, ContentId)>,
}

impl Grouping {
    pub fn group_of(&self, f: FileId) -> Option<&Group> {
        self.groups.iter().find(|g| g.contains(f))
    }

    /// File -> group index.
    pub fn index(&self) -> FxHashMap<FileId, usize> {
        let mut m = FxHashMap::default();
        for (gi, g) in self.groups.iter().enumerate() {
            for mem in &g.members {
                m.insert(mem.file, gi);
            }
        }
        m
    }

    pub fn suggestions_for(&self, f: FileId) -> impl Iterator<Item = &Suggestion> {
        self.suggestions
            .iter()
            .filter(move |s| s.file_a == f || s.file_b == f)
    }
}

/// Grouping engine over one complete set of inputs: candidate indexes (content identity and the MIH
/// near-duplicate index) plus constraints. Build once; group fully or incrementally.
pub struct Grouper<'a> {
    inputs: &'a [DedupInput],
    constraints: &'a Constraints,
    pos: FxHashMap<FileId, usize>,
    exact: FxHashMap<ExactKey, Vec<u32>>,
    exact_of: Vec<Option<ExactKey>>,
    by_content: FxHashMap<ContentId, Vec<u32>>,
    sep_partners: FxHashMap<ContentId, Vec<ContentId>>,
    index: NearDupIndex<u32>,
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    a: u32,
    b: u32,
    kind: EdgeKind,
}

impl<'a> Grouper<'a> {
    /// Index `inputs` (file ids must be unique).
    pub fn new(inputs: &'a [DedupInput], constraints: &'a Constraints) -> Self {
        let mut pos = FxHashMap::default();
        let mut exact: FxHashMap<ExactKey, Vec<u32>> = FxHashMap::default();
        let mut by_content: FxHashMap<ContentId, Vec<u32>> = FxHashMap::default();
        let mut exact_of = Vec::with_capacity(inputs.len());
        for (i, x) in inputs.iter().enumerate() {
            let prev = pos.insert(x.file, i);
            assert!(prev.is_none(), "duplicate file id {}", x.file);
            let k = x.exact_key();
            if let Some(k) = k {
                exact.entry(k).or_default().push(i as u32);
            }
            if let Some(c) = x.content {
                by_content.entry(c).or_default().push(i as u32);
            }
            exact_of.push(k);
        }
        let index = NearDupIndex::build(
            PERCEPTUAL_HASH_VERSION,
            inputs
                .iter()
                .enumerate()
                .filter(|(_, x)| derivative_eligible(x).is_ok())
                .filter_map(|(i, x)| Some((i as u32, x.hash?))),
        )
        .unwrap_or_else(|_| unreachable!("eligible hashes have the current version"));
        let mut sep_partners: FxHashMap<ContentId, Vec<ContentId>> = FxHashMap::default();
        for &(a, b) in &constraints.separations {
            sep_partners.entry(a).or_default().push(b);
            sep_partners.entry(b).or_default().push(a);
        }
        Self {
            inputs,
            constraints,
            pos,
            exact,
            exact_of,
            by_content,
            sep_partners,
            index,
        }
    }

    pub fn inputs(&self) -> &'a [DedupInput] {
        self.inputs
    }

    /// Group every input.
    pub fn group_all(&self) -> Grouping {
        let all: Vec<u32> = (0..self.inputs.len() as u32).collect();
        let (groups, suggestions) = self.compute(&all);
        self.finish(groups, suggestions)
    }

    /// Regroup after `changed` files were added, changed or removed (removed ones are simply absent from the
    /// inputs). `prev` is the grouping of the previous input set under the same constraints (when constraints
    /// change, pass the files of the affected contents as changed). The result equals [`Self::group_all`].
    pub fn regroup(&self, prev: &Grouping, changed: &[FileId]) -> Grouping {
        let changed: FxHashSet<FileId> = changed.iter().copied().collect();
        let prev_index = prev.index();
        let mut in_a = vec![false; self.inputs.len()];
        let mut queue: Vec<u32> = Vec::new();
        let mut touched_groups: FxHashSet<usize> = FxHashSet::default();
        let push = |i: u32, in_a: &mut Vec<bool>, q: &mut Vec<u32>| {
            if !in_a[i as usize] {
                in_a[i as usize] = true;
                q.push(i);
            }
        };
        for f in &changed {
            if let Some(&i) = self.pos.get(f) {
                push(i as u32, &mut in_a, &mut queue);
            }
            if let Some(&g) = prev_index.get(f) {
                touched_groups.insert(g);
            }
        }
        let mut expanded_groups: FxHashSet<usize> = FxHashSet::default();
        loop {
            for &g in &touched_groups {
                if expanded_groups.insert(g) {
                    for m in &prev.groups[g].members {
                        if let Some(&i) = self.pos.get(&m.file) {
                            push(i as u32, &mut in_a, &mut queue);
                        }
                    }
                }
            }
            if queue.is_empty() {
                break;
            }
            while let Some(i) = queue.pop() {
                if let Some(&g) = prev_index.get(&self.inputs[i as usize].file)
                    && !expanded_groups.contains(&g)
                {
                    touched_groups.insert(g);
                }
                for j in self.raw_links(i) {
                    push(j, &mut in_a, &mut queue);
                }
            }
        }
        let affected: Vec<u32> = (0..self.inputs.len() as u32)
            .filter(|&i| in_a[i as usize])
            .collect();
        let (mut groups, mut suggestions) = self.compute(&affected);
        let gone = |f: FileId| changed.contains(&f) || self.pos.get(&f).is_none_or(|&i| in_a[i]);
        for g in &prev.groups {
            if !g.members.iter().any(|m| gone(m.file)) {
                groups.push(g.clone());
            }
        }
        for s in &prev.suggestions {
            if !gone(s.file_a) && !gone(s.file_b) {
                suggestions.push(s.clone());
            }
        }
        self.finish(groups, suggestions)
    }

    fn finish(&self, mut groups: Vec<Group>, mut suggestions: Vec<Suggestion>) -> Grouping {
        groups.sort_by_key(|g| g.members[0].file);
        suggestions.sort_by_key(|s| (s.file_a, s.file_b));
        suggestions.dedup();
        let separations = self
            .constraints
            .separations
            .iter()
            .filter(|(a, b)| self.by_content.contains_key(a) && self.by_content.contains_key(b))
            .copied()
            .collect();
        Grouping {
            groups,
            suggestions,
            separations,
        }
    }

    /// Every file linked to `i` by any automatic edge or a user merge, ignoring guards and separations (the
    /// closure that bounds what a change can affect).
    fn raw_links(&self, i: u32) -> Vec<u32> {
        let x = &self.inputs[i as usize];
        let mut out = Vec::new();
        if let Some(k) = &self.exact_of[i as usize]
            && let Some(b) = self.exact.get(k)
        {
            out.extend(b.iter().copied().filter(|&j| j != i));
        }
        self.derivative_pairs(i, |j, v| {
            if matches!(v, PairVerdict::Edge { .. }) {
                out.push(j);
            }
        });
        if let Some(c) = x.content {
            for &(a, b) in &self.constraints.merges {
                let other = if a == c {
                    b
                } else if b == c {
                    a
                } else {
                    continue;
                };
                if let Some(fs) = self.by_content.get(&other) {
                    out.extend(fs.iter().copied());
                }
            }
        }
        out
    }

    /// Derivative verdicts of `i` with every index candidate (other than exact copies).
    fn derivative_pairs(&self, i: u32, mut f: impl FnMut(u32, PairVerdict)) {
        let x = &self.inputs[i as usize];
        if derivative_eligible(x).is_err() {
            return;
        }
        let Some(h) = x.hash else { return };
        let Ok(cands) = self.index.query(&h, Thresholds::DERIVATIVE) else {
            return;
        };
        for c in cands {
            let j = c.key;
            if j == i {
                continue;
            }
            if self.exact_of[i as usize].is_some()
                && self.exact_of[i as usize] == self.exact_of[j as usize]
            {
                continue;
            }
            let y = &self.inputs[j as usize];
            f(
                j,
                derivative_verdict(x, y, c.phash_distance, c.dhash_distance),
            );
        }
    }

    fn sep_list(&self, i: u32) -> Option<ContentId> {
        let c = self.inputs[i as usize].content?;
        self.sep_partners.contains_key(&c).then_some(c)
    }

    fn violates(&self, a: &[ContentId], b: &[ContentId]) -> bool {
        a.iter().any(|x| {
            self.sep_partners
                .get(x)
                .is_some_and(|ps| ps.iter().any(|p| b.contains(p)))
        })
    }

    /// Group the closed node set `nodes` (sorted global indices). Returns groups of >= 2 members and the
    /// suggestions with at least one endpoint in `nodes`.
    fn compute(&self, nodes: &[u32]) -> (Vec<Group>, Vec<Suggestion>) {
        let n = nodes.len();
        let mut local: FxHashMap<u32, u32> = FxHashMap::default();
        for (l, &g) in nodes.iter().enumerate() {
            local.insert(g, l as u32);
        }
        // Edges and suggestions.
        let mut edges: Vec<Edge> = Vec::new();
        let mut suggestions = Vec::new();
        for (li, &gi) in nodes.iter().enumerate() {
            let x = &self.inputs[gi as usize];
            self.derivative_pairs(gi, |gj, v| {
                let lj = local.get(&gj).copied();
                match v {
                    PairVerdict::Edge { edge } => {
                        if let Some(lj) = lj
                            && gi < gj
                        {
                            edges.push(Edge {
                                a: li as u32,
                                b: lj,
                                kind: edge,
                            });
                        }
                    }
                    PairVerdict::Suggestion {
                        phash_distance,
                        dhash_distance,
                        reason,
                    } => {
                        if lj.is_none() || gi < gj {
                            let y = &self.inputs[gj as usize];
                            let (fa, fb, reason) = if x.file < y.file {
                                (x.file, y.file, reason)
                            } else {
                                (y.file, x.file, swap_reason(reason))
                            };
                            suggestions.push(Suggestion {
                                file_a: fa,
                                file_b: fb,
                                phash_distance,
                                dhash_distance,
                                reason,
                            });
                        }
                    }
                    PairVerdict::NoMatch { .. } => {}
                }
            });
        }
        let file = |l: u32| self.inputs[nodes[l as usize] as usize].file;
        // Exact buckets (local), each sorted by file id.
        let mut buckets: FxHashMap<ExactKey, Vec<u32>> = FxHashMap::default();
        for (l, &g) in nodes.iter().enumerate() {
            if let Some(k) = self.exact_of[g as usize] {
                buckets.entry(k).or_default().push(l as u32);
            }
        }

        // Union-find: exact copies first, then derivative edges strongest first, never joining a separated
        // pair (ADR-0033 cannot-link).
        let mut uf = UnionFind::new(n);
        let mut cons: FxHashMap<usize, Vec<ContentId>> = FxHashMap::default();
        for (l, &g) in nodes.iter().enumerate() {
            if let Some(c) = self.sep_list(g) {
                cons.insert(l, vec![c]);
            }
        }
        let mut join = |uf: &mut UnionFind, a: u32, b: u32| {
            let (ra, rb) = (uf.find(a as usize), uf.find(b as usize));
            if ra == rb {
                return;
            }
            if let (Some(la), Some(lb)) = (cons.get(&ra), cons.get(&rb))
                && self.violates(la, lb)
            {
                return;
            }
            let la = cons.remove(&ra).unwrap_or_default();
            let lb = cons.remove(&rb).unwrap_or_default();
            if let Some(r) = uf.union(ra, rb) {
                let mut l = la;
                l.extend(lb);
                if !l.is_empty() {
                    cons.insert(r, l);
                }
            }
        };
        let mut bucket_list: Vec<Vec<u32>> = buckets
            .into_values()
            .filter(|b| b.len() > 1)
            .map(|mut b| {
                b.sort_by_key(|&l| file(l));
                b
            })
            .collect();
        bucket_list.sort_by_key(|b| file(b[0]));
        for b in &bucket_list {
            for &m in &b[1..] {
                join(&mut uf, b[0], m);
            }
        }
        edges.sort_by(|e, f| {
            f.kind.strength().cmp(&e.kind.strength()).then_with(|| {
                (file(e.a).min(file(e.b)), file(e.a).max(file(e.b)))
                    .cmp(&(file(f.a).min(file(f.b)), file(f.a).max(file(f.b))))
            })
        });
        for e in &edges {
            join(&mut uf, e.a, e.b);
        }

        // Adjacency for the guard and explanations (exact copies as a star around the bucket's first member).
        let mut adj: Vec<Vec<(u32, EdgeKind)>> = vec![Vec::new(); n];
        for e in &edges {
            adj[e.a as usize].push((e.b, e.kind));
            adj[e.b as usize].push((e.a, e.kind));
        }
        for b in &bucket_list {
            for &m in &b[1..] {
                adj[b[0] as usize].push((m, EdgeKind::Exact));
                adj[m as usize].push((b[0], EdgeKind::Exact));
            }
        }

        // Components, chain guard, connectivity within each part.
        let mut comps: FxHashMap<usize, Vec<u32>> = FxHashMap::default();
        for l in 0..n {
            comps.entry(uf.find(l)).or_default().push(l as u32);
        }
        let mut comp_list: Vec<Vec<u32>> = comps.into_values().collect();
        comp_list.sort_by_key(|c| c[0]);
        let mut group_of = vec![u32::MAX; n];
        let mut auto_groups: Vec<Vec<u32>> = Vec::new();
        for comp in comp_list {
            if comp.len() == 1 {
                group_of[comp[0] as usize] = auto_groups.len() as u32;
                auto_groups.push(comp);
                continue;
            }
            for part in self.chain_guard(&comp, &adj, nodes, &file) {
                for sub in connected_parts(&part, &adj) {
                    for &m in &sub {
                        group_of[m as usize] = auto_groups.len() as u32;
                    }
                    auto_groups.push(sub);
                }
            }
        }

        // User merges (ADR-0033): join whole groups unless that would pair separated contents.
        let mut guf = UnionFind::new(auto_groups.len());
        let mut gcons: Vec<Vec<ContentId>> = auto_groups
            .iter()
            .map(|g| {
                let mut v: Vec<ContentId> = g
                    .iter()
                    .filter_map(|&l| self.sep_list(nodes[l as usize]))
                    .collect();
                v.sort();
                v.dedup();
                v
            })
            .collect();
        let mut merge_links: Vec<(u32, u32)> = Vec::new();
        let mut local_by_content: FxHashMap<ContentId, u32> = FxHashMap::default();
        for l in 0..n as u32 {
            if let Some(c) = self.inputs[nodes[l as usize] as usize].content {
                local_by_content
                    .entry(c)
                    .and_modify(|e| {
                        if file(l) < file(*e) {
                            *e = l;
                        }
                    })
                    .or_insert(l);
            }
        }
        for &(ca, cb) in &self.constraints.merges {
            let (Some(&la), Some(&lb)) = (local_by_content.get(&ca), local_by_content.get(&cb))
            else {
                continue;
            };
            let (ga, gb) = (
                guf.find(group_of[la as usize] as usize),
                guf.find(group_of[lb as usize] as usize),
            );
            if ga == gb || self.violates(&gcons[ga], &gcons[gb]) {
                continue;
            }
            let (a, b) = (
                std::mem::take(&mut gcons[ga]),
                std::mem::take(&mut gcons[gb]),
            );
            if let Some(r) = guf.union(ga, gb) {
                gcons[r] = a.into_iter().chain(b).collect();
            }
            merge_links.push((la, lb));
        }
        for &(a, b) in &merge_links {
            adj[a as usize].push((b, EdgeKind::UserMerge));
            adj[b as usize].push((a, EdgeKind::UserMerge));
        }
        let mut final_groups: FxHashMap<usize, Vec<u32>> = FxHashMap::default();
        for (gi, g) in auto_groups.iter().enumerate() {
            final_groups
                .entry(guf.find(gi))
                .or_default()
                .extend(g.iter().copied());
        }

        let mut groups = Vec::new();
        for (_, mut members) in final_groups {
            if members.len() < 2 {
                continue;
            }
            members.sort_by_key(|&l| file(l));
            groups.push(self.build_group(&members, &adj, nodes));
        }
        (groups, suggestions)
    }

    /// Split a component whose capture times span more than 60 s: cut the time-sorted members at the largest
    /// gaps until every part spans <= 60 s; members without capture times follow their strongest edge.
    fn chain_guard(
        &self,
        comp: &[u32],
        adj: &[Vec<(u32, EdgeKind)>],
        nodes: &[u32],
        file: &impl Fn(u32) -> FileId,
    ) -> Vec<Vec<u32>> {
        let time = |l: u32| {
            self.inputs[nodes[l as usize] as usize]
                .meta
                .capture_time
                .map(|t| t.local_seconds * 1000 + i64::from(t.millis))
        };
        let mut timed: Vec<(i64, FileId, u32)> = comp
            .iter()
            .filter_map(|&l| time(l).map(|t| (t, file(l), l)))
            .collect();
        timed.sort();
        let span = match (timed.first(), timed.last()) {
            (Some(a), Some(b)) => b.0 - a.0,
            _ => 0,
        };
        if span <= MAX_GROUP_SPAN_MS {
            return vec![comp.to_vec()];
        }
        let mut segments: Vec<(usize, usize)> = Vec::new();
        split_segments(&timed, 0, timed.len(), &mut segments);
        segments.sort();
        let mut part: FxHashMap<u32, usize> = FxHashMap::default();
        for (si, &(s, e)) in segments.iter().enumerate() {
            for t in &timed[s..e] {
                part.insert(t.2, si);
            }
        }
        // Attach untimed members, strongest edge first (deterministic tie-break on file ids).
        let in_comp: FxHashSet<u32> = comp.iter().copied().collect();
        let mut heap = BinaryHeap::new();
        let push_edges = |u: u32, heap: &mut BinaryHeap<_>, part: &FxHashMap<u32, usize>| {
            for &(v, k) in &adj[u as usize] {
                if in_comp.contains(&v) && !part.contains_key(&v) {
                    heap.push((k.strength(), Reverse(file(v)), Reverse(file(u)), v, u));
                }
            }
        };
        let mut seeds: Vec<u32> = part.keys().copied().collect();
        seeds.sort_unstable();
        for u in seeds {
            push_edges(u, &mut heap, &part);
        }
        while let Some((_, _, _, v, u)) = heap.pop() {
            if part.contains_key(&v) {
                continue;
            }
            let p = part[&u];
            part.insert(v, p);
            push_edges(v, &mut heap, &part);
        }
        let mut parts: Vec<Vec<u32>> = vec![Vec::new(); segments.len()];
        let mut leftover = Vec::new();
        for &l in comp {
            match part.get(&l) {
                Some(&p) => parts[p].push(l),
                None => leftover.push(l),
            }
        }
        if !leftover.is_empty() {
            parts.push(leftover);
        }
        parts
    }

    /// Build the group: rank, then explain each member by the strongest path from the keeper.
    fn build_group(&self, members: &[u32], adj: &[Vec<(u32, EdgeKind)>], nodes: &[u32]) -> Group {
        let inp = |l: u32| &self.inputs[nodes[l as usize] as usize];
        let refs: Vec<&DedupInput> = members.iter().map(|&l| inp(l)).collect();
        let r = rank(&refs, &self.constraints.keeper_overrides);
        let keeper_l = members
            .iter()
            .copied()
            .find(|&l| inp(l).file == r.keeper)
            .unwrap_or(members[0]);
        let in_group: FxHashSet<u32> = members.iter().copied().collect();
        let mut reason: FxHashMap<u32, MemberReason> = FxHashMap::default();
        reason.insert(keeper_l, MemberReason::Keeper);
        let mut heap = BinaryHeap::new();
        let mut expanded: FxHashSet<ExactKey> = FxHashSet::default();
        let mut visit =
            |u: u32, heap: &mut BinaryHeap<_>, reason: &FxHashMap<u32, MemberReason>| {
                let x = inp(u);
                // Exact copies of `u` join through `u` directly.
                if let Some(k) = self.exact_of[nodes[u as usize] as usize]
                    && expanded.insert(k)
                {
                    for &v in members {
                        if v != u && self.exact_of[nodes[v as usize] as usize] == Some(k) {
                            heap.push((
                                EdgeKind::Exact.strength(),
                                Reverse(inp(v).file),
                                Reverse(x.file),
                                v,
                                u,
                                EdgeKind::Exact,
                            ));
                        }
                    }
                }
                for &(v, k) in &adj[u as usize] {
                    if in_group.contains(&v) && !reason.contains_key(&v) {
                        heap.push((k.strength(), Reverse(inp(v).file), Reverse(x.file), v, u, k));
                    }
                }
            };
        visit(keeper_l, &mut heap, &reason);
        while let Some((_, _, _, v, u, k)) = heap.pop() {
            if reason.contains_key(&v) {
                continue;
            }
            reason.insert(
                v,
                MemberReason::Joined {
                    edge: k,
                    via: inp(u).file,
                },
            );
            visit(v, &mut heap, &reason);
        }
        let set: FxHashSet<FileId> = r.keeper_set.iter().copied().collect();
        let reclaimable_bytes = refs
            .iter()
            .filter(|x| !set.contains(&x.file))
            .map(|x| x.size)
            .sum();
        Group {
            members: members
                .iter()
                .map(|&l| Member {
                    file: inp(l).file,
                    reason: reason.get(&l).copied().unwrap_or(MemberReason::Joined {
                        edge: EdgeKind::UserMerge,
                        via: r.keeper,
                    }),
                })
                .collect(),
            keeper: r.keeper,
            keeper_set: r.keeper_set,
            display: r.display,
            keeper_reasons: r.reasons,
            ranking: r.order,
            reclaimable_bytes,
        }
    }
}

fn swap_reason(r: crate::edges::Contradiction) -> crate::edges::Contradiction {
    use crate::edges::Contradiction as C;
    match r {
        C::CaptureSecond { a, b } => C::CaptureSecond { a: b, b: a },
        C::SubSecond { a_millis, b_millis } => C::SubSecond {
            a_millis: b_millis,
            b_millis: a_millis,
        },
        C::CameraSerial { a, b } => C::CameraSerial { a: b, b: a },
        C::CameraModel { a, b } => C::CameraModel { a: b, b: a },
        other => other,
    }
}

/// Cut `timed[s..e]` at its largest gap (earliest on ties) until every segment spans <= 60 s.
fn split_segments(timed: &[(i64, FileId, u32)], s: usize, e: usize, out: &mut Vec<(usize, usize)>) {
    if e - s <= 1 || timed[e - 1].0 - timed[s].0 <= MAX_GROUP_SPAN_MS {
        out.push((s, e));
        return;
    }
    let mut cut = s + 1;
    let mut best = i64::MIN;
    for k in s + 1..e {
        let gap = timed[k].0 - timed[k - 1].0;
        if gap > best {
            best = gap;
            cut = k;
        }
    }
    split_segments(timed, s, cut, out);
    split_segments(timed, cut, e, out);
}

/// Connected parts of `part` using only edges inside it.
fn connected_parts(part: &[u32], adj: &[Vec<(u32, EdgeKind)>]) -> Vec<Vec<u32>> {
    let inside: FxHashSet<u32> = part.iter().copied().collect();
    let mut seen: FxHashSet<u32> = FxHashSet::default();
    let mut sorted = part.to_vec();
    sorted.sort_unstable();
    let mut out = Vec::new();
    for &start in &sorted {
        if !seen.insert(start) {
            continue;
        }
        let mut comp = vec![start];
        let mut stack = vec![start];
        while let Some(u) = stack.pop() {
            for &(v, _) in &adj[u as usize] {
                if inside.contains(&v) && seen.insert(v) {
                    comp.push(v);
                    stack.push(v);
                }
            }
        }
        comp.sort_unstable();
        out.push(comp);
    }
    out
}

/// Group `inputs` under `constraints` (convenience for [`Grouper::group_all`]).
pub fn group(inputs: &[DedupInput], constraints: &Constraints) -> Grouping {
    Grouper::new(inputs, constraints).group_all()
}
