//! Selection (gallery spec "Selection"): click, Shift-click range, Ctrl/Cmd-click toggle, select all, and
//! the focused item that arrow keys move.
//!
//! Selected items are a bitset over item ids, so a selection survives re-sorting and live updates of the
//! list and "select all" of a million items costs 125 KB. The anchor (Shift ranges) and focus are positions
//! in the current list.

use ps_model::ItemId;

#[derive(Clone, Debug, Default)]
pub struct Selection {
    bits: Vec<u64>,
    count: usize,
    /// Start of Shift-click ranges (an index into the current list).
    pub anchor: Option<usize>,
    /// The keyboard focus (an index into the current list).
    pub focus: Option<usize>,
}

impl Selection {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn contains(&self, id: ItemId) -> bool {
        let (w, b) = (id.0 as usize / 64, id.0 % 64);
        self.bits.get(w).is_some_and(|word| word & (1 << b) != 0)
    }

    fn set(&mut self, id: ItemId, on: bool) {
        let (w, b) = (id.0 as usize / 64, id.0 % 64);
        if w >= self.bits.len() {
            if !on {
                return;
            }
            self.bits.resize(w + 1, 0);
        }
        let was = self.bits[w] & (1 << b) != 0;
        if on && !was {
            self.bits[w] |= 1 << b;
            self.count += 1;
        } else if !on && was {
            self.bits[w] &= !(1 << b);
            self.count -= 1;
        }
    }

    pub fn clear(&mut self) {
        self.bits.clear();
        self.count = 0;
    }

    /// Plain click: select only this item, and make it the anchor and focus.
    pub fn click(&mut self, ids: &[ItemId], index: usize) {
        self.clear();
        if let Some(id) = ids.get(index) {
            self.set(*id, true);
            self.anchor = Some(index);
            self.focus = Some(index);
        }
    }

    /// Ctrl/Cmd-click: toggle one item; it becomes the anchor and focus.
    pub fn toggle(&mut self, ids: &[ItemId], index: usize) {
        if let Some(id) = ids.get(index) {
            let on = !self.contains(*id);
            self.set(*id, on);
            self.anchor = Some(index);
            self.focus = Some(index);
        }
    }

    /// Shift-click (or Shift+arrow): select the range from the anchor to `index`, replacing the selection
    /// unless `extend` (Ctrl+Shift) is set. The anchor stays.
    pub fn select_range(&mut self, ids: &[ItemId], index: usize, extend: bool) {
        if ids.is_empty() {
            return;
        }
        let index = index.min(ids.len() - 1);
        let anchor = self.anchor.unwrap_or(index).min(ids.len() - 1);
        if !extend {
            self.clear();
        }
        let (a, b) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        for id in &ids[a..=b] {
            self.set(*id, true);
        }
        self.anchor = Some(anchor);
        self.focus = Some(index);
    }

    pub fn select_all(&mut self, ids: &[ItemId]) {
        self.clear();
        if let Some(max) = ids.iter().map(|i| i.0).max() {
            self.bits.reserve(max as usize / 64 + 1);
        }
        for id in ids {
            self.set(*id, true);
        }
    }

    /// Selected ids in list order (at most `limit`).
    pub fn ids_in(&self, ids: &[ItemId], limit: usize) -> Vec<ItemId> {
        if self.is_empty() {
            return Vec::new();
        }
        ids.iter()
            .copied()
            .filter(|id| self.contains(*id))
            .take(limit)
            .collect()
    }

    /// Re-point anchor and focus after the list changed (e.g. live updates), by id.
    pub fn remap(&mut self, old: &[ItemId], new_index_of: impl Fn(ItemId) -> Option<usize>) {
        self.anchor = self
            .anchor
            .and_then(|i| old.get(i).copied())
            .and_then(&new_index_of);
        self.focus = self
            .focus
            .and_then(|i| old.get(i).copied())
            .and_then(&new_index_of);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u32) -> Vec<ItemId> {
        (0..n).map(|i| ItemId(i * 3 + 1)).collect()
    }

    #[test]
    fn click_toggle_and_range() {
        let ids = ids(50);
        let mut s = Selection::default();
        s.click(&ids, 10);
        assert_eq!(s.len(), 1);
        s.select_range(&ids, 20, false);
        assert_eq!(s.len(), 11);
        assert!(ids[10..=20].iter().all(|i| s.contains(*i)));
        s.toggle(&ids, 15);
        assert_eq!(s.len(), 10);
        assert!(!s.contains(ids[15]));
        s.select_all(&ids);
        assert_eq!(s.len(), 50);
        // A backwards range from the anchor.
        s.click(&ids, 30);
        s.select_range(&ids, 25, false);
        assert_eq!(s.ids_in(&ids, 100), ids[25..=30].to_vec());
    }
}
