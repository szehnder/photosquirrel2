//! The public keys an update manifest may be signed with (ADR-0125): two slots for rotation, from
//! `packaging/update-key.pub` and `packaging/update-key-next.pub` (a slot reads `none` when empty), plus the test
//! key in test builds only.

use minisign_verify::PublicKey;

const RELEASE_KEY: &str = include_str!("../../../packaging/update-key.pub");
const NEXT_KEY: &str = include_str!("../../../packaging/update-key-next.pub");

/// The test public key (`tests/data/test-key.pub`). Its secret half is in the repository, so a release binary
/// must never contain it; `cargo xtask package` checks for this exact text.
#[cfg(feature = "test-key")]
pub const TEST_KEY: &str = include_str!("../tests/data/test-key.pub");

/// The base64 line of a key file, or `None` for an empty slot (`none`) or a file that is not a key.
pub fn key_line(text: &str) -> Option<&str> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))?;
    (line != "none").then_some(line)
}

/// The base64 lines of the keys this build trusts, release slots first.
pub fn trusted_lines() -> Vec<&'static str> {
    let lines: Vec<&'static str> = [RELEASE_KEY, NEXT_KEY]
        .into_iter()
        .filter_map(key_line)
        .collect();
    #[cfg(feature = "test-key")]
    let lines = {
        let mut lines = lines;
        lines.extend(key_line(TEST_KEY));
        lines
    };
    lines
}

/// The keys this build trusts. A slot that does not parse is skipped (and logged): it can never verify anything.
pub fn trusted() -> Vec<PublicKey> {
    trusted_lines()
        .into_iter()
        .filter_map(|line| match PublicKey::from_base64(line) {
            Ok(k) => Some(k),
            Err(e) => {
                log::error!("update key {line:?} does not parse: {e}");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slots_are_skipped() {
        assert_eq!(key_line("none\n"), None);
        assert_eq!(
            key_line("untrusted comment: minisign public key: X\nRWQabc\n"),
            Some("RWQabc")
        );
        assert_eq!(key_line(""), None);
    }

    #[test]
    fn test_key_signs_and_verifies() {
        let keys = trusted();
        assert!(!keys.is_empty(), "the test key is trusted in tests");
        let sig = crate::testing::sign("hello", "photosquirrel-update 1.0.0");
        let sig = minisign_verify::Signature::decode(&sig).expect("signature");
        assert!(keys.iter().any(|k| k.verify(b"hello", &sig, false).is_ok()));
        assert!(
            keys.iter()
                .all(|k| k.verify(b"hellO", &sig, false).is_err())
        );
    }
}
