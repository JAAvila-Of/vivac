//! `vivac flagged` -- every flag on open work, and the reviews to come. `d909`.
//!
//! Due-ness compares against the real local date: `flagged` takes no
//! `--now`, like `parked`. A sleeping review uses a date far enough ahead
//! that `flag --on` accepts it through the CLI; a due one is written
//! straight into the log, since `flag --on` never accepts a date that has
//! already passed.

mod common;
use common::Sandbox;
use serde_json::Value;

/// The ULID `node.created` gave `num`, read back off the log.
fn node_id_of(c: &Sandbox, num: u64) -> String {
    for line in c.log().lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        if v["payload"]["type"] == "node.created" && v["payload"]["num"] == num {
            return v["payload"]["node"].as_str().unwrap().to_string();
        }
    }
    panic!("no node.created for num {num} in:\n{}", c.log());
}

/// Raises a review on `num` with `on` already in the past, straight in the log.
fn review_already_due(c: &Sandbox, seq: u64, num: u64, on: &str) {
    let node = node_id_of(c, num);
    c.append_raw_line(&format!(
        r#"{{"seq":{seq},"id":"01FLAGGEDDUE{seq:014}","ts":"2020-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{{"type":"flag.raised","node":"{node}","flag":"review","reason":"long overdue","on":"{on}"}}}}"#
    ));
}

/// Four tasks under a goal, for one flag of each kind and one to close.
fn tree(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    for title in [
        "Write the notes",
        "Cut the tag",
        "Tell the users",
        "Old work",
    ] {
        c.ok(&["add", title, "--parent", "1", "--why", "seed"]);
    }
    c
}

#[test]
fn nothing_flagged_says_so() {
    let c = tree("flagged-none");
    let text = c.ok(&["flagged"]);
    assert!(text.contains("  Nothing flagged."), "{text}");
    assert_eq!(c.ok(&["flagged", "--json"]).trim(), "[]");
}

#[test]
fn the_three_groups_each_carry_their_own_count() {
    let c = tree("flagged-groups");
    c.ok(&["flag", "2", "suspect", "--why", "the notes rest on a draft"]);
    review_already_due(&c, 900, 3, "2020-01-02");
    c.ok(&[
        "flag",
        "4",
        "review",
        "--why",
        "look at it again",
        "--on",
        "9999-12-31",
    ]);

    let text = c.ok(&["flagged"]);
    assert!(text.contains("FLAGGED (1)"), "{text}");
    assert!(text.contains("DUE FOR REVIEW (1)"), "{text}");
    assert!(text.contains("REVIEW LATER (1)"), "{text}");
    assert!(
        text.contains("         suspect: the notes rest on a draft"),
        "{text}"
    );
    assert!(text.contains("         long overdue"), "{text}");
    assert!(text.contains("         since 2020-01-02"), "{text}");
    assert!(text.contains("         look at it again"), "{text}");
    assert!(text.contains("         on 9999-12-31"), "{text}");

    let flagged_at = text.find("FLAGGED (1)").unwrap();
    let due_at = text.find("DUE FOR REVIEW (1)").unwrap();
    let later_at = text.find("REVIEW LATER (1)").unwrap();
    assert!(flagged_at < due_at && due_at < later_at, "{text}");
    assert!(
        text[flagged_at..due_at].contains("Write the notes"),
        "{text}"
    );
    assert!(text[due_at..later_at].contains("Cut the tag"), "{text}");
    assert!(text[later_at..].contains("Tell the users"), "{text}");
}

#[test]
fn an_empty_group_is_not_printed() {
    let c = tree("flagged-one-group");
    c.ok(&["flag", "2", "stale", "--why", "the code moved"]);
    let text = c.ok(&["flagged"]);
    assert!(text.contains("FLAGGED (1)"), "{text}");
    assert!(!text.contains("DUE FOR REVIEW"), "{text}");
    assert!(!text.contains("REVIEW LATER"), "{text}");
}

#[test]
fn a_review_with_no_date_is_a_plain_flag() {
    let c = tree("flagged-review-awake");
    c.ok(&["flag", "2", "review", "--why", "look again"]);
    let text = c.ok(&["flagged"]);
    assert!(text.contains("FLAGGED (1)"), "{text}");
    assert!(text.contains("         review: look again"), "{text}");
}

#[test]
fn a_due_review_keeps_the_other_flags_of_its_node_in_flagged() {
    let c = tree("flagged-due-and-suspect");
    review_already_due(&c, 900, 2, "2020-01-02");
    c.ok(&["flag", "2", "suspect", "--why", "the notes rest on a draft"]);
    let text = c.ok(&["flagged"]);
    assert!(text.contains("FLAGGED (1)"), "{text}");
    assert!(text.contains("DUE FOR REVIEW (1)"), "{text}");
    assert!(!text.contains("         review:"), "{text}");
}

#[test]
fn a_closed_node_with_a_flag_is_not_listed() {
    let c = tree("flagged-closed");
    c.ok(&["flag", "5", "suspect", "--why", "it may be wrong"]);
    c.ok(&["done", "5", "shipped"]);
    let text = c.ok(&["flagged"]);
    assert!(text.contains("Nothing flagged."), "{text}");
    assert_eq!(c.ok(&["flagged", "--json"]).trim(), "[]");
}

#[test]
fn the_json_has_one_object_per_flagged_node_with_a_flag_entry_each() {
    let c = tree("flagged-json");
    c.ok(&["flag", "2", "suspect", "--why", "the notes rest on a draft"]);
    c.ok(&[
        "flag",
        "2",
        "review",
        "--why",
        "look again",
        "--on",
        "9999-12-31",
    ]);
    review_already_due(&c, 900, 3, "2020-01-02");

    let v: Value = serde_json::from_str(&c.ok(&["flagged", "--json"])).unwrap();
    let nodes = v.as_array().unwrap();
    assert_eq!(nodes.len(), 2, "{v}");

    assert_eq!(nodes[0]["alias"], "t2", "the shape of json_node is kept");
    let flags = nodes[0]["flags"].as_array().unwrap();
    assert_eq!(flags.len(), 2, "{v}");
    assert_eq!(flags[0]["flag"], "suspect");
    assert_eq!(flags[0]["reason"], "the notes rest on a draft");
    assert!(flags[0]["on"].is_null());
    assert_eq!(flags[0]["due"], false);
    assert_eq!(flags[1]["flag"], "review");
    assert_eq!(flags[1]["on"], "9999-12-31");
    assert_eq!(flags[1]["due"], false);

    let flags = nodes[1]["flags"].as_array().unwrap();
    assert_eq!(flags.len(), 1, "{v}");
    assert_eq!(flags[0]["flag"], "review");
    assert_eq!(flags[0]["on"], "2020-01-02");
    assert_eq!(flags[0]["due"], true);
}
