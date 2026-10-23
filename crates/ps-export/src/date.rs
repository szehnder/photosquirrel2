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
//! 5. a date in the name of a folder holding a copy (the catalog's `folder-name` date, ADR-0069), keeper
//!    first, with its precision (a month folder files under `<YYYY>/<YYYY-MM>/`);
//! 6. the keeper's modification time (its local day, ADR-0070), filed under `Undated/`.
//!
//! Every planned file records where its date came from ([`DateOrigin`]); the plan, the CLI and the manifest
//! show it.

use ps_model::{CaptureDate, CaptureTime, DatePrecision, DateProvenance, FileId};
use serde::Serialize;

/// The file-name and date-text parsers live with the other date rules in `ps-media` (re-exported here).
pub use ps_media::date::{
    date_in_name, date_in_name_precise, parse_date_text, parse_date_text_precise,
};

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
    /// A date in the name of a folder holding the file (ADR-0069).
    FolderName,
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
            DateSource::FolderName => "folder name".into(),
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
    /// The catalog's resolved capture date (embedded, folder name, or `undated` when it is only the file
    /// time).
    pub embedded: Option<CaptureDate>,
}

impl DateFacts<'_> {
    fn real_embedded(&self) -> Option<(DateProvenance, CaptureTime)> {
        self.embedded
            .filter(|c| !c.undated && c.is_embedded())
            .map(|c| (c.provenance, c.value))
    }

    fn folder_date(&self) -> Option<CaptureDate> {
        self.embedded
            .filter(|c| c.provenance == DateProvenance::FolderName)
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
    /// Second, day or month: a month-precision date files under `<YYYY>/<YYYY-MM>/` (ADR-0069).
    pub precision: DatePrecision,
}

/// The best filing date for `keeper` given the other copies of its group (module docs). `sidecar` reads a
/// copy's XMP sidecar date; it is called only when no embedded or file-name date exists.
pub fn resolve(
    keeper: &DateFacts<'_>,
    others: &[DateFacts<'_>],
    mut sidecar: impl FnMut(&DateFacts<'_>) -> Option<(CaptureTime, DatePrecision)>,
) -> FilingDate {
    let dated_at = |capture, source, copy, precision| FilingDate {
        capture: Some(capture),
        undated: false,
        origin: DateOrigin { source, copy },
        precision,
    };
    let dated = |capture, source, copy| dated_at(capture, source, copy, DatePrecision::Second);
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
    // A name or sidecar date without a time is a day, not midnight.
    if let Some(((t, p), f)) = order
        .clone()
        .find_map(|o| date_in_name_precise(o.name).map(|t| (t, o.file)))
    {
        return dated_at(t, DateSource::FileName, copy_of(f), p);
    }
    if let Some(((t, p), f)) = order.clone().find_map(|o| sidecar(o).map(|t| (t, o.file))) {
        return dated_at(t, DateSource::Sidecar, copy_of(f), p);
    }
    if let Some((d, f)) = order
        .into_iter()
        .find_map(|o| o.folder_date().map(|d| (d, o.file)))
    {
        return FilingDate {
            precision: d.precision,
            ..dated(d.value, DateSource::FolderName, copy_of(f))
        };
    }
    FilingDate {
        // The keeper's own file time (its local day, from the catalog); `None` lets the layout use `mtime_ns`.
        capture: keeper
            .embedded
            .filter(|c| c.provenance == DateProvenance::FileMtime)
            .map(|c| c.value),
        undated: true,
        origin: DateOrigin::MTIME,
        precision: DatePrecision::Second,
    }
}

/// The capture date recorded in an XMP sidecar: `exif:DateTimeOriginal`, else `photoshop:DateCreated`, else
/// `xmp:CreateDate`, as an attribute (`exif:DateTimeOriginal="…"`) or an element (`<exif:DateTimeOriginal>…`).
pub fn xmp_sidecar_date(xml: &[u8]) -> Option<CaptureTime> {
    xmp_sidecar_date_precise(xml).map(|(t, _)| t)
}

/// [`xmp_sidecar_date`] with its precision (a date-only value is [`DatePrecision::Day`]).
pub fn xmp_sidecar_date_precise(xml: &[u8]) -> Option<(CaptureTime, DatePrecision)> {
    let text = String::from_utf8_lossy(xml);
    [
        "exif:DateTimeOriginal",
        "photoshop:DateCreated",
        "xmp:CreateDate",
    ]
    .iter()
    .find_map(|key| xmp_value(&text, key).and_then(|v| parse_date_text_precise(&v)))
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
                precision: if provenance == DateProvenance::FolderName {
                    DatePrecision::Day
                } else {
                    DatePrecision::Second
                },
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
            (f.file == FileId(2)).then(|| (t(2015, 3, 3, 0, 0, 0), DatePrecision::Second))
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

    #[test]
    fn folder_dates_rank_below_names_and_sidecars() {
        let folder = |d| Some((DateProvenance::FolderName, t(2019, 7, d, 0, 0, 0)));
        let mtime = Some((DateProvenance::FileMtime, t(2024, 1, 1, 0, 0, 0)));
        let none = |_: &DateFacts<'_>| None;
        // The keeper's own folder date before a copy's.
        let keeper = facts(1, "IMG_0001.jpg", folder(4));
        let r = resolve(&keeper, &[facts(2, "b.jpg", folder(5))], none);
        assert_eq!(r.capture, Some(t(2019, 7, 4, 0, 0, 0)));
        assert_eq!(r.origin, DateOrigin::own(DateSource::FolderName));
        assert_eq!(r.origin.describe(), "folder name");
        assert_eq!(r.precision, DatePrecision::Day);
        assert!(!r.undated);
        // A copy's folder date files a keeper dated only by its file time.
        let plain = facts(1, "IMG_0001.jpg", mtime);
        let r = resolve(&plain, &[facts(2, "b.jpg", folder(5))], none);
        assert_eq!(r.capture, Some(t(2019, 7, 5, 0, 0, 0)));
        assert_eq!(r.origin.copy, Some(FileId(2)));
        assert_eq!(r.origin.describe(), "folder name of another copy");
        // A file name or a sidecar of any copy wins over every folder.
        let r = resolve(&keeper, &[facts(2, "IMG_20180101_120000.jpg", mtime)], none);
        assert_eq!(r.origin.source, DateSource::FileName);
        let r = resolve(&keeper, &[facts(2, "b.jpg", mtime)], |f| {
            (f.file == FileId(2)).then(|| (t(2017, 1, 1, 0, 0, 0), DatePrecision::Second))
        });
        assert_eq!(r.origin.source, DateSource::Sidecar);
        // An embedded date of any copy wins over the keeper's folder.
        let r = resolve(
            &keeper,
            &[facts(
                2,
                "b.jpg",
                Some((DateProvenance::DateTimeOriginal, t(2019, 7, 3, 21, 0, 0))),
            )],
            none,
        );
        assert_eq!(r.capture, Some(t(2019, 7, 3, 21, 0, 0)));
        assert_eq!(r.precision, DatePrecision::Second);
    }

    /// Gate review (task 8.8): a file-name date without a time and a date-only XMP value are days.
    #[test]
    fn file_name_and_date_only_xmp_dates_have_day_precision() {
        let mtime = Some((DateProvenance::FileMtime, t(2024, 1, 1, 0, 0, 0)));
        let none = |_: &DateFacts<'_>| None;
        let r = resolve(&facts(1, "IMG-20230626-WA0001.jpg", mtime), &[], none);
        assert_eq!(r.origin.source, DateSource::FileName);
        assert_eq!(r.capture, Some(t(2023, 6, 26, 0, 0, 0)));
        assert_eq!(r.precision, DatePrecision::Day);
        assert_eq!(r.precision.iso(&r.capture.unwrap()), "2023-06-26");
        let r = resolve(&facts(1, "IMG_20230626_143012.jpg", mtime), &[], none);
        assert_eq!(r.precision, DatePrecision::Second);
        let elem = b"<rdf:Description><photoshop:DateCreated>2018-05-06</photoshop:DateCreated></rdf:Description>";
        let r = resolve(&facts(1, "x.jpg", mtime), &[], |_| {
            xmp_sidecar_date_precise(elem)
        });
        assert_eq!(r.origin.source, DateSource::Sidecar);
        assert_eq!(r.precision, DatePrecision::Day);
        let attr = br#"<rdf:Description xmp:CreateDate="2020-01-02T03:04:05"/>"#;
        assert_eq!(
            xmp_sidecar_date_precise(attr),
            Some((t(2020, 1, 2, 3, 4, 5), DatePrecision::Second))
        );
    }
}
