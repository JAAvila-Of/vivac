//! `flag <id> review --on <date>` -- a review that sleeps until a day. `d906`.
//!
//! What the brief and `flagged` make of the date is tested where they are
//! (`brief.rs`, `flagged.rs`); this file is the write side: what the flag
//! command takes, refuses, and leaves in the log.

mod common;
use common::Sandbox;
use serde_json::Value;

/// `g1` on the stack and `t2` beside it: a focus, and something else to flag.
fn tree(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Ship the release", "--why", "the tag is cut"]);
    c.ok(&[
        "add",
        "Write the notes",
        "--parent",
        "1",
        "--why",
        "they are missing",
    ]);
    c
}

/// The `flags` array `flagged --json` gives for the node at `at`.
fn flags_of(c: &Sandbox, at: usize) -> Value {
    let v: Value = serde_json::from_str(&c.ok(&["flagged", "--json"])).unwrap();
    v[at]["flags"].clone()
}

#[test]
fn on_writes_the_date_and_says_where_it_shows_up() {
    let c = tree("flag-on-writes");
    let out = c.ok(&[
        "flag",
        "2",
        "review",
        "--why",
        "look again",
        "--on",
        "9999-12-31",
    ]);
    assert!(out.contains("-> review on 9999-12-31"), "{out}");
    assert!(out.contains("        shows up in:  vivac flagged"), "{out}");
    assert!(c.log().contains(r#""on":"9999-12-31""#), "{}", c.log());
}

#[test]
fn without_on_the_output_and_the_event_are_unchanged() {
    let c = tree("flag-on-none");
    let out = c.ok(&["flag", "2", "review", "--why", "look again"]);
    assert!(out.contains("-> review\n"), "{out}");
    assert!(!out.contains("shows up in"), "{out}");
    assert!(!c.log().contains("\"on\""), "{}", c.log());
}

#[test]
fn on_refuses_a_date_that_has_already_passed() {
    let c = tree("flag-on-past");
    let (out, code) = c.run(&["flag", "2", "review", "--why", "x", "--on", "2000-01-01"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--on 2000-01-01 has already passed; a review is set for a day still ahead."),
        "{out}"
    );
    assert!(
        !c.log().contains("flag.raised"),
        "a refused write must not land:\n{}",
        c.log()
    );
}

/// `TZ=UTC` on every `Sandbox::run` is what `--on` is checked against too, so
/// today's own date, read off `brief`'s header, is exactly what a same-day
/// `--on` has to be refused for.
#[test]
fn on_refuses_the_same_day_as_today() {
    let c = tree("flag-on-today");
    let brief = c.ok(&["brief"]);
    let today = brief
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().last())
        .expect("the header line names today");
    let (out, code) = c.run(&["flag", "2", "review", "--why", "x", "--on", today]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains(&format!(
            "--on {today} is today; a review is set for a day still ahead."
        )),
        "{out}"
    );
}

#[test]
fn on_refuses_a_malformed_or_relative_date() {
    let c = tree("flag-on-malformed");
    for bad in ["2026-9-1", "2026-02-30", "tomorrow", "+7d"] {
        let (out, code) = c.run(&["flag", "2", "review", "--why", "x", "--on", bad]);
        assert_eq!(code, 2, "{bad}:\n{out}");
        assert!(
            out.contains("--on takes a date as YYYY-MM-DD."),
            "{bad}:\n{out}"
        );
    }
}

#[test]
fn on_and_off_do_not_go_together() {
    let c = tree("flag-on-off");
    let (out, code) = c.run(&["flag", "2", "review", "--off", "--on", "9999-12-31"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--on and --off do not go together: --off clears the flag."),
        "{out}"
    );
}

#[test]
fn on_goes_with_review_only() {
    let c = tree("flag-on-review-only");
    for other in ["suspect", "stale"] {
        let (out, code) = c.run(&["flag", "2", other, "--why", "x", "--on", "9999-12-31"]);
        assert_eq!(code, 2, "{other}:\n{out}");
        assert!(
            out.contains("--on goes with review only: suspect and stale are about now."),
            "{other}:\n{out}"
        );
    }
    assert!(
        !c.log().contains("flag.raised"),
        "a refused write must not land:\n{}",
        c.log()
    );
}

#[test]
fn on_still_asks_for_a_reason() {
    let c = tree("flag-on-why");
    let (out, code) = c.run(&["flag", "2", "review", "--on", "9999-12-31"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--why"), "{out}");
}

#[test]
fn off_clears_the_date() {
    let c = tree("flag-on-cleared");
    c.ok(&[
        "flag",
        "2",
        "review",
        "--why",
        "look again",
        "--on",
        "9999-12-31",
    ]);
    c.ok(&["flag", "2", "review", "--off"]);
    assert_eq!(
        c.ok(&["flagged", "--json"]).trim(),
        "[]",
        "a cleared review left something behind"
    );
    // Raised again with no date it is not asleep, so the date did not come back
    // with it.
    c.ok(&["flag", "2", "review", "--why", "look again"]);
    assert!(flags_of(&c, 0)[0]["on"].is_null());
}

#[test]
fn a_review_with_no_date_after_one_with_a_date_is_not_asleep() {
    let c = tree("flag-on-wakes");
    c.ok(&[
        "flag",
        "2",
        "review",
        "--why",
        "look again",
        "--on",
        "9999-12-31",
    ]);
    assert_eq!(flags_of(&c, 0)[0]["on"], "9999-12-31");
    c.ok(&["flag", "2", "review", "--why", "look again now"]);
    let f = flags_of(&c, 0);
    assert!(f[0]["on"].is_null(), "{f}");
    let text = c.ok(&["flagged"]);
    assert!(!text.contains("REVIEW LATER"), "{text}");
}

#[test]
fn a_date_on_a_review_that_had_none_sets_it_asleep() {
    let c = tree("flag-on-sleeps");
    c.ok(&["flag", "2", "review", "--why", "look again"]);
    let text = c.ok(&["flagged"]);
    assert!(text.contains("FLAGGED (1)"), "{text}");
    c.ok(&[
        "flag",
        "2",
        "review",
        "--why",
        "look again",
        "--on",
        "9999-12-31",
    ]);
    let text = c.ok(&["flagged"]);
    assert!(text.contains("REVIEW LATER (1)"), "{text}");
    assert!(!text.contains("FLAGGED ("), "{text}");
}
