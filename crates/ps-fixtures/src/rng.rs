//! Seeded randomness. Every random decision in the corpus derives from the corpus seed through
//! [`derive`], so work can run in parallel (rayon) without the result depending on scheduling order.

pub use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// The RNG used everywhere in the generator.
pub type CorpusRng = ChaCha8Rng;

/// SplitMix64 finaliser: a cheap, well-mixed 64-bit hash.
#[inline]
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// FNV-1a over a tag, so streams can be named (`"scene"`, `"junk"`, ...).
fn tag_hash(tag: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in tag.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01B3);
    }
    h
}

/// A sub-seed for stream `tag`, item `index` of the corpus seeded with `seed`.
pub fn sub_seed(seed: u64, tag: &str, index: u64) -> u64 {
    mix64(mix64(seed ^ tag_hash(tag)) ^ mix64(index.wrapping_add(0x5851_F42D_4C95_7F2D)))
}

/// An independent RNG for stream `tag`, item `index`.
pub fn derive(seed: u64, tag: &str, index: u64) -> CorpusRng {
    CorpusRng::seed_from_u64(sub_seed(seed, tag, index))
}

/// Pick one element of a non-empty slice.
pub fn pick<'a, T>(rng: &mut CorpusRng, items: &'a [T]) -> &'a T {
    &items[rng.random_range(0..items.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_streams_are_reproducible_and_independent() {
        let take = |mut r: CorpusRng| (0..8).map(|_| r.random::<u32>()).collect::<Vec<_>>();
        let a = take(derive(42, "x", 3));
        let b = take(derive(42, "x", 3));
        let c = take(derive(42, "x", 4));
        let d = take(derive(42, "y", 3));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }
}
