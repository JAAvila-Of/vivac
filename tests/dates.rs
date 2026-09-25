//! `d797`/`f731` -- the date a person is shown is the local calendar date of
//! an instant, in the machine's own zone at that instant's own offset, never
//! the UTC date the log stores. `src/clock.rs`'s own unit tests cover the
//! pure arithmetic with an offset named by hand; this is the one place that
//! exercises the real platform call, through a real `TZ`, end to end.
//!
//! Unix only: `TZ` is what steers the real call here, and `common::mod.rs`'s
//! shared spawn helpers already set `TZ=UTC` on every child so the rest of
//! the suite reads the same date on any machine it runs on -- this test is
//! the one place that deliberately asks for a different zone instead.
//! Windows has no `TZ`-driven equivalent to ask for, so the whole file is
//! Unix only rather than one test inside it, which would leave its own
//! helpers unused everywhere else.
#![cfg(unix)]

mod common;
use common::Sandbox;
use serde_json::Value;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// Rewrites the `ts` of the `node.created` line for `num` to `stamp`
/// exactly, the same substring replace `tests/open.rs`'s own `backdate`
/// uses to move a node's `opened` day without waiting real days for it --
/// this one takes the target stamp literally, since the point here is a
/// specific instant near a day boundary, not an offset from today.
fn set_created_at(c: &Sandbox, num: u64, stamp: &str) {
    let path = c.0.join(".vivac").join("events");
    let raw = std::fs::read_to_string(&path).unwrap();
    let marker = format!("\"num\":{num},");
    let mut done = false;
    let out: Vec<String> = raw
        .lines()
        .map(|l| {
            if !done && l.contains("\"type\":\"node.created\"") && l.contains(&marker) {
                let ts_at = l.find("\"ts\":\"").unwrap() + 6;
                let ts_end = ts_at + l[ts_at..].find('"').unwrap();
                done = true;
                format!("{}{}{}", &l[..ts_at], stamp, &l[ts_end..])
            } else {
                l.to_string()
            }
        })
        .collect();
    assert!(done, "no node.created line found for num {num}");
    std::fs::write(&path, out.join("\n") + "\n").unwrap();
}

/// [`Sandbox::run`], with `TZ` overridden to `tz` rather than the `UTC`
/// every other call in this suite gets: the one deliberate exception, for
/// the one test that has to ask the real platform for a real zone.
fn why_json_with_tz(c: &Sandbox, num: &str, tz: &str) -> Value {
    let out = Command::new(BIN)
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .env("TZ", tz)
        .args(["why", num, "--json"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// `f731` itself: a person five hours west of Greenwich used to be told a
/// node born at 02:00 UTC opened "today" while their own calendar still
/// read yesterday. `EST5` is a fixed `UTC-5` with no daylight-saving rule,
/// so the offset this test checks is exactly the one `f731`'s report named.
#[test]
fn a_stamp_past_utc_midnight_reads_as_the_day_before_five_hours_west() {
    let c = Sandbox::new_seeded("dates-midnight");
    c.ok(&["push", "Born just past UTC midnight", "--why", "seed"]);
    set_created_at(&c, 1, "2026-09-08T02:00:00Z");

    let local = why_json_with_tz(&c, "1", "EST5");
    assert_eq!(
        local["node"]["opened"], "2026-09-07",
        "21:00 the evening before, five hours west of a 02:00 UTC stamp:\n{local}"
    );

    let utc = why_json_with_tz(&c, "1", "UTC");
    assert_eq!(
        utc["node"]["opened"], "2026-09-08",
        "the same instant, read in UTC, is still the day the log stamped it:\n{utc}"
    );
}
