//! Gallery/catalog query model: filters, sorts, item ids and 16-byte sort keys (ADR-0018).

use serde::{Deserialize, Serialize};

use crate::{FileId, MediaKind, PhotoClass, SourceId};

/// A gallery item: a catalogued file, as a compact 32-bit id (1M items = 4 MB).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(pub u32);

impl From<ItemId> for FileId {
    fn from(id: ItemId) -> FileId {
        FileId(i64::from(id.0))
    }
}

impl TryFrom<FileId> for ItemId {
    type Error = std::num::TryFromIntError;
    fn try_from(id: FileId) -> Result<ItemId, Self::Error> {
        u32::try_from(id.0).map(ItemId)
    }
}

/// What to sort by. Ties are always broken by item id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortField {
    /// Wall-clock capture time; undated items come last in either direction.
    #[default]
    CaptureDate,
    /// File name (last path component), byte order.
    FileName,
    /// File size in bytes.
    FileSize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Sort {
    pub field: SortField,
    pub descending: bool,
}

impl Sort {
    pub fn by(field: SortField) -> Sort {
        Sort {
            field,
            descending: false,
        }
    }
}

/// A typed filter; every set field narrows the result (AND). `Default` = every present file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// Only these classes (`None` = any, including not-yet-classified files).
    pub classes: Option<Vec<PhotoClass>>,
    /// Only this media kind.
    pub kind: Option<MediaKind>,
    /// Only these sources.
    pub sources: Option<Vec<SourceId>>,
    /// Also include files that have gone missing from their source.
    pub include_missing: bool,
}

impl Filter {
    pub fn class(class: PhotoClass) -> Filter {
        Filter {
            classes: Some(vec![class]),
            ..Filter::default()
        }
    }
}

/// 16-byte sort key (ADR-0018): `[0..8]` primary sort value big-endian (order-preserving), `[8..12]` item id
/// big-endian, `[12]` class code, `[13]` flags, `[14..16]` reserved for group data (member count).
///
/// Along an id list the primaries are always non-decreasing, so a scrubber can binary-search them: for
/// descending sorts the catalog stores the bitwise inverse (`!value`) of the sort value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct SortKey(pub [u8; 16]);

impl SortKey {
    /// Primary value of an undated item when sorting by capture date.
    pub const UNDATED: u64 = u64::MAX;
    /// Flag: the file is missing from its source.
    pub const FLAG_MISSING: u8 = 1;
    /// Flag: the file is a video.
    pub const FLAG_VIDEO: u8 = 2;
    /// Class byte of a file not classified yet.
    pub const CLASS_NONE: u8 = 0xFF;

    pub fn new(primary: u64, id: ItemId, class: u8, flags: u8) -> SortKey {
        let mut k = [0u8; 16];
        k[..8].copy_from_slice(&primary.to_be_bytes());
        k[8..12].copy_from_slice(&id.0.to_be_bytes());
        k[12] = class;
        k[13] = flags;
        SortKey(k)
    }

    /// Order-preserving map of a signed value (e.g. capture milliseconds) into the primary.
    pub fn signed_primary(v: i64) -> u64 {
        (v as u64) ^ (1 << 63)
    }

    /// Inverse of [`SortKey::signed_primary`].
    pub fn primary_signed(p: u64) -> i64 {
        (p ^ (1 << 63)) as i64
    }

    /// Big-endian prefix of a byte string (name sorts): order-preserving but not injective.
    pub fn bytes_primary(b: &[u8]) -> u64 {
        let mut p = [0u8; 8];
        let n = b.len().min(8);
        p[..n].copy_from_slice(&b[..n]);
        u64::from_be_bytes(p)
    }

    pub fn primary(&self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.0[..8]);
        u64::from_be_bytes(b)
    }

    pub fn item(&self) -> ItemId {
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.0[8..12]);
        ItemId(u32::from_be_bytes(b))
    }

    pub fn class_code(&self) -> u8 {
        self.0[12]
    }

    /// The class, `None` when not classified yet.
    pub fn class(&self) -> Option<PhotoClass> {
        PhotoClass::from_code(self.0[12])
    }

    pub fn flags(&self) -> u8 {
        self.0[13]
    }

    pub fn group_data(&self) -> u16 {
        u16::from_be_bytes([self.0[14], self.0[15]])
    }

    pub fn with_group_data(mut self, v: u16) -> SortKey {
        self.0[14..16].copy_from_slice(&v.to_be_bytes());
        self
    }
}

impl std::fmt::Debug for SortKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SortKey(primary={:#x}, item={}, class={}, flags={:#x})",
            self.primary(),
            self.item().0,
            self.class_code(),
            self.flags()
        )
    }
}

/// The complete ordered result of a filter + sort (ADR-0018): `ids[i]` has key `keys[i]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdList {
    pub ids: Vec<ItemId>,
    pub keys: Vec<SortKey>,
}

impl IdList {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Index of the first item whose primary sort value is >= `primary` (ascending lists).
    pub fn lower_bound(&self, primary: u64) -> usize {
        self.keys.partition_point(|k| k.primary() < primary)
    }
}

impl PhotoClass {
    /// Stable one-byte storage code (catalog columns and [`SortKey`]). Never reuse a code.
    pub fn code(self) -> u8 {
        match self {
            PhotoClass::Photo => 0,
            PhotoClass::LikelyPhoto => 1,
            PhotoClass::ThumbnailCache => 2,
            PhotoClass::Graphic => 3,
            PhotoClass::Junk => 4,
        }
    }

    pub fn from_code(code: u8) -> Option<PhotoClass> {
        Some(match code {
            0 => PhotoClass::Photo,
            1 => PhotoClass::LikelyPhoto,
            2 => PhotoClass::ThumbnailCache,
            3 => PhotoClass::Graphic,
            4 => PhotoClass::Junk,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_key_layout_and_order() {
        let a = SortKey::new(
            SortKey::signed_primary(-5),
            ItemId(7),
            0,
            SortKey::FLAG_VIDEO,
        );
        let b = SortKey::new(SortKey::signed_primary(3), ItemId(2), 1, 0);
        assert!(a < b, "negative primary sorts first");
        assert_eq!(SortKey::primary_signed(a.primary()), -5);
        assert_eq!(a.item(), ItemId(7));
        assert_eq!(a.class(), Some(PhotoClass::Photo));
        assert_eq!(a.flags(), SortKey::FLAG_VIDEO);
        assert_eq!(a.with_group_data(4).group_data(), 4);
        // Same primary: id breaks the tie.
        let c = SortKey::new(b.primary(), ItemId(3), 0, 0);
        assert!(b < c);
        assert!(SortKey::bytes_primary(b"abc") < SortKey::bytes_primary(b"abd"));
    }

    #[test]
    fn class_codes_round_trip() {
        for c in [
            PhotoClass::Photo,
            PhotoClass::LikelyPhoto,
            PhotoClass::ThumbnailCache,
            PhotoClass::Graphic,
            PhotoClass::Junk,
        ] {
            assert_eq!(PhotoClass::from_code(c.code()), Some(c));
        }
        assert_eq!(PhotoClass::from_code(SortKey::CLASS_NONE), None);
    }

    #[test]
    fn id_list_lower_bound() {
        let keys: Vec<SortKey> = [1u64, 3, 3, 9]
            .iter()
            .enumerate()
            .map(|(i, p)| SortKey::new(*p, ItemId(i as u32), 0, 0))
            .collect();
        let l = IdList {
            ids: keys.iter().map(SortKey::item).collect(),
            keys,
        };
        assert_eq!(l.lower_bound(3), 1);
        assert_eq!(l.lower_bound(4), 3);
        assert_eq!(l.lower_bound(10), 4);
    }
}
