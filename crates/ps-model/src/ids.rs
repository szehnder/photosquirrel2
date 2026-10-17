use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id!(
    /// A root folder the user added (ADR-0006).
    SourceId
);
id!(
    /// One catalogued file at one path inside a source.
    FileId
);
id!(
    /// A duplicate group (ADR-0013).
    GroupId
);
id!(
    /// One scan run (ADR-0008).
    ScanRunId
);
id!(
    /// One export run (ADR-0022).
    ExportRunId
);

/// How a [`ContentId`] was computed (ADR-0011).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentIdKind {
    /// BLAKE3 of every byte.
    Full,
    /// BLAKE3 of size + first, middle and last 4 MB (files > 256 MB).
    Sampled,
}

/// Content identity of a file: a BLAKE3 digest plus how it was computed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContentId {
    pub digest: [u8; 32],
    pub kind: ContentIdKind,
}

impl ContentId {
    pub fn full(digest: [u8; 32]) -> Self {
        Self {
            digest,
            kind: ContentIdKind::Full,
        }
    }

    pub fn sampled(digest: [u8; 32]) -> Self {
        Self {
            digest,
            kind: ContentIdKind::Sampled,
        }
    }

    /// Lower-case hex of the digest (64 chars).
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.digest {
            s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            s.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
        }
        s
    }

    /// Parse 64 hex chars.
    pub fn from_hex(hex: &str, kind: ContentIdKind) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut digest = [0u8; 32];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            digest[i] = (hi * 16 + lo) as u8;
        }
        Some(Self { digest, kind })
    }

    /// First 8 bytes as a little-endian integer; handy for sharding and texture keys.
    pub fn short(&self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.digest[..8]);
        u64::from_le_bytes(b)
    }
}

impl fmt::Debug for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = self.to_hex();
        write!(f, "ContentId({}…, {:?})", &hex[..12], self.kind)
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Stat fingerprint used to detect unchanged files without reading them (ADR-0007).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StatFingerprint {
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch (0 when unavailable).
    pub mtime_ns: i64,
    /// Inode (Unix) or file index (Windows) when the platform exposes one.
    pub file_index: Option<u64>,
}

impl StatFingerprint {
    /// Two fingerprints describe the same, unchanged file. A missing file index on either side is ignored
    /// (NAS and exFAT often report none or unstable ones).
    pub fn unchanged(&self, other: &StatFingerprint) -> bool {
        self.size == other.size
            && self.mtime_ns == other.mtime_ns
            && match (self.file_index, other.file_index) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_id_hex_round_trip() {
        let mut d = [0u8; 32];
        for (i, b) in d.iter_mut().enumerate() {
            *b = (i * 7) as u8;
        }
        let id = ContentId::full(d);
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(ContentId::from_hex(&hex, ContentIdKind::Full), Some(id));
        assert_eq!(ContentId::from_hex("zz", ContentIdKind::Full), None);
    }

    #[test]
    fn fingerprint_ignores_missing_file_index() {
        let a = StatFingerprint {
            size: 10,
            mtime_ns: 5,
            file_index: Some(1),
        };
        let b = StatFingerprint {
            file_index: None,
            ..a
        };
        let c = StatFingerprint {
            file_index: Some(2),
            ..a
        };
        assert!(a.unchanged(&b));
        assert!(!a.unchanged(&c));
        assert!(!a.unchanged(&StatFingerprint { size: 11, ..a }));
    }
}
