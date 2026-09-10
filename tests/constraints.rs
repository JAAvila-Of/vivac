//! `d336`/`d414` — a constraint, a pillar and a rule are not open fronts.
//!
//! `d336` carried a mechanical acceptance criterion: **one test that fails
//! today for each of its two changes**, with the rest of the battery green
//! and no existing test touched. Writing it first is how that criterion gets
//! used instead of admired.
//!
//! It could only be met for one of the two, and finding out why was worth
//! more than the tests were:
//!
//! - **Change one, `is_front()`.** `a_rule_is_not_an_open_front` fails today,
//!   as intended, and it is the test that survives here.
//! - **Change two, the `constraints()` predicate.** No test could fail,
//!   because the change was unobservable: `constraints()` admits a node when
//!   it is project-wide **or** when its ancestry meets the focus path, and
//!   `ancestors()` runs all the way to the root, which sits on every focus
//!   path there is. `d414` retires that change outright rather than fixing
//!   it: gone by type is what §5 of `t411` reads for governance from now on,
//!   and the pinning test this file used to carry for the no-op --
//!   `a_task_local_constraint_reaches_every_brief` -- is retired with it.
//!
//! `d414` widens what change one excludes: `Pillar` and `Rule` join
//! `Constraint`, for the same reason `Decision` was excluded to begin with.

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
/// `Decision` was excluded for this exact reason; `Constraint`, `Pillar` and
/// `Rule` qualify for it just as squarely (`d414`).
///
/// Six of them go unnoticed. Forty-seven would make half the view be
/// governance, which is `f334`.
#[test]
fn a_constraint_a_pillar_and_a_rule_are_not_open_fronts() {
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
    c.ok(&[
        "add",
        "Security",
        "--parent",
        "1",
        "--type",
        "pillar",
        "--why",
        "the arbiter for this tree",
    ]);
    c.ok(&[
        "add",
        "Never store a secret",
        "--parent",
        "3",
        "--type",
        "rule",
        "--why",
        "what the pillar arbitrates",
    ]);

    let out = c.ok(&["open"]);

    for title in [
        "No dependencies under a copyleft licence",
        "Security",
        "Never store a secret",
    ] {
        assert!(
            !out.contains(title),
            "a {title} is governance, not a front, and `open` still lists it:\n{out}"
        );
    }
    assert!(
        out.contains("Pick a token store"),
        "the real front disappeared, which is a different bug:\n{out}"
    );
}
