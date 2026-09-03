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

/// `--since manual` reaches past the stops the hook wrote, which is the whole
/// reason it exists: the last stop is written on every turn that moves the
/// tree, so on its own it means "since my last action".
#[test]
fn since_manual_reaches_past_the_stops_the_hook_wrote() {
    let c = Sandbox::new_seeded("since-manual");
    c.ok(&["add", "Before the save", "--why", "opened earlier"]);
    c.ok(&["save", "where I stopped looking"]);
    c.ok(&[
        "add",
        "First after the save",
        "--why",
        "opened right after the stop was made",
    ]);
    // `push` and `pop` leave stops of their own, and neither is one a person
    // sat down and made.
    c.ok(&["push", "A detour", "--why", "it leaves a stop behind it"]);
    c.ok(&["pop", "and another one on the way out"]);
    c.ok(&[
        "add",
        "Second after the save",
        "--why",
        "opened after two more stops",
    ]);

    let manual = c.ok(&["changes", "--since", "manual"]);
    assert!(manual.contains("First after the save"), "{manual}");
    assert!(manual.contains("Second after the save"), "{manual}");
    assert!(!manual.contains("Before the save"), "{manual}");

    let default = c.ok(&["changes"]);
    assert!(
        !default.contains("First after the save"),
        "the last stop alone reached too far back:
{default}"
    );
    assert!(default.contains("Second after the save"), "{default}");
}

/// The header says the boundary was made by hand, and counts the stops
/// written since as `since` rather than `ago`: they are inside the stretch,
/// not between it and now.
#[test]
fn since_manual_says_so_in_the_header() {
    let c = Sandbox::new_seeded("since-manual-header");
    c.ok(&["add", "Something", "--why", "so a stop has work behind it"]);
    c.ok(&["save", "a stop made by hand"]);
    c.ok(&[
        "add",
        "After it",
        "--why",
        "so the hook has something to see",
    ]);
    let s = c.ok(&["changes", "--since", "manual"]);
    assert!(s.contains("the last stop you made"), "{s}");
}

/// A tree whose every stop came from the hook cannot answer `--since manual`
/// with one, so it falls back to the beginning and says why, and it still
/// exits `0`: this is a reading, not a check.
#[test]
fn since_manual_with_no_stop_made_by_hand_falls_back_to_the_beginning() {
    let c = Sandbox::new_seeded("since-manual-none");
    let (s, code) = c.run(&["changes", "--since", "manual"]);
    assert_eq!(code, 0, "{s}");
    assert!(s.contains("CHANGES SINCE THE BEGINNING"), "{s}");
    assert!(s.contains("no stop here was made by hand"), "{s}");
}

/// A stop made by hand with nothing after it is an empty stretch, not an
/// error, and the sentence names the stop.
#[test]
fn since_manual_with_nothing_after_it_says_nothing_moved() {
    let c = Sandbox::new_seeded("since-manual-empty");
    c.ok(&[
        "add",
        "Something",
        "--why",
        "so the stop has work behind it",
    ]);
    c.ok(&["save", "nothing follows this"]);
    let s = c.ok(&["changes", "--since", "manual"]);
    assert!(s.contains("Nothing has moved since"), "{s}");
}

/// The JSON names which boundary answered, so a consumer does not have to
/// guess it from which fields showed up.
#[test]
fn the_json_names_which_boundary_answered() {
    let c = Sandbox::new_seeded("since-manual-json");
    c.ok(&[
        "add",
        "Something",
        "--why",
        "so the stop has work behind it",
    ]);
    c.ok(&["save", "checkpoint"]);

    let manual_out = c.ok(&["changes", "--since", "manual", "--json"]);
    let manual_json: serde_json::Value = serde_json::from_str(&manual_out).expect("valid json");
    assert_eq!(manual_json["since"]["kind"], "manual", "{manual_out}");

    let stop_out = c.ok(&["changes", "--json"]);
    let stop_json: serde_json::Value = serde_json::from_str(&stop_out).expect("valid json");
    assert_eq!(stop_json["since"]["kind"], "stop", "{stop_out}");
    assert!(stop_json["since"]["alias"].is_string(), "{stop_out}");
    assert!(stop_json["since"]["ts"].is_string(), "{stop_out}");
    assert!(stop_json["since"]["stops_since"].is_number(), "{stop_out}");
}

/// `--since v999` is refused, and the message names `manual` as the other way
/// of spelling a boundary, so a slip between the two is easy to fix.
#[test]
fn an_unknown_since_still_names_manual_as_an_option() {
    let c = Sandbox::new_seeded("unknown-since-mentions-manual");
    let (out, code) = c.run(&["changes", "--since", "v999"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("manual"), "{out}");
}
