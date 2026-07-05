//! Civil-date math, strict ISO parsing, and calendar-ladder ticks for the
//! temporal field type.
//!
//! A temporal value is one f64: milliseconds since the Unix epoch, naive.
//! Conversion between civil dates and epoch days uses Howard Hinnant's
//! `days_from_civil` / `civil_from_days` algorithms. No timezone database,
//! no locale, no clock — this module never calls `now()`.

const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_HOUR: i64 = 3_600_000;
const MS_PER_MIN: i64 = 60_000;
const MS_PER_SEC: i64 = 1_000;

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

/// Render an epoch-millis instant as an ISO string for `--meta`: a bare date
/// at midnight, `T`-joined datetime otherwise, with `.fff` only when the
/// millisecond remainder is non-zero. The inverse shape of `parse_temporal`
/// on the values the temporal axis actually reports (calendar boundaries or
/// true data extremes), so a caller reading `--meta` sees back what it fed in.
pub(crate) fn format_iso(ms: f64) -> String {
    let ms = ms as i64;
    let (y, m, d) = civil_from_days(ms.div_euclid(MS_PER_DAY));
    let rem = ms.rem_euclid(MS_PER_DAY);
    let (hh, mi, ss, milli) = (
        rem / MS_PER_HOUR,
        rem / MS_PER_MIN % 60,
        rem / MS_PER_SEC % 60,
        rem % MS_PER_SEC,
    );
    if rem == 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else if milli == 0 {
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mi:02}:{ss:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mi:02}:{ss:02}.{milli:03}")
    }
}

/// The label class of a ladder rung: which delta form its ticks show and
/// which rollover restores full context.
#[derive(Clone, Copy)]
enum Unit {
    Sec,
    Min,
    Hour,
    /// Days and weeks share the `Jun 12` idiom.
    Day,
    Month,
    Quarter,
    Year,
}

/// One rung of the calendar ladder: knows how to floor a timestamp to its
/// boundary and how to step a boundary to the next.
#[derive(Clone, Copy)]
enum Rung {
    /// Fixed width in ms, seconds through days. Every width divides a day,
    /// so flooring raw epoch millis lands on the civil boundary.
    Fixed(i64, Unit),
    /// Monday-anchored weeks.
    Week,
    /// `n`-month steps (1 = months, 3 = quarters), stepped on the CIVIL
    /// form: fixed-width ms is wrong across month lengths.
    Months(i64, Unit),
    /// `n`-year steps on the civil form (leap years); `n` walks the 1/2/5
    /// ladder without a cap.
    Years(i64),
}

/// Finest to coarsest below years; year rungs continue inside
/// `temporal_axis` itself, since they are unbounded. To add a rung: touch
/// LADDER + `Unit` + `tick_label` + `Rung::unit`; `Fixed` widths must
/// divide a day (a 2d rung would float off calendar boundaries).
const LADDER: [Rung; 16] = [
    Rung::Fixed(MS_PER_SEC, Unit::Sec),
    Rung::Fixed(5 * MS_PER_SEC, Unit::Sec),
    Rung::Fixed(15 * MS_PER_SEC, Unit::Sec),
    Rung::Fixed(30 * MS_PER_SEC, Unit::Sec),
    Rung::Fixed(MS_PER_MIN, Unit::Min),
    Rung::Fixed(5 * MS_PER_MIN, Unit::Min),
    Rung::Fixed(15 * MS_PER_MIN, Unit::Min),
    Rung::Fixed(30 * MS_PER_MIN, Unit::Min),
    Rung::Fixed(MS_PER_HOUR, Unit::Hour),
    Rung::Fixed(3 * MS_PER_HOUR, Unit::Hour),
    Rung::Fixed(6 * MS_PER_HOUR, Unit::Hour),
    Rung::Fixed(12 * MS_PER_HOUR, Unit::Hour),
    Rung::Fixed(MS_PER_DAY, Unit::Day),
    Rung::Week,
    Rung::Months(1, Unit::Month),
    Rung::Months(3, Unit::Quarter),
];

/// A rung is sensible from three ticks up. Two reasons: a coarse two-tick
/// rung expands the domain to boundaries far outside the data (six years
/// shown on a ten-year axis) and domain inflation is a geometric lie; and
/// three is the only threshold under which width-starved plots reach the
/// first-and-last fallback instead of landing on such a rung.
const MIN_TICKS: usize = 3;

impl Rung {
    fn unit(self) -> Unit {
        match self {
            Rung::Fixed(_, u) | Rung::Months(_, u) => u,
            Rung::Week => Unit::Day,
            Rung::Years(_) => Unit::Year,
        }
    }

    /// Floor a timestamp to this rung's calendar boundary.
    fn floor(self, ms: i64) -> i64 {
        match self {
            Rung::Fixed(w, _) => ms.div_euclid(w) * w,
            Rung::Week => {
                // Epoch day 0 is a Thursday: (z + 3) mod 7 is 0 on Mondays.
                let z = ms.div_euclid(MS_PER_DAY);
                (z - (z + 3).rem_euclid(7)) * MS_PER_DAY
            }
            Rung::Months(n, _) => {
                let (y, m, _) = civil_from_days(ms.div_euclid(MS_PER_DAY));
                month_start((y * 12 + m as i64 - 1).div_euclid(n) * n)
            }
            Rung::Years(n) => {
                let (y, _, _) = civil_from_days(ms.div_euclid(MS_PER_DAY));
                days_from_civil(y.div_euclid(n) * n, 1, 1) * MS_PER_DAY
            }
        }
    }

    /// Step a boundary (a value `floor` returned) to the next one.
    fn next(self, ms: i64) -> i64 {
        match self {
            Rung::Fixed(w, _) => ms + w,
            Rung::Week => ms + 7 * MS_PER_DAY,
            Rung::Months(n, _) => {
                let (y, m, _) = civil_from_days(ms.div_euclid(MS_PER_DAY));
                month_start(y * 12 + m as i64 - 1 + n)
            }
            Rung::Years(n) => {
                let (y, _, _) = civil_from_days(ms.div_euclid(MS_PER_DAY));
                days_from_civil(y + n, 1, 1) * MS_PER_DAY
            }
        }
    }
}

/// Midnight opening month index `m0` (years * 12 + zero-based month).
fn month_start(m0: i64) -> i64 {
    days_from_civil(m0.div_euclid(12), (m0.rem_euclid(12) + 1) as u64, 1) * MS_PER_DAY
}

/// Boundaries from `floor(min)` through the first one at or past `max`.
/// `None` once the count exceeds `cap` — the rung cannot fit anyway.
fn rung_ticks(rung: Rung, min: i64, max: i64, cap: usize) -> Option<Vec<i64>> {
    let mut t = rung.floor(min);
    let mut out = vec![t];
    while t < max {
        t = rung.next(t);
        out.push(t);
        if out.len() > cap {
            return None;
        }
    }
    Some(out)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Civil parts of a timestamp: (year, month, day, hour, minute, second).
fn parts(ms: i64) -> (i64, u64, u64, i64, i64, i64) {
    let (y, m, d) = civil_from_days(ms.div_euclid(MS_PER_DAY));
    let s = ms.rem_euclid(MS_PER_DAY) / 1000;
    (y, m, d, s / 3600, s / 60 % 60, s % 60)
}

/// Context + delta labeling: the delta form shows only what the step
/// changes; full context appears where `prev` is None (the first tick, and
/// fallback endpoints) and at each rollover — a new civil day for sub-day
/// units, a new year for day-and-coarser.
fn tick_label(unit: Unit, ms: i64, prev: Option<i64>) -> String {
    let (y, mo, d, hh, mi, ss) = parts(ms);
    let month = MONTHS[mo as usize - 1];
    let yy = y.rem_euclid(100);
    let new_day = prev.is_none_or(|p| p.div_euclid(MS_PER_DAY) != ms.div_euclid(MS_PER_DAY));
    let new_year = prev.is_none_or(|p| parts(p).0 != y);
    match unit {
        Unit::Sec => {
            let t = format!("{hh:02}:{mi:02}:{ss:02}");
            if new_day {
                format!("{month} {d} {t}")
            } else {
                t
            }
        }
        Unit::Min | Unit::Hour => {
            let t = format!("{hh:02}:{mi:02}");
            if new_day {
                format!("{month} {d} {t}")
            } else {
                t
            }
        }
        Unit::Day => {
            if new_year {
                format!("{month} {d} '{yy:02}")
            } else {
                format!("{month} {d}")
            }
        }
        Unit::Month => {
            if new_year {
                format!("{month} '{yy:02}")
            } else {
                month.to_string()
            }
        }
        Unit::Quarter => {
            let q = (mo - 1) / 3 + 1;
            if new_year {
                format!("Q{q} '{yy:02}")
            } else {
                format!("Q{q}")
            }
        }
        Unit::Year => y.to_string(),
    }
}

/// Invariant the wiring leans on: `ticks` is ordered and spans the domain
/// exactly — `ticks[0].0 == domain[0]` and `ticks[last].0 == domain[1]`, in
/// the fallback too (trivially, for a degenerate single tick). The compile
/// side maps ticks to columns with `compile::x_col` over a Linear built from
/// this domain, which reproduces `accept`'s column arithmetic only because
/// of this; it is a contract, not a coincidence.
pub(crate) struct TemporalAxis {
    /// Expanded to the enclosing boundaries (tight data extent under the
    /// first-and-last fallback), in epoch ms. min == max yields the
    /// zero-width domain [x, x]: consumers must guard before feeding it to
    /// `Linear` (norm would be NaN), as the quantitative path already does
    /// for degenerate spans.
    pub domain: [f64; 2],
    /// (position ms, label), positions on true calendar boundaries.
    pub ticks: Vec<(f64, String)>,
}

/// Label the rung's ticks and test them under the same greedy rule
/// `place_x_labels` applies downstream (gutter 0, width = plot_w): each
/// label centered on its column, clamped inside the plot, one column of
/// separation. The column formula below is `compile::x_col` with the norm
/// inlined over this rung's own domain — keep the two in lockstep. Any
/// collision rejects the whole RUNG, so the compile-side placement never
/// drops a label this function accepted.
fn accept(rung: Rung, ticks: &[i64], plot_w: usize) -> Option<TemporalAxis> {
    let (lo, hi) = (ticks[0], ticks[ticks.len() - 1]);
    let span = (hi - lo) as f64;
    let mut labeled = Vec::with_capacity(ticks.len());
    let mut next_free = 0usize;
    let mut prev = None;
    for &t in ticks {
        let label = tick_label(rung.unit(), t, prev);
        prev = Some(t);
        let len = label.chars().count();
        if len > plot_w {
            return None;
        }
        let col =
            ((((t - lo) as f64 / span) * (plot_w - 1) as f64).round() as usize).min(plot_w - 1);
        let start = (1 + col).saturating_sub(len / 2).min(plot_w - len);
        if start < next_free {
            return None;
        }
        next_free = start + len + 1;
        labeled.push((t as f64, label));
    }
    Some(TemporalAxis {
        domain: [lo as f64, hi as f64],
        ticks: labeled,
    })
}

/// A temporal x axis: ticks on true calendar boundaries with context+delta
/// labels, domain expanded outward to the enclosing boundaries. Walks the
/// ladder finest to coarsest and accepts the first rung whose labels all
/// fit `plot_w`; when none does, falls back to full-context first-and-last
/// labels over the TIGHT data domain — the temporal twin of the linear
/// two-endpoint fallback.
///
/// Callers must pass millis produced by `parse_temporal` (four-digit years,
/// so |ms| <= ~2.6e14); synthetic astronomic millis (>= ~6.3e18) can
/// overflow i64 in the year walk's closing boundary.
pub(crate) fn temporal_axis(min_ms: f64, max_ms: f64, plot_w: usize) -> TemporalAxis {
    let (min, max) = (min_ms as i64, max_ms as i64);
    // n labels need at least 2n - 1 columns (each at least one char plus a
    // column of separation), so a rung emitting more than plot_w / 2 + 1
    // ticks can never fit — generation bails early.
    let cap = plot_w / 2 + 1;
    // A cap under MIN_TICKS admits no acceptable rung at all — and the year
    // walk below would coarsen forever chasing a tick count the cap cannot
    // let through: straight to the fallback.
    if cap < MIN_TICKS {
        return fallback(min_ms, max_ms);
    }
    for rung in LADDER {
        let Some(ticks) = rung_ticks(rung, min, max, cap) else {
            continue;
        };
        if ticks.len() < MIN_TICKS {
            continue;
        }
        if let Some(axis) = accept(rung, &ticks, plot_w) {
            return axis;
        }
    }
    // Year rungs coarsen 1 -> 2 -> 5 -> 10 ... without a cap: a rung that
    // overflows generation just coarsens (over a many-century span the
    // fitting rung may still be far up the ladder). Termination: the tick
    // count shrinks toward 2 as the step widens, and 2 passes the
    // generation cap (cap >= MIN_TICKS here), so a `Some` under MIN_TICKS
    // always arrives — and proves every coarser rung is under it too.
    let mut n = 1;
    loop {
        match rung_ticks(Rung::Years(n), min, max, cap) {
            Some(ticks) if ticks.len() < MIN_TICKS => break,
            Some(ticks) => {
                if let Some(axis) = accept(Rung::Years(n), &ticks, plot_w) {
                    return axis;
                }
            }
            None => {}
        }
        n = next_year_step(n);
    }
    fallback(min_ms, max_ms)
}

/// First-and-last fallback: ticks at the true data extremes, each with a
/// full context label sized to the span — one tick when they coincide. The
/// domain stays TIGHT; no boundary expansion.
fn fallback(min_ms: f64, max_ms: f64) -> TemporalAxis {
    let (min, max) = (min_ms as i64, max_ms as i64);
    let unit = if max - min >= MS_PER_DAY {
        Unit::Day
    } else if max - min >= MS_PER_MIN {
        Unit::Min
    } else {
        Unit::Sec
    };
    let mut ticks = vec![(min_ms, tick_label(unit, min, None))];
    if max > min {
        ticks.push((max_ms, tick_label(unit, max, None)));
    }
    TemporalAxis {
        domain: [min_ms, max_ms],
        ticks,
    }
}

/// The next year step up the 1/2/5 ladder (1 -> 2 -> 5 -> 10 -> 20 ...).
fn next_year_step(n: i64) -> i64 {
    let mut pow = 1;
    while n >= 10 * pow {
        pow *= 10;
    }
    match n / pow {
        1 => 2 * pow,
        2 => 5 * pow,
        _ => 10 * pow,
    }
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

    /// ISO-render a millisecond position through the module's own civil
    /// math, so expected positions in the pins below are derived strings —
    /// never hand-computed epoch millis. Midnight renders as a bare date.
    fn iso(ms: f64) -> String {
        let ms = ms as i64;
        let (y, m, d) = civil_from_days(ms.div_euclid(MS_PER_DAY));
        let s = ms.rem_euclid(MS_PER_DAY) / 1000;
        if s == 0 {
            format!("{y:04}-{m:02}-{d:02}")
        } else {
            format!(
                "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
                s / 3600,
                s / 60 % 60,
                s % 60
            )
        }
    }

    /// Run `temporal_axis` over ISO endpoints; ISO-ify the ticks and domain.
    fn axis(min: &str, max: &str, plot_w: usize) -> (Vec<(String, String)>, [String; 2]) {
        let ax = temporal_axis(
            parse_temporal(min).unwrap(),
            parse_temporal(max).unwrap(),
            plot_w,
        );
        (
            ax.ticks.into_iter().map(|(t, l)| (iso(t), l)).collect(),
            [iso(ax.domain[0]), iso(ax.domain[1])],
        )
    }

    /// Whole-output pin: every (position, label) pair in order, plus the
    /// returned domain.
    fn assert_axis(
        got: (Vec<(String, String)>, [String; 2]),
        want_ticks: &[(&str, &str)],
        want_domain: [&str; 2],
    ) {
        let ticks: Vec<(&str, &str)> = got
            .0
            .iter()
            .map(|(p, l)| (p.as_str(), l.as_str()))
            .collect();
        assert_eq!(ticks, want_ticks);
        assert_eq!(got.1, want_domain);
    }

    #[test]
    fn temporal_axis_week_steps_expand_domain_to_mondays() {
        // Jun 3..Jun 27 2026 at week steps: the domain expands outward to
        // the surrounding Mondays (2026-06-01 is a Monday), and the first
        // tick carries the year context.
        assert_axis(
            axis("2026-06-03", "2026-06-27", 72),
            &[
                ("2026-06-01", "Jun 1 '26"),
                ("2026-06-08", "Jun 8"),
                ("2026-06-15", "Jun 15"),
                ("2026-06-22", "Jun 22"),
                ("2026-06-29", "Jun 29"),
            ],
            ["2026-06-01", "2026-06-29"],
        );
    }

    #[test]
    fn temporal_axis_quarter_steps_year_rollover() {
        // Two years of monthly data: quarter steps, year context at the
        // first tick and again at each year rollover.
        assert_axis(
            axis("2025-01-01", "2026-12-01", 72),
            &[
                ("2025-01-01", "Q1 '25"),
                ("2025-04-01", "Q2"),
                ("2025-07-01", "Q3"),
                ("2025-10-01", "Q4"),
                ("2026-01-01", "Q1 '26"),
                ("2026-04-01", "Q2"),
                ("2026-07-01", "Q3"),
                ("2026-10-01", "Q4"),
                ("2027-01-01", "Q1 '27"),
            ],
            ["2025-01-01", "2027-01-01"],
        );
    }

    #[test]
    fn temporal_axis_six_hour_steps_day_rollover() {
        // 24 hours starting on a 6h boundary: date context at the first
        // tick and again when the civil day rolls over at midnight.
        assert_axis(
            axis("2026-06-14T06:00:00", "2026-06-15T06:00:00", 72),
            &[
                ("2026-06-14T06:00:00", "Jun 14 06:00"),
                ("2026-06-14T12:00:00", "12:00"),
                ("2026-06-14T18:00:00", "18:00"),
                ("2026-06-15", "Jun 15 00:00"),
                ("2026-06-15T06:00:00", "06:00"),
            ],
            ["2026-06-14T06:00:00", "2026-06-15T06:00:00"],
        );
    }

    #[test]
    fn temporal_axis_thirty_second_steps() {
        // One minute of data: the 15-char context label "Jun 14 14:30:00"
        // crowds out 15s steps even at 72 columns, so the ladder lands on
        // 30s — pinning the seconds delta form.
        assert_axis(
            axis("2026-06-14T14:30:00", "2026-06-14T14:31:00", 72),
            &[
                ("2026-06-14T14:30:00", "Jun 14 14:30:00"),
                ("2026-06-14T14:30:30", "14:30:30"),
                ("2026-06-14T14:31:00", "14:31:00"),
            ],
            ["2026-06-14T14:30:00", "2026-06-14T14:31:00"],
        );
    }

    #[test]
    fn temporal_axis_thirty_minute_steps() {
        // Ninety minutes of data: 15m steps collide with the 12-char
        // context label at 72 columns, so the ladder lands on 30m —
        // pinning the minutes delta form.
        assert_axis(
            axis("2026-06-14T14:30:00", "2026-06-14T16:00:00", 72),
            &[
                ("2026-06-14T14:30:00", "Jun 14 14:30"),
                ("2026-06-14T15:00:00", "15:00"),
                ("2026-06-14T15:30:00", "15:30"),
                ("2026-06-14T16:00:00", "16:00"),
            ],
            ["2026-06-14T14:30:00", "2026-06-14T16:00:00"],
        );
    }

    #[test]
    fn temporal_axis_century_span_coarsens_past_cap() {
        // Eight hundred years: every year rung through 20y exceeds the
        // generation cap, and the walk must keep coarsening past those —
        // never fall back — until 100y fits. No year cap; only the
        // three-tick rule ends the ladder.
        assert_axis(
            axis("1600-01-01", "2400-01-01", 72),
            &[
                ("1600-01-01", "1600"),
                ("1700-01-01", "1700"),
                ("1800-01-01", "1800"),
                ("1900-01-01", "1900"),
                ("2000-01-01", "2000"),
                ("2100-01-01", "2100"),
                ("2200-01-01", "2200"),
                ("2300-01-01", "2300"),
                ("2400-01-01", "2400"),
            ],
            ["1600-01-01", "2400-01-01"],
        );
    }

    #[test]
    fn temporal_axis_degenerate_widths_fall_back() {
        // Widths whose cap admits fewer than MIN_TICKS ticks can accept no
        // rung: straight to the first-and-last fallback — pinned down to
        // width zero, where the year walk once coarsened forever.
        for w in [0, 1, 2] {
            assert_axis(
                axis("2026-06-03", "2026-06-27", w),
                &[("2026-06-03", "Jun 3 '26"), ("2026-06-27", "Jun 27 '26")],
                ["2026-06-03", "2026-06-27"],
            );
        }
    }

    #[test]
    fn temporal_axis_single_instant_dedupes_fallback() {
        // min == max: one fallback tick, not two identical ones.
        assert_axis(
            axis("2026-06-14T14:30:00", "2026-06-14T14:30:00", 12),
            &[("2026-06-14T14:30:00", "Jun 14 14:30:00")],
            ["2026-06-14T14:30:00", "2026-06-14T14:30:00"],
        );
    }

    #[test]
    fn temporal_axis_year_steps() {
        // Six years: plain year labels, no context prefix to roll over.
        assert_axis(
            axis("2024-01-01", "2030-01-01", 72),
            &[
                ("2024-01-01", "2024"),
                ("2025-01-01", "2025"),
                ("2026-01-01", "2026"),
                ("2027-01-01", "2027"),
                ("2028-01-01", "2028"),
                ("2029-01-01", "2029"),
                ("2030-01-01", "2030"),
            ],
            ["2024-01-01", "2030-01-01"],
        );
    }

    #[test]
    fn temporal_axis_three_months_daily_coarsens_to_months() {
        // Coarsening regression pin: over three months even week steps
        // collide at 72 columns (the 9-char context label "Jun 1 '26"
        // overlaps its 5-columns-away neighbor), so the ladder lands on
        // months.
        assert_axis(
            axis("2026-06-01", "2026-08-31", 72),
            &[
                ("2026-06-01", "Jun '26"),
                ("2026-07-01", "Jul"),
                ("2026-08-01", "Aug"),
                ("2026-09-01", "Sep"),
            ],
            ["2026-06-01", "2026-09-01"],
        );
    }

    #[test]
    fn temporal_axis_thirty_six_hours_coarsens_to_twelve_hours() {
        // Coarsening regression pin: over 36 hours the 12-char context
        // label "Jun 14 06:00" makes 6h steps collide at 72 columns, so
        // the ladder lands on 12h.
        assert_axis(
            axis("2026-06-14T06:00:00", "2026-06-15T18:00:00", 72),
            &[
                ("2026-06-14", "Jun 14 00:00"),
                ("2026-06-14T12:00:00", "12:00"),
                ("2026-06-15", "Jun 15 00:00"),
                ("2026-06-15T12:00:00", "12:00"),
                ("2026-06-16", "Jun 16 00:00"),
            ],
            ["2026-06-14", "2026-06-16"],
        );
    }

    #[test]
    fn temporal_axis_narrow_plot_coarsens() {
        // At 30 columns each range climbs the ladder further — and the week
        // range finds NO rung with three fitting ticks (months over it give
        // only two), so it falls back to first-and-last over the tight
        // data domain.
        assert_axis(
            axis("2026-06-03", "2026-06-27", 30),
            &[("2026-06-03", "Jun 3 '26"), ("2026-06-27", "Jun 27 '26")],
            ["2026-06-03", "2026-06-27"],
        );
        assert_axis(
            axis("2025-01-01", "2026-12-01", 30),
            &[
                ("2025-01-01", "2025"),
                ("2026-01-01", "2026"),
                ("2027-01-01", "2027"),
            ],
            ["2025-01-01", "2027-01-01"],
        );
        assert_axis(
            axis("2026-06-14T06:00:00", "2026-06-15T06:00:00", 30),
            &[
                ("2026-06-14", "Jun 14 '26"),
                ("2026-06-15", "Jun 15"),
                ("2026-06-16", "Jun 16"),
            ],
            ["2026-06-14", "2026-06-16"],
        );
        assert_axis(
            axis("2024-01-01", "2030-01-01", 30),
            &[
                ("2024-01-01", "2024"),
                ("2026-01-01", "2026"),
                ("2028-01-01", "2028"),
                ("2030-01-01", "2030"),
            ],
            ["2024-01-01", "2030-01-01"],
        );
    }

    #[test]
    fn temporal_axis_tiny_plot_first_and_last_fallback() {
        // At 12 columns no rung fits any of the four ranges: first-and-last
        // ticks at the true data extremes, each with a FULL context label,
        // and the domain stays tight (no boundary expansion).
        assert_axis(
            axis("2026-06-03", "2026-06-27", 12),
            &[("2026-06-03", "Jun 3 '26"), ("2026-06-27", "Jun 27 '26")],
            ["2026-06-03", "2026-06-27"],
        );
        assert_axis(
            axis("2025-01-01", "2026-12-01", 12),
            &[("2025-01-01", "Jan 1 '25"), ("2026-12-01", "Dec 1 '26")],
            ["2025-01-01", "2026-12-01"],
        );
        assert_axis(
            axis("2026-06-14T06:00:00", "2026-06-15T06:00:00", 12),
            &[
                ("2026-06-14T06:00:00", "Jun 14 '26"),
                ("2026-06-15T06:00:00", "Jun 15 '26"),
            ],
            ["2026-06-14T06:00:00", "2026-06-15T06:00:00"],
        );
        assert_axis(
            axis("2024-01-01", "2030-01-01", 12),
            &[("2024-01-01", "Jan 1 '24"), ("2030-01-01", "Jan 1 '30")],
            ["2024-01-01", "2030-01-01"],
        );
    }

    #[test]
    fn temporal_axis_sub_day_fallback_minute_context() {
        // A sub-day span that fits no rung labels its fallback endpoints in
        // the minute-datetime form.
        assert_axis(
            axis("2026-06-14T14:30:00", "2026-06-14T18:45:00", 12),
            &[
                ("2026-06-14T14:30:00", "Jun 14 14:30"),
                ("2026-06-14T18:45:00", "Jun 14 18:45"),
            ],
            ["2026-06-14T14:30:00", "2026-06-14T18:45:00"],
        );
    }

    #[test]
    fn temporal_axis_sub_minute_fallback_second_context() {
        // A sub-minute span keeps the seconds in its fallback labels.
        assert_axis(
            axis("2026-06-14T14:30:05", "2026-06-14T14:30:45", 12),
            &[
                ("2026-06-14T14:30:05", "Jun 14 14:30:05"),
                ("2026-06-14T14:30:45", "Jun 14 14:30:45"),
            ],
            ["2026-06-14T14:30:05", "2026-06-14T14:30:45"],
        );
    }

    #[test]
    fn format_iso_round_trips_the_shapes() {
        // format_iso is the meta-facing inverse of parse_temporal across the
        // three render shapes: bare date at midnight, T-joined datetime, and
        // datetime with a millisecond remainder.
        for s in [
            "2026-07-05",
            "2026-07-05T14:30:00",
            "2026-07-05T14:30:00.123",
            "1600-01-01",
            "2400-12-31T23:59:59",
        ] {
            let ms = parse_temporal(s).expect("parses");
            assert_eq!(format_iso(ms), s, "round-trip {s}");
        }
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
