//! `vivac changes` — what a stretch of work moved: opened, closed, flagged,
//! moved, and everything that touched the tree without naming a node move.
//!
//! `t148`: the log has every event and no read handed them back grouped by
//! what happened, so answering "what did this stretch move" meant opening
//! `events` by hand.

mod common;
use common::Sandbox;

fn section(s: &str, title: &str) -> bool {
    s.lines().any(|l| l.trim_start().starts_with(title))
}

/// With no `--since`, the boundary is the last stop, and what came before it
/// does not come back.
#[test]
fn measures_since_the_last_stop_by_default() {
    let c = Sandbox::new_seeded("default-boundary");
    c.ok(&["push", "Old node", "--why", "already handled"]);
    c.ok(&["pop", "done with it"]);
    c.ok(&["save", "checkpoint"]);
    c.ok(&["add", "New node", "--why", "born after the stop"]);
    let s = c.ok(&["changes"]);
    assert!(s.contains("the last stop"), "{s}");
    assert!(s.contains("New node"), "{s}");
    assert!(
        !s.contains("Old node"),
        "it went past its own boundary:\n{s}"
    );
}

/// Right after a stop with nothing behind it, the stretch is empty, and the
/// header still comes out: it is not a check, it always says where it is
/// measuring from.
#[test]
fn right_after_a_stop_nothing_has_moved_and_the_header_still_prints() {
    let c = Sandbox::new_seeded("nothing-moved");
    c.ok(&["push", "Something", "--why", "opened and closed already"]);
    c.ok(&["pop", "done"]);
    c.ok(&["save", "checkpoint"]);
    let s = c.ok(&["changes"]);
    assert!(s.contains("CHANGES SINCE"), "no header:\n{s}");
    assert!(s.contains("Nothing has moved since"), "{s}");
}

/// An older `--since` reaches past the last stop and picks up what it
/// already lost, and the header counts the stops in between instead of
/// calling it the last one.
#[test]
fn an_older_since_includes_what_the_last_stop_no_longer_does() {
    let c = Sandbox::new_seeded("older-since");
    c.ok(&["push", "Node A", "--why", "opened early"]);
    c.ok(&["pop", "closed A"]);
    c.ok(&["push", "Node B", "--why", "opened later"]);
    c.ok(&["pop", "closed B"]);

    let last = c.ok(&["changes"]);
    assert!(!last.contains("Node A"), "{last}");
    assert!(!last.contains("Node B"), "{last}");

    let older = c.ok(&["changes", "--since", "v1"]);
    assert!(older.contains("Node A"), "{older}");
    assert!(older.contains("Node B"), "{older}");
    assert!(older.contains("stops ago"), "{older}");
    assert!(!older.contains("the last stop"), "{older}");
}

/// `--since` with an id that names no vivac is refused, not silently
/// answered from the last stop instead: `park` with an unknown id did
/// exactly that for three releases.
#[test]
fn an_unknown_since_is_refused_and_prints_no_group() {
    let c = Sandbox::new_seeded("unknown-since");
    c.ok(&["push", "Something", "--why", "any reason"]);
    let (out, code) = c.run(&["changes", "--since", "v999"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("v999"), "{out}");
    for g in ["OPENED", "CLOSED", "FLAGGED", "MOVED"] {
        assert!(!out.contains(g), "it printed a group anyway:\n{out}");
    }
}

/// A node born and closed within the same stretch is what happened: it
/// shows up in both groups, not merged into one.
#[test]
fn a_node_born_and_closed_in_the_same_stretch_appears_in_both_groups() {
    let c = Sandbox::new_seeded("born-and-closed");
    c.ok(&["save", "checkpoint"]);
    c.ok(&[
        "push",
        "Quick fix",
        "--why",
        "spotted and fixed immediately",
    ]);
    c.ok(&["pop", "shipped"]);
    let s = c.ok(&["changes", "--since", "v1"]);
    assert!(section(&s, "OPENED"), "{s}");
    assert!(section(&s, "CLOSED"), "{s}");
    assert_eq!(s.matches("Quick fix").count(), 2, "{s}");
}

/// A forced close is the false-marker case the model exists to catch: it
/// cannot pass without a mark on its own line.
#[test]
fn a_forced_close_comes_out_marked() {
    let c = Sandbox::new_seeded("forced-close");
    c.ok(&["save", "checkpoint"]);
    c.ok(&["push", "Audit", "--why", "it is due for review"]);
    c.ok(&[
        "add",
        "Unfixed finding",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "came out of the audit, late",
    ]);
    c.ok(&["done", "1", "closing anyway", "--force"]);
    let s = c.ok(&["changes", "--since", "v1"]);
    assert!(s.contains("forced: closing anyway"), "{s}");
}

/// A stretch that only moved the focus is still a stretch that moved
/// something: the tail line prints and the empty-stretch sentence does not.
#[test]
fn a_stretch_that_only_moved_the_focus_prints_the_tail_line() {
    let c = Sandbox::new_seeded("focus-only");
    c.ok(&["push", "Root", "--why", "top of the stack"]);
    c.ok(&["push", "Child", "--why", "a detour under it"]);
    c.ok(&["save", "checkpoint"]);
    c.ok(&["focus", "1"]);
    let s = c.ok(&["changes"]);
    assert!(!s.contains("Nothing has moved"), "{s}");
    assert!(s.contains("focus move"), "{s}");
}

/// `--json` carries the limit and the four lists, and the `tail` keys come
/// out even at zero: a consumer should not have to guess whether a key is
/// missing or the count is zero.
#[test]
fn the_json_carries_the_limit_and_the_four_lists() {
    let c = Sandbox::new_seeded("json");
    c.ok(&["save", "checkpoint"]);
    // `add` touches no stack and stops none, so the tail stays at zero while
    // `opened` still carries an entry.
    c.ok(&["add", "A goal", "--why", "it is needed"]);
    let s = c.ok(&["changes", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
    assert!(v["since"].is_object(), "{s}");
    for k in ["opened", "closed", "flagged", "moved"] {
        assert!(v[k].is_array(), "{k} missing:\n{s}");
    }
    for k in [
        "focus_moves",
        "notes",
        "flags_cleared",
        "edge_changes",
        "unreadable",
    ] {
        assert_eq!(v["tail"][k], 0, "{k} missing or not zero:\n{s}");
    }
}

/// With no stop at all, the stretch is the whole log, and the header says so
/// instead of naming a vivac that does not exist.
#[test]
fn with_no_stop_at_all_the_header_measures_from_the_beginning() {
    let c = Sandbox::new_seeded("no-stops");
    c.ok(&["add", "Something", "--why", "no stop has happened yet"]);
    let s = c.ok(&["changes"]);
    assert!(
        s.contains("CHANGES SINCE THE BEGINNING - no stops yet"),
        "{s}"
    );
}
