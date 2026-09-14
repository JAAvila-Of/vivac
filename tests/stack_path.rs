//! The stack is always a contiguous stretch of its top's lineage (`d554`,
//! `t533` §2).
//!
//! `done` and `park` used to unstack a node wherever it sat, which let the
//! stack skip over an ancestor still open below the one just closed. From
//! here on, `done` and `park` only unstack when the node they act on is the
//! stack's own top; anywhere else, the path still runs through it and the
//! spine marks it (`f134`, `f55`).

mod common;
use common::Sandbox;
use serde_json::Value;

fn stack_aliases(c: &Sandbox) -> Vec<String> {
    let s = c.ok(&["stack", "--json"]);
    let v: Value = serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"));
    v["stack"]
        .as_array()
        .expect("stack is an array")
        .iter()
        .map(|n| n["alias"].as_str().unwrap().to_string())
        .collect()
}

fn why_json(c: &Sandbox, id: &str) -> Value {
    let s = c.ok(&["why", id, "--json"]);
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// `g > t > u`. `done t` (the middle of the stack, not its top) leaves the
/// stack exactly as it was, the footer reports the same depth, and the spine
/// marks `t` as closed. Popping from `u` afterwards still lands back on `t`.
#[test]
fn done_in_the_middle_of_the_stack_does_not_unstack_it() {
    let c = Sandbox::new_seeded("stack-done-middle");
    c.ok(&["push", "Base goal", "--why", "it anchors the branch"]);
    c.ok(&["push", "Task two", "--why", "the goal needs it"]);
    c.ok(&["push", "Task three", "--why", "one more step"]);
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3"]);

    c.ok(&["done", "t2", "settled early"]);
    assert_eq!(
        stack_aliases(&c),
        vec!["g1", "t2", "t3"],
        "done in the middle popped the stack"
    );
    assert_eq!(why_json(&c, "t2")["node"]["state"], "done");

    let brief = c.ok(&["brief"]);
    assert!(brief.contains("depth 3"), "{brief}");
    assert!(brief.contains("Task two  [closed]"), "{brief}");

    let out = c.ok(&["pop", "wrapped up"]);
    assert!(out.contains("back to t2"), "{out}");
    assert_eq!(stack_aliases(&c), vec!["g1", "t2"]);
}

/// The same shape, with `park` instead of `done`: the middle node stays on
/// the stack and the spine marks it parked.
#[test]
fn park_in_the_middle_of_the_stack_does_not_unstack_it() {
    let c = Sandbox::new_seeded("stack-park-middle");
    c.ok(&["push", "Base goal", "--why", "it anchors the branch"]);
    c.ok(&["push", "Task two", "--why", "the goal needs it"]);
    c.ok(&["push", "Task three", "--why", "one more step"]);

    c.ok(&["park", "t2", "stuck on something else"]);
    assert_eq!(
        stack_aliases(&c),
        vec!["g1", "t2", "t3"],
        "park in the middle popped the stack"
    );
    assert_eq!(why_json(&c, "t2")["node"]["state"], "suspended");

    let brief = c.ok(&["brief"]);
    assert!(brief.contains("Task two  [parked]"), "{brief}");

    let parked = c.ok(&["parked"]);
    assert!(parked.contains("Task two"), "{parked}");
}

/// `g > a > b > c > d`, `done g` (the base, not the top), then another push:
/// the depth advice still names the bottom of the stack, now with its mark.
#[test]
fn the_depth_advice_carries_the_bottom_mark_once_it_is_closed() {
    let c = Sandbox::new_seeded("stack-depth-mark");
    c.ok(&["push", "Base goal", "--why", "root of the branch"]);
    c.ok(&["push", "A", "--why", "first step"]);
    c.ok(&["push", "B", "--why", "second step"]);
    c.ok(&["push", "C", "--why", "third step"]);
    c.ok(&["push", "D", "--why", "fourth step"]);
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3", "t4", "t5"]);

    c.ok(&["done", "g1", "superseded already"]);

    let out = c.ok(&["push", "E", "--why", "fifth step"]);
    assert!(
        out.contains("You are 6 levels away from g1 \"Base goal\" [achieved]."),
        "{out}"
    );
}

/// `f552`: a decision that becomes superseded while still on the stack stays
/// superseded when `pop` finally reaches it, its outcome untouched, and the
/// pop line says so instead of claiming a close.
#[test]
fn a_pop_into_a_superseded_focus_leaves_it_as_it_was() {
    let c = Sandbox::new_seeded("stack-pop-superseded");
    c.ok(&["push", "Base goal", "--why", "root of the branch"]);
    c.ok(&[
        "push",
        "Old approach",
        "--why",
        "the call being made",
        "--type",
        "decision",
    ]);
    c.ok(&["push", "Follow-up", "--why", "work under the decision"]);
    c.ok(&[
        "decide",
        "New approach",
        "--reason",
        "the old one did not hold",
        "--supersedes",
        "2",
    ]);
    assert_eq!(why_json(&c, "d2")["node"]["state"], "superseded");
    assert_eq!(why_json(&c, "d2")["node"]["outcome"], "superseded by d4");

    c.ok(&["pop", "follow-up done"]);
    assert_eq!(stack_aliases(&c), vec!["g1", "d2"]);

    let out = c.ok(&["pop", "moving on"]);
    assert!(
        out.contains("d2  Old approach  -> already superseded, left as it was"),
        "{out}"
    );

    assert_eq!(why_json(&c, "d2")["node"]["state"], "superseded");
    assert_eq!(
        why_json(&c, "d2")["node"]["outcome"],
        "superseded by d4",
        "the pop overwrote the outcome the supersession wrote"
    );
}

/// `restore` rebuilds a contiguous path from the point's frozen stack: it
/// starts at the saved node nearest the root that is the deepest still-open
/// node or one of its ancestors, and reports every closed node still on that
/// path, separately from what fell off the stack entirely.
#[test]
fn restore_reports_closed_nodes_still_on_the_path_apart_from_what_left() {
    let c = Sandbox::new_seeded("stack-restore-path");
    c.ok(&["push", "Base goal", "--why", "root of the branch"]);
    c.ok(&["push", "A", "--why", "first step"]);
    c.ok(&["push", "B", "--why", "second step"]);
    c.ok(&["push", "C", "--why", "third step"]);
    let saved = c.ok(&["save", "checkpoint", "--next", "carry on"]);
    let vivac_num = saved
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    c.ok(&["done", "g1", "closed from below"]);

    let out = c.ok(&["restore", &vivac_num]);
    assert!(
        out.contains("still on the path:  g1 Base goal [achieved]"),
        "{out}"
    );
    assert!(
        !out.contains("no longer on the stack"),
        "nothing should have fallen off yet:\n{out}"
    );
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3", "t4"]);

    c.ok(&["done", "t4", "closed the tip"]);
    let out = c.ok(&["restore", &vivac_num]);
    assert!(
        out.contains("still on the path:  g1 Base goal [achieved]"),
        "{out}"
    );
    assert!(
        out.contains("no longer on the stack:  t4 C [closed]"),
        "{out}"
    );
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3"]);
}
