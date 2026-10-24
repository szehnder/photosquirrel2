//! Capture-date rules (media-metadata spec; ADR-0022, ADR-0043, ADR-0069, ADR-0070, ADR-0071).
//!
//! One pure resolver, [`resolve_capture_date`], used by analysis ([`crate::extract_metadata`]), by the rescan
//! re-date pass (from stored facts, [`EmbeddedDates::from_stored`]) and by tests, so the three cannot disagree:
//!
//! 1. EXIF DateTimeOriginal, 2. EXIF CreateDate (both wall clocks, never shifted);
//! 3. the container creation date: a wall clock with its recorded offset (a keyed QuickTime date, whatever its
//!    offset, ADR-0081), or, when only the `mvhd` time at +00:00 is known, a UTC instant made local
//!    ([`LocalZone`]);
//! 4. a date in the name of a folder inside the source ([`date_in_folders`]), with its precision and the
//!    plausibility check against the file time;
//! 5. the file modification time, an instant made local, marked undated.
//!
//! Also the date parsers of file names and XMP text ([`date_in_name`], [`parse_date_text`]), which the export
//! uses for its filing order (ADR-0043). Instants become wall clocks with the offset the file records, else the
//! zone rules at that instant: [`FixedZone`], [`RuleZone`] (deterministic rules for tests) and, behind the
//! `os-zone` feature that only the I/O crates enable, [`os::OsZone`] (the machine's rules through `chrono`).

use ps_model::{
    CaptureDate, CaptureTime, DatePrecision, DateProvenance, Metadata, RelPath, civil_from_days,
    days_from_civil,
};

pub use ps_model::DATE_LOGIC_VERSION;

// ---- zones ---------------------------------------------------------------------------------------------------

/// The UTC offset rules of a time zone: the offset in minutes in force at a UTC instant.
pub trait LocalZone: Send + Sync {
    /// Minutes east of UTC at `utc_secs` (seconds since the Unix epoch).
    fn offset_at(&self, utc_secs: i64) -> i32;
}

/// One offset at every instant (UTC is `FixedZone(0)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedZone(pub i32);

impl LocalZone for FixedZone {
    fn offset_at(&self, _utc_secs: i64) -> i32 {
        self.0
    }
}

/// When a [`RuleZone`] observes daylight saving time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DstRule {
    /// United States since 2007: from the second Sunday of March 02:00 local standard time to the first Sunday
    /// of November 02:00 local daylight time.
    UnitedStates,
    /// European Union: from the last Sunday of March 01:00 UTC to the last Sunday of October 01:00 UTC.
    EuropeanUnion,
}

/// A zone with a standard and a daylight offset and a yearly daylight-saving window: deterministic rules for
/// tests and fixtures (America/Los_Angeles, America/New_York, Europe/Berlin), never the test machine's zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleZone {
    /// Minutes east of UTC outside daylight saving time.
    pub standard: i32,
    /// Minutes east of UTC during daylight saving time.
    pub daylight: i32,
    pub rule: DstRule,
}

impl RuleZone {
    /// America/Los_Angeles: -08:00, daylight -07:00.
    pub const LOS_ANGELES: RuleZone = RuleZone {
        standard: -480,
        daylight: -420,
        rule: DstRule::UnitedStates,
    };
    /// America/New_York: -05:00, daylight -04:00.
    pub const NEW_YORK: RuleZone = RuleZone {
        standard: -300,
        daylight: -240,
        rule: DstRule::UnitedStates,
    };
    /// Europe/Berlin: +01:00, daylight +02:00.
    pub const BERLIN: RuleZone = RuleZone {
        standard: 60,
        daylight: 120,
        rule: DstRule::EuropeanUnion,
    };

    /// Daylight saving time of year `y` as UTC seconds `[start, end)`.
    fn window(&self, y: i32) -> (i64, i64) {
        match self.rule {
            DstRule::UnitedStates => {
                let start = nth_sunday(y, 3, 2) * 86_400 + 2 * 3600 - i64::from(self.standard) * 60;
                let end = nth_sunday(y, 11, 1) * 86_400 + 2 * 3600 - i64::from(self.daylight) * 60;
                (start, end)
            }
            DstRule::EuropeanUnion => (
                last_sunday(y, 3) * 86_400 + 3600,
                last_sunday(y, 10) * 86_400 + 3600,
            ),
        }
    }
}

impl LocalZone for RuleZone {
    fn offset_at(&self, utc_secs: i64) -> i32 {
        let (y, _, _) = civil_from_days(utc_secs.div_euclid(86_400));
        let (start, end) = self.window(y);
        if (start..end).contains(&utc_secs) {
            self.daylight
        } else {
            self.standard
        }
    }
}

/// 0 = Sunday.
fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

/// Days since the epoch of the `n`-th Sunday of month `m` of year `y`.
fn nth_sunday(y: i32, m: u8, n: i64) -> i64 {
    let first = days_from_civil(y, m, 1);
    first + (7 - weekday(first)) % 7 + 7 * (n - 1)
}

/// Days since the epoch of the last Sunday of month `m` of year `y`.
fn last_sunday(y: i32, m: u8) -> i64 {
    let last = days_from_civil(y, m, days_in_month(y, m));
    last - weekday(last)
}

/// The machine's time-zone rules (ADR-0070), read through `chrono::Local`. Only the I/O crates enable the
/// `os-zone` feature, so core builds and core tests never consult the OS zone.
#[cfg(feature = "os-zone")]
pub mod os {
    use chrono::{DateTime, Offset, TimeZone};

    /// The operating system's local zone. Created once per scan run and per export plan.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct OsZone;

    impl super::LocalZone for OsZone {
        fn offset_at(&self, utc_secs: i64) -> i32 {
            if !zone_known() {
                fallback("the machine's time zone is not set (no TZ and no /etc/localtime)");
            }
            // chrono falls back to UTC when the zone cannot be determined: today's behaviour.
            DateTime::from_timestamp(utc_secs, 0).map_or_else(
                || {
                    fallback("an instant outside the supported range");
                    0
                },
                |t| {
                    chrono::Local
                        .offset_from_utc_datetime(&t.naive_utc())
                        .fix()
                        .local_minus_utc()
                        / 60
                },
            )
        }
    }

    static FALLBACK: std::sync::Once = std::sync::Once::new();

    /// Offset 0 is used instead of the machine's rules: logged once per process (ADR-0070).
    fn fallback(why: &str) {
        FALLBACK.call_once(|| {
            log::warn!(
                "local time zone unavailable ({why}); file times are read as UTC (offset 0)"
            );
        });
    }

    /// The fallback was logged in this process.
    pub fn fallback_logged() -> bool {
        FALLBACK.is_completed()
    }

    /// Whether the OS can tell chrono the local zone. On Unix chrono reads `TZ`, else `/etc/localtime`, and
    /// silently uses UTC when neither exists; elsewhere the OS always answers.
    fn zone_known() -> bool {
        static KNOWN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *KNOWN.get_or_init(|| {
            !cfg!(unix)
                || std::env::var_os("TZ").is_some_and(|v| !v.is_empty())
                || std::path::Path::new("/etc/localtime").exists()
        })
    }
}

#[cfg(all(test, feature = "os-zone"))]
mod os_tests {
    use super::LocalZone;
    use chrono::{DateTime, Local, Offset, TimeZone};

    /// Smoke test on the build machine: the OS zone agrees with `chrono::Local` for three instants.
    #[test]
    fn os_zone_agrees_with_chrono_local() {
        for secs in [0i64, 1_562_297_400, 1_547_537_400] {
            let t = DateTime::from_timestamp(secs, 0).expect("instant");
            let want = Local
                .offset_from_utc_datetime(&t.naive_utc())
                .fix()
                .local_minus_utc()
                / 60;
            assert_eq!(super::os::OsZone.offset_at(secs), want, "{secs}");
        }
    }

    /// An instant chrono cannot represent falls back to offset 0, and the fallback is logged (once).
    #[test]
    fn os_zone_fallback_is_offset_zero_and_logged() {
        assert_eq!(super::os::OsZone.offset_at(i64::MAX), 0);
        assert!(super::os::fallback_logged());
        assert_eq!(super::os::OsZone.offset_at(i64::MAX), 0);
    }
}

/// The wall clock of the instant `utc_secs` with `offset` minutes.
fn local_of(utc_secs: i64, millis: u16, offset: i32) -> CaptureTime {
    CaptureTime {
        local_seconds: utc_secs + i64::from(offset) * 60,
        millis,
        offset_minutes: i16::try_from(offset).ok(),
    }
}

// ---- the resolver --------------------------------------------------------------------------------------------

/// The dates a file records itself, as analysis extracted them (or as rebuilt from the stored metadata).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmbeddedDates {
    /// EXIF DateTimeOriginal (wall clock, with OffsetTimeOriginal when recorded).
    pub original: Option<CaptureTime>,
    /// EXIF CreateDate.
    pub create: Option<CaptureTime>,
    /// QuickTime/MP4 creation date: a wall clock with its offset; +00:00 is a UTC instant (`mvhd`) unless
    /// `container_keyed`.
    pub container: Option<CaptureTime>,
    /// `container` is the keyed `com.apple.quicktime.creationdate`: a wall clock with a real offset, never
    /// shifted, even at +00:00 (ADR-0081).
    pub container_keyed: bool,
    /// An offset the file records without a matching date (EXIF `OffsetTime`, `OffsetTimeOriginal`,
    /// `OffsetTimeDigitized`), applied to instants before the local zone (ADR-0070).
    pub recorded_offset: Option<i16>,
}

impl EmbeddedDates {
    /// The embedded capture time grouping and ranking read: original, else create, else container (as recorded).
    pub fn capture_time(&self) -> Option<CaptureTime> {
        self.original.or(self.create).or(self.container)
    }

    /// Rebuild the inputs from stored metadata JSON (`fields.capture_time`, the provenance of
    /// `capture_date`, the `OffsetTime*` tags), without reading the file (ADR-0071).
    pub fn from_stored(m: &Metadata) -> EmbeddedDates {
        let mut e = EmbeddedDates {
            recorded_offset: recorded_offset(&m.fields.all_tags),
            ..EmbeddedDates::default()
        };
        if let Some(t) = m.fields.capture_time {
            match m.capture_date.map(|d| d.provenance) {
                Some(DateProvenance::CreateDate) => e.create = Some(t),
                Some(DateProvenance::Container) => {
                    e.container = Some(t);
                    e.container_keyed = m.fields.all_tags.iter().any(|(g, n, v)| {
                        g == "QuickTime" && n == CREATION_DATE_SOURCE_TAG && v == "keyed"
                    });
                }
                // DateTimeOriginal, or a capture time without a matching provenance (treated as the original,
                // as the export always has).
                _ => e.original = Some(t),
            }
        }
        e
    }
}

/// The QuickTime tag naming where a video's creation date came from: `keyed` (`com.apple.quicktime.creationdate`)
/// or `mvhd` (ADR-0081). Rows analysed before it read as `mvhd`.
pub const CREATION_DATE_SOURCE_TAG: &str = "CreationDateSource";

/// The tags [`EmbeddedDates::from_stored`] reads (by name or code): what a catalog must return of a stored tag
/// list for re-dating (`ps_catalog::Readers::date_facts`).
pub const STORED_DATE_TAGS: &[&str] = &[
    "OffsetTime",
    "0x9010",
    "OffsetTimeOriginal",
    "0x9011",
    "OffsetTimeDigitized",
    "0x9012",
    CREATION_DATE_SOURCE_TAG,
];

/// The offset recorded in the tag list: `OffsetTime`, else `OffsetTimeOriginal`, else `OffsetTimeDigitized`
/// (by name, or by code for tags the RAW walker lists).
pub fn recorded_offset(tags: &[(String, String, String)]) -> Option<i16> {
    [
        ("OffsetTime", "0x9010"),
        ("OffsetTimeOriginal", "0x9011"),
        ("OffsetTimeDigitized", "0x9012"),
    ]
    .iter()
    .find_map(|(name, code)| {
        tags.iter()
            .filter(|(_, n, _)| n == name || n == code)
            .find_map(|(_, _, v)| crate::util::parse_offset(v.trim().trim_matches('"')))
    })
}

/// The inputs of the resolver besides the file's own dates.
#[derive(Clone, Copy)]
pub struct DateContext<'a> {
    /// The file's path relative to its source root (folder dates come only from inside the source).
    pub rel_path: Option<&'a RelPath>,
    /// The zone that makes instants local.
    pub zone: &'a dyn LocalZone,
}

impl std::fmt::Debug for DateContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DateContext")
            .field("rel_path", &self.rel_path)
            .finish_non_exhaustive()
    }
}

/// 1990-01-01T00:00:00: file times before it are an unset clock and never veto a folder date (ADR-0069).
const PLAUSIBLE_FROM: i64 = 631_152_000;

/// The one capture date of a file (module docs). `mtime_unix_secs` is the file modification time (floor of
/// seconds since the epoch). `None` only when there is no date at all.
pub fn resolve_capture_date(
    embedded: &EmbeddedDates,
    rel_path: Option<&RelPath>,
    mtime_unix_secs: Option<i64>,
    zone: &dyn LocalZone,
) -> Option<CaptureDate> {
    let dated = |value, provenance, precision| CaptureDate {
        value,
        provenance,
        undated: false,
        precision,
    };
    if let Some(t) = embedded.original {
        return Some(dated(
            t,
            DateProvenance::DateTimeOriginal,
            DatePrecision::Second,
        ));
    }
    if let Some(t) = embedded.create {
        return Some(dated(t, DateProvenance::CreateDate, DatePrecision::Second));
    }
    let instant = |utc: i64, millis: u16| {
        let off = embedded
            .recorded_offset
            .map_or_else(|| zone.offset_at(utc), i32::from);
        local_of(utc, millis, off)
    };
    if let Some(t) = embedded.container {
        let value = if t.offset_minutes == Some(0) && !embedded.container_keyed {
            instant(t.local_seconds, t.millis)
        } else {
            t
        };
        return Some(dated(
            value,
            DateProvenance::Container,
            DatePrecision::Second,
        ));
    }
    let mtime = mtime_unix_secs.map(|s| instant(s, 0));
    if let Some((t, precision)) = rel_path.and_then(date_in_folders) {
        let plausible = mtime.is_none_or(|m| {
            mtime_unix_secs.is_some_and(|s| s < PLAUSIBLE_FROM)
                || m.local_seconds >= t.local_seconds - 86_400
        });
        if plausible {
            return Some(dated(t, DateProvenance::FolderName, precision));
        }
    }
    mtime.map(|value| CaptureDate {
        value,
        provenance: DateProvenance::FileMtime,
        undated: true,
        precision: DatePrecision::Second,
    })
}

// ---- folder names ----------------------------------------------------------------------------------------------

/// The date in the name of a folder holding the file, nearest folder first (ADR-0069): a component that starts
/// with `YYYY-MM-DD`, `YYYY_MM_DD`, `YYYY.MM.DD` (1900-2099) or `YYYYMMDD` (1990-2099) followed by the end or a
/// non-digit gives a day; one that starts with `YYYY-MM`, `YYYY_MM` or `YYYY.MM` gives a month; consecutive
/// components `YYYY/MM/DD` or `YYYY/MM` (each optionally followed by a space and text) give a day or a month.
/// Year-only folders are never a date. Only `rel_path`'s folders count (never the source root).
pub fn date_in_folders(rel_path: &RelPath) -> Option<(CaptureTime, DatePrecision)> {
    let comps: Vec<&[u8]> = rel_path.as_bytes().split(|b| *b == b'/').collect();
    let folders = &comps[..comps.len().saturating_sub(1)];
    (0..folders.len()).rev().find_map(|i| {
        let parent = i.checked_sub(1).map(|p| folders[p]);
        start_date(folders[i], parent).or_else(|| chain_date(&folders[..=i]))
    })
}

fn date_value(y: i32, m: u8, d: u8, precision: DatePrecision) -> (CaptureTime, DatePrecision) {
    (CaptureTime::from_parts(y, m, d, 0, 0, 0), precision)
}

/// A date at the start of a folder name. `parent` is the folder holding it inside the source, if any (it tells a
/// month folder `2010/2010-11` from a range of two years, ADR-0083).
fn start_date(name: &[u8], parent: Option<&[u8]>) -> Option<(CaptureTime, DatePrecision)> {
    let y = num(name, 0, 4)? as i32;
    let ends = |i: usize| !digit_at(name, i);
    match name.get(4) {
        Some(&sep) if b"-_.".contains(&sep) => {
            let m = num(name, 5, 2)? as u8;
            if !(1900..=2099).contains(&y) || !(1..=12).contains(&m) {
                return None;
            }
            if name.get(7) == Some(&sep) && digit_at(name, 8) {
                // A day follows the month: it must be a valid two-digit day, else this is no date.
                let d = num(name, 8, 2)? as u8;
                return (ends(10) && d >= 1 && d <= days_in_month(y, m))
                    .then(|| date_value(y, m, d, DatePrecision::Day));
            }
            // `2011-12 School Year`, `2008_09 Season`: a range of two years, not December 2011 (ADR-0080). Only
            // with text after it and not inside a folder of that year: a bare `2010-11`, or `2010/2010-11 Trip`,
            // is November 2010 (ADR-0083).
            let in_its_year = parent.and_then(|p| chain_part(p, 4)) == Some(y as u32);
            let school_year = i32::from(m) == (y + 1) % 100 && name.len() > 7 && !in_its_year;
            (ends(7) && !school_year).then(|| date_value(y, m, 1, DatePrecision::Month))
        }
        _ => {
            let m = num(name, 4, 2)? as u8;
            let d = num(name, 6, 2)? as u8;
            let ok = (1990..=2099).contains(&y)
                && (1..=12).contains(&m)
                && d >= 1
                && d <= days_in_month(y, m)
                && ends(8);
            ok.then(|| date_value(y, m, d, DatePrecision::Day))
        }
    }
}

/// The number a whole chain component holds: `digits` digits (one or two when `digits == 2`), optionally
/// followed by a space and text (`07 July`).
fn chain_part(c: &[u8], digits: usize) -> Option<u32> {
    let n = c.iter().take_while(|b| b.is_ascii_digit()).count();
    let len_ok = if digits == 4 {
        n == 4
    } else {
        (1..=2).contains(&n)
    };
    if !len_ok || !(n == c.len() || (c[n] == b' ' && c.len() > n + 1)) {
        return None;
    }
    num(c, 0, n)
}

/// A `YYYY/MM/DD` or `YYYY/MM` chain ending at the last of `folders`.
fn chain_date(folders: &[&[u8]]) -> Option<(CaptureTime, DatePrecision)> {
    let year = |c: &[u8]| {
        chain_part(c, 4)
            .map(|y| y as i32)
            .filter(|y| (1900..=2099).contains(y))
    };
    let month = |c: &[u8]| {
        chain_part(c, 2)
            .map(|m| m as u8)
            .filter(|m| (1..=12).contains(m))
    };
    let n = folders.len();
    if n >= 3
        && let (Some(y), Some(m), Some(d)) = (
            year(folders[n - 3]),
            month(folders[n - 2]),
            chain_part(folders[n - 1], 2),
        )
    {
        let d = d as u8;
        if d >= 1 && d <= days_in_month(y, m) {
            return Some(date_value(y, m, d, DatePrecision::Day));
        }
    }
    if n >= 2
        && let (Some(y), Some(m)) = (year(folders[n - 2]), month(folders[n - 1]))
    {
        return Some(date_value(y, m, 1, DatePrecision::Month));
    }
    None
}

// ---- file names and XMP text (moved from ps-export, ADR-0043) ------------------------------------------------

pub fn days_in_month(y: i32, m: u8) -> u8 {
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
    date_in_name_precise(name).map(|(t, _)| t)
}

/// [`date_in_name`] with its precision: [`DatePrecision::Day`] when the name gives no time (`IMG-20230626-WA0001`),
/// else [`DatePrecision::Second`].
pub fn date_in_name_precise(name: &[u8]) -> Option<(CaptureTime, DatePrecision)> {
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

fn date_at(b: &[u8], i: usize) -> Option<(CaptureTime, DatePrecision)> {
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
    let (secs, precision) = match time {
        // Milliseconds may follow a compact time; any other digit run makes it a counter, not a time.
        Some((secs, _)) => (secs, DatePrecision::Second),
        None if digit_at(b, j) => return None,
        None => (0, DatePrecision::Day),
    };
    let base = CaptureTime::from_parts(y, m, d, 0, 0, 0);
    Some((
        CaptureTime {
            local_seconds: base.local_seconds + i64::from(secs),
            ..base
        },
        precision,
    ))
}

/// `YYYY-MM-DD[THH:MM[:SS]]` or `YYYY:MM:DD HH:MM:SS` text (XMP and EXIF date values). Zone suffixes are
/// ignored: the wall clock is what the library files by.
pub fn parse_date_text(s: &str) -> Option<CaptureTime> {
    parse_date_text_precise(s).map(|(t, _)| t)
}

/// [`parse_date_text`] with its precision: [`DatePrecision::Day`] for a date without a time (`2018-05-06`).
pub fn parse_date_text_precise(s: &str) -> Option<(CaptureTime, DatePrecision)> {
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
    let mut precision = DatePrecision::Day;
    if matches!(b.get(10), Some(b'T' | b' ')) {
        precision = DatePrecision::Second;
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
    Some((
        CaptureTime {
            local_seconds: base.local_seconds + i64::from(secs),
            ..base
        },
        precision,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(y: i32, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> CaptureTime {
        CaptureTime::from_parts(y, mo, d, h, mi, s)
    }

    fn rel(s: &str) -> RelPath {
        RelPath::from_bytes(s.as_bytes().to_vec()).expect("rel path")
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
    fn date_text() {
        assert_eq!(
            parse_date_text("2023:06:26 14:30:12"),
            Some(t(2023, 6, 26, 14, 30, 12))
        );
        assert_eq!(parse_date_text("2023-02-30"), None);
        assert_eq!(
            parse_date_text("2019-12-31T23:59:58+01:00"),
            Some(t(2019, 12, 31, 23, 59, 58))
        );
    }

    #[test]
    fn folder_patterns_table() {
        use DatePrecision::{Day, Month};
        let day = |y, m, d| Some((t(y, m, d, 0, 0, 0), Day));
        let month = |y, m| Some((t(y, m, 1, 0, 0, 0), Month));
        let cases: &[(&str, Option<(CaptureTime, DatePrecision)>)] = &[
            ("2019-07-04/a.jpg", day(2019, 7, 4)),
            ("2019_07_04 Beach/a.jpg", day(2019, 7, 4)),
            ("2019.07.04/a.jpg", day(2019, 7, 4)),
            ("20190704/a.jpg", day(2019, 7, 4)),
            ("20190704 Party/a.jpg", day(2019, 7, 4)),
            ("2019/07/04/a.jpg", day(2019, 7, 4)),
            ("2019/7/4/a.jpg", day(2019, 7, 4)),
            ("2019/07 July/04/a.jpg", day(2019, 7, 4)),
            ("2019-07 Italy/a.jpg", month(2019, 7)),
            ("2019_07/a.jpg", month(2019, 7)),
            ("2019/07/a.jpg", month(2019, 7)),
            ("2019/07 July/a.jpg", month(2019, 7)),
            ("2019/a.jpg", None),
            ("Backup 2019-07-04/a.jpg", None),
            ("DSC_19871234/a.jpg", None),
            ("100CANON/a.jpg", None),
            ("19871234/a.jpg", None),
            ("2019-02-30/a.jpg", None),
            ("201907041/a.jpg", None),
            ("2019-07-041/a.jpg", None),
            ("2019-07-4 Beach/a.jpg", None),
            ("2019-07- Beach/a.jpg", month(2019, 7)),
            ("2019-13/a.jpg", None),
            ("1850-07-04/a.jpg", None),
            ("2019/13/a.jpg", None),
            ("2019/07/32/a.jpg", month(2019, 7)),
            ("2019-07-04.jpg", None),
            ("a.jpg", None),
            ("2018/2019-07-04 Beach/raw/IMG_0001.JPG", day(2019, 7, 4)),
            ("2019-07-04/2020-01 Later/a.jpg", month(2020, 1)),
        ];
        for (path, want) in cases {
            assert_eq!(date_in_folders(&rel(path)), *want, "{path}");
        }
        // Folder names need not be Unicode.
        let raw = RelPath::from_bytes(b"2019-07-04 \xff\xfe/a.jpg".to_vec()).expect("raw");
        assert_eq!(date_in_folders(&raw), day(2019, 7, 4));
    }

    #[test]
    fn rule_zone_switches_daylight_saving() {
        let la = RuleZone::LOS_ANGELES;
        let utc = |y, m, d, h, mi| t(y, m, d, h, mi, 0).local_seconds;
        // 2019: DST from 10 March 10:00 UTC to 3 November 09:00 UTC.
        assert_eq!(la.offset_at(utc(2019, 3, 10, 9, 59)), -480);
        assert_eq!(la.offset_at(utc(2019, 3, 10, 10, 0)), -420);
        assert_eq!(la.offset_at(utc(2019, 7, 5, 3, 30)), -420);
        assert_eq!(la.offset_at(utc(2019, 11, 3, 8, 59)), -420);
        assert_eq!(la.offset_at(utc(2019, 11, 3, 9, 0)), -480);
        assert_eq!(la.offset_at(utc(2019, 1, 15, 7, 30)), -480);
        let ny = RuleZone::NEW_YORK;
        assert_eq!(ny.offset_at(utc(2019, 7, 5, 2, 0)), -240);
        assert_eq!(ny.offset_at(utc(2019, 12, 5, 2, 0)), -300);
        let be = RuleZone::BERLIN;
        // 2019: 31 March 01:00 UTC to 27 October 01:00 UTC.
        assert_eq!(be.offset_at(utc(2019, 3, 31, 0, 59)), 60);
        assert_eq!(be.offset_at(utc(2019, 3, 31, 1, 0)), 120);
        assert_eq!(be.offset_at(utc(2019, 10, 27, 0, 59)), 120);
        assert_eq!(be.offset_at(utc(2019, 10, 27, 1, 0)), 60);
        assert_eq!(FixedZone(330).offset_at(0), 330);
    }

    #[test]
    fn recorded_offsets_from_tags() {
        let tag = |n: &str, v: &str| ("Exif".to_string(), n.to_string(), v.to_string());
        assert_eq!(
            recorded_offset(&[tag("OffsetTimeOriginal", "+09:00")]),
            Some(540)
        );
        assert_eq!(
            recorded_offset(&[
                tag("OffsetTimeOriginal", "+09:00"),
                tag("OffsetTime", "-05:00")
            ]),
            Some(-300)
        );
        assert_eq!(recorded_offset(&[tag("0x9012", "\"+01:00\"")]), Some(60));
        assert_eq!(recorded_offset(&[tag("Make", "+01:00")]), None);
    }
}
