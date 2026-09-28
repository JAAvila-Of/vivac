//! `done` on a node that is not open already (`f562`, `d878`).
//!
//! `pop` has answered this since `d554`: a focus that is not open leaves it
//! exactly as it was and says so, rather than closing it a second time.
//! `done` reaches the same state through `close_node` and used to skip the
//! check entirely, writing `State::Done` over whatever the node already
//! held and letting `--force` change nothing about that. A superseded
//! decision is the case that surfaced it: `vivac done` on one left it
//! `done`, with a new outcome that erased which decision replaced it.

mod common;
use common::Sandbox;

fn why_json(c: &Sandbox, id: &str) -> serde_json::Value {
    let s = c.ok(&["why", id, "--json"]);
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// `done` a second time on a node it already closed: the outcome from the
/// first close survives, nothing new is written, and the CLI says so.
#[test]
fn done_on_a_done_node_leaves_its_outcome_unchanged() {
    let c = Sandbox::new_seeded("done-already-done");
    c.ok(&["push", "Ship the release", "--why", "the tag is cut"]);
    c.ok(&["push", "Write the notes", "--why", "they are missing"]);
    c.ok(&["done", "2", "written"]);
    let log_before = c.log();

    let out = c.ok(&["done", "2", "written again by mistake"]);
    assert!(out.contains("-> already closed, left as it was"), "{out}");
    assert_eq!(
        why_json(&c, "2")["node"]["outcome"],
        "written",
        "the second done overwrote the first outcome"
    );
    assert_eq!(
        c.log(),
        log_before,
        "done on a closed node wrote to the log"
    );
}

/// The exact case in `f562`: a decision superseded by a later one is not
/// open, and `done` on it must leave it superseded, not `done`, and must not
/// touch the outcome that names its successor.
#[test]
fn done_on_a_superseded_decision_leaves_it_superseded() {
    let c = Sandbox::new_seeded("done-already-superseded");
    c.ok(&[
        "decide",
        "Old approach",
        "--reason",
        "the call being made",
        "--root",
    ]);
    c.ok(&[
        "decide",
        "New approach",
        "--reason",
        "the old one did not hold",
        "--supersedes",
        "1",
        "--root",
    ]);
    assert_eq!(why_json(&c, "1")["node"]["state"], "superseded");
    let log_before = c.log();

    let out = c.ok(&["done", "1", "closing it anyway"]);
    assert!(
        out.contains("-> already superseded, left as it was"),
        "{out}"
    );
    assert_eq!(
        why_json(&c, "1")["node"]["outcome"],
        "superseded by d2",
        "done overwrote the outcome the supersession wrote"
    );
    assert_eq!(
        c.log(),
        log_before,
        "done on a superseded decision wrote to the log"
    );
}

/// A parked node: `done` leaves it parked, the word `State::word` gives a
/// suspended node regardless of kind.
#[test]
fn done_on_a_parked_node_leaves_it_parked() {
    let c = Sandbox::new_seeded("done-already-parked");
    c.ok(&["push", "Ship the release", "--why", "the tag is cut"]);
    c.ok(&["park", "waiting on something else"]);
    assert_eq!(why_json(&c, "1")["node"]["state"], "suspended");
    let log_before = c.log();

    let out = c.ok(&["done", "1", "closing it anyway"]);
    assert!(out.contains("-> already parked, left as it was"), "{out}");
    assert_eq!(
        why_json(&c, "1")["node"]["outcome"],
        "waiting on something else",
        "done overwrote the reason it was parked for"
    );
    assert_eq!(
        c.log(),
        log_before,
        "done on a parked node wrote to the log"
    );
}

/// `--force` is for closure conditions (`MODEL.md` §7); it does not make a
/// node that is not open closeable a second time.
#[test]
fn force_does_not_reopen_a_node_that_is_not_open() {
    let c = Sandbox::new_seeded("done-already-done-forced");
    c.ok(&["push", "Ship the release", "--why", "the tag is cut"]);
    c.ok(&["push", "Write the notes", "--why", "they are missing"]);
    c.ok(&["done", "2", "written"]);
    let log_before = c.log();

    let out = c.ok(&["done", "2", "written again", "--force"]);
    assert!(out.contains("-> already closed, left as it was"), "{out}");
    assert_eq!(
        c.log(),
        log_before,
        "done --force on a closed node wrote to the log"
    );
}
