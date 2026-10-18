//! Small shared helpers: EXIF date parsing, panic containment.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ps_model::CaptureTime;

/// Seconds between the QuickTime epoch (1904-01-01) and the Unix epoch.
pub const MAC_EPOCH_OFFSET: i64 = 2_082_844_800;

/// Run third-party parsing/decoding so that a panic inside it becomes `None` instead of taking the scan
/// down. Our own code is written to be panic-free; this is defence in depth for dependencies.
pub fn contain<T>(f: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}

/// Parse an EXIF date `YYYY:MM:DD HH:MM:SS` (also accepts `-` separators and a `T`).
pub fn parse_exif_datetime(s: &str) -> Option<CaptureTime> {
    let b = s.trim().as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = std::str::from_utf8(b.get(r)?).ok()?;
        if !t.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        t.parse().ok()
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if y == 0 || !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(CaptureTime::from_parts(
        y as i32,
        mo as u8,
        d as u8,
        h as u8,
        mi as u8,
        se.min(59) as u8,
    ))
}

/// Parse an EXIF offset `+HH:MM` / `-HH:MM` into minutes.
pub fn parse_offset(s: &str) -> Option<i16> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 6 || b[3] != b':' {
        return None;
    }
    let sign = match b[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let h: i16 = s.get(1..3)?.parse().ok()?;
    let m: i16 = s.get(4..6)?.parse().ok()?;
    if h > 14 || m > 59 {
        return None;
    }
    Some(sign * (h * 60 + m))
}

/// Milliseconds from an EXIF sub-second string ("5" = 500 ms, "123" = 123 ms).
pub fn parse_subsec(s: &str) -> Option<u16> {
    let digits: String = s
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .take(3)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let v: u16 = digits.parse().ok()?;
    Some(match digits.len() {
        1 => v * 100,
        2 => v * 10,
        _ => v,
    })
}

/// A capture time for a UTC Unix timestamp (offset +00:00).
pub fn capture_time_from_unix(secs: i64) -> CaptureTime {
    CaptureTime {
        local_seconds: secs,
        millis: 0,
        offset_minutes: Some(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_and_reject() {
        let t = parse_exif_datetime("2010:05:01 10:00:00").expect("date");
        assert_eq!(t.display(), "2010-05-01 10:00:00");
        assert!(parse_exif_datetime("0000:00:00 00:00:00").is_none());
        assert!(parse_exif_datetime("    :  :     :  :  ").is_none());
        assert!(parse_exif_datetime("2010").is_none());
        assert_eq!(parse_offset("+02:00"), Some(120));
        assert_eq!(parse_offset("-07:30"), Some(-450));
        assert_eq!(parse_subsec("5"), Some(500));
        assert_eq!(parse_subsec("123456"), Some(123));
        assert_eq!(contain(|| 1), Some(1));
    }
}
