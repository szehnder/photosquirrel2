//! The selection tree of the Cleanup panel (change `cleanup-selection`, ADR-0068): a plan's (or a run's
//! quarantined) folders folded into a path tree with subtree totals and tri-state check boxes. Pure: no GPUI,
//! no I/O; the view loads the data on background threads and renders [`SelectionTree::rows`] in a
//! virtualized list.
//!
//! Layout: nodes are stored in depth-first preorder, so a node's subtree is the contiguous node range
//! `n..end` and its files are the contiguous file range `files`; ticking a folder flips one slice of flags.
//! Every file's id and size are known from the start (`CleanupTree`), so the picked count and bytes are
//! exact and the [`Selection`] is precise; only file *names* load when a folder is expanded.

use std::collections::HashMap;
use std::ops::Range;

use ps_model::{FileId, RelPath};

use crate::scan_backend::{
    CleanupFileRow, CleanupSelection as Selection, CleanupTree, CleanupTreeKind,
};

/// Shown for the files directly in the source root.
pub const SOURCE_ROOT_LABEL: &str = "(source root)";

/// A check box's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    All,
    Some,
    None,
}

#[derive(Clone, Debug)]
struct Node {
    /// The last path component (or [`SOURCE_ROOT_LABEL`]).
    name: String,
    /// `None` for the tree's root and for the source-root pseudo folder.
    rel: Option<RelPath>,
    depth: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    /// One past the last node of the subtree.
    end: usize,
    /// Files directly in this folder.
    direct: Range<usize>,
    /// Files of the whole subtree.
    files: Range<usize>,
    total_bytes: u64,
    picked: u64,
    picked_bytes: u64,
}

impl Node {
    fn total(&self) -> u64 {
        self.files.len() as u64
    }
}

#[derive(Clone, Copy, Debug)]
struct File {
    id: FileId,
    size: u64,
}

/// One visible row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Folder(usize),
    /// A file of an expanded folder (node, file index).
    File(usize, usize),
    /// An expanded folder whose file names are being loaded.
    Loading(usize),
}

/// What a visible folder row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderView {
    pub name: String,
    /// The full relative path (`(source root)` for root files).
    pub path: String,
    pub depth: usize,
    pub expandable: bool,
    pub expanded: bool,
    pub tick: Tick,
    pub files: u64,
    pub bytes: u64,
    pub picked: u64,
    pub picked_bytes: u64,
}

/// What a visible file row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileView {
    pub name: String,
    pub depth: usize,
    pub size: u64,
    pub picked: bool,
}

/// A run's folders with check boxes.
#[derive(Clone, Debug)]
pub struct SelectionTree {
    pub run: i64,
    pub kind: CleanupTreeKind,
    nodes: Vec<Node>,
    files: Vec<File>,
    picked: Vec<bool>,
    /// The flags as the catalog holds them (to know whether the selection must be stored first).
    stored: Vec<bool>,
    expanded: Vec<bool>,
    /// Loaded file names per node (by file index), and nodes whose names are loading.
    names: HashMap<usize, HashMap<usize, String>>,
    loading: Vec<bool>,
    rows: Vec<Row>,
}

fn components(rel: &RelPath) -> Vec<&[u8]> {
    rel.as_bytes().split(|b| *b == b'/').collect()
}

impl SelectionTree {
    pub fn new(tree: &CleanupTree) -> Self {
        // Every folder with files, plus its ancestors, in preorder (sorted by components).
        let mut paths: Vec<Vec<u8>> = Vec::new();
        let mut direct: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut root_files: Option<usize> = None;
        for (i, f) in tree.folders.iter().enumerate() {
            match &f.folder {
                None => root_files = Some(i),
                Some(rel) => {
                    let parts = components(rel);
                    for k in 1..=parts.len() {
                        paths.push(parts[..k].join(&b'/'));
                    }
                    direct.insert(rel.as_bytes().to_vec(), i);
                }
            }
        }
        paths.sort_by(|a, b| a.split(|c| *c == b'/').cmp(b.split(|c| *c == b'/')));
        paths.dedup();

        let mut nodes = vec![Node {
            name: String::new(),
            rel: None,
            depth: 0,
            parent: None,
            children: Vec::new(),
            end: 0,
            direct: 0..0,
            files: 0..0,
            total_bytes: 0,
            picked: 0,
            picked_bytes: 0,
        }];
        let mut files: Vec<File> = Vec::new();
        let mut picked: Vec<bool> = Vec::new();
        let push_files = |i: Option<usize>, files: &mut Vec<File>, picked: &mut Vec<bool>| {
            let start = files.len();
            if let Some(i) = i {
                for f in &tree.folders[i].files {
                    files.push(File {
                        id: f.file,
                        size: f.size,
                    });
                    picked.push(f.picked);
                }
            }
            start..files.len()
        };
        if root_files.is_some() {
            let range = push_files(root_files, &mut files, &mut picked);
            nodes.push(Node {
                name: SOURCE_ROOT_LABEL.into(),
                rel: None,
                depth: 0,
                parent: Some(0),
                children: Vec::new(),
                end: 0,
                direct: range.clone(),
                files: range,
                total_bytes: 0,
                picked: 0,
                picked_bytes: 0,
            });
            nodes[0].children.push(1);
        }
        // Preorder: a stack of open ancestors (node index, path bytes).
        let mut stack: Vec<(usize, Vec<u8>)> = Vec::new();
        for p in paths {
            while let Some((_, top)) = stack.last() {
                if p.starts_with(top) && p.get(top.len()) == Some(&b'/') {
                    break;
                }
                stack.pop();
            }
            let parent = stack.last().map_or(0, |(n, _)| *n);
            let depth = stack.len();
            let name =
                String::from_utf8_lossy(p.rsplit(|c| *c == b'/').next().unwrap_or(&p)).into_owned();
            let range = push_files(direct.get(&p).copied(), &mut files, &mut picked);
            let id = nodes.len();
            nodes.push(Node {
                name,
                rel: RelPath::from_bytes(p.clone()),
                depth,
                parent: Some(parent),
                children: Vec::new(),
                end: 0,
                direct: range.clone(),
                files: range,
                total_bytes: 0,
                picked: 0,
                picked_bytes: 0,
            });
            nodes[parent].children.push(id);
            stack.push((id, p));
        }
        // Subtree ends (nodes and files), totals and picked counts, bottom-up.
        let n = nodes.len();
        for i in (0..n).rev() {
            let (mut end, mut files_end) = (i + 1, nodes[i].direct.end);
            if let Some(&last) = nodes[i].children.last() {
                end = nodes[last].end;
                files_end = nodes[last].files.end;
            }
            let start = if i == 0 { 0 } else { nodes[i].direct.start };
            nodes[i].end = end;
            nodes[i].files = start..files_end.max(start);
            let r = nodes[i].files.clone();
            nodes[i].total_bytes = files[r.clone()].iter().map(|f| f.size).sum();
            nodes[i].picked = picked[r.clone()].iter().filter(|p| **p).count() as u64;
            nodes[i].picked_bytes = r.filter(|&k| picked[k]).map(|k| files[k].size).sum();
        }
        let len = nodes.len();
        let mut t = SelectionTree {
            run: tree.run,
            kind: tree.kind,
            stored: picked.clone(),
            nodes,
            files,
            picked,
            expanded: vec![false; len],
            names: HashMap::new(),
            loading: vec![false; len],
            rows: Vec::new(),
        };
        // A tree with a single top folder opens it, so the first level the user sees is never one row.
        if t.nodes[0].children.len() == 1 && !t.nodes[t.nodes[0].children[0]].children.is_empty() {
            let only = t.nodes[0].children[0];
            t.expanded[only] = true;
        }
        t.rebuild();
        t
    }

    fn rebuild(&mut self) {
        // Depth-first; a folder's subfolders come before its own files.
        enum Item {
            Node(usize),
            Files(usize),
        }
        let mut rows = Vec::new();
        let mut work: Vec<Item> = self.nodes[0]
            .children
            .iter()
            .rev()
            .map(|&c| Item::Node(c))
            .collect();
        while let Some(item) = work.pop() {
            match item {
                Item::Node(n) => {
                    rows.push(Row::Folder(n));
                    if self.expanded[n] {
                        if !self.nodes[n].direct.is_empty() {
                            work.push(Item::Files(n));
                        }
                        for &c in self.nodes[n].children.iter().rev() {
                            work.push(Item::Node(c));
                        }
                    }
                }
                Item::Files(n) => match self.names.get(&n) {
                    Some(names) => {
                        let mut ids: Vec<usize> = self.nodes[n]
                            .direct
                            .clone()
                            .filter(|k| names.contains_key(k))
                            .collect();
                        ids.sort_by(|a, b| names[a].cmp(&names[b]));
                        rows.extend(ids.into_iter().map(|k| Row::File(n, k)));
                    }
                    None => rows.push(Row::Loading(n)),
                },
            }
        }
        self.rows = rows;
    }

    /// The visible rows (folders of expanded folders, and their files once loaded).
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row(&self, ix: usize) -> Option<&Row> {
        self.rows.get(ix)
    }

    fn tick_of(&self, n: usize) -> Tick {
        let node = &self.nodes[n];
        if node.picked == 0 {
            Tick::None
        } else if node.picked == node.total() {
            Tick::All
        } else {
            Tick::Some
        }
    }

    fn path_of(&self, n: usize) -> String {
        self.nodes[n].rel.as_ref().map_or_else(
            || SOURCE_ROOT_LABEL.to_owned(),
            |r| r.display().into_owned(),
        )
    }

    pub fn folder(&self, n: usize) -> FolderView {
        let node = &self.nodes[n];
        FolderView {
            name: node.name.clone(),
            path: self.path_of(n),
            depth: node.depth,
            expandable: !node.children.is_empty() || !node.direct.is_empty(),
            expanded: self.expanded[n],
            tick: self.tick_of(n),
            files: node.total(),
            bytes: node.total_bytes,
            picked: node.picked,
            picked_bytes: node.picked_bytes,
        }
    }

    pub fn file(&self, n: usize, k: usize) -> FileView {
        FileView {
            name: self
                .names
                .get(&n)
                .and_then(|m| m.get(&k))
                .cloned()
                .unwrap_or_default(),
            depth: self.nodes[n].depth + 1,
            size: self.files[k].size,
            picked: self.picked[k],
        }
    }

    /// Folders (nodes) in the tree, without its root.
    pub fn folder_count(&self) -> usize {
        self.nodes.len() - 1
    }

    /// Folders holding at least one ticked file directly (what the plan's "Folders" counts).
    pub fn picked_folder_count(&self) -> usize {
        (1..self.nodes.len())
            .filter(|&i| {
                let node = &self.nodes[i];
                let below: u64 = node.children.iter().map(|&c| self.nodes[c].picked).sum();
                !node.direct.is_empty() && node.picked > below
            })
            .count()
    }

    /// Files and bytes picked now.
    pub fn picked(&self) -> (u64, u64) {
        (self.nodes[0].picked, self.nodes[0].picked_bytes)
    }

    /// Files and bytes in the tree.
    pub fn total(&self) -> (u64, u64) {
        (self.nodes[0].total(), self.nodes[0].total_bytes)
    }

    /// The picks differ from the stored ones (the selection must be stored before the confirmation).
    pub fn changed(&self) -> bool {
        self.picked != self.stored
    }

    /// The selection was stored as it is now.
    pub fn mark_stored(&mut self) {
        self.stored.clone_from(&self.picked);
    }

    /// Back to what the catalog holds.
    pub fn revert(&mut self) {
        self.picked.clone_from(&self.stored);
        self.recount_all();
    }

    fn recount_all(&mut self) {
        for i in (0..self.nodes.len()).rev() {
            let r = self.nodes[i].files.clone();
            self.nodes[i].picked = self.picked[r.clone()].iter().filter(|p| **p).count() as u64;
            self.nodes[i].picked_bytes = r
                .filter(|&k| self.picked[k])
                .map(|k| self.files[k].size)
                .sum();
        }
    }

    fn add_up(&mut self, mut n: Option<usize>, files: i64, bytes: i64) {
        while let Some(i) = n {
            let node = &mut self.nodes[i];
            node.picked = node.picked.saturating_add_signed(files);
            node.picked_bytes = node.picked_bytes.saturating_add_signed(bytes);
            n = node.parent;
        }
    }

    /// Tick or untick a folder with everything below it: a folder that is all ticked becomes unticked,
    /// otherwise (some or none) all ticked, which also clears every exception below it.
    pub fn toggle_folder(&mut self, n: usize) {
        if n == 0 || n >= self.nodes.len() {
            return;
        }
        let to = self.tick_of(n) != Tick::All;
        let (before, before_bytes) = (self.nodes[n].picked, self.nodes[n].picked_bytes);
        for k in self.nodes[n].files.clone() {
            self.picked[k] = to;
        }
        for i in n..self.nodes[n].end {
            let node = &mut self.nodes[i];
            if to {
                node.picked = node.total();
                node.picked_bytes = node.total_bytes;
            } else {
                node.picked = 0;
                node.picked_bytes = 0;
            }
        }
        let (after, after_bytes) = (self.nodes[n].picked, self.nodes[n].picked_bytes);
        let parent = self.nodes[n].parent;
        self.add_up(
            parent,
            after as i64 - before as i64,
            after_bytes as i64 - before_bytes as i64,
        );
    }

    /// Tick or untick one file of folder `n` (file index `k`).
    pub fn toggle_file(&mut self, n: usize, k: usize) {
        if n >= self.nodes.len() || !self.nodes[n].direct.contains(&k) {
            return;
        }
        self.picked[k] = !self.picked[k];
        let sign = if self.picked[k] { 1 } else { -1 };
        self.add_up(Some(n), sign, sign * self.files[k].size as i64);
    }

    /// Tick or untick everything.
    pub fn toggle_all(&mut self) {
        let to = self.tick_of(0) != Tick::All;
        self.picked.iter_mut().for_each(|p| *p = to);
        self.recount_all();
    }

    pub fn all_tick(&self) -> Tick {
        self.tick_of(0)
    }

    /// Expand or collapse a folder. Returns the folder whose file names must be loaded now
    /// ([`SelectionTree::set_names`]), if any.
    pub fn toggle_expand(&mut self, n: usize) -> Option<Option<RelPath>> {
        if n == 0 || n >= self.nodes.len() {
            return None;
        }
        self.expanded[n] = !self.expanded[n];
        let need = self.expanded[n]
            && !self.nodes[n].direct.is_empty()
            && !self.names.contains_key(&n)
            && !self.loading[n];
        if need {
            self.loading[n] = true;
        }
        self.rebuild();
        need.then(|| self.nodes[n].rel.clone())
    }

    /// The node of a folder (`None`: the source-root files).
    pub fn node_of(&self, folder: Option<&RelPath>) -> Option<usize> {
        (1..self.nodes.len()).find(|&i| {
            self.nodes[i].rel.as_ref() == folder
                && (folder.is_some() || !self.nodes[i].direct.is_empty())
        })
    }

    /// The file names of folder `n` arrived.
    pub fn set_names(&mut self, n: usize, rows: &[CleanupFileRow]) {
        if n >= self.nodes.len() {
            return;
        }
        let by_id: HashMap<FileId, usize> = self.nodes[n]
            .direct
            .clone()
            .map(|k| (self.files[k].id, k))
            .collect();
        let names = rows
            .iter()
            .filter_map(|r| Some((*by_id.get(&r.file)?, r.name.clone())))
            .collect();
        self.loading[n] = false;
        self.names.insert(n, names);
        self.rebuild();
    }

    /// Loading the names of folder `n` failed: collapse it again (expanding retries).
    pub fn names_failed(&mut self, n: usize) {
        if n < self.nodes.len() {
            self.loading[n] = false;
            self.expanded[n] = false;
            self.rebuild();
        }
    }

    /// The selection that picks exactly the ticked files: subtrees with nothing ticked as excluded folders,
    /// other unticked files by id. Empty (everything) when all is ticked.
    pub fn selection(&self) -> Selection {
        let mut s = Selection::default();
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if node.picked == node.total() {
                continue;
            }
            if node.picked == 0
                && let Some(rel) = &node.rel
            {
                s.except_folders.push(rel.clone());
                continue;
            }
            for k in node.direct.clone() {
                if !self.picked[k] {
                    s.except_files.push(self.files[k].id);
                }
            }
            stack.extend(node.children.iter().rev().copied());
        }
        s
    }

    /// The ids of the ticked files (tests).
    pub fn picked_ids(&self) -> Vec<FileId> {
        self.files
            .iter()
            .zip(&self.picked)
            .filter(|(_, p)| **p)
            .map(|(f, _)| f.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::scan_backend::{CleanupTreeFile, CleanupTreeFolder};

    fn rel(s: &str) -> RelPath {
        RelPath::from_bytes(s.as_bytes().to_vec()).unwrap()
    }

    /// root.jpg; A/a1 a2; A/B/b1; A/B/C/c1 c2; D/d1 (A has files and a 2-level subtree).
    fn tree() -> (CleanupTree, Vec<(i64, &'static str, u64)>) {
        let files = vec![
            (1, "root.jpg", 5),
            (2, "A/a1.jpg", 10),
            (3, "A/a2.jpg", 20),
            (4, "A/B/b1.jpg", 40),
            (5, "A/B/C/c1.jpg", 80),
            (6, "A/B/C/c2.jpg", 160),
            (7, "D/d1.jpg", 320),
            (8, "A B/x.jpg", 640),
        ];
        let mut by: std::collections::BTreeMap<Option<RelPath>, Vec<CleanupTreeFile>> =
            Default::default();
        for (id, p, size) in &files {
            by.entry(rel(p).parent())
                .or_default()
                .push(CleanupTreeFile {
                    file: FileId(*id),
                    size: *size,
                    picked: true,
                });
        }
        (
            CleanupTree {
                run: 1,
                kind: CleanupTreeKind::Plan,
                folders: by
                    .into_iter()
                    .map(|(folder, files)| CleanupTreeFolder { folder, files })
                    .collect(),
            },
            files,
        )
    }

    fn picks(t: &SelectionTree, files: &[(i64, &str, u64)]) -> Vec<i64> {
        let s = t.selection();
        let mut out = Vec::new();
        for (id, p, _) in files {
            let m = ps_model::CleanupMove {
                file: FileId(*id),
                rel_path: rel(p),
                quarantine_rel_path: rel(p),
                size: 0,
                mtime_ns: 0,
                file_index: None,
                content: ps_model::ContentId::full([0; 32]),
                disposition: ps_model::Disposition::Exported,
                covering_library_path: rel("x"),
                state: ps_model::MoveState::Planned,
                note: None,
                at_ms: None,
            };
            if s.picks(&m) {
                out.push(*id);
            }
        }
        out
    }

    fn node(t: &SelectionTree, path: &str) -> usize {
        t.node_of(Some(&rel(path))).unwrap()
    }

    fn folder_rows(t: &SelectionTree) -> Vec<String> {
        t.rows()
            .iter()
            .filter_map(|r| match r {
                Row::Folder(n) => Some(t.folder(*n).path),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn builds_a_preorder_tree_with_subtree_totals() {
        let (tr, _) = tree();
        let t = SelectionTree::new(&tr);
        assert_eq!(t.total(), (8, 1275));
        assert_eq!(t.picked(), (8, 1275));
        assert_eq!(t.folder_count(), 6, "root files, A, A/B, A/B/C, A B, D");
        assert_eq!(folder_rows(&t), vec!["(source root)", "A", "A B", "D"]);
        assert_eq!(
            t.picked_folder_count(),
            6,
            "every folder with files: root, A, A/B, A/B/C, A B, D"
        );
        let a = t.folder(node(&t, "A"));
        assert_eq!((a.files, a.bytes, a.tick), (5, 310, Tick::All));
        assert!(!t.changed());
        assert_eq!(
            t.selection(),
            Selection::default(),
            "all ticked: the empty selection"
        );
    }

    #[test]
    fn untick_a_folder_then_retick_its_parent_clears_child_exceptions() {
        let (tr, files) = tree();
        let mut t = SelectionTree::new(&tr);
        let b = node(&t, "A/B");
        t.toggle_folder(b);
        assert_eq!(t.folder(node(&t, "A")).tick, Tick::Some);
        assert_eq!(t.picked(), (5, 1275 - 280));
        assert_eq!(t.picked_folder_count(), 4);
        assert_eq!(t.selection().except_folders, vec![rel("A/B")]);
        assert_eq!(picks(&t, &files), vec![1, 2, 3, 7, 8]);
        assert!(t.changed());
        // Re-tick the parent: everything below it is ticked again.
        t.toggle_folder(node(&t, "A"));
        assert_eq!(t.folder(b).tick, Tick::All);
        assert_eq!(t.folder(node(&t, "A/B/C")).tick, Tick::All);
        assert_eq!(t.selection(), Selection::default());
        assert!(!t.changed());
    }

    #[test]
    fn a_parent_unticked_with_a_child_ticked_again_excludes_only_its_own_files() {
        let (tr, files) = tree();
        let mut t = SelectionTree::new(&tr);
        t.toggle_folder(node(&t, "A"));
        t.toggle_folder(node(&t, "A/B/C"));
        assert_eq!(t.folder(node(&t, "A")).tick, Tick::Some);
        assert_eq!(t.folder(node(&t, "A/B")).tick, Tick::Some);
        assert_eq!(picks(&t, &files), vec![1, 5, 6, 7, 8]);
        let mut ids: Vec<i64> = t.picked_ids().iter().map(|f| f.0).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 5, 6, 7, 8]);
    }

    #[test]
    fn picked_bytes_equal_the_sum_of_picked_files_on_a_3_level_tree() {
        let (tr, files) = tree();
        let mut t = SelectionTree::new(&tr);
        let c = node(&t, "A/B/C");
        // Expand to C and untick one of its files.
        assert!(
            t.toggle_expand(node(&t, "A")).is_some(),
            "A has files to load"
        );
        t.toggle_expand(node(&t, "A/B"));
        let need = t.toggle_expand(c);
        assert_eq!(need, Some(Some(rel("A/B/C"))));
        assert!(t.rows().contains(&Row::Loading(c)));
        t.set_names(
            c,
            &[
                CleanupFileRow {
                    file: FileId(6),
                    name: "c2.jpg".into(),
                    size: 160,
                    picked: true,
                },
                CleanupFileRow {
                    file: FileId(5),
                    name: "c1.jpg".into(),
                    size: 80,
                    picked: true,
                },
            ],
        );
        let file_rows: Vec<(usize, usize)> = t
            .rows()
            .iter()
            .filter_map(|r| match r {
                Row::File(n, k) if *n == c => Some((*n, *k)),
                _ => None,
            })
            .collect();
        assert_eq!(file_rows.len(), 2);
        assert_eq!(
            t.file(file_rows[0].0, file_rows[0].1).name,
            "c1.jpg",
            "sorted by name"
        );
        t.toggle_file(file_rows[1].0, file_rows[1].1);
        t.toggle_folder(node(&t, "D"));
        let ids = picks(&t, &files);
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 8]);
        let want: u64 = files
            .iter()
            .filter(|f| ids.contains(&f.0))
            .map(|f| f.2)
            .sum();
        assert_eq!(t.picked(), (6, want));
        assert_eq!(t.folder(node(&t, "A")).picked_bytes, 10 + 20 + 40 + 80);
        assert_eq!(t.folder(c).tick, Tick::Some);
        let s = t.selection();
        assert_eq!(s.except_folders, vec![rel("D")]);
        assert_eq!(s.except_files, vec![FileId(6)]);
        // Untick everything: nothing is picked; tick all again.
        t.toggle_all();
        assert_eq!(t.picked(), (8, 1275), "some -> all");
        t.toggle_all();
        assert_eq!(t.picked(), (0, 0));
        assert!(picks(&t, &files).is_empty());
        t.revert();
        assert_eq!(t.picked(), (8, 1275));
    }

    #[test]
    fn stored_picks_come_from_the_catalog() {
        let (mut tr, files) = tree();
        for f in &mut tr.folders {
            for x in &mut f.files {
                x.picked = x.file.0 != 4 && x.file.0 != 7;
            }
        }
        let t = SelectionTree::new(&tr);
        assert!(!t.changed());
        assert_eq!(t.folder(node(&t, "D")).tick, Tick::None);
        assert_eq!(picks(&t, &files), vec![1, 2, 3, 5, 6, 8]);
    }
}
