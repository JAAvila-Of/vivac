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
