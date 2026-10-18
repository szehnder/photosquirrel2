//! Size limits shared by the scanner, media analysis and hashing (ADR-0010, ADR-0011).

/// Files up to this many bytes (256 MB) get a full BLAKE3 content identity; larger ones a sampled one.
pub const FULL_IDENTITY_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Size of each of the first, middle and last windows of a sampled identity (4 MB).
pub const SAMPLE_WINDOW_BYTES: u64 = 4 * 1024 * 1024;

/// Files up to this many bytes (64 MB) are read once into memory and the same buffer is hashed and decoded;
/// larger ones are hashed from a stream.
pub const ANALYSIS_BUFFER_MAX_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_consistent() {
        assert_eq!(FULL_IDENTITY_MAX_BYTES, 268_435_456);
        assert_eq!(3 * SAMPLE_WINDOW_BYTES, 12_582_912);
        const { assert!(ANALYSIS_BUFFER_MAX_BYTES <= FULL_IDENTITY_MAX_BYTES) };
        const { assert!(3 * SAMPLE_WINDOW_BYTES < FULL_IDENTITY_MAX_BYTES) };
    }
}
