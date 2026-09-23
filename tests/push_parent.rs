//! `push --parent` (`d757`): opens under a node other than the focus in one
//! move -- what `focus` followed by a plain `push` would do, folded into a
//! single vivac. The focus is wherever work was left, maybe by another
//! session and about something else, so naming the node this work continues
//! has to work even when that node is nowhere near the current stack.
//!
//! `--parent` refused together with `--root` on `push` lives in `root.rs`,
//! next to the same refusal on `add` and `decide`.

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

fn state_of(c: &Sandbox, id: &str) -> String {
    why_json(c, id)["node"]["state"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The stack sits on a branch that shares nothing with the node named:
/// the new node is born under it, the stack is rebuilt to that node's own
/// path plus the new node, and popping lands back on the node it continues.
#[test]
fn push_parent_off_the_current_branch_moves_the_stack_to_it() {
    let c = Sandbox::new_seeded("push-parent-branch");
    c.ok(&["push", "Goal A", "--why", "first branch"]);
    c.ok(&["push", "Task under A", "--why", "detail on A"]);
    c.ok(&["push", "Goal B", "--why", "second branch", "--root"]);
    assert_eq!(stack_aliases(&c), vec!["g3"]);

    let out = c.ok(&[
        "push",
        "Continue A's task",
        "--why",
        "it continues t2",
        "--parent",
        "2",
    ]);
    assert!(out.contains("  t4  Continue A's task"), "{out}");
    assert!(
        out.contains("        under t2: g3 left the stack, not closed by this"),
        "{out}"
    );
    assert!(
        out.contains("        back there with:  vivac focus g3"),
        "{out}"
    );

    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t4"]);
    assert_eq!(why_json(&c, "t4")["node"]["parent"], "t2");
    // Nothing that left the stack was closed by this.
    assert_eq!(state_of(&c, "g3"), "active");

    let popped = c.ok(&["pop", "the continuation is done"]);
    assert!(popped.contains("back to t2"), "{popped}");
    assert_eq!(stack_aliases(&c), vec!["g1", "t2"]);
}

/// `--parent` naming the current focus itself moves nothing: the stack
/// already runs through it, so this reads exactly as a plain `push`.
#[test]
fn push_parent_naming_the_focus_reads_as_a_plain_push() {
    let c = Sandbox::new_seeded("push-parent-is-focus");
    c.ok(&["push", "Goal A", "--why", "root of the branch"]);
    let plain = Sandbox::new_seeded("push-parent-is-focus-plain");
    plain.ok(&["push", "Goal A", "--why", "root of the branch"]);

    let with_parent = c.ok(&["push", "A step", "--why", "next", "--parent", "1"]);
    let without = plain.ok(&["push", "A step", "--why", "next"]);
    assert_eq!(with_parent, without);
    assert_eq!(stack_aliases(&c), vec!["g1", "t2"]);
}

/// `--parent` on a node `done` already closed: refused, with the same
/// vocabulary `focus` uses for the same claim, and nothing is written.
#[test]
fn push_parent_on_a_closed_node_is_refused() {
    let c = Sandbox::new_seeded("push-parent-closed");
    c.ok(&["push", "Goal A", "--why", "root of the branch"]);
    c.ok(&["done", "1", "shipped"]);
    let before = c.log();

    let (out, code) = c.run(&["push", "A follow-up", "--why", "next", "--parent", "1"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("g1 is achieved. New work does not open under it."),
        "{out}"
    );
    assert!(
        out.contains("If it really was not finished:  vivac focus 1 --reopen"),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused push wrote to the log");
}

/// `--parent` on a parked node: refused with its own wording, and the node
/// is left parked -- `focus` revives a parked node without asking, and
/// `push --parent` must not do that in silence.
#[test]
fn push_parent_on_a_parked_node_is_refused_and_leaves_it_parked() {
    let c = Sandbox::new_seeded("push-parent-parked");
    c.ok(&["push", "Goal A", "--why", "root of the branch"]);
    c.ok(&["park", "1", "waiting on the review"]);
    let before = c.log();

    let (out, code) = c.run(&["push", "A follow-up", "--why", "next", "--parent", "1"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("g1 is parked. New work does not open under it until someone takes it back."),
        "{out}"
    );
    assert!(out.contains("To take it back:  vivac focus 1"), "{out}");
    assert_eq!(before, c.log(), "a refused push wrote to the log");
    assert_eq!(state_of(&c, "1"), "suspended");
}
