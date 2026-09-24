//! Time without dependencies.
//!
//! The performance pillar puts writing a node at p99 < 5 ms, and the security
//! one wants few dependencies to audit. Formatting a date justifies neither:
//! it is thirty lines of arithmetic.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since epoch. This is what goes inside the ULID.
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(not(test))]
fn now_secs() -> u64 {
    unix_millis() / 1000
}

#[cfg(test)]
fn now_secs() -> u64 {
    TICKING.with(|t| match t.get() {
        Some(secs) => {
            t.set(Some(secs + 1));
            secs
        }
        None => unix_millis() / 1000,
    })
}

#[cfg(test)]
thread_local! {
    static TICKING: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// `f590`: while one of these is alive, every event stamp read on this
/// thread is one second later than the one before, starting at `secs` since
/// the epoch. A write that reads the clock twice then shows every time,
/// instead of only when the second happens to turn between the two reads.
#[cfg(test)]
pub struct Ticking;

#[cfg(test)]
impl Ticking {
    pub fn start(secs: u64) -> Ticking {
        TICKING.with(|t| t.set(Some(secs)));
        Ticking
    }
}

#[cfg(test)]
impl Drop for Ticking {
    fn drop(&mut self) {
        TICKING.with(|t| t.set(None));
    }
}

/// Instant in UTC, RFC 3339 with seconds. This is what goes in the event.
pub fn now_rfc3339() -> String {
    let secs = now_secs();
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// The first ten characters of an RFC 3339 stamp, for display.
pub fn date_of(ts: &str) -> &str {
    if ts.len() >= 10 {
        &ts[..10]
    } else {
        ts
    }
}

/// Whole days from `from` to `to`, both RFC 3339 stamps, or `None` if either
/// is not one. Only the date part counts: two stamps on the same day are zero
/// days apart however many hours separate them, which is what "how long since
/// it moved" means to a person looking at a list of projects.
///
/// Here rather than in the caller because `civil_from_days` already lives in
/// this module and its inverse is eight lines of the same arithmetic. The
/// alternative was a date crate, which the module header already refuses.
pub fn days_between(from: &str, to: &str) -> Option<i64> {
    Some(days_from_civil(parse_date(to)?) - days_from_civil(parse_date(from)?))
}

/// `YYYY-MM-DD`, from the front of an RFC 3339 stamp. Anything else is `None`:
/// a log written by hand is not a reason to render a wrong number.
fn parse_date(ts: &str) -> Option<(i64, u32, u32)> {
    let d = date_of(ts);
    let b = d.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    Some((
        d[0..4].parse().ok()?,
        d[5..7].parse().ok()?,
        d[8..10].parse().ok()?,
    ))
}

/// Seconds since the epoch, from a full RFC 3339 stamp with seconds --
/// `now_rfc3339`'s own shape. `None` for anything shorter or differently
/// punctuated: `session prompt` (`d779`) compares two of these and a wrong
/// number is worse than no nudge at all.
///
/// `days_between` already has the date half of this; what it throws away is
/// the position within the day, which a stretch measured in minutes cannot
/// do without.
pub fn epoch_seconds(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let y: i64 = ts[0..4].parse().ok()?;
    let m: u32 = ts[5..7].parse().ok()?;
    let d: u32 = ts[8..10].parse().ok()?;
    let hh: i64 = ts[11..13].parse().ok()?;
    let mm: i64 = ts[14..16].parse().ok()?;
    let ss: i64 = ts[17..19].parse().ok()?;
    Some(days_from_civil((y, m, d)) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// The inverse of [`civil_from_days`], same source.
fn days_from_civil((y, m, d): (i64, u32, u32)) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's algorithm: days since epoch to proleptic Gregorian civil
/// date. Valid for any date, not just the 32-bit range.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_apart_is_one_day() {
        assert_eq!(
            days_between("2026-09-07T23:59:00Z", "2026-09-08T00:01:00Z"),
            Some(1)
        );
    }

    #[test]
    fn the_same_day_is_zero_however_many_hours_apart() {
        assert_eq!(
            days_between("2026-09-08T00:01:00Z", "2026-09-08T23:59:00Z"),
            Some(0)
        );
    }

    #[test]
    fn a_month_boundary_counts_the_real_days() {
        // February 2026 is not a leap year: the 28th is the last day.
        assert_eq!(
            days_between("2026-02-27T00:00:00Z", "2026-03-01T00:00:00Z"),
            Some(2)
        );
    }

    #[test]
    fn a_leap_day_counts() {
        assert_eq!(
            days_between("2024-02-28T00:00:00Z", "2024-03-01T00:00:00Z"),
            Some(2)
        );
    }

    #[test]
    fn going_backwards_is_negative() {
        assert_eq!(
            days_between("2026-09-08T00:00:00Z", "2026-09-01T00:00:00Z"),
            Some(-7)
        );
    }

    #[test]
    fn a_stamp_that_is_not_one_is_none() {
        assert_eq!(days_between("yesterday", "2026-09-08T00:00:00Z"), None);
        assert_eq!(days_between("2026-09-08T00:00:00Z", "2026-9-8"), None);
    }

    #[test]
    fn epoch_is_1970() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn known_dates() {
        // 2026-08-31 is 20696 days since epoch.
        assert_eq!(civil_from_days(20_696), (2026, 8, 31));
        // A 29th of February, which is where naive implementations break.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        // Before epoch: the sign has to take the negative branch.
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn format_is_stable() {
        let s = now_rfc3339();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
    }

    #[test]
    fn epoch_seconds_reads_the_epoch_itself() {
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn epoch_seconds_counts_the_time_of_day_too() {
        // `days_between` only ever compares dates; `session prompt` needs
        // the minutes within a day as well, which is the one thing that
        // function throws away.
        assert_eq!(
            epoch_seconds("1970-01-01T00:01:00Z"),
            Some(60),
            "a minute past the epoch is 60 seconds, not 0"
        );
        assert_eq!(
            epoch_seconds("2026-09-08T12:00:00Z").unwrap()
                - epoch_seconds("2026-09-08T00:00:00Z").unwrap(),
            43_200,
            "noon is half a day past midnight on the same date"
        );
    }

    #[test]
    fn epoch_seconds_of_a_bare_date_is_none() {
        assert_eq!(epoch_seconds("2026-09-08"), None);
    }

    #[test]
    fn epoch_seconds_of_garbage_is_none() {
        assert_eq!(epoch_seconds("not a timestamp"), None);
    }
}
