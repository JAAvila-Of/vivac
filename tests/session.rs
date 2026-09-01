//! The two session hooks, against the binary.
//!
//! `f35`: Claude Code has no end-of-session event. `Stop` is the closest thing
//! and it runs **on every turn**, so the automatic stop has to know when there
//! is nothing to stop for.

mod common;
use common::Sandbox;

fn how_many(vivacs: &str, kind: &str) -> usize {
    vivacs.lines().filter(|l| l.contains(kind)).count()
}

/// Forty turns are not forty stops. A stop that repeats identically is not a
/// stop: it is a log.
#[test]
fn one_stop_per_turn_does_not_leave_one_stop_per_turn() {
    let c = Sandbox::new_seeded("turns");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    for _ in 0..5 {
        c.ok(&["session", "end", "--hook"]);
    }
    let v = c.ok(&["vivacs"]);
    assert_eq!(how_many(&v, "auto"), 1, "one stop per turn:\n{v}");
}

/// But as soon as the tree changes, the next stop does count.
#[test]
fn a_new_stop_once_something_changed() {
    let c = Sandbox::new_seeded("change");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["note", "1", "something happened"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert_eq!(how_many(&v, "auto"), 2, "it swallowed the good stop:\n{v}");
}

/// With no stack there is no pitch to close.
#[test]
fn no_stack_no_stop() {
    let c = Sandbox::new_seeded("nostack");
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert_eq!(how_many(&v, "auto"), 0, "it invented an empty stop:\n{v}");
}

/// The brief goes inside the envelope the agent reads, and nothing loose
/// outside it: what is not in the envelope, the agent never sees.
#[test]
fn the_start_hook_travels_in_its_envelope() {
    let c = Sandbox::new_seeded("envelope");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let s = c.ok(&["session", "start", "--hook"]);
    assert_eq!(s.lines().filter(|l| !l.trim().is_empty()).count(), 1);
    assert!(s.contains("hookSpecificOutput"), "{s}");
    assert!(s.contains("SessionStart"), "{s}");
    assert!(s.contains("additionalContext"), "{s}");
    assert!(s.contains("A goal"), "the envelope went out empty:\n{s}");
}

/// **A hook that fails in every directory without a tree gets switched off
/// within two days.** Both stay quiet and exit 0 where there is no `.vivac/`,
/// which is what makes it safe to leave them in the global configuration.
#[test]
fn they_stay_quiet_where_there_is_no_tree() {
    let c = Sandbox::new_empty("notree");
    for args in [["session", "start", "--hook"], ["session", "end", "--hook"]] {
        let (s, code) = c.run(&args);
        assert_eq!(code, 0, "{args:?} failed outside a tree:\n{s}");
        assert_eq!(s.trim(), "", "{args:?} said too much:\n{s}");
    }
}

/// An automatic stop nobody declared still has to say something. Two autos in
/// a row that read identically do not segment a session, they log it (`f59`).
/// The label is derived from the seams --what the segment contained-- and never
/// from a judgement of relevance, which `DX` already measured at zero uses.
#[test]
fn an_automatic_stop_says_what_its_segment_contained() {
    let c = Sandbox::new_seeded("segment");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["add", "First finding", "--why", "it turned up"]);
    c.ok(&["add", "Second finding", "--why", "it turned up too"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("2 new"),
        "the automatic stop did not say what it closed:\n{v}"
    );
}

/// Closing is as much of a seam as opening. A segment that only settled things
/// would otherwise read as if nothing had happened in it.
#[test]
fn an_automatic_stop_counts_what_its_segment_closed() {
    let c = Sandbox::new_seeded("closed");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["add", "First finding", "--why", "it turned up"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["done", "2", "it was settled"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("1 closed"),
        "the automatic stop counted no closes:\n{v}"
    );
}

/// A segment made only of notes is still a segment. The real tree has turns
/// that wrote nothing but notes, and they have to be tellable apart.
#[test]
fn an_automatic_stop_counts_the_notes_of_its_segment() {
    let c = Sandbox::new_seeded("notes");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["note", "1", "something turned up"]);
    c.ok(&["note", "1", "and something else"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("2 notes"),
        "the automatic stop counted no notes:\n{v}"
    );
}

/// One note is a note, not notes. The label is prose the maintainer reads in
/// `vivac vivacs`, and prose that counts wrong reads like a machine talking.
#[test]
fn a_single_note_is_not_pluralised() {
    let c = Sandbox::new_seeded("plural");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["note", "1", "something turned up"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(v.contains("1 note"), "it counted no notes:\n{v}");
    assert!(!v.contains("1 notes"), "it said `1 notes`:\n{v}");
}

/// Not every seam is a birth, a close or a note. A segment that only raised a
/// flag still moved the tree, and if the label came out empty the stop would be
/// back to being the blank line `f59` was about.
#[test]
fn a_segment_of_none_of_the_three_still_says_something() {
    let c = Sandbox::new_seeded("other");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["flag", "1", "review", "--why", "it needs a second look"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("1 change"),
        "the stop came out blank after a flag:\n{v}"
    );
}
