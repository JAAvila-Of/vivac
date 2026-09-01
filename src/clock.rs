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

/// Instant in UTC, RFC 3339 with seconds. This is what goes in the event.
pub fn now_rfc3339() -> String {
    let secs = unix_millis() / 1000;
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
}
