//! Time without dependencies.
//!
//! The performance pillar puts writing a node at p99 < 5 ms, and the security
//! one wants few dependencies to audit. Formatting a date justifies neither:
//! it is thirty lines of arithmetic.
//!
//! `d797`: the log keeps UTC instants, untouched -- an instant has to mean
//! the same thing on every machine that reads it. Only the DATE a person is
//! shown is local: [`date_of`] and [`days_between`] ask the machine for its
//! own zone at that instant's own offset, DST included, never today's
//! offset applied to a stamp from another day. Asking the zone is still no
//! crate: [`windows_zone`] and [`unix_zone`] below call the platform
//! directly, the same way [`crate::style`]'s console detection already does.

use std::cell::RefCell;
use std::collections::HashMap;
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

/// The calendar date `ts` reads as to a person on this machine: a full RFC
/// 3339 stamp is placed at its own instant in the machine's own zone, at
/// that instant's own offset (its own DST, never today's applied to a
/// stamp from another day) -- `d797`. Anything shorter than a full stamp
/// has no time of day to place in a zone, and is returned unchanged: a bare
/// `YYYY-MM-DD` (`brief --now` accepts one) or whatever a hand-edited log
/// left behind, the same fallback `date_of` always had.
pub fn date_of(ts: &str) -> String {
    match local_ymd(ts) {
        Some((y, m, d)) => format!("{y:04}-{m:02}-{d:02}"),
        None => ts.chars().take(10).collect(),
    }
}

/// Whole days from `from` to `to`, both RFC 3339 stamps, or `None` if either
/// is not one. Counted on the LOCAL date each stamp reads as, not the UTC
/// one the log stores (`d797`): two stamps that fall on the same day where
/// this machine is are zero days apart however many hours separate them or
/// which side of UTC midnight they sit on, which is what "how long since it
/// moved" means to a person looking at a list of projects.
///
/// Here rather than in the caller because `civil_from_days` already lives in
/// this module and its inverse is eight lines of the same arithmetic. The
/// alternative was a date crate, which the module header already refuses.
pub fn days_between(from: &str, to: &str) -> Option<i64> {
    Some(days_from_civil(local_ymd(to)?) - days_from_civil(local_ymd(from)?))
}

/// [`date_of`] and [`days_between`]'s shared arithmetic: the local civil
/// date a stamp reads as, or [`parse_date`]'s bare `YYYY-MM-DD` when there
/// is no time of day to place in a zone at all.
fn local_ymd(ts: &str) -> Option<(i64, u32, u32)> {
    match epoch_seconds(ts) {
        Some(secs) => Some(local_date_from(secs, local_offset_secs(secs))),
        None => parse_date(ts),
    }
}

/// `YYYY-MM-DD`, from the front of an RFC 3339 stamp. Anything else is `None`:
/// a log written by hand is not a reason to render a wrong number.
fn parse_date(ts: &str) -> Option<(i64, u32, u32)> {
    let b = ts.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    Some((
        ts[0..4].parse().ok()?,
        ts[5..7].parse().ok()?,
        ts[8..10].parse().ok()?,
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

// ---------------------------------------------------------------------------
// `d797`: the local date an instant reads as, in the machine's own zone.
// ---------------------------------------------------------------------------

/// The civil date `secs` seconds after the epoch reads as once shifted
/// `offset_secs` east of Greenwich (west is negative) -- the pure
/// arithmetic half of "what date does this instant read as here",
/// decoupled from asking the machine what "here" is so it can be tested
/// without one. `div_euclid` rounds toward negative infinity rather than
/// toward zero: the second before an offset instant crosses a day boundary
/// has to land in the day *before* it, never wrap forward.
fn local_date_from(secs: i64, offset_secs: i64) -> (i64, u32, u32) {
    civil_from_days((secs + offset_secs).div_euclid(86_400))
}

thread_local! {
    /// One entry per fifteen minutes of UTC asked about, not one system
    /// call per instant: `why` or `tree` over ten thousand nodes spread
    /// across a real project's history touches far fewer distinct buckets
    /// than nodes, and the performance pillar's ceiling is about how many
    /// are shown, not how long the log behind them runs. Per-thread and
    /// never cleared -- each run of the binary is its own process, so
    /// nothing here outlives the command it answered.
    ///
    /// Fifteen minutes, not an hour: every real UTC offset in use today is
    /// a multiple of fifteen minutes -- India's `+5:30`, Nepal's `+5:45`,
    /// same as every whole-hour zone -- and every DST transition lands on
    /// a fifteen-minute-aligned UTC instant, so the offset is genuinely
    /// constant across one bucket for every zone, not just the common
    /// ones. The value cached is the offset itself, in seconds, exact --
    /// `d797` ruled out a granularity that could misdate a fractional
    /// zone, which the coarser one-hour bucket this used to be could.
    static OFFSET_SECS_CACHE: RefCell<HashMap<i64, i64>> = RefCell::new(HashMap::new());
}

/// `secs`'s own offset from UTC, in whole seconds, exact:
/// [`OFFSET_SECS_CACHE`] answers from the cache when this fifteen-minute
/// bucket has been asked about already, and asks the machine only once per
/// bucket otherwise.
fn local_offset_secs(secs: i64) -> i64 {
    let bucket = secs.div_euclid(900);
    let cached = OFFSET_SECS_CACHE.with(|c| c.borrow().get(&bucket).copied());
    match cached {
        Some(v) => v,
        None => {
            let v = os_offset_secs(secs);
            OFFSET_SECS_CACHE.with(|c| c.borrow_mut().insert(bucket, v));
            v
        }
    }
}

/// `secs`'s own offset from UTC, in seconds, exact -- east of Greenwich
/// positive. Real, asking the platform, outside a test; a constant `0` --
/// the same as running on UTC -- inside one, the same trade [`Ticking`]
/// makes for the wall clock: a unit test that asserts a date has to get
/// the same answer on every machine that runs it, and this crate's own
/// test suite is not spawned with any particular `TZ` the way the
/// integration tests below `tests/` are. Three bodies rather than one that
/// branches, the same shape `style::system_width` already uses to pick
/// between its own two platform modules.
#[cfg(all(not(test), windows))]
fn os_offset_secs(secs: i64) -> i64 {
    windows_zone::offset_secs(secs)
}

#[cfg(all(not(test), unix))]
fn os_offset_secs(secs: i64) -> i64 {
    unix_zone::offset_secs(secs)
}

#[cfg(all(not(test), not(any(windows, unix))))]
fn os_offset_secs(_secs: i64) -> i64 {
    0
}

#[cfg(test)]
fn os_offset_secs(_secs: i64) -> i64 {
    0
}

/// Windows: `SystemTimeToTzSpecificLocalTime` reads the *system* zone, not
/// `TZ` -- there is no such thing on this platform outside a shell that
/// happens to export it for its own children. `TZ=UTC` (and the three
/// other spellings POSIX treats the same way) is honoured as a synonym for
/// UTC so the integration tests under `tests/` can pin this platform down
/// exactly the way `TZ` already pins Unix's own `libc`; any other value is
/// ignored and the real system zone answers instead.
#[cfg(all(not(test), windows))]
mod windows_zone {
    use std::ffi::c_void;

    /// `SYSTEMTIME`: a fixed, documented layout, unlike Unix's `struct tm`.
    /// [`offset_secs`] reads `year`/`month`/`day`/`hour`/`minute`/`second`
    /// back off the one this fills -- the offset has to be exact, not just
    /// the date -- and leaves `day_of_week` and `milliseconds` unread.
    #[repr(C)]
    #[allow(dead_code)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn SystemTimeToTzSpecificLocalTime(
            zone: *const c_void,
            utc: *const SystemTime,
            local: *mut SystemTime,
        ) -> i32;
    }

    fn is_utc_spelling(tz: &str) -> bool {
        matches!(tz, "UTC" | "UTC0" | "GMT" | "GMT0")
    }

    /// `secs`'s own offset from UTC, in seconds, exact. `0` -- never
    /// asking the OS at all -- when `TZ` names UTC by one of its four
    /// spellings; any failure along the way answers `0` too, the side to
    /// be wrong on.
    pub(super) fn offset_secs(secs: i64) -> i64 {
        if std::env::var("TZ")
            .ok()
            .is_some_and(|tz| is_utc_spelling(&tz))
        {
            return 0;
        }
        let utc_days = secs.div_euclid(86_400);
        let (y, m, d) = super::civil_from_days(utc_days);
        let rem = secs - utc_days * 86_400;
        let utc = SystemTime {
            year: y as u16,
            month: m as u16,
            day_of_week: 0,
            day: d as u16,
            hour: (rem / 3600) as u16,
            minute: ((rem % 3600) / 60) as u16,
            second: (rem % 60) as u16,
            milliseconds: 0,
        };
        let mut local = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        // SAFETY: `utc` is a live, aligned `SYSTEMTIME` holding a valid
        // Gregorian date and a time of day within the same day; `local` is
        // a live, aligned buffer of the same shape the API fills on
        // success; a null time zone pointer asks for the machine's own
        // active zone, which is the documented meaning of that argument.
        let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
        if ok == 0 {
            return 0;
        }
        let local_secs =
            super::days_from_civil((local.year as i64, local.month as u32, local.day as u32))
                * 86_400
                + local.hour as i64 * 3600
                + local.minute as i64 * 60
                + local.second as i64;
        local_secs - secs
    }
}

/// Unix: `libc` already reads `TZ` for every process on its own, so
/// nothing here has to parse it -- `tzset` just makes sure a `TZ` exported
/// after the process started is not missed.
#[cfg(all(not(test), unix))]
mod unix_zone {
    use std::os::raw::c_int;

    /// `struct tm` from `<time.h>`, declared only as deep as every `libc`
    /// this binary ships for agrees on it: the first nine `int` fields,
    /// POSIX's own order, followed by padding generous enough to absorb
    /// whatever a particular `libc` appends after them -- glibc and the
    /// BSDs add `tm_gmtoff` and `tm_zone`, musl does not, and this struct
    /// is never asked to read either. [`offset_secs`] needs the offset
    /// exact, to the second, so it reads `sec`/`min`/`hour` as well as
    /// `mday`/`mon`/`year`; only `wday`/`yday`/`isdst` sit in an unread
    /// group nothing here looks at.
    #[repr(C)]
    #[allow(dead_code)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        mday: c_int,
        mon: c_int,
        year: c_int,
        _day_counters: [c_int; 3],
        _reserved: [u64; 8],
    }

    extern "C" {
        fn tzset();
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }

    /// `secs`'s own offset from UTC, in seconds, exact. Any failure --
    /// `libc` handing back a null pointer -- answers `0`, the side to be
    /// wrong on.
    pub(super) fn offset_secs(secs: i64) -> i64 {
        let mut tm = Tm {
            sec: 0,
            min: 0,
            hour: 0,
            mday: 0,
            mon: 0,
            year: 0,
            _day_counters: [0; 3],
            _reserved: [0; 8],
        };
        // SAFETY: `tzset` only ever reads the process environment and
        // libc's own global zone state, never a pointer this crate owns.
        // `secs` is a live, aligned `time_t`-shaped `i64` -- `time_t` is a
        // 64-bit integer on every release target this crate builds for --
        // and `tm` is a live, aligned buffer at least as large as any
        // libc's own `struct tm`, which `localtime_r` fills in place
        // through the pointer given it and retains no reference to
        // afterward.
        let filled = unsafe {
            tzset();
            localtime_r(&secs, &mut tm)
        };
        if filled.is_null() {
            return 0;
        }
        let local_secs =
            super::days_from_civil((1900 + tm.year as i64, (tm.mon + 1) as u32, tm.mday as u32))
                * 86_400
                + tm.hour as i64 * 3600
                + tm.min as i64 * 60
                + tm.sec as i64;
        local_secs - secs
    }
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

    #[test]
    fn date_of_reads_the_date_of_a_full_stamp() {
        // `os_offset_secs` is `0` under `#[cfg(test)]`, so this exercises
        // the whole path -- `epoch_seconds`, `local_date_from`, formatting
        // -- without ever asking the machine for its zone.
        assert_eq!(date_of("2026-09-08T23:30:00Z"), "2026-09-08");
    }

    #[test]
    fn date_of_of_a_bare_date_is_unchanged() {
        // `brief --now` accepts a bare date, with no time of day to place
        // in a zone -- `date_of`'s oldest contract, kept.
        assert_eq!(date_of("2026-09-08"), "2026-09-08");
    }

    #[test]
    fn date_of_of_garbage_is_the_first_ten_bytes() {
        assert_eq!(date_of("not a timestamp"), "not a time");
    }

    // `d797`: `local_date_from` is the pure arithmetic a real offset from
    // the machine's own zone feeds into -- these test it directly, with an
    // offset named by hand, so the boundary cases hold on every machine
    // this suite ever runs on regardless of its own zone.

    #[test]
    fn a_negative_offset_short_of_midnight_keeps_the_same_day() {
        // 23:30 UTC, five hours west, is 18:30 local -- still the 8th.
        let secs = epoch_seconds("2026-09-08T23:30:00Z").unwrap();
        assert_eq!(local_date_from(secs, -5 * 3600), (2026, 9, 8));
    }

    #[test]
    fn a_negative_offset_past_midnight_falls_back_a_day() {
        // 02:00 UTC, five hours west, is 21:00 the day before -- the 7th,
        // not the 8th: this is `f731` itself, the hours a person west of
        // Greenwich used to be told it was already tomorrow.
        let secs = epoch_seconds("2026-09-08T02:00:00Z").unwrap();
        assert_eq!(local_date_from(secs, -5 * 3600), (2026, 9, 7));
    }

    #[test]
    fn a_negative_offset_crosses_a_month_boundary() {
        // 02:00 UTC on the 1st of March, five hours west, is still the
        // last day of February -- and 2026 is not a leap year.
        let secs = epoch_seconds("2026-03-01T02:00:00Z").unwrap();
        assert_eq!(local_date_from(secs, -5 * 3600), (2026, 2, 28));
    }

    #[test]
    fn a_negative_offset_crosses_a_year_boundary() {
        let secs = epoch_seconds("2026-01-01T02:00:00Z").unwrap();
        assert_eq!(local_date_from(secs, -5 * 3600), (2025, 12, 31));
    }

    #[test]
    fn a_positive_offset_crosses_into_the_next_day() {
        // 22:00 UTC, five hours east, is 03:00 the day after.
        let secs = epoch_seconds("2026-09-08T22:00:00Z").unwrap();
        assert_eq!(local_date_from(secs, 5 * 3600), (2026, 9, 9));
    }

    // India (`+5:30`) and Nepal (`+5:45`): neither offset is a whole hour,
    // so the fifteen-minute cache bucket is what makes these safe to
    // answer at all -- an hour-wide bucket could hand an instant on one
    // side of local midnight the other side's date.

    #[test]
    fn india_fractional_offset_keeps_the_same_day_short_of_its_own_midnight() {
        // India is UTC+5:30, so its own midnight falls at 18:30 UTC.
        // 18:25 UTC is five minutes short of it: still the 8th, local.
        let secs = epoch_seconds("2026-09-08T18:25:00Z").unwrap();
        assert_eq!(local_date_from(secs, 19_800), (2026, 9, 8));
    }

    #[test]
    fn india_fractional_offset_crosses_at_its_own_midnight() {
        // 18:35 UTC is five minutes past India's own midnight: the 9th,
        // local, while UTC itself is still on the 8th.
        let secs = epoch_seconds("2026-09-08T18:35:00Z").unwrap();
        assert_eq!(local_date_from(secs, 19_800), (2026, 9, 9));
    }

    #[test]
    fn nepal_fractional_offset_keeps_the_same_day_short_of_its_own_midnight() {
        // Nepal is UTC+5:45, so its own midnight falls at 18:15 UTC.
        let secs = epoch_seconds("2026-09-08T18:10:00Z").unwrap();
        assert_eq!(local_date_from(secs, 20_700), (2026, 9, 8));
    }

    #[test]
    fn nepal_fractional_offset_crosses_at_its_own_midnight() {
        let secs = epoch_seconds("2026-09-08T18:20:00Z").unwrap();
        assert_eq!(local_date_from(secs, 20_700), (2026, 9, 9));
    }
}
