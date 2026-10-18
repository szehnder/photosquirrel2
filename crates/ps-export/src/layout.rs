//! Library layout (ADR-0022): `<YYYY>/<YYYY-MM-DD>/<name>`, `Undated/<YYYY>/<YYYY-MM-DD>/<name>` for files dated
//! only by their modification time, and ` (n)` collision suffixes.
//!
//! Names are bytes (source names need not be Unicode). Collision keys are ASCII-case-folded so that two
//! names differing only in case never land in one folder of a case-insensitive library volume.

use ps_model::{CaptureTime, RelPath, civil_from_days};

use crate::input::KeeperFile;

/// Folder name for files without a capture date.
pub const UNDATED_DIR: &str = "Undated";

/// Largest collision suffix tried.
pub const MAX_SUFFIX: u32 = 9_999;

/// `YYYY/YYYY-MM-DD` for a calendar day.
fn day_folder((y, m, d): (i32, u8, u8)) -> String {
    format!("{y:04}/{y:04}-{m:02}-{d:02}")
}

/// The day of a modification time (UTC; a file time carries no camera wall-clock).
pub fn mtime_ymd(mtime_ns: i64) -> (i32, u8, u8) {
    civil_from_days(mtime_ns.div_euclid(1_000_000_000).div_euclid(86_400))
}

/// The library folder of a file, from its capture date or (undated) its modification time.
pub fn date_folder(capture: Option<&CaptureTime>, undated: bool, mtime_ns: i64) -> String {
    match capture {
        Some(c) if !undated => day_folder(c.ymd()),
        Some(c) => format!("{UNDATED_DIR}/{}", day_folder(c.ymd())),
        None => format!("{UNDATED_DIR}/{}", day_folder(mtime_ymd(mtime_ns))),
    }
}

pub fn folder_of(f: &KeeperFile) -> String {
    date_folder(f.capture.as_ref(), f.undated, f.mtime_ns)
}

/// Split a file name into stem and extension (with its dot). A leading dot is part of the stem.
pub fn split_name(name: &[u8]) -> (&[u8], &[u8]) {
    match name.iter().rposition(|b| *b == b'.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, &[]),
    }
}

/// `stem + ext` for `n == 1`, else `stem (n) + ext`.
pub fn candidate(stem: &[u8], ext: &[u8], n: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(stem.len() + ext.len() + 8);
    out.extend_from_slice(stem);
    if n > 1 {
        out.extend_from_slice(format!(" ({n})").as_bytes());
    }
    out.extend_from_slice(ext);
    out
}

/// `folder/name` as a library-relative path. `None` when the name is not a single path component.
pub fn join(folder: &str, name: &[u8]) -> Option<RelPath> {
    if name.is_empty() || name.contains(&b'/') || name == b"." || name == b".." {
        return None;
    }
    let mut v = folder.as_bytes().to_vec();
    v.push(b'/');
    v.extend_from_slice(name);
    RelPath::from_bytes(v)
}

/// Case-folded collision key of a library path.
pub fn collision_key(p: &RelPath) -> Vec<u8> {
    p.as_bytes().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_and_names() {
        let c = CaptureTime::from_parts(2011, 8, 14, 10, 0, 0);
        assert_eq!(date_folder(Some(&c), false, 0), "2011/2011-08-14");
        assert_eq!(date_folder(Some(&c), true, 0), "Undated/2011/2011-08-14");
        // 2009-02-03T12:00:00Z
        let ns = (ps_model::days_from_civil(2009, 2, 3) * 86_400 + 43_200) * 1_000_000_000;
        assert_eq!(date_folder(None, true, ns), "Undated/2009/2009-02-03");
        assert_eq!(
            split_name(b"IMG_0001.JPG"),
            (&b"IMG_0001"[..], &b".JPG"[..])
        );
        assert_eq!(split_name(b".hidden"), (&b".hidden"[..], &b""[..]));
        assert_eq!(split_name(b"a.tar.gz"), (&b"a.tar"[..], &b".gz"[..]));
        assert_eq!(candidate(b"IMG_0001", b".JPG", 1), b"IMG_0001.JPG");
        assert_eq!(candidate(b"IMG_0001", b".JPG", 2), b"IMG_0001 (2).JPG");
        assert!(join("2011/2011-08-14", b"..").is_none());
        assert!(join("2011/2011-08-14", b"a/b").is_none());
    }
}
