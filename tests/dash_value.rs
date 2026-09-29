//! `f900`, `d919`: a value that starts with `--` is still read as a flag.
//!
//! Taking the next word as-is would turn `decide --reason --alternative b`,
//! refused today, into a decision whose reason is the word `--alternative`.
//! So the parser stays, and the refusal says how to pass such a value: the
//! attached form, which already kept it verbatim.

mod common;
use common::Sandbox;

/// `--alternative "--until later"` is refused, and the refusal names the
/// flag that went without a value and the attached form that carries it.
#[test]
fn a_value_read_as_a_flag_is_refused_with_the_attached_form() {
    let c = Sandbox::new_seeded("dash-value-refused");
    c.ok(&["push", "x", "--why", "y", "--root"]);
    let (out, code) = c.run(&[
        "decide",
        "t",
        "--reason",
        "r",
        "--alternative",
        "--until later",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("decide does not take --until later."), "{out}");
    assert!(
        out.contains("--alternative got no value: \"--until later\" came right after it"),
        "{out}"
    );
    assert!(out.contains("--alternative=\"--until later\""), "{out}");
}

/// The form the refusal names does keep the value, word for word.
#[test]
fn the_attached_form_keeps_a_value_that_starts_with_dashes() {
    let c = Sandbox::new_seeded("dash-value-attached");
    c.ok(&["push", "x", "--why", "y", "--root"]);
    c.ok(&[
        "decide",
        "t",
        "--reason",
        "r",
        "--alternative=--until later",
    ]);
    let why = c.ok(&["why", "2"]);
    assert!(why.contains("discarded: --until later"), "{why}");
}

/// A flag that is simply unknown, with nothing before it left empty, gets
/// the plain refusal and no hint about values.
#[test]
fn a_plain_unknown_flag_gets_no_hint_about_values() {
    let c = Sandbox::new_seeded("dash-value-plain");
    let (out, code) = c.run(&["push", "x", "--why", "y", "--bogus"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("does not take --bogus"), "{out}");
    assert!(!out.contains("got no value"), "{out}");
}

/// A flag the command does take, right after one left empty, is not the
/// case either: the empty one is what is missing, and that is what it says.
#[test]
fn a_known_flag_after_an_empty_one_still_reports_the_missing_value() {
    let c = Sandbox::new_seeded("dash-value-known");
    c.ok(&["push", "x", "--why", "y", "--root"]);
    let (out, code) = c.run(&["decide", "t", "--reason", "--alternative", "b"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("Missing --reason"), "{out}");
    assert!(!out.contains("got no value"), "{out}");
}
