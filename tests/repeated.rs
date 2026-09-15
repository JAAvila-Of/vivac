//! `t429`'s second fix, by `d423`'s rule: a number two nodes share is
//! something the tree can only show one half of, so it says so.

mod common;
use common::Sandbox;
use serde_json::Value;

/// A second `node.created` for number 1, the shape a merged log leaves.
const SECOND_CLAIMANT: &str = r#"{"seq":900,"id":"01REPEATEDNUMAAAAAAAAAAAAA","ts":"2026-09-15T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"node.created","node":"01REPEATEDNUMBBBBBBBBBBBBB","num":1,"kind":"task","title":"The second claimant of number one"}}"#;

fn with_a_repeated_number(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "The first claimant of number one",
        "--why",
        "it came first",
    ]);
    c.append_raw_line(SECOND_CLAIMANT);
    c
}

#[test]
fn tree_says_a_number_is_repeated() {
    let c = with_a_repeated_number("repeated-tree");
    let out = c.ok(&["tree"]);
    assert!(
        out.contains("1 is repeated: g1 is shown and t1 is not"),
        "{out}"
    );
}

#[test]
fn why_says_the_node_shares_its_number() {
    let c = with_a_repeated_number("repeated-why");
    let out = c.ok(&["why", "g1"]);
    assert!(out.contains("g1 also names another node, t1"), "{out}");
}

#[test]
fn the_brief_says_there_are_repeated_numbers() {
    let c = with_a_repeated_number("repeated-brief");
    let out = c.ok(&["brief"]);
    assert!(out.contains("REPEATED NUMBERS  1"), "{out}");
}

#[test]
fn a_tree_with_no_repeated_number_says_nothing_about_it() {
    let c = Sandbox::new_seeded("not-repeated");
    c.ok(&["push", "Only one", "--why", "nothing repeats"]);
    assert!(!c.ok(&["tree"]).contains("is repeated"));
    assert!(!c.ok(&["why", "g1"]).contains("also names another node"));
    assert!(!c.ok(&["brief"]).contains("REPEATED NUMBERS"));
}

#[test]
fn why_json_names_the_hidden_claimant() {
    let c = with_a_repeated_number("repeated-why-json");
    let v: Value = serde_json::from_str(&c.ok(&["why", "g1", "--json"])).unwrap();
    assert_eq!(v["node"]["repeated"]["num"], 1, "{v}");
    assert_eq!(v["node"]["repeated"]["hidden"], "t1", "{v}");
}

#[test]
fn why_json_carries_no_repeated_key_when_nothing_repeats() {
    let c = Sandbox::new_seeded("not-repeated-json");
    c.ok(&["push", "Only one", "--why", "nothing repeats"]);
    let v: Value = serde_json::from_str(&c.ok(&["why", "g1", "--json"])).unwrap();
    assert!(
        v["node"].as_object().unwrap().get("repeated").is_none(),
        "{v}"
    );
}

/// Six repeated numbers, so the brief's five-wide cap actually has something
/// to cut: 1 through 5 named, then a `+1` for the one left out.
#[test]
fn the_brief_caps_the_repeated_numbers_list_at_five() {
    let c = Sandbox::new_seeded("repeated-brief-cap");
    for i in 1..=6 {
        c.ok(&[
            "push",
            &format!("The first claimant of number {i}"),
            "--why",
            "it came first",
            "--root",
        ]);
    }
    for i in 1..=6u64 {
        c.append_raw_line(&format!(
            r#"{{"seq":{seq},"id":"01REPEATEDNUM{i}AAAAAAAAAAAA","ts":"2026-09-15T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{{"type":"node.created","node":"01REPEATEDNUM{i}BBBBBBBBBBBB","num":{i},"kind":"task","title":"The second claimant of number {i}"}}}}"#,
            seq = 900 + i,
        ));
    }
    let out = c.ok(&["brief"]);
    assert!(out.contains("REPEATED NUMBERS  1, 2, 3, 4, 5, +1"), "{out}");
}
