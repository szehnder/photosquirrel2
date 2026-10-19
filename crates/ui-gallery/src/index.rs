//! The gallery's ordered list (ADR-0018): the complete ID list and sort keys of the current query, plus a
//! local overlay for expanded stacks (members spliced in after their keeper, no re-query of the list).

use std::collections::HashMap;
use std::sync::Arc;

use ps_model::{IdList, ItemId, SortKey};

/// An item's part in a duplicate stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackRole {
    /// Not part of a stack.
    Single,
    /// The keeper tile of a stack of `count` copies.
    Keeper { count: u16, expanded: bool },
    /// A member shown because its stack is expanded.
    Member { keeper: ItemId },
}

#[derive(Clone, Debug, Default)]
pub struct GalleryIndex {
    pub ids: Arc<[ItemId]>,
    pub keys: Arc<[SortKey]>,
    /// Bumped on every change, so pages and requests of an older list are ignored.
    pub generation: u64,
    /// Expanded stacks: keeper -> the other members, in order.
    expanded: HashMap<ItemId, Vec<ItemId>>,
    member_of: HashMap<ItemId, ItemId>,
}

impl GalleryIndex {
    pub fn new(list: IdList, generation: u64) -> Self {
        Self {
            ids: list.ids.into(),
            keys: list.keys.into(),
            generation,
            expanded: HashMap::new(),
            member_of: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn id(&self, index: usize) -> Option<ItemId> {
        self.ids.get(index).copied()
    }

    /// Position of an item (linear; for rare lookups such as restoring the loupe's photo).
    pub fn position_of(&self, id: ItemId) -> Option<usize> {
        self.ids.iter().position(|i| *i == id)
    }

    /// Where an item with `key` is, or would be, in this list: binary search on the (non-decreasing)
    /// primary, then the exact item among equal primaries. Used to keep the scroll anchor when the list
    /// changes under the user (live updates).
    pub fn position_of_key(&self, key: &SortKey) -> usize {
        let lo = self.keys.partition_point(|k| k.primary() < key.primary());
        let hi = self.keys.partition_point(|k| k.primary() <= key.primary());
        self.keys[lo..hi]
            .iter()
            .position(|k| k.item() == key.item())
            .map_or(lo, |p| lo + p)
    }

    /// Members of the stack at `index` (its keeper's count), 1 for single items.
    pub fn stack_count(&self, index: usize) -> u16 {
        self.keys.get(index).map_or(1, |k| k.group_data().max(1))
    }

    pub fn role(&self, index: usize) -> StackRole {
        let Some(id) = self.id(index) else {
            return StackRole::Single;
        };
        if let Some(keeper) = self.member_of.get(&id) {
            return StackRole::Member { keeper: *keeper };
        }
        match self.stack_count(index) {
            0 | 1 => StackRole::Single,
            n => StackRole::Keeper {
                count: n,
                expanded: self.expanded.contains_key(&id),
            },
        }
    }

    pub fn is_expanded(&self, keeper: ItemId) -> bool {
        self.expanded.contains_key(&keeper)
    }

    /// Splice the stack's other members in after its keeper. `members` may include the keeper (ignored).
    pub fn expand(&mut self, keeper: ItemId, members: &[ItemId]) -> bool {
        if self.expanded.contains_key(&keeper) {
            return false;
        }
        let Some(at) = self.position_of(keeper) else {
            return false;
        };
        let others: Vec<ItemId> = members.iter().copied().filter(|m| *m != keeper).collect();
        if others.is_empty() {
            return false;
        }
        let key = self.keys[at];
        let mut ids = Vec::with_capacity(self.ids.len() + others.len());
        let mut keys = Vec::with_capacity(self.keys.len() + others.len());
        ids.extend_from_slice(&self.ids[..=at]);
        keys.extend_from_slice(&self.keys[..=at]);
        for m in &others {
            ids.push(*m);
            keys.push(SortKey::new(
                key.primary(),
                *m,
                key.class_code(),
                key.flags(),
            ));
            self.member_of.insert(*m, keeper);
        }
        ids.extend_from_slice(&self.ids[at + 1..]);
        keys.extend_from_slice(&self.keys[at + 1..]);
        self.ids = ids.into();
        self.keys = keys.into();
        self.expanded.insert(keeper, others);
        self.generation += 1;
        true
    }

    /// Fold an expanded stack back into its keeper.
    pub fn collapse(&mut self, keeper: ItemId) -> bool {
        let Some(others) = self.expanded.remove(&keeper) else {
            return false;
        };
        for m in &others {
            self.member_of.remove(m);
        }
        let drop: std::collections::HashSet<ItemId> = others.into_iter().collect();
        let (ids, keys): (Vec<ItemId>, Vec<SortKey>) = self
            .ids
            .iter()
            .zip(self.keys.iter())
            .filter(|(i, _)| !drop.contains(i))
            .map(|(i, k)| (*i, *k))
            .unzip();
        self.ids = ids.into();
        self.keys = keys.into();
        self.generation += 1;
        true
    }

    /// Replace the list (live update or new query), re-expanding stacks that are still present.
    pub fn replace(&mut self, list: IdList) {
        let expanded = std::mem::take(&mut self.expanded);
        self.member_of.clear();
        let generation = self.generation + 1;
        *self = GalleryIndex::new(list, generation);
        for (keeper, members) in expanded {
            self.expand(keeper, &members);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: u32) -> IdList {
        let keys: Vec<SortKey> = (0..n)
            .map(|i| SortKey::new(u64::from(i) * 10, ItemId(i), 0, 0))
            .collect();
        IdList {
            ids: keys.iter().map(SortKey::item).collect(),
            keys,
        }
    }

    #[test]
    fn expand_and_collapse_splice_in_place() {
        let mut l = list(5);
        l.keys[2] = l.keys[2].with_group_data(3);
        let mut ix = GalleryIndex::new(l, 0);
        assert_eq!(
            ix.role(2),
            StackRole::Keeper {
                count: 3,
                expanded: false
            }
        );
        assert!(ix.expand(ItemId(2), &[ItemId(2), ItemId(70), ItemId(71)]));
        assert_eq!(
            ix.ids.to_vec(),
            [0, 1, 2, 70, 71, 3, 4].map(ItemId).to_vec()
        );
        assert_eq!(ix.role(3), StackRole::Member { keeper: ItemId(2) });
        assert!(ix.collapse(ItemId(2)));
        assert_eq!(ix.len(), 5);
    }

    #[test]
    fn key_positions() {
        let ix = GalleryIndex::new(list(100), 0);
        let k = ix.keys[42];
        assert_eq!(ix.position_of_key(&k), 42);
        assert_eq!(
            ix.position_of_key(&SortKey::new(425, ItemId(999), 0, 0)),
            43
        );
    }
}
