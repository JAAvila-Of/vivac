//! `f433`, `d931`: what one `why` costs.
//!
//! The newest three notes of a node come whole and the rest are counted,
//! with the command that brings them; `--full` brings everything. `--only`
//! is the node alone, for walking siblings whose shared path was already
//! read once.

mod common;
use common::Sandbox;

fn noted(name: &str, notes: usize) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Goal", "--why", "the branch"]);
    c.ok(&["push", "Step", "--why", "the work"]);
    for i in 1..=notes {
        c.ok(&["note", "2", &format!("note number {i}")]);
    }
    c
}

/// Five notes: the newest three whole, the other two counted.
#[test]
fn why_shows_the_newest_three_notes_and_counts_the_rest() {
    let c = noted("why-notes-cap", 5);
    let out = c.ok(&["why", "2"]);
    assert!(
        out.contains("2 earlier notes:  vivac why t2 --full"),
        "{out}"
    );
    for i in 3..=5 {
        assert!(out.contains(&format!("note number {i}")), "{out}");
    }
    assert!(!out.contains("note number 1"), "{out}");
    assert!(!out.contains("note number 2"), "{out}");
}

/// `--full` brings every note, and says nothing about earlier ones.
#[test]
fn full_brings_every_note() {
    let c = noted("why-notes-full", 5);
    let out = c.ok(&["why", "2", "--full"]);
    for i in 1..=5 {
        assert!(out.contains(&format!("note number {i}")), "{out}");
    }
    assert!(!out.contains("earlier note"), "{out}");
}

/// Three notes or fewer lose nothing and print no count.
#[test]
fn three_notes_print_as_before() {
    let c = noted("why-notes-three", 3);
    let out = c.ok(&["why", "2"]);
    assert!(out.contains("note number 1"), "{out}");
    assert!(!out.contains("earlier note"), "{out}");
}

/// A step of the path gets the same cap, and its count points at itself.
#[test]
fn an_ancestor_is_capped_the_same_way() {
    let c = Sandbox::new_seeded("why-notes-path");
    c.ok(&["push", "Goal", "--why", "the branch"]);
    for i in 1..=4 {
        c.ok(&["note", "1", &format!("goal note {i}")]);
    }
    c.ok(&["push", "Step", "--why", "the work"]);
    let out = c.ok(&["why", "2"]);
    assert!(
        out.contains("1 earlier note:  vivac why g1 --full"),
        "{out}"
    );
    assert!(!out.contains("goal note 1"), "{out}");
    assert!(out.contains("goal note 4"), "{out}");
}

/// The JSON keeps the same notes and says how many went.
#[test]
fn the_json_keeps_the_newest_and_counts_the_rest() {
    let c = noted("why-notes-json", 5);
    let v: serde_json::Value = serde_json::from_str(&c.ok(&["why", "2", "--json"])).unwrap();
    assert_eq!(v["node"]["notes"].as_array().unwrap().len(), 3, "{v}");
    assert_eq!(v["node"]["notes_earlier"], 2, "{v}");
    assert_eq!(v["node"]["notes"][0]["note"], "note number 3", "{v}");

    let full: serde_json::Value =
        serde_json::from_str(&c.ok(&["why", "2", "--json", "--full"])).unwrap();
    assert_eq!(full["node"]["notes"].as_array().unwrap().len(), 5, "{full}");
    assert!(full["node"].get("notes_earlier").is_none(), "{full}");
}

/// `--only` prints the node and none of its neighbourhood.
#[test]
fn only_prints_the_node_alone() {
    let c = Sandbox::new_seeded("why-only");
    c.ok(&["push", "Goal", "--why", "the branch"]);
    c.ok(&["add", "Sibling one", "--why", "w"]);
    c.ok(&["push", "Step", "--why", "the work"]);
    c.ok(&["add", "A child", "--why", "w"]);
    let out = c.ok(&["why", "3", "--only"]);
    assert!(out.contains("Step"), "{out}");
    assert!(out.contains("the work"), "{out}");
    for gone in [
        "Goal",
        "Sibling one",
        "A child",
        "In parallel",
        "you are here",
    ] {
        assert!(!out.contains(gone), "{gone} came with --only:\n{out}");
    }

    let v: serde_json::Value =
        serde_json::from_str(&c.ok(&["why", "3", "--only", "--json"])).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    assert_eq!(keys, vec!["node"], "{v}");
}
