//! `decide --parent` — `d181`, symmetric with the flag `add` already has.
//!
//! `decide` used to hang **always** off the focus, with no way to say
//! otherwise. A decision unrelated to the work in progress was born on
//! whatever branch happened to be the focus at the time, and stayed there:
//! the tree does not reparent (`d33`), so a misplaced decision was
//! permanent. `f169` measured it happening for real: a release-policy
//! decision was born under "Build vivac web" only because that goal was the
//! focus when it was signed.

mod common;
use common::Sandbox;
use serde_json::Value;

/// `g1` (root), `t2` under it and stacked as the focus, and `t3` beside `t2`
/// -- also under `g1`, but reached with `add --parent` so the stack never
/// moves onto it. `t3` is where `--parent` points; `t2` is what the focus
/// would give by default.
fn branch(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Ship the release", "--why", "the tag is cut"]);
    c.ok(&["push", "Work in progress", "--why", "what the focus is on"]);
    c.ok(&[
        "add",
        "A different branch",
        "--parent",
        "1",
        "--why",
        "where the decision actually belongs",
    ]);
    c
}

fn why_json(c: &Sandbox, id: &str) -> Value {
    let s = c.ok(&["why", id, "--json"]);
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// `--parent` sends the decision to the node it names, not to the focus.
#[test]
fn decide_with_parent_hangs_off_the_named_node_not_the_focus() {
    let c = branch("decide-parent-wins");
    c.ok(&[
        "decide",
        "Adopt the new policy",
        "--reason",
        "it settles the question",
        "--parent",
        "3",
    ]);
    let v = why_json(&c, "4");
    assert_eq!(
        v["node"]["parent"], "t3",
        "the decision did not hang off the node --parent named:\n{v}"
    );
}

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

/// The stack is untouched: `decide --parent` does not move the focus, the
/// way `add --parent` never has either. Compared by the aliases on the
/// stack rather than the raw JSON, because a new node born anywhere in the
/// tree shifts `open_below`/`total_below` on every ancestor it has -- `g1`
/// included -- without the stack itself moving at all.
#[test]
fn decide_with_parent_leaves_the_focus_where_it_was() {
    let c = branch("decide-parent-focus-stays");
    let before = stack_aliases(&c);
    c.ok(&[
        "decide",
        "Adopt the new policy",
        "--reason",
        "it settles the question",
        "--parent",
        "3",
    ]);
    let after = stack_aliases(&c);
    assert_eq!(
        before, after,
        "decide --parent moved the stack:\nbefore: {before:?}\nafter: {after:?}"
    );
}

/// An id that resolves to nothing is refused, not guessed, and nothing gets
/// written on the way.
#[test]
fn decide_with_a_parent_that_does_not_resolve_fails_and_writes_nothing() {
    let c = branch("decide-parent-does-not-resolve");
    let before = c.log();
    let (out, code) = c.run(&[
        "decide",
        "Adopt the new policy",
        "--reason",
        "it settles the question",
        "--parent",
        "no-such-node",
    ]);
    assert_ne!(
        code, 0,
        "an unresolvable --parent should not succeed:\n{out}"
    );
    assert!(
        out.contains("no-such-node"),
        "the refusal does not name the id that failed to resolve:\n{out}"
    );
    let after = c.log();
    assert_eq!(
        before, after,
        "a failed decide still wrote to the log:\nbefore:\n{before}\nafter:\n{after}"
    );
}
