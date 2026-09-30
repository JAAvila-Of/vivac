//! `f355`, `d934`: the focus stays, and what was written away from it shows.
//!
//! `add`, `decide`, `note` and `done` leave the stack alone, so a session
//! that works only through them ended with HERE naming what it started on
//! and nothing on the page saying where the work went. The brief now names
//! the nodes the lane wrote to since its stack last moved, leaving out the
//! focus and what hangs under it, newest first.

mod common;
use common::Sandbox;

const AWAY: &str = "WRITTEN AWAY FROM HERE";

/// Two goals, the focus on the second: `g1` and `g2`.
fn two_goals(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Elsewhere", "--why", "w", "--root"]);
    c.ok(&["pop", "left"]);
    c.ok(&["push", "Here", "--why", "w", "--root"]);
    c
}

/// A finding added under another node is named; one added under the focus
/// is not, since BORN FROM HERE already shows it.
#[test]
fn a_write_away_from_the_focus_is_named() {
    let c = two_goals("written-away");
    c.ok(&[
        "add", "Far off", "--type", "finding", "--why", "w", "--parent", "1",
    ]);
    c.ok(&["add", "Close by", "--type", "finding", "--why", "w"]);
    let out = c.ok(&["brief"]);
    let section = out.split(AWAY).nth(1).expect(&out);
    assert!(section.contains("f3     Far off"), "{out}");
    assert!(!section.contains("Close by"), "{out}");
}

/// A note counts as writing to the node it lands on, and so does a close
/// (below).
#[test]
fn a_note_counts_as_writing() {
    let c = two_goals("written-note");
    c.ok(&["note", "1", "a correction"]);
    let out = c.ok(&["brief"]);
    assert!(out.contains(AWAY), "{out}");
    assert!(out.contains("g1     Elsewhere"), "{out}");
}

/// Moving the focus clears the list: what came before is the old focus's
/// business, not the new one's.
#[test]
fn moving_the_focus_clears_it() {
    let c = two_goals("written-moved");
    c.ok(&[
        "add", "Far off", "--type", "finding", "--why", "w", "--parent", "1",
    ]);
    c.ok(&["push", "Next step", "--why", "w"]);
    let out = c.ok(&["brief"]);
    assert!(!out.contains(AWAY), "{out}");
}

/// Newest first, three by name and the rest counted; a closed node says so.
#[test]
fn newest_first_three_named_and_the_rest_counted() {
    let c = two_goals("written-many");
    for i in 1..=5 {
        c.ok(&[
            "add",
            &format!("Finding {i}"),
            "--type",
            "finding",
            "--why",
            "w",
            "--parent",
            "1",
        ]);
    }
    c.ok(&["done", "f3", "Settled"]);
    let out = c.ok(&["brief"]);
    let section = out.split(AWAY).nth(1).expect(&out);
    let first = section.find("f3     Finding 1").expect(&out);
    let newest = section.find("f7     Finding 5").expect(&out);
    let next = section.find("f6     Finding 4").expect(&out);
    assert!(
        first < newest && newest < next,
        "newest write first:\n{out}"
    );
    assert!(section.contains("[closed]"), "{out}");
    assert!(section.contains("... and 2 more"), "{out}");
    assert!(!section.contains("Finding 3"), "{out}");
}

/// With the stack empty there is no HERE to be away from: everything the
/// lane wrote since it emptied is named.
#[test]
fn with_no_focus_everything_written_since_is_named() {
    let c = two_goals("written-no-focus");
    c.ok(&["pop", "done"]);
    c.ok(&[
        "add",
        "Loose end",
        "--type",
        "finding",
        "--why",
        "w",
        "--parent",
        "2",
    ]);
    let out = c.ok(&["brief"]);
    assert!(out.contains("WRITTEN WITH NO FOCUS"), "{out}");
    assert!(out.contains("f3     Loose end"), "{out}");
    assert!(!out.contains(AWAY), "{out}");
}

/// A session that never wrote away from the focus reads as it always did.
#[test]
fn nothing_written_away_prints_no_section() {
    let c = two_goals("written-none");
    let out = c.ok(&["brief"]);
    assert!(!out.contains("WRITTEN"), "{out}");
}
