//! Near-duplicate candidate index: a multi-index hash (MIH) over the pHash (ADR-0012).
//!
//! The 64-bit pHash is split into four 16-bit bands. If two hashes differ in at most 7 bits, at least one band
//! differs in at most 1 bit (otherwise the total would be >= 8), so probing each band's exact key and its 16
//! one-bit neighbours (4 x 17 = 68 probes) finds every stored hash within distance 7. Candidates are then
//! verified on both pHash and dHash with `popcnt`.
//!
//! Each band is a direct-addressed table (keys are `u16`): a compressed-sparse-row layout of 65,537 offsets
//! into slot and pHash arrays sorted by band key, built with a counting sort in O(n). Bucket scans are
//! sequential and the pHash is stored inline, so a lookup touches the dHash array only for pHash matches.
//! A candidate is reported once, from the first band where it is within one bit, so no visited set is needed
//! and queries take `&self`. Inserts after a build go to small per-band hash maps that are folded into the
//! tables when they grow past a fraction of the index.

use ps_model::PerceptualHash;
use rustc_hash::FxHashMap;

use crate::perceptual::{VersionMismatch, hamming};

/// Largest pHash distance for which the index guarantees recall.
pub const MAX_RECALL_PHASH_DISTANCE: u32 = 7;

const BANDS: usize = 4;
const BUCKETS: usize = 1 << 16;

/// Verification thresholds for a query (inclusive). A pHash threshold above
/// [`MAX_RECALL_PHASH_DISTANCE`] is lowered to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Thresholds {
    pub phash: u32,
    pub dhash: u32,
}

impl Thresholds {
    /// The derivative-edge thresholds of ADR-0013 (pHash <= 6, dHash <= 8).
    pub const DERIVATIVE: Thresholds = Thresholds { phash: 6, dhash: 8 };
}

/// A verified candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Candidate<K> {
    pub key: K,
    pub phash_distance: u32,
    pub dhash_distance: u32,
}

#[inline]
fn band_key(phash: u64, band: usize) -> u16 {
    (phash >> (16 * band)) as u16
}

/// One band in CSR layout: bucket `k` is `offsets[k]..offsets[k + 1]` of `slots` / `phashes`.
#[derive(Clone, Debug, Default)]
struct Band {
    offsets: Vec<u32>,
    slots: Vec<u32>,
    phashes: Vec<u64>,
}

impl Band {
    fn build(band: usize, phashes: &[u64]) -> Band {
        let mut offsets = vec![0u32; BUCKETS + 1];
        for &h in phashes {
            offsets[band_key(h, band) as usize + 1] += 1;
        }
        for k in 0..BUCKETS {
            offsets[k + 1] += offsets[k];
        }
        let mut cursor = offsets[..BUCKETS].to_vec();
        let mut slots = vec![0u32; phashes.len()];
        let mut sorted = vec![0u64; phashes.len()];
        for (slot, &h) in phashes.iter().enumerate() {
            let c = &mut cursor[band_key(h, band) as usize];
            slots[*c as usize] = slot as u32;
            sorted[*c as usize] = h;
            *c += 1;
        }
        Band {
            offsets,
            slots,
            phashes: sorted,
        }
    }

    #[inline]
    fn bucket(&self, key: u16) -> (&[u32], &[u64]) {
        let (a, b) = (
            self.offsets[key as usize] as usize,
            self.offsets[key as usize + 1] as usize,
        );
        (&self.slots[a..b], &self.phashes[a..b])
    }
}

/// In-memory near-duplicate index of perceptual hashes of one algorithm version, keyed by `K` (typically
/// [`ps_model::FileId`]). Storage is struct-of-arrays.
#[derive(Clone, Debug)]
pub struct NearDupIndex<K> {
    version: u8,
    keys: Vec<K>,
    phash: Vec<u64>,
    dhash: Vec<u64>,
    /// Bands over slots `0..built`.
    bands: [Band; BANDS],
    built: usize,
    /// Slots `built..` by band key.
    pending: [FxHashMap<u16, Vec<u32>>; BANDS],
}

impl<K: Copy> NearDupIndex<K> {
    /// An empty index for hashes of `version`.
    pub fn new(version: u8) -> Self {
        Self::build(version, std::iter::empty()).unwrap_or_else(|_| unreachable!())
    }

    /// Build from stored hashes. Fails if any hash has a version other than `version`.
    pub fn build(
        version: u8,
        entries: impl IntoIterator<Item = (K, PerceptualHash)>,
    ) -> Result<Self, VersionMismatch> {
        let entries = entries.into_iter();
        let cap = entries.size_hint().0;
        let (mut keys, mut phash, mut dhash) = (
            Vec::with_capacity(cap),
            Vec::with_capacity(cap),
            Vec::with_capacity(cap),
        );
        for (k, h) in entries {
            check_version(version, &h)?;
            keys.push(k);
            phash.push(h.phash);
            dhash.push(h.dhash);
        }
        assert!(
            keys.len() < u32::MAX as usize,
            "index holds at most 2^32 - 1 hashes"
        );
        let mut index = Self {
            version,
            keys,
            phash,
            dhash,
            bands: Default::default(),
            built: 0,
            pending: Default::default(),
        };
        index.rebuild();
        Ok(index)
    }

    fn rebuild(&mut self) {
        let phash = &self.phash;
        self.bands = std::array::from_fn(|b| Band::build(b, phash));
        self.built = self.phash.len();
        for p in &mut self.pending {
            p.clear();
        }
    }

    /// Add one hash. Fails if its version differs from the index's.
    pub fn insert(&mut self, key: K, hash: PerceptualHash) -> Result<(), VersionMismatch> {
        check_version(self.version, &hash)?;
        assert!(self.keys.len() < u32::MAX as usize - 1, "index is full");
        let slot = self.keys.len() as u32;
        self.keys.push(key);
        self.phash.push(hash.phash);
        self.dhash.push(hash.dhash);
        for (b, p) in self.pending.iter_mut().enumerate() {
            p.entry(band_key(hash.phash, b)).or_default().push(slot);
        }
        if self.keys.len() - self.built > (self.built / 16).max(4096) {
            self.rebuild();
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    /// Every stored hash within `t` of `query` on both hashes, appended to `out` (each at most once, in no
    /// particular order). Recall is guaranteed for pHash distances up to [`MAX_RECALL_PHASH_DISTANCE`].
    pub fn query_into(
        &self,
        query: &PerceptualHash,
        t: Thresholds,
        out: &mut Vec<Candidate<K>>,
    ) -> Result<(), VersionMismatch> {
        check_version(self.version, query)?;
        let max_p = t.phash.min(MAX_RECALL_PHASH_DISTANCE);
        let q = query.phash;
        let verify = |slot: u32, x: u64, band: usize, out: &mut Vec<Candidate<K>>| {
            let d = x.count_ones();
            if d > max_p {
                return;
            }
            // Report from the first band within one bit only.
            for earlier in 0..band {
                if band_key(x, earlier).count_ones() <= 1 {
                    return;
                }
            }
            let dd = hamming(self.dhash[slot as usize], query.dhash);
            if dd <= t.dhash {
                out.push(Candidate {
                    key: self.keys[slot as usize],
                    phash_distance: d,
                    dhash_distance: dd,
                });
            }
        };
        for (b, band) in self.bands.iter().enumerate() {
            let key = band_key(q, b);
            for probe in std::iter::once(key).chain((0..16).map(|i| key ^ (1 << i))) {
                let (slots, phashes) = band.bucket(probe);
                for (&slot, &h) in slots.iter().zip(phashes) {
                    verify(slot, h ^ q, b, out);
                }
                if self.built < self.keys.len()
                    && let Some(slots) = self.pending[b].get(&probe)
                {
                    for &slot in slots {
                        verify(slot, self.phash[slot as usize] ^ q, b, out);
                    }
                }
            }
        }
        Ok(())
    }

    /// [`Self::query_into`] into a new vector.
    pub fn query(
        &self,
        query: &PerceptualHash,
        t: Thresholds,
    ) -> Result<Vec<Candidate<K>>, VersionMismatch> {
        let mut out = Vec::new();
        self.query_into(query, t, &mut out)?;
        Ok(out)
    }
}

fn check_version(version: u8, h: &PerceptualHash) -> Result<(), VersionMismatch> {
    if h.version == version {
        Ok(())
    } else {
        Err(VersionMismatch {
            left: version,
            right: h.version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(phash: u64, dhash: u64) -> PerceptualHash {
        PerceptualHash {
            phash,
            dhash,
            version: 1,
        }
    }

    #[test]
    fn finds_neighbours_in_built_and_pending_parts() {
        let base = 0x0123_4567_89ab_cdefu64;
        let mut idx = NearDupIndex::build(1, [(0u32, h(base, 0)), (1, h(!base, 0))]).unwrap();
        idx.insert(2, h(base ^ 0b1011_0000_0001, 1)).unwrap();
        let t = Thresholds { phash: 7, dhash: 8 };
        let mut got: Vec<u32> = idx
            .query(&h(base, 0), t)
            .unwrap()
            .iter()
            .map(|c| c.key)
            .collect();
        got.sort();
        assert_eq!(got, vec![0, 2]);
    }

    #[test]
    fn version_mismatch_is_rejected() {
        let mut idx = NearDupIndex::<u32>::new(1);
        let v2 = PerceptualHash {
            version: 2,
            ..h(0, 0)
        };
        assert!(idx.insert(0, v2).is_err());
        assert!(idx.query(&v2, Thresholds::DERIVATIVE).is_err());
        assert!(NearDupIndex::build(1, [(0u32, v2)]).is_err());
    }

    #[test]
    fn many_inserts_trigger_rebuild_and_stay_correct() {
        let mut idx = NearDupIndex::<u32>::new(1);
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut all = Vec::new();
        for i in 0..10_000u32 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            idx.insert(i, h(x, x.rotate_left(7))).unwrap();
            all.push(x);
        }
        assert!(idx.built > 0);
        for (i, &p) in all.iter().enumerate().step_by(97) {
            let got = idx
                .query(&h(p, p.rotate_left(7)), Thresholds::DERIVATIVE)
                .unwrap();
            assert!(got.iter().any(|c| c.key == i as u32));
        }
    }
}
