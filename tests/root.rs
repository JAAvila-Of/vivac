//! `--root` — born with no parent, on `push`, `add` and `decide` (`t533` §1).
//!
//! `push --root` also leaves the stack holding only the new node: what was on
//! it stays open in the tree, nothing closes, and the answer says how to get
//! back (`d551`).

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

fn state_of(c: &Sandbox, id: &str) -> String {
    let s = c.ok(&["why", id, "--json"]);
    let v: Value = serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"));
    v["node"]["state"].as_str().unwrap().to_string()
}

/// `push --root` with three levels already on the stack: the new node is
/// born at the root, the stack ends up with only it left on it, and the
/// three that left are still open in the tree, not closed by this.
#[test]
fn push_root_with_a_stack_leaves_only_the_new_node_on_it() {
    let c = Sandbox::new_seeded("root-push-stack");
    c.ok(&["push", "First goal", "--why", "it came first"]);
    c.ok(&["push", "A branch", "--why", "spotted along the way"]);
    c.ok(&["push", "Deeper still", "--why", "one more level"]);
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3"]);

    let out = c.ok(&["push", "The successor", "--why", "g1 is done for", "--root"]);
    assert!(out.contains("  g4  The successor"), "{out}");
    assert!(
        out.contains("        at the root: 3 left the stack, g1 to t3, none closed by this"),
        "{out}"
    );
    assert!(
        out.contains("        back there with:  vivac focus t3"),
        "{out}"
    );

    assert_eq!(stack_aliases(&c), vec!["g4"]);
    assert_eq!(state_of(&c, "g1"), "active");
    assert_eq!(state_of(&c, "t2"), "active");
    assert_eq!(state_of(&c, "t3"), "active");
}

/// A single node on the stack: the short line, not the plural one.
#[test]
fn push_root_with_a_single_node_prints_the_short_line() {
    let c = Sandbox::new_seeded("root-push-single");
    c.ok(&["push", "First goal", "--why", "it came first"]);

    let out = c.ok(&["push", "The successor", "--why", "moving on", "--root"]);
    assert!(
        out.contains("        at the root: g1 left the stack, not closed by this"),
        "{out}"
    );
    assert!(
        !out.contains("left the stack, g1 to"),
        "it used the plural line for a single node:\n{out}"
    );
    assert!(
        out.contains("        back there with:  vivac focus g1"),
        "{out}"
    );
}

/// An empty stack: `push --root` prints exactly what `push` prints today.
/// `left_stack`/`back_to`, both empty in this case, only reach JSON over MCP
/// -- `push` itself takes no `--json` -- and are covered there.
#[test]
fn push_root_with_an_empty_stack_matches_plain_push() {
    let sandbox_plain = Sandbox::new_seeded("root-push-empty-plain");
    let plain = sandbox_plain.ok(&["push", "First goal", "--why", "it came first"]);

    let sandbox_root = Sandbox::new_seeded("root-push-empty-root");
    let with_root = sandbox_root.ok(&["push", "First goal", "--why", "it came first", "--root"]);
    assert_eq!(plain, with_root);
}

/// `add --root` and `decide --root`: born with no parent, and the stack does
/// not move.
#[test]
fn add_root_and_decide_root_leave_the_stack_alone() {
    let c = Sandbox::new_seeded("root-add-decide");
    c.ok(&["push", "First goal", "--why", "it came first"]);
    let before = stack_aliases(&c);

    let s = c.ok(&[
        "add",
        "A stray finding",
        "--why",
        "noticed in passing",
        "--root",
    ]);
    assert!(s.contains("(root)"), "{s}");
    let v: Value = serde_json::from_str(&c.ok(&["why", "2", "--json"])).unwrap();
    assert_eq!(v["node"]["parent"], Value::Null);
    assert_eq!(stack_aliases(&c), before);

    c.ok(&[
        "decide",
        "Adopt the new approach",
        "--reason",
        "it settles the question",
        "--root",
    ]);
    let v: Value = serde_json::from_str(&c.ok(&["why", "3", "--json"])).unwrap();
    assert_eq!(v["node"]["parent"], Value::Null);
    assert_eq!(stack_aliases(&c), before);
}

/// `--root` together with `--parent` is a usage error naming both flags, on
/// both `add` and `decide`, and nothing gets written on the way.
#[test]
fn root_with_parent_is_refused_on_add_and_decide() {
    let c = Sandbox::new_seeded("root-parent-conflict");
    c.ok(&["push", "First goal", "--why", "it came first"]);
    let before = c.log();

    let (out, code) = c.run(&[
        "add",
        "A stray finding",
        "--parent",
        "1",
        "--why",
        "noticed in passing",
        "--root",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--root and --parent both say where it is born"),
        "{out}"
    );
    assert_eq!(before, c.log(), "add wrote despite the conflict");

    let (out, code) = c.run(&[
        "decide",
        "Adopt the new approach",
        "--reason",
        "it settles the question",
        "--parent",
        "1",
        "--root",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--root and --parent both say where it is born"),
        "{out}"
    );
    assert_eq!(before, c.log(), "decide wrote despite the conflict");

    let (out, code) = c.run(&[
        "push",
        "A follow-up",
        "--why",
        "noticed in passing",
        "--parent",
        "1",
        "--root",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--root and --parent both say where it is born"),
        "{out}"
    );
    assert_eq!(before, c.log(), "push wrote despite the conflict");
}
