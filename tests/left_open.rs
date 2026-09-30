//! `q664`, `d926`: closing a finding had no seam.
//!
//! The work that fixes one usually names it, and the task it was found
//! under usually closes before anyone does. So `pop`, `done` and `decide`
//! show the findings still open that were born under what they close or
//! that they name. Nothing is asked and nothing waits.

mod common;
use common::Sandbox;

const HEADING: &str = "findings still open, born under it or named in it:";

/// A finding reported during a task and left open is listed when the task
/// closes, with the command that closes it.
#[test]
fn pop_lists_a_finding_born_under_the_task_it_closes() {
    let c = Sandbox::new_seeded("left-open-born");
    c.ok(&["push", "task", "--why", "w", "--root"]);
    c.ok(&["add", "a leak", "--type", "finding", "--why", "seen"]);
    let out = c.ok(&["pop", "done with it"]);
    assert!(out.contains(HEADING), "{out}");
    assert!(out.contains("f2     a leak"), "{out}");
    assert!(out.contains("vivac done <id> \"<how>\""), "{out}");
}

/// The case `q664` measured: the fix lands under a sibling, and names the
/// finding in its outcome rather than being born above it.
#[test]
fn pop_lists_a_finding_its_outcome_names() {
    let c = Sandbox::new_seeded("left-open-named");
    c.ok(&["push", "goal", "--why", "w", "--root"]);
    c.ok(&["add", "undo lies", "--type", "finding", "--why", "seen"]);
    c.ok(&["add", "unrelated", "--type", "finding", "--why", "seen"]);
    c.ok(&["push", "fix it", "--why", "w"]);
    let out = c.ok(&["pop", "fixed f2, and 3 more things"]);
    assert!(out.contains("f2     undo lies"), "{out}");
    assert!(!out.contains("unrelated"), "a bare 3 is not f3: {out}");
}

/// Once the finding is closed, nothing is listed.
#[test]
fn a_closed_finding_is_not_listed() {
    let c = Sandbox::new_seeded("left-open-closed");
    c.ok(&["push", "task", "--why", "w", "--root"]);
    c.ok(&["add", "a leak", "--type", "finding", "--why", "seen"]);
    c.ok(&["done", "f2", "Settled by the patch"]);
    let out = c.ok(&["pop", "done"]);
    assert!(!out.contains(HEADING), "{out}");
}

/// A parked finding was already put off by someone, and a task born
/// under the node is not a finding: neither is listed.
#[test]
fn parked_findings_and_other_kinds_are_left_out() {
    let c = Sandbox::new_seeded("left-open-kinds");
    c.ok(&["push", "task", "--why", "w", "--root"]);
    c.ok(&["add", "later", "--type", "finding", "--why", "seen"]);
    c.ok(&["park", "f2", "not now"]);
    c.ok(&["add", "a subtask", "--why", "w"]);
    let out = c.ok(&["pop", "done"]);
    assert!(!out.contains(HEADING), "{out}");
}

/// `decide` names findings through `--ref` and through its reason.
#[test]
fn decide_lists_the_findings_it_names() {
    let c = Sandbox::new_seeded("left-open-decide");
    c.ok(&["push", "goal", "--why", "w", "--root"]);
    c.ok(&["add", "first", "--type", "finding", "--why", "seen"]);
    c.ok(&["add", "second", "--type", "finding", "--why", "seen"]);
    let out = c.ok(&[
        "decide",
        "undo only its own",
        "--reason",
        "answers f3",
        "--alternative",
        "none",
        "--ref",
        "f2",
    ]);
    assert!(out.contains("f2     first"), "{out}");
    assert!(out.contains("f3     second"), "{out}");
}

/// `done` on a finding does not list the finding itself, even when its
/// own outcome names it.
#[test]
fn done_does_not_list_the_node_it_closes() {
    let c = Sandbox::new_seeded("left-open-self");
    c.ok(&["push", "task", "--why", "w", "--root"]);
    c.ok(&["add", "a leak", "--type", "finding", "--why", "seen"]);
    let out = c.ok(&["done", "f2", "Record: f2 was measured"]);
    assert!(!out.contains(HEADING), "{out}");
}

/// Past five, the list stops and says how many it left out.
#[test]
fn a_long_list_is_capped_and_counted() {
    let c = Sandbox::new_seeded("left-open-cap");
    c.ok(&["push", "task", "--why", "w", "--root"]);
    for i in 0..7 {
        c.ok(&[
            "add",
            &format!("leak {i}"),
            "--type",
            "finding",
            "--why",
            "seen",
        ]);
    }
    let out = c.ok(&["pop", "done"]);
    assert!(out.contains("leak 4"), "{out}");
    assert!(!out.contains("leak 5"), "{out}");
    assert!(out.contains("... and 2 more"), "{out}");
}
