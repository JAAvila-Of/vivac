//! `park` and the words it is handed.
//!
//! The id is optional and so is the reason, which makes **one** word on its
//! own genuinely ambiguous: a reason is as good a word as an alias. **Two**
//! words are not ambiguous at all. The first is an id, and an id that names
//! nothing is a typo, not a reason.
//!
//! `f74` is what guessing instead of refusing costs. `park f74 "<reason>"`
//! parked the root goal of a real tree --the focus, which nobody had named--
//! filed `f74` itself as the reason, and threw away the reason that had been
//! written. Exit code 0. These tests are what keeps that refused.

mod common;
use common::Sandbox;

/// `g1` on the stack and `t2` beside it: a focus, and something else to name.
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

#[test]
fn an_id_that_names_nothing_is_refused_not_guessed() {
    let c = tree("park-unknown");
    let (out, code) = c.run(&["park", "f99", "a reason worth keeping"]);
    assert_ne!(code, 0, "park took an id that names nothing:\n{out}");
    assert!(out.contains("f99"), "the refusal does not name it:\n{out}");
    let (parked, _) = c.run(&["parked"]);
    assert!(
        !parked.contains("Ship the release"),
        "the focus was parked in its place:\n{parked}"
    );
}

/// The lone-word case, where the ambiguity is real. A word *shaped* like an
/// alias that resolves to nothing is a typo: prose does not look like `f99`.
#[test]
fn a_lone_word_shaped_like_an_alias_is_refused_too() {
    let c = tree("park-typo");
    let (out, code) = c.run(&["park", "f99"]);
    assert_ne!(code, 0, "park read a typo'd alias as a reason:\n{out}");
    let (parked, _) = c.run(&["parked"]);
    assert!(
        !parked.contains("Ship the release"),
        "the focus was parked in its place:\n{parked}"
    );
}

/// And the other half of the ambiguity keeps working: prose is a reason.
#[test]
fn a_lone_reason_still_parks_the_focus() {
    let c = tree("park-reason");
    c.ok(&["park", "waiting on the release"]);
    let parked = c.ok(&["parked"]);
    assert!(parked.contains("Ship the release"), "{parked}");
    assert!(parked.contains("waiting on the release"), "{parked}");
}

#[test]
fn an_id_and_a_reason_park_what_was_named() {
    let c = tree("park-named");
    c.ok(&["park", "t2", "the notes can wait"]);
    let parked = c.ok(&["parked"]);
    assert!(parked.contains("Write the notes"), "{parked}");
    assert!(parked.contains("the notes can wait"), "{parked}");
    assert!(
        !parked.contains("Ship the release"),
        "it parked the focus as well:\n{parked}"
    );
}

/// `f75`. The tree these commands serve is written in Spanish, so a reason
/// starting with `ultima`, `arbol` or `unico` --spelled properly-- is the
/// ordinary case, not the exotic one.
#[test]
fn a_reason_starting_with_a_multibyte_letter_does_not_crash() {
    let c = tree("park-accent");
    let (out, _) = c.run(&["park", "última revisión antes de cerrar"]);
    assert!(!out.contains("panicked"), "it aborted the process:\n{out}");
    let parked = c.ok(&["parked"]);
    assert!(parked.contains("Ship the release"), "{parked}");
}

#[test]
fn an_empty_word_does_not_crash() {
    let c = tree("park-empty");
    let (out, _) = c.run(&["park", ""]);
    assert!(!out.contains("panicked"), "it aborted the process:\n{out}");
}

// `d899`: `--until`, the return date a park can carry.

#[test]
fn until_accepts_a_future_date_and_writes_it_to_the_log() {
    let c = tree("park-until-future");
    c.ok(&["park", "waiting on the release", "--until", "9999-12-31"]);
    assert!(c.log().contains(r#""until":"9999-12-31""#), "{}", c.log());
}

#[test]
fn without_until_the_event_carries_none() {
    let c = tree("park-until-none");
    c.ok(&["park", "waiting on the release"]);
    assert!(!c.log().contains("\"until\""), "{}", c.log());
}

#[test]
fn until_refuses_a_date_that_has_already_passed() {
    let c = tree("park-until-past");
    let (out, code) = c.run(&["park", "waiting", "--until", "2000-01-01"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("has already passed"), "{out}");
    assert!(
        !c.log().contains("\"until\""),
        "a refused write must not land:\n{}",
        c.log()
    );
}

/// `TZ=UTC` on every `Sandbox::run` (`tests/common/mod.rs`) is what `--until`
/// is checked against too, so today's own date, read off `brief`'s header,
/// is exactly what a same-day `--until` has to be refused for.
#[test]
fn until_refuses_the_same_day_as_today() {
    let c = tree("park-until-today");
    let brief = c.ok(&["brief"]);
    let today = brief
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().last())
        .expect("the header line names today");
    let (out, code) = c.run(&["park", "waiting", "--until", today]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("is today"), "{out}");
}

#[test]
fn until_refuses_a_malformed_or_relative_date() {
    let c = tree("park-until-malformed");
    for bad in ["2026-9-1", "2026-02-30", "tomorrow", "+7d"] {
        let (out, code) = c.run(&["park", "waiting", "--until", bad]);
        assert_eq!(code, 2, "{bad}:\n{out}");
        assert!(out.contains("YYYY-MM-DD"), "{bad}:\n{out}");
    }
}
