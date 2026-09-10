//! `d336` — a constraint is not an open front.
//!
//! `d336` carried a mechanical acceptance criterion: **one test that fails
//! today for each of its two changes**, with the rest of the battery green
//! and no existing test touched. Writing it first is how that criterion gets
//! used instead of admired.
//!
//! It could only be met for one of the two, and finding out why was worth
//! more than the tests were:
//!
//! - **Change one, `is_front()`.** `a_constraint_is_not_an_open_front` fails
//!   today, as intended, and it is the test that survives here.
//! - **Change two, the `constraints()` predicate.** No test could fail,
//!   because the change was unobservable: `constraints()` admits a node when
//!   it is project-wide **or** when its ancestry meets the focus path, and
//!   `ancestors()` runs all the way to the root, which sits on every focus
//!   path there is. In a single-root tree the second clause already admits
//!   every open constraint, so widening the first one changes nothing
//!   anybody can see, and the pinning test this file would carry for that
//!   no-op is not included here.

mod common;
use common::Sandbox;

/// A root goal with real work under it, so `open` has something to show
/// besides whatever this test is asking about.
fn seeded(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "Migrate authentication to OIDC",
        "--why",
        "the old provider is shutting down",
    ]);
    c.ok(&[
        "add",
        "Pick a token store",
        "--parent",
        "1",
        "--why",
        "the migration needs one",
    ]);
    c
}

/// A permanent rule is neither worked on nor ever closed, so counting it
/// among the open fronts answers "what do I have open?" with governance.
/// `Decision` was excluded for this exact reason; a constraint qualifies for
/// it just as squarely.
///
/// Six of them go unnoticed. Forty-seven would make half the view be
/// governance, which is `f334`.
#[test]
fn a_constraint_is_not_an_open_front() {
    let c = seeded("front");
    c.ok(&[
        "add",
        "No dependencies under a copyleft licence",
        "--parent",
        "1",
        "--type",
        "constraint",
        "--why",
        "company policy",
    ]);

    let out = c.ok(&["open"]);

    assert!(
        !out.contains("No dependencies under a copyleft licence"),
        "a constraint is governance, not a front, and `open` still lists it:\n{out}"
    );
    assert!(
        out.contains("Pick a token store"),
        "the real front disappeared, which is a different bug:\n{out}"
    );
}
