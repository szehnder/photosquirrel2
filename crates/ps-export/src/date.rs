//! The filing date of an exported file (ADR-0043, refining ADR-0022's date rule).
//!
//! A keeper that lost its EXIF (a messaging-app or editor copy kept for its resolution) still belongs on the day
//! the photo was taken when another copy in its duplicate group remembers that day. [`resolve`] picks the best
//! date known for the group, in this order:
//!
//! 1. the keeper's own embedded date (DateTimeOriginal, CreateDate, container creation date);
//! 2. the embedded date of another copy in the group (best provenance first, then the earliest, then the
//!    lowest file id, so the choice is deterministic);
//! 3. a date in a file name (`IMG_20230626_143012.jpg`, `2023-06-26 14.30.12.jpg`), keeper first;
//! 4. an XMP sidecar next to a copy (`exif:DateTimeOriginal`, `photoshop:DateCreated`, `xmp:CreateDate`),
//!    keeper first; sidecars are read only when nothing above gave a date, so planning stays cheap;
//! 5. the keeper's modification time, filed under `Undated/`.
//!
//! Every planned file records where its date came from ([`DateOrigin`]); the plan, the CLI and the manifest
//! show it.

use ps_model::{CaptureDate, CaptureTime, DateProvenance, FileId};
use serde::Serialize;

/// Where a filing date came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DateSource {
    /// An embedded date (EXIF or container).
    Embedded { provenance: DateProvenance },
    /// A date in a file name.
    FileName,
    /// An XMP sidecar next to the file.
    Sidecar,
    /// Only the modification time: the file goes under `Undated/`.
    FileMtime,
}

/// A filing date's source and the copy it was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct DateOrigin {
    pub source: DateSource,
    /// Another copy of the group the date was taken from; `None` when it is the file's own.
    pub copy: Option<FileId>,
}

impl DateOrigin {
    pub const fn own(source: DateSource) -> Self {
        Self { source, copy: None }
    }

    /// The file's own DateTimeOriginal (test inputs, singletons with EXIF).
    pub const OWN_EXIF: DateOrigin = DateOrigin::own(DateSource::Embedded {
        provenance: DateProvenance::DateTimeOriginal,
    });

    /// Only the file time.
    pub const MTIME: DateOrigin = DateOrigin::own(DateSource::FileMtime);

    /// "DateTimeOriginal", "DateTimeOriginal of another copy", "file name", "file modification time".
    pub fn describe(&self) -> String {
        let what = match self.source {
            DateSource::Embedded { provenance } => provenance.as_str().to_string(),
            DateSource::FileName => "file name".into(),
            DateSource::Sidecar => "XMP sidecar".into(),
            DateSource::FileMtime => return "file modification time (undated)".into(),
        };
        if self.copy.is_some() {
            format!("{what} of another copy")
        } else {
            what
        }
    }

    pub fn is_undated(&self) -> bool {
        self.source == DateSource::FileMtime
    }
}

/// What the catalog knows about one copy's date.
#[derive(Clone, Debug, PartialEq)]
pub struct DateFacts<'a> {
    pub file: FileId,
    /// File name (bytes; need not be Unicode).
    pub name: &'a [u8],
    /// The resolved capture date from the metadata, `undated` when it is only the file time.
    pub embedded: Option<CaptureDate>,
}

impl DateFacts<'_> {
    fn real_embedded(&self) -> Option<(DateProvenance, CaptureTime)> {
        self.embedded
            .filter(|c| !c.undated && c.provenance != DateProvenance::FileMtime)
            .map(|c| (c.provenance, c.value))
    }
}

/// The date a file is filed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilingDate {
    /// `None` only for an undated file whose metadata has no file time either (the layout then uses the
    /// catalog's `mtime_ns`).
    pub capture: Option<CaptureTime>,
    pub undated: bool,
    pub origin: DateOrigin,
}

/// The best filing date for `keeper` given the other copies of its group (module docs). `sidecar` reads a
/// copy's XMP sidecar date; it is called only when no embedded or file-name date exists.
pub fn resolve(
    keeper: &DateFacts<'_>,
    others: &[DateFacts<'_>],
    mut sidecar: impl FnMut(&DateFacts<'_>) -> Option<CaptureTime>,
) -> FilingDate {
    let dated = |capture, source, copy| FilingDate {
        capture: Some(capture),
        undated: false,
        origin: DateOrigin { source, copy },
    };
    if let Some((provenance, t)) = keeper.real_embedded() {
        return dated(t, DateSource::Embedded { provenance }, None);
    }
    if let Some((provenance, t, file)) = others
        .iter()
        .filter(|o| o.file != keeper.file)
        .filter_map(|o| o.real_embedded().map(|(p, t)| (p, t, o.file)))
        .min()
    {
        return dated(t, DateSource::Embedded { provenance }, Some(file));
    }
    let copy_of = |f: FileId| (f != keeper.file).then_some(f);
    let order = std::iter::once(keeper).chain(others.iter().filter(|o| o.file != keeper.file));
    if let Some((t, f)) = order
        .clone()
        .find_map(|o| date_in_name(o.name).map(|t| (t, o.file)))
    {
        return dated(t, DateSource::FileName, copy_of(f));
    }
    if let Some((t, f)) = order
        .into_iter()
        .find_map(|o| sidecar(o).map(|t| (t, o.file)))
    {
        return dated(t, DateSource::Sidecar, copy_of(f));
    }
    FilingDate {
        capture: keeper.embedded.map(|c| c.value),
        undated: true,
        origin: DateOrigin::MTIME,
    }
}

fn days_in_month(y: i32, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

fn num(b: &[u8], at: usize, len: usize) -> Option<u32> {
    let s = b.get(at..at + len)?;
    s.iter()
        .all(u8::is_ascii_digit)
        .then(|| s.iter().fold(0u32, |acc, d| acc * 10 + u32::from(d - b'0')))
}

fn digit_at(b: &[u8], i: usize) -> bool {
    b.get(i).is_some_and(u8::is_ascii_digit)
}

/// `HH MM SS` at `at` with an optional separator from `seps` used consistently; returns seconds of day and
/// the end offset.
fn time_at(b: &[u8], at: usize, seps: &[u8]) -> Option<(u32, usize)> {
    let h = num(b, at, 2)?;
    let (sep, mut i) = match b.get(at + 2) {
        Some(c) if seps.contains(c) => (Some(*c), at + 3),
        _ => (None, at + 2),
    };
    let mi = num(b, i, 2)?;
    i += 2;
    if let Some(s) = sep {
        if b.get(i) != Some(&s) {
            return None;
        }
        i += 1;
    }
    let s = num(b, i, 2)?;
    i += 2;
    (h < 24 && mi < 60 && s < 60).then_some((h * 3600 + mi * 60 + s, i))
}

/// A calendar date (and time, when it follows) in a file name: `YYYYMMDD`, `YYYY-MM-DD`, `YYYY_MM_DD` or
/// `YYYY.MM.DD`, optionally followed by `_HHMMSS`, `-HH-MM-SS`, ` HH.MM.SS`, `THH:MM:SS`. Compact dates must
/// lie in 1990-2099 (to skip counters like `DSC_19871234`); separated dates in 1900-2099. Digits may not touch
/// the date on either side, except a compact time (`IMG_20230626_143012345`, milliseconds).
pub fn date_in_name(name: &[u8]) -> Option<CaptureTime> {
    let stem = match name.iter().rposition(|b| *b == b'.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    };
    (0..stem.len()).find_map(|i| {
        if i > 0 && digit_at(stem, i - 1) {
            return None;
        }
        date_at(stem, i)
    })
}

fn date_at(b: &[u8], i: usize) -> Option<CaptureTime> {
    let y = num(b, i, 4)? as i32;
    let sep = b.get(i + 4).copied().filter(|c| b"-_.".contains(c));
    let mut j = i + 4 + usize::from(sep.is_some());
    let m = num(b, j, 2)? as u8;
    j += 2;
    if let Some(s) = sep {
        if b.get(j) != Some(&s) {
            return None;
        }
        j += 1;
    }
    let d = num(b, j, 2)? as u8;
    j += 2;
    let years = if sep.is_some() {
        1900..=2099
    } else {
        1990..=2099
    };
    if !years.contains(&y) || m == 0 || m > 12 || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    // Optional time: after a separator, or directly after a compact date.
    let time = match b.get(j) {
        Some(c) if b"_- T".contains(c) => time_at(b, j + 1, b"-.:"),
        Some(c) if c.is_ascii_digit() && sep.is_none() => time_at(b, j, b""),
        _ => None,
    };
    let secs = match time {
        // Milliseconds may follow a compact time; any other digit run makes it a counter, not a time.
        Some((secs, _)) => secs,
        None if digit_at(b, j) => return None,
        None => 0,
    };
    let base = CaptureTime::from_parts(y, m, d, 0, 0, 0);
    Some(CaptureTime {
        local_seconds: base.local_seconds + i64::from(secs),
        ..base
    })
}

/// `YYYY-MM-DD[THH:MM[:SS]]` or `YYYY:MM:DD HH:MM:SS` text (XMP and EXIF date values). Zone suffixes are
/// ignored: the wall clock is what the library files by.
pub fn parse_date_text(s: &str) -> Option<CaptureTime> {
    let b = s.trim().as_bytes();
    let y = num(b, 0, 4)? as i32;
    let sep = *b.get(4)?;
    if sep != b'-' && sep != b':' {
        return None;
    }
    let m = num(b, 5, 2)? as u8;
    if b.get(7) != Some(&sep) {
        return None;
    }
    let d = num(b, 8, 2)? as u8;
    if !(1800..=2199).contains(&y) || m == 0 || m > 12 || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    let mut secs = 0u32;
    if matches!(b.get(10), Some(b'T' | b' ')) {
        let h = num(b, 11, 2)?;
        let mi = if b.get(13) == Some(&b':') {
            num(b, 14, 2)?
        } else {
            return None;
        };
        let sec = if b.get(16) == Some(&b':') {
            num(b, 17, 2)?
        } else {
            0
        };
        if h >= 24 || mi >= 60 || sec >= 60 {
            return None;
        }
        secs = h * 3600 + mi * 60 + sec;
    }
    let base = CaptureTime::from_parts(y, m, d, 0, 0, 0);
    Some(CaptureTime {
        local_seconds: base.local_seconds + i64::from(secs),
        ..base
    })
}

/// The capture date recorded in an XMP sidecar: `exif:DateTimeOriginal`, else `photoshop:DateCreated`, else
/// `xmp:CreateDate`, as an attribute (`exif:DateTimeOriginal="…"`) or an element (`<exif:DateTimeOriginal>…`).
pub fn xmp_sidecar_date(xml: &[u8]) -> Option<CaptureTime> {
    let text = String::from_utf8_lossy(xml);
    [
        "exif:DateTimeOriginal",
        "photoshop:DateCreated",
        "xmp:CreateDate",
    ]
    .iter()
    .find_map(|key| xmp_value(&text, key).and_then(|v| parse_date_text(&v)))
}

fn xmp_value(text: &str, key: &str) -> Option<String> {
    let mut rest = text;
    while let Some(i) = rest.find(key) {
        let after = &rest[i + key.len()..];
        let before = rest[..i].chars().next_back();
        rest = after;
        let after = after.trim_start();
        if before == Some('<') {
            let body = after.strip_prefix('>')?;
            let end = body.find('<')?;
            return Some(body[..end].trim().to_string());
        }
        if let Some(v) = after.strip_prefix('=') {
            let v = v.trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let v = &v[1..];
                let end = v.find(q)?;
                return Some(v[..end].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(y: i32, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> CaptureTime {
        CaptureTime::from_parts(y, mo, d, h, mi, s)
    }

    #[test]
    fn dates_in_file_names() {
        let cases: &[(&str, Option<CaptureTime>)] = &[
            ("IMG_20230626_143012.jpg", Some(t(2023, 6, 26, 14, 30, 12))),
            (
                "PXL_20230626_143012345.jpg",
                Some(t(2023, 6, 26, 14, 30, 12)),
            ),
            ("VID_20230626_143012.mp4", Some(t(2023, 6, 26, 14, 30, 12))),
            ("IMG-20230626-WA0001.jpg", Some(t(2023, 6, 26, 0, 0, 0))),
            ("2023-06-26 14.30.12.jpg", Some(t(2023, 6, 26, 14, 30, 12))),
            (
                "Screenshot_2023-06-26-14-30-12.png",
                Some(t(2023, 6, 26, 14, 30, 12)),
            ),
            ("1984-07-04 picnic.jpg", Some(t(1984, 7, 4, 0, 0, 0))),
            ("20230626143012.jpg", Some(t(2023, 6, 26, 14, 30, 12))),
            ("IMG_0042.JPG", None),
            ("DSC01234.JPG", None),
            ("P1020345.JPG", None),
            ("IMG_20230631_120000.jpg", None),
            ("19871234.jpg", None),
            ("photo_120230626.jpg", None),
            ("IMG_20230626123.jpg", None),
            ("vacation.20230626", None),
        ];
        for (name, want) in cases {
            assert_eq!(date_in_name(name.as_bytes()), *want, "{name}");
        }
    }

    #[test]
    fn sidecar_dates() {
        let attr = br#"<x:xmpmeta><rdf:RDF><rdf:Description xmp:CreateDate="2020-01-02T03:04:05"
            exif:DateTimeOriginal="2019-12-31T23:59:58+01:00"/></rdf:RDF></x:xmpmeta>"#;
        assert_eq!(xmp_sidecar_date(attr), Some(t(2019, 12, 31, 23, 59, 58)));
        let elem = b"<rdf:Description><photoshop:DateCreated>2018-05-06</photoshop:DateCreated></rdf:Description>";
        assert_eq!(xmp_sidecar_date(elem), Some(t(2018, 5, 6, 0, 0, 0)));
        assert_eq!(xmp_sidecar_date(b"<x:xmpmeta/>"), None);
        assert_eq!(
            parse_date_text("2023:06:26 14:30:12"),
            Some(t(2023, 6, 26, 14, 30, 12))
        );
        assert_eq!(parse_date_text("2023-02-30"), None);
    }

    fn facts(
        file: i64,
        name: &str,
        embedded: Option<(DateProvenance, CaptureTime)>,
    ) -> DateFacts<'_> {
        DateFacts {
            file: FileId(file),
            name: name.as_bytes(),
            embedded: embedded.map(|(provenance, value)| CaptureDate {
                value,
                provenance,
                undated: provenance == DateProvenance::FileMtime,
            }),
        }
    }

    #[test]
    fn resolution_order() {
        let dto = DateProvenance::DateTimeOriginal;
        let mtime = Some((DateProvenance::FileMtime, t(2024, 1, 1, 0, 0, 0)));
        let none = |_: &DateFacts<'_>| None;
        // Own EXIF wins over everything.
        let own = facts(
            1,
            "IMG_20200101_000000.jpg",
            Some((dto, t(2011, 8, 14, 10, 0, 0))),
        );
        let r = resolve(
            &own,
            &[facts(2, "b.jpg", Some((dto, t(2001, 1, 1, 0, 0, 0))))],
            none,
        );
        assert_eq!(
            (r.capture, r.origin),
            (Some(t(2011, 8, 14, 10, 0, 0)), DateOrigin::OWN_EXIF)
        );
        // Stripped keeper: another copy's EXIF, best provenance then earliest.
        let stripped = facts(1, "IMG_20200101_000000.jpg", mtime);
        let others = [
            facts(
                2,
                "a.jpg",
                Some((DateProvenance::CreateDate, t(2022, 1, 1, 0, 0, 0))),
            ),
            facts(3, "b.jpg", Some((dto, t(2023, 6, 26, 9, 0, 0)))),
            facts(4, "c.jpg", Some((dto, t(2023, 6, 27, 9, 0, 0)))),
        ];
        let r = resolve(&stripped, &others, none);
        assert_eq!(r.capture, Some(t(2023, 6, 26, 9, 0, 0)));
        assert!(!r.undated);
        assert_eq!(r.origin.copy, Some(FileId(3)));
        assert_eq!(r.origin.describe(), "DateTimeOriginal of another copy");
        // No embedded date anywhere: the keeper's file name, then a copy's.
        let r = resolve(
            &stripped,
            &[facts(2, "IMG_20190101_000000.jpg", mtime)],
            none,
        );
        assert_eq!(
            (r.capture, r.origin.copy),
            (Some(t(2020, 1, 1, 0, 0, 0)), None)
        );
        let plain = facts(1, "IMG_0001.jpg", mtime);
        let r = resolve(&plain, &[facts(2, "IMG_20190101_000000.jpg", mtime)], none);
        assert_eq!(
            r.origin,
            DateOrigin {
                source: DateSource::FileName,
                copy: Some(FileId(2))
            }
        );
        // Then sidecars (only asked for when needed), then the mtime.
        let mut asked = 0;
        let r = resolve(&plain, &[facts(2, "x.jpg", mtime)], |f| {
            asked += 1;
            (f.file == FileId(2)).then(|| t(2015, 3, 3, 0, 0, 0))
        });
        assert_eq!(asked, 2);
        assert_eq!(
            r.origin,
            DateOrigin {
                source: DateSource::Sidecar,
                copy: Some(FileId(2))
            }
        );
        let r = resolve(&plain, &[facts(2, "x.jpg", None)], none);
        assert!(r.undated);
        assert_eq!(r.origin, DateOrigin::MTIME);
        assert_eq!(r.capture, Some(t(2024, 1, 1, 0, 0, 0)));
    }
}
