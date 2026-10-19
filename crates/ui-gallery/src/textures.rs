//! The thumbnail texture LRU (ADR-0018): keyed by content identity (and decode level), bounded by entry
//! count and bytes (`w * h * 4`). Eviction hands back the evicted values so the caller can release their
//! GPU atlas tiles (`window.drop_image`). Generic over the value so the policy is tested without a GPU.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::time::Instant;

/// ADR-0018 budget: 3,000 textures and 512 MB.
pub const MAX_TEXTURES: usize = 3_000;
pub const MAX_TEXTURE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub entries: usize,
    pub bytes: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            entries: MAX_TEXTURES,
            bytes: MAX_TEXTURE_BYTES,
        }
    }
}

struct Entry<V> {
    value: V,
    bytes: u64,
    tick: u64,
    /// First time the texture was painted (fade-in start); `None` until then.
    shown: Option<Instant>,
}

pub struct TextureCache<K, V> {
    budget: Budget,
    map: HashMap<K, Entry<V>>,
    order: BTreeMap<u64, K>,
    tick: u64,
    bytes: u64,
    /// Highest entry count and byte total ever held (tests and the perf HUD).
    pub peak_entries: usize,
    pub peak_bytes: u64,
    pub evictions: u64,
}

impl<K: Clone + Eq + Hash, V: Clone> TextureCache<K, V> {
    pub fn new(budget: Budget) -> Self {
        Self {
            budget,
            map: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            bytes: 0,
            peak_entries: 0,
            peak_bytes: 0,
            evictions: 0,
        }
    }

    pub fn budget(&self) -> Budget {
        self.budget
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.map.keys()
    }

    pub fn contains(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    /// Look up and mark as recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        let e = self.map.get_mut(key)?;
        self.order.remove(&e.tick);
        e.tick = tick;
        self.order.insert(tick, key.clone());
        Some(&e.value)
    }

    /// Look up without changing the order.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.map.get(key).map(|e| &e.value)
    }

    /// When the texture was first shown, setting it to `now` on the first call.
    pub fn shown_at(&mut self, key: &K, now: Instant) -> Option<Instant> {
        let e = self.map.get_mut(key)?;
        Some(*e.shown.get_or_insert(now))
    }

    pub fn was_shown(&self, key: &K) -> bool {
        self.map.get(key).is_some_and(|e| e.shown.is_some())
    }

    /// Insert (or replace) a value; returns everything evicted to stay within budget (including a
    /// replaced value). A single value larger than the byte budget is still kept (alone).
    pub fn insert(&mut self, key: K, value: V, bytes: u64) -> Vec<V> {
        let mut out = Vec::new();
        if let Some(old) = self.map.remove(&key) {
            self.order.remove(&old.tick);
            self.bytes -= old.bytes;
            out.push(old.value);
        }
        self.tick += 1;
        self.order.insert(self.tick, key.clone());
        self.map.insert(
            key,
            Entry {
                value,
                bytes,
                tick: self.tick,
                shown: None,
            },
        );
        self.bytes += bytes;
        while self.map.len() > 1
            && (self.map.len() > self.budget.entries || self.bytes > self.budget.bytes)
        {
            let Some((&tick, _)) = self.order.iter().next() else {
                break;
            };
            if let Some(k) = self.order.remove(&tick)
                && let Some(e) = self.map.remove(&k)
            {
                self.bytes -= e.bytes;
                self.evictions += 1;
                out.push(e.value);
            }
        }
        self.peak_entries = self.peak_entries.max(self.map.len());
        self.peak_bytes = self.peak_bytes.max(self.bytes);
        out
    }

    /// Remove everything (theme or source changes); returns the values to release.
    pub fn clear(&mut self) -> Vec<V> {
        self.order.clear();
        self.bytes = 0;
        self.map.drain().map(|(_, e)| e.value).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_within_both_budgets() {
        let mut c = TextureCache::new(Budget {
            entries: 3,
            bytes: 1_000,
        });
        assert!(c.insert(1, "a", 100).is_empty());
        assert!(c.insert(2, "b", 100).is_empty());
        assert!(c.insert(3, "c", 100).is_empty());
        c.get(&1);
        assert_eq!(c.insert(4, "d", 100), vec!["b"]);
        assert_eq!(c.insert(5, "e", 900), vec!["c", "a"]);
        assert!(c.len() <= 3 && c.bytes() <= 1_000);
    }
}
