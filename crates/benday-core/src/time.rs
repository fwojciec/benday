//! Civil-date math and strict ISO parsing for the temporal field type.
//!
//! A temporal value is one f64: milliseconds since the Unix epoch, naive.
//! Conversion between civil dates and epoch days uses Howard Hinnant's
//! `days_from_civil` / `civil_from_days` algorithms. No timezone database,
//! no locale, no clock — this module never calls `now()`.
//!
//! Nothing outside this module reads these functions yet — the compiler wires
//! them in later tasks of the temporal family — so the item lints are relaxed
//! for the non-test build.
#![cfg_attr(not(test), allow(dead_code))]

const MS_PER_DAY: i64 = 86_400_000;

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
pub(crate) fn days_from_civil(y: i64, m: u64, d: u64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe as i64 - 719468
}

/// The inverse of `days_from_civil`: the civil `(year, month, day)` for a
/// given count of days since 1970-01-01.
pub(crate) fn civil_from_days(z: i64) -> (i64, u64, u64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + (m <= 2) as i64, m, d)
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u64) -> u64 {
    debug_assert!((1..=12).contains(&m), "month {m} out of range");
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Read `n` ASCII digits at `start`; `None` if any are missing or non-digit.
fn digits(b: &[u8], start: usize, n: usize) -> Option<u64> {
    if start + n > b.len() {
        return None;
    }
    let mut val = 0u64;
    for &c in &b[start..start + n] {
        if !c.is_ascii_digit() {
            return None;
        }
        val = val * 10 + (c - b'0') as u64;
    }
    Some(val)
}

/// Parse a zero-padded `YYYY-MM-DD` at the start of `b`, returning epoch days
/// and the index past the date. The day is range-checked against the month.
fn parse_date(b: &[u8]) -> Option<(i64, usize)> {
    let y = digits(b, 0, 4)? as i64;
    if b.get(4) != Some(&b'-') {
        return None;
    }
    let m = digits(b, 5, 2)?;
    if b.get(7) != Some(&b'-') {
        return None;
    }
    let d = digits(b, 8, 2)?;
    if !(1..=12).contains(&m) || !(1..=days_in_month(y, m)).contains(&d) {
        return None;
    }
    Some((days_from_civil(y, m, d), 10))
}

/// Parse a zero-padded `HH:MM:SS` at `i`, returning milliseconds-of-day and
/// the index past the time.
fn parse_time(b: &[u8], i: usize) -> Option<(i64, usize)> {
    let h = digits(b, i, 2)?;
    if b.get(i + 2) != Some(&b':') {
        return None;
    }
    let min = digits(b, i + 3, 2)?;
    if b.get(i + 5) != Some(&b':') {
        return None;
    }
    let s = digits(b, i + 6, 2)?;
    if h > 23 || min > 59 || s > 59 {
        return None;
    }
    Some(((h * 3600 + min * 60 + s) as i64 * 1000, i + 8))
}

/// Parse a `.fff` fraction (1 to 3 digits) at `i` into milliseconds.
fn parse_fraction(b: &[u8], i: usize) -> Option<(i64, usize)> {
    let start = i + 1; // skip '.'
    let mut j = start;
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    let n = j - start;
    if n == 0 || n > 3 {
        return None;
    }
    let mut frac = 0i64;
    for &c in &b[start..j] {
        frac = frac * 10 + (c - b'0') as i64;
    }
    // Scale to milliseconds: 1 digit -> tenths, 2 -> hundredths, 3 -> exact.
    let scale = match n {
        1 => 100,
        2 => 10,
        _ => 1,
    };
    Some((frac * scale, j))
}

/// Parse an offset (`Z` or `±hh:mm`) at `i`, returning its signed millisecond
/// value and the index past it.
fn parse_offset(b: &[u8], i: usize) -> Option<(i64, usize)> {
    match b.get(i) {
        Some(b'Z') => Some((0, i + 1)),
        Some(&c @ (b'+' | b'-')) => {
            let sign = if c == b'+' { 1 } else { -1 };
            let h = digits(b, i + 1, 2)?;
            if b.get(i + 3) != Some(&b':') {
                return None;
            }
            let m = digits(b, i + 4, 2)?;
            if h > 23 || m > 59 {
                return None;
            }
            Some((sign * (h * 60 + m) as i64 * 60_000, i + 6))
        }
        _ => None,
    }
}

/// Parse one temporal value to milliseconds since the Unix epoch (naive UTC;
/// an explicit offset is applied, then discarded). `None` means not temporal.
pub(crate) fn parse_temporal(s: &str) -> Option<f64> {
    let b = s.as_bytes();

    // A date opens the string when a '-' sits where `YYYY-MM-DD` needs it;
    // otherwise the whole value is a time-of-day anchored to epoch day zero.
    let (days, mut i) = if b.len() >= 10 && b[4] == b'-' {
        let (days, mut i) = parse_date(b)?;
        if i == b.len() {
            return Some((days * MS_PER_DAY) as f64); // bare date
        }
        // A datetime: 'T', or the space DuckDB/BigQuery emit, joins the parts.
        match b[i] {
            b'T' | b' ' => i += 1,
            _ => return None,
        }
        (days, i)
    } else {
        (0, 0)
    };

    let (mut ms, ni) = parse_time(b, i)?;
    i = ni;
    if i < b.len() && b[i] == b'.' {
        let (frac, ni) = parse_fraction(b, i)?;
        ms += frac;
        i = ni;
    }
    let mut offset = 0;
    if i < b.len() {
        let (off, ni) = parse_offset(b, i)?;
        offset = off;
        i = ni;
    }
    if i != b.len() {
        return None; // trailing junk
    }
    Some((days * MS_PER_DAY + ms - offset) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent reference calendar for the exhaustive walk (the leap rule
    /// spelled out per the design, so the test does not lean on the module's
    /// own `days_in_month`).
    fn ref_days_in_month(y: i64, m: u64) -> u64 {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => unreachable!("month {m} out of range"),
        }
    }

    #[test]
    fn civil_epoch_round_trip_exhaustive() {
        let start = days_from_civil(1600, 1, 1);
        let end = days_from_civil(2400, 12, 31);
        let (mut y, mut m, mut d) = (1600i64, 1u64, 1u64);
        for z in start..=end {
            // civil_from_days inverts days_from_civil, and each step walks the
            // reference calendar by exactly one day.
            assert_eq!(civil_from_days(z), (y, m, d), "civil_from_days({z})");
            assert_eq!(days_from_civil(y, m, d), z, "round-trip at {z}");
            d += 1;
            if d > ref_days_in_month(y, m) {
                d = 1;
                m += 1;
                if m > 12 {
                    m = 1;
                    y += 1;
                }
            }
        }
    }

    #[test]
    fn leap_and_month_length_edges() {
        assert!(parse_temporal("2000-02-29").is_some()); // divisible by 400: leap
        assert!(parse_temporal("1900-02-29").is_none()); // by 100 not 400: common
        assert!(parse_temporal("2024-02-29").is_some()); // divisible by 4: leap
        assert!(parse_temporal("2026-02-29").is_none()); // common year
        assert!(parse_temporal("2024-01-31").is_some()); // January has 31
        assert!(parse_temporal("2024-04-31").is_none()); // April has 30
    }

    #[test]
    fn parse_table_maps_to_millis() {
        const DAY: i64 = 86_400_000;
        let date = days_from_civil(2026, 7, 5) * DAY; // 2026-07-05T00:00
        let noon = 14 * 3_600_000 + 30 * 60_000; // 14:30:00 -> 52_200_000 ms
        assert_eq!(parse_temporal("2026-07-05"), Some(date as f64));
        assert_eq!(
            parse_temporal("2026-07-05T14:30:00"),
            Some((date + noon) as f64)
        );
        // Space separator: DuckDB and BigQuery emit it.
        assert_eq!(
            parse_temporal("2026-07-05 14:30:00"),
            Some((date + noon) as f64)
        );
        assert_eq!(
            parse_temporal("2026-07-05T14:30:00.123"),
            Some((date + noon + 123) as f64)
        );
        // Short fractions scale to milliseconds: .1 is tenths, .12 hundredths.
        assert_eq!(
            parse_temporal("2026-07-05T14:30:00.1"),
            Some((date + noon + 100) as f64)
        );
        assert_eq!(
            parse_temporal("2026-07-05T14:30:00.12"),
            Some((date + noon + 120) as f64)
        );
        assert_eq!(
            parse_temporal("2026-07-05T14:30:00Z"),
            Some((date + noon) as f64)
        );
        // Offsets in either direction land on the same UTC instant as the
        // Z case at 14:30.
        assert_eq!(
            parse_temporal("2026-07-05T12:30:00-02:00"),
            Some((date + noon) as f64)
        );
        assert_eq!(
            parse_temporal("2026-07-05T16:30:00+02:00"),
            Some((date + noon) as f64)
        );
        // Time-only anchors to epoch day zero.
        assert_eq!(parse_temporal("14:30:00"), Some(noon as f64));
    }

    #[test]
    fn rejects_non_iso_shapes() {
        for s in [
            "2026/07/05",               // wrong separator
            "07-05-2026",               // wrong field order
            "2026-7-5",                 // not zero-padded
            "2026-13-01",               // month out of range
            "2026-07-05T25:00:00",      // hour out of range
            "2026-07-05T14:60:00",      // minute out of range
            "2026-07-05T14:30:60",      // second out of range
            "2026-07-05T14:30:00.1234", // fraction longer than millis
            "2026-07-05x",              // trailing junk after a valid date
            "hello",                    // not a date at all
            "",                         // empty
        ] {
            assert_eq!(parse_temporal(s), None, "expected {s:?} to be rejected");
        }
    }
}
