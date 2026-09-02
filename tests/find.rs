//! `find` — text search over the tree.
//!
//! `PILLARS.md` budgets text search at under 100 ms, and nothing implemented
//! it: a ceiling with no floor under it, the same class of unchecked claim as
//! the test count that lied and the minimum toolchain nobody verified.
//!
//! What made it urgent is that searching is the main read of a memory, and a
//! memory you cannot search is a folder. So the search has to reach the
//! fields that carry the meaning -- `why`, the note, the outcome -- and not
//! just the titles, and it has to reach closed nodes, because what you look
//! for months later is usually finished.

mod common;
use common::Sandbox;

/// A tree with the same words spread across different fields, so a test can
/// tell which field a hit came from.
fn seeded(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "the version was a hand edit",
    ]);
    c.ok(&[
        "add",
        "Guard the commit messages",
        "--why",
        "a malformed one does not fail loudly, it silently does not count",
        "--type",
        "task",
    ]);
    c.ok(&[
        "add",
        "The sandbox named its directory from the clock",
        "--why",
        "unique by luck rather than by construction",
        "--type",
        "finding",
    ]);
    c
}

#[test]
fn a_word_in_the_title_is_found() {
    let c = seeded("title");
    let s = c.ok(&["find", "sandbox"]);
    assert!(s.contains("The sandbox named its directory"), "{s}");
}

/// The reason is where the content lives. A search that only reads titles
/// finds the label and misses the thinking.
#[test]
fn a_word_only_in_the_why_is_found() {
    let c = seeded("why");
    let s = c.ok(&["find", "malformed"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

#[test]
fn a_word_only_in_a_note_is_found() {
    let c = seeded("note");
    c.ok(&["note", "t2", "the fixtures live under tests/data"]);
    let s = c.ok(&["find", "fixtures"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

/// What you look for months later is usually finished. A search that stopped
/// at the open fronts would be a to-do list, not a memory.
#[test]
fn a_closed_node_is_still_found() {
    let c = seeded("closed");
    c.ok(&["done", "t2", "the guard runs on every pull request"]);
    let s = c.ok(&["find", "malformed"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

#[test]
fn the_outcome_is_searched_too() {
    let c = seeded("outcome");
    c.ok(&["done", "t2", "it runs on every pull request now"]);
    let s = c.ok(&["find", "pull"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

/// Every term has to appear. Otherwise a second word widens the search
/// instead of narrowing it, which is the opposite of what typing more means.
#[test]
fn every_term_has_to_appear() {
    let c = seeded("terms");
    let both = c.ok(&["find", "guard messages"]);
    assert!(both.contains("Guard the commit messages"), "{both}");
    let one_missing = c.ok(&["find", "guard bicycle"]);
    assert!(
        !one_missing.contains("Guard the commit messages"),
        "a term that appears nowhere still matched:\n{one_missing}"
    );
}

#[test]
fn case_does_not_matter() {
    let c = seeded("case");
    let s = c.ok(&["find", "SANDBOX"]);
    assert!(s.contains("The sandbox named its directory"), "{s}");
}

/// A hit with no lineage is a line of text. The whole product is the edge.
#[test]
fn the_lineage_travels_with_the_hit() {
    let c = seeded("lineage");
    let s = c.ok(&["find", "malformed"]);
    assert!(s.contains("via"), "no lineage:\n{s}");
    assert!(s.contains("g1"), "the lineage does not name the goal:\n{s}");
}

/// A result you cannot judge is noise: the hit says where it matched.
#[test]
fn it_says_where_the_hit_came_from() {
    let c = seeded("field");
    let s = c.ok(&["find", "malformed"]);
    assert!(s.contains("why:"), "it did not name the field:\n{s}");
    assert!(s.contains("malformed"), "it did not show the text:\n{s}");
}

#[test]
fn nothing_matching_says_so_and_is_not_an_error() {
    let c = seeded("empty");
    let (s, code) = c.run(&["find", "bicycle"]);
    assert_eq!(code, 0, "{s}");
    assert!(s.to_lowercase().contains("nothing"), "{s}");
}

#[test]
fn find_without_a_query_is_a_usage_error() {
    let c = seeded("usage");
    let (s, code) = c.run(&["find"]);
    assert_eq!(code, 2, "{s}");
    assert!(s.contains("usage"), "{s}");
}

#[test]
fn the_json_twin_carries_the_hits_and_says_where_they_matched() {
    let c = seeded("json");
    let s = c.ok(&["find", "malformed", "--json"]);
    assert!(s.contains("\"lineage\""), "{s}");
    assert!(s.contains("\"matched\""), "{s}");
    assert!(s.contains("\"why\""), "{s}");
}
