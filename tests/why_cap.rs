//! `d771`: `why` caps how many open siblings and how many open children it
//! lists -- eight of each -- never hiding a blocker, and points at `--full`
//! for whatever the cap left out. Prose and JSON must agree on which ones
//! survive the cap, the same parity `d468` already holds for the rest of
//! `why`'s shape.

mod common;
use common::Sandbox;
use serde_json::Value;

/// Everything after `header` up to the first blank line or a `+ n more`
/// line, one alias per row -- a leading `*` (`born_here`'s blocking marker)
/// is skipped, the same convention `tests/why.rs` uses for the uncapped
/// case.
fn shown_aliases(s: &str, header: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in s.lines() {
        if line.trim_start().starts_with(header) {
            inside = true;
            continue;
        }
        if inside {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('+') {
                break;
            }
            let mut words = trimmed.split_whitespace();
            let mut tok = words.next().expect("a listed line names an alias");
            if tok == "*" {
                tok = words.next().expect("a starred line still names an alias");
            }
            out.push(tok.to_string());
        }
    }
    out
}

/// Root, a target node (`t2`), and twelve open siblings of the target. The
/// fourth one born (`t6`) blocks and sits well outside the seven most
/// recent -- exactly what proves a blocker survives the cap even when its
/// birth is not recent.
fn seeded_with_many_siblings(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Root", "--why", "root reason"]);
    c.ok(&[
        "add",
        "Target node",
        "--parent",
        "1",
        "--why",
        "target reason",
    ]);
    for i in 0..12 {
        let mut args: Vec<&str> = vec!["add", "Sibling", "--parent", "1", "--why", "sibling"];
        if i == 3 {
            args.push("--blocks");
        }
        c.ok(&args);
    }
    c
}

#[test]
fn prose_caps_open_siblings_but_keeps_the_blocker_and_points_at_full() {
    let c = seeded_with_many_siblings("siblings-cap-prose");
    let s = c.ok(&["why", "2"]);
    assert!(
        s.contains("In parallel, still open (12):"),
        "the header must still count every open sibling:\n{s}"
    );
    let shown = shown_aliases(&s, "In parallel, still open (");
    assert_eq!(
        shown,
        vec!["t6", "t8", "t9", "t10", "t11", "t12", "t13", "t14"],
        "the blocker (t6) and the seven most recently opened should survive the cap:\n{s}"
    );
    assert!(
        s.contains("      + 4 more:  vivac why t2 --full"),
        "the cut-off line is missing or malformed:\n{s}"
    );
}

#[test]
fn full_lists_every_open_sibling_with_no_cut_off_line() {
    let c = seeded_with_many_siblings("siblings-full-prose");
    let s = c.ok(&["why", "2", "--full"]);
    assert!(s.contains("In parallel, still open (12):"), "{s}");
    let shown = shown_aliases(&s, "In parallel, still open (");
    assert_eq!(shown.len(), 12, "--full should list every sibling:\n{s}");
    assert!(
        !s.contains("more:  vivac why"),
        "--full must not print a cut-off line:\n{s}"
    );
}

#[test]
fn json_caps_in_parallel_and_matches_the_prose() {
    let c = seeded_with_many_siblings("siblings-cap-json");
    let prose = c.ok(&["why", "2"]);
    let v: Value = serde_json::from_str(&c.ok(&["why", "2", "--json"])).unwrap();
    let siblings = v["in_parallel"].as_array().expect("in_parallel is a list");
    assert_eq!(siblings.len(), 8, "{v}");
    assert_eq!(v["in_parallel_more"], 4, "{v}");
    let json_aliases: Vec<String> = siblings
        .iter()
        .map(|h| h["alias"].as_str().unwrap().to_string())
        .collect();
    let prose_aliases = shown_aliases(&prose, "In parallel, still open (");
    assert_eq!(
        json_aliases, prose_aliases,
        "prose and JSON disagree on which siblings survived the cap:\n{prose}\n{v}"
    );
}

#[test]
fn json_full_lists_every_sibling_and_more_is_zero() {
    let c = seeded_with_many_siblings("siblings-full-json");
    let v: Value = serde_json::from_str(&c.ok(&["why", "2", "--full", "--json"])).unwrap();
    assert_eq!(v["in_parallel"].as_array().unwrap().len(), 12, "{v}");
    assert_eq!(v["in_parallel_more"], 0, "{v}");
}

/// Goal `g1`, nine blocking children and three that do not: enough
/// blockers on their own to clear the cap, so nothing but a blocker should
/// ever be hidden here.
fn seeded_with_many_blocking_children(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Target", "--why", "target reason"]);
    for _ in 0..9 {
        c.ok(&[
            "add",
            "Blocking child",
            "--parent",
            "1",
            "--why",
            "child reason",
            "--blocks",
        ]);
    }
    for _ in 0..3 {
        c.ok(&[
            "add",
            "Open child",
            "--parent",
            "1",
            "--why",
            "child reason",
        ]);
    }
    c
}

#[test]
fn prose_shows_every_blocking_child_even_past_the_cap_and_points_at_full() {
    let c = seeded_with_many_blocking_children("children-cap-prose");
    let s = c.ok(&["why", "1"]);
    assert!(
        s.contains("Born here and still open (12):"),
        "the header must still count every open child:\n{s}"
    );
    let shown = shown_aliases(&s, "Born here and still open (");
    assert_eq!(
        shown,
        vec!["t2", "t3", "t4", "t5", "t6", "t7", "t8", "t9", "t10"],
        "every one of the nine blockers should survive the cap, nothing else:\n{s}"
    );
    assert!(
        s.contains("      + 3 more:  vivac why g1 --full"),
        "the cut-off line is missing or malformed:\n{s}"
    );
}

#[test]
fn full_lists_every_open_child_with_no_cut_off_line() {
    let c = seeded_with_many_blocking_children("children-full-prose");
    let s = c.ok(&["why", "1", "--full"]);
    let shown = shown_aliases(&s, "Born here and still open (");
    assert_eq!(shown.len(), 12, "--full should list every child:\n{s}");
    assert!(
        !s.contains("more:  vivac why"),
        "--full must not print a cut-off line:\n{s}"
    );
}

#[test]
fn json_caps_born_here_and_matches_the_prose() {
    let c = seeded_with_many_blocking_children("children-cap-json");
    let prose = c.ok(&["why", "1"]);
    let v: Value = serde_json::from_str(&c.ok(&["why", "1", "--json"])).unwrap();
    let born_here = v["born_here"].as_array().expect("born_here is a list");
    assert_eq!(born_here.len(), 9, "{v}");
    assert_eq!(v["born_here_more"], 3, "{v}");
    let json_aliases: Vec<String> = born_here
        .iter()
        .map(|h| h["alias"].as_str().unwrap().to_string())
        .collect();
    let prose_aliases = shown_aliases(&prose, "Born here and still open (");
    assert_eq!(
        json_aliases, prose_aliases,
        "prose and JSON disagree on which children survived the cap:\n{prose}\n{v}"
    );
}

#[test]
fn json_full_lists_every_child_and_more_is_zero() {
    let c = seeded_with_many_blocking_children("children-full-json");
    let v: Value = serde_json::from_str(&c.ok(&["why", "1", "--full", "--json"])).unwrap();
    assert_eq!(v["born_here"].as_array().unwrap().len(), 12, "{v}");
    assert_eq!(v["born_here_more"], 0, "{v}");
}
