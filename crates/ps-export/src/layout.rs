//! Library layout (ADR-0022): `<YYYY>/<YYYY-MM-DD>/<name>`, `<YYYY>/<YYYY-MM>/<name>` for month-precision dates
//! (a `2019-07 Italy` folder, ADR-0069), `Undated/<YYYY>/<YYYY-MM-DD>/<name>` for files dated only by their
//! modification time (its local day, ADR-0070), and ` (n)` collision suffixes.
//!
//! Names are bytes (source names need not be Unicode). Collision keys are ASCII-case-folded so that two
//! names differing only in case never land in one folder of a case-insensitive library volume.

use ps_media::LocalZone;
use ps_model::{CaptureTime, DatePrecision, RelPath, civil_from_days};

use crate::input::KeeperFile;

/// Folder name for files without a capture date.
pub const UNDATED_DIR: &str = "Undated";

/// Largest collision suffix tried.
pub const MAX_SUFFIX: u32 = 9_999;

/// `YYYY/YYYY-MM-DD` for a calendar day.
fn day_folder((y, m, d): (i32, u8, u8)) -> String {
    format!("{y:04}/{y:04}-{m:02}-{d:02}")
}

/// The UTC day of a modification time: the last resort for an input without any capture date (the service
/// gives such files their local day, [`local_mtime`]).
pub fn mtime_ymd(mtime_ns: i64) -> (i32, u8, u8) {
    civil_from_days(mtime_ns.div_euclid(1_000_000_000).div_euclid(86_400))
}

/// The wall clock of a modification time in `zone` at that instant (ADR-0070), for files the catalog has no
/// date for at all.
pub fn local_mtime(mtime_ns: i64, zone: &dyn LocalZone) -> CaptureTime {
    let secs = mtime_ns.div_euclid(1_000_000_000);
    let offset = zone.offset_at(secs);
    CaptureTime {
        local_seconds: secs + i64::from(offset) * 60,
        millis: 0,
        offset_minutes: i16::try_from(offset).ok(),
    }
}

/// The library folder of a file, from its filing date and precision or (undated without a date) its
/// modification time.
pub fn date_folder(
    capture: Option<&CaptureTime>,
    undated: bool,
    precision: DatePrecision,
    mtime_ns: i64,
) -> String {
    match capture {
        Some(c) if !undated && precision == DatePrecision::Month => {
            let (y, m, _) = c.ymd();
            format!("{y:04}/{y:04}-{m:02}")
        }
        Some(c) if !undated => day_folder(c.ymd()),
        Some(c) => format!("{UNDATED_DIR}/{}", day_folder(c.ymd())),
        None => format!("{UNDATED_DIR}/{}", day_folder(mtime_ymd(mtime_ns))),
    }
}

pub fn folder_of(f: &KeeperFile) -> String {
    date_folder(f.capture.as_ref(), f.undated, f.precision, f.mtime_ns)
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

/// A cleaner library name for `name` (bytes) when another copy of the same photo is called that: the keeper
/// "Copy of IMG_5437.JPG", "IMG_5437 copy.JPG", "IMG_5437 - Copy.JPG" or "IMG_5437 (1).JPG" becomes
/// "IMG_5437.JPG" if one of `others` has that name (ASCII case-insensitive; the other copy's spelling is used).
/// `None` when the name is already clean or no copy has the cleaner name.
pub fn cleaner_name(name: &[u8], others: &[&[u8]]) -> Option<Vec<u8>> {
    let (stem, ext) = split_name(name);
    let mut s = stem.to_vec();
    loop {
        let lower = s.to_ascii_lowercase();
        let next: Option<Vec<u8>> = if lower.starts_with(b"copy of ") {
            Some(s[8..].to_vec())
        } else if let Some(cut) = [&b" - copy"[..], b" copy", b"-copy", b"_copy"]
            .iter()
            .find(|suffix| lower.ends_with(suffix))
        {
            Some(s[..s.len() - cut.len()].to_vec())
        } else if lower.ends_with(b")")
            && let Some(open) = lower.iter().rposition(|b| *b == b'(')
            && open >= 1
            && lower[open - 1] == b' '
            && (1..=2).contains(&(lower.len() - open - 2))
            && lower[open + 1..lower.len() - 1]
                .iter()
                .all(u8::is_ascii_digit)
        {
            Some(s[..open - 1].to_vec())
        } else {
            None
        };
        let n = next.filter(|n| !n.is_empty())?;
        let mut candidate = n.clone();
        candidate.extend_from_slice(ext);
        if let Some(found) = others
            .iter()
            .find(|o| o.eq_ignore_ascii_case(&candidate) && !o.eq_ignore_ascii_case(name))
        {
            return Some(found.to_vec());
        }
        s = n;
    }
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
        let s = DatePrecision::Second;
        assert_eq!(date_folder(Some(&c), false, s, 0), "2011/2011-08-14");
        assert_eq!(date_folder(Some(&c), true, s, 0), "Undated/2011/2011-08-14");
        assert_eq!(
            date_folder(Some(&c), false, DatePrecision::Day, 0),
            "2011/2011-08-14"
        );
        assert_eq!(
            date_folder(Some(&c), false, DatePrecision::Month, 0),
            "2011/2011-08"
        );
        // 2009-02-03T12:00:00Z
        let ns = (ps_model::days_from_civil(2009, 2, 3) * 86_400 + 43_200) * 1_000_000_000;
        assert_eq!(date_folder(None, true, s, ns), "Undated/2009/2009-02-03");
        // 2019-07-05T03:30Z is the evening of 4 July in Los Angeles.
        let ns = (ps_model::days_from_civil(2019, 7, 5) * 86_400 + 3 * 3600 + 1800) * 1_000_000_000;
        let t = local_mtime(ns, &ps_media::RuleZone::LOS_ANGELES);
        assert_eq!(t.display(), "2019-07-04 20:30:00");
        assert_eq!(t.offset_minutes, Some(-420));
        assert_eq!(
            split_name(b"IMG_0001.JPG"),
            (&b"IMG_0001"[..], &b".JPG"[..])
        );
        assert_eq!(split_name(b".hidden"), (&b".hidden"[..], &b""[..]));
        assert_eq!(split_name(b"a.tar.gz"), (&b"a.tar"[..], &b".gz"[..]));
        assert_eq!(candidate(b"IMG_0001", b".JPG", 1), b"IMG_0001.JPG");
        assert_eq!(candidate(b"IMG_0001", b".JPG", 2), b"IMG_0001 (2).JPG");
        assert!(join("2011/2011-08-14", b"..").is_none());
        let others: [&[u8]; 2] = [b"IMG_5437.JPG", b"Copy of IMG_5437.JPG"];
        assert_eq!(
            cleaner_name(b"Copy of IMG_5437.JPG", &others).as_deref(),
            Some(&b"IMG_5437.JPG"[..])
        );
        for n in [
            &b"IMG_5437 copy.JPG"[..],
            b"IMG_5437 - Copy.JPG",
            b"IMG_5437 (1).JPG",
        ] {
            assert_eq!(
                cleaner_name(n, &[b"img_5437.jpg"]).as_deref(),
                Some(&b"img_5437.jpg"[..]),
                "{}",
                String::from_utf8_lossy(n)
            );
        }
        assert_eq!(
            cleaner_name(b"IMG_5437.JPG", &others),
            None,
            "already clean"
        );
        assert_eq!(
            cleaner_name(b"Copy of IMG_9.JPG", &others),
            None,
            "no copy has the clean name"
        );
        assert_eq!(cleaner_name(b"Holiday (2019).JPG", &[b"Holiday.JPG"]), None);
        assert!(join("2011/2011-08-14", b"a/b").is_none());
    }
}
