//! The two session hooks, against the binary.
//!
//! `f35`: Claude Code has no end-of-session event. `Stop` is the closest thing
//! and it runs **on every turn**, so the automatic stop has to know when there
//! is nothing to stop for.

mod common;
use common::Caja;

fn cuantos(vivacs: &str, kind: &str) -> usize {
    vivacs.lines().filter(|l| l.contains(kind)).count()
}

/// Forty turns are not forty stops. A stop that repeats identically is not a
/// stop: it is a log.
#[test]
fn one_stop_per_turn_does_not_leave_one_stop_per_turn() {
    let c = Caja::nueva("turns");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    for _ in 0..5 {
        c.ok(&["session", "end", "--hook"]);
    }
    let v = c.ok(&["vivacs"]);
    assert_eq!(cuantos(&v, "auto"), 1, "one stop per turn:\n{v}");
}

/// But as soon as the tree changes, the next stop does count.
#[test]
fn a_new_stop_once_something_changed() {
    let c = Caja::nueva("change");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["note", "1", "something happened"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert_eq!(cuantos(&v, "auto"), 2, "it swallowed the good stop:\n{v}");
}

/// With no stack there is no pitch to close.
#[test]
fn no_stack_no_stop() {
    let c = Caja::nueva("nostack");
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert_eq!(cuantos(&v, "auto"), 0, "it invented an empty stop:\n{v}");
}

/// The brief goes inside the envelope the agent reads, and nothing loose
/// outside it: what is not in the envelope, the agent never sees.
#[test]
fn the_start_hook_travels_in_its_envelope() {
    let c = Caja::nueva("envelope");
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
    let c = Caja::vacia("notree");
    for args in [["session", "start", "--hook"], ["session", "end", "--hook"]] {
        let (s, cod) = c.correr(&args);
        assert_eq!(cod, 0, "{args:?} failed outside a tree:\n{s}");
        assert_eq!(s.trim(), "", "{args:?} said too much:\n{s}");
    }
}
