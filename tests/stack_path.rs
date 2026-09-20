//! The stack is always a contiguous stretch of its top's lineage (`d554`,
//! `t533` §2).
//!
//! `done` and `park` used to unstack a node wherever it sat, which let the
//! stack skip over an ancestor still open below the one just closed. From
//! here on, `done` and `park` only unstack when the node they act on is the
//! stack's own top; anywhere else, the path still runs through it and the
//! spine marks it (`f134`, `f55`).

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

fn why_json(c: &Sandbox, id: &str) -> Value {
    let s = c.ok(&["why", id, "--json"]);
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// `g > t > u`. `done t` (the middle of the stack, not its top) leaves the
/// stack exactly as it was, the footer reports the same depth, and the spine
/// marks `t` as closed. Popping from `u` afterwards still lands back on `t`.
#[test]
fn done_in_the_middle_of_the_stack_does_not_unstack_it() {
    let c = Sandbox::new_seeded("stack-done-middle");
    c.ok(&["push", "Base goal", "--why", "it anchors the branch"]);
    c.ok(&["push", "Task two", "--why", "the goal needs it"]);
    c.ok(&["push", "Task three", "--why", "one more step"]);
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3"]);

    c.ok(&["done", "t2", "settled early"]);
    assert_eq!(
        stack_aliases(&c),
        vec!["g1", "t2", "t3"],
        "done in the middle popped the stack"
    );
    assert_eq!(why_json(&c, "t2")["node"]["state"], "done");

    let brief = c.ok(&["brief"]);
    assert!(brief.contains("depth 3"), "{brief}");
    assert!(brief.contains("Task two  [closed]"), "{brief}");

    let out = c.ok(&["pop", "wrapped up"]);
    assert!(out.contains("back to t2"), "{out}");
    assert_eq!(stack_aliases(&c), vec!["g1", "t2"]);
}

/// The same shape, with `park` instead of `done`: the middle node stays on
/// the stack and the spine marks it parked.
#[test]
fn park_in_the_middle_of_the_stack_does_not_unstack_it() {
    let c = Sandbox::new_seeded("stack-park-middle");
    c.ok(&["push", "Base goal", "--why", "it anchors the branch"]);
    c.ok(&["push", "Task two", "--why", "the goal needs it"]);
    c.ok(&["push", "Task three", "--why", "one more step"]);

    c.ok(&["park", "t2", "stuck on something else"]);
    assert_eq!(
        stack_aliases(&c),
        vec!["g1", "t2", "t3"],
        "park in the middle popped the stack"
    );
    assert_eq!(why_json(&c, "t2")["node"]["state"], "suspended");

    let brief = c.ok(&["brief"]);
    assert!(brief.contains("Task two  [parked]"), "{brief}");

    let parked = c.ok(&["parked"]);
    assert!(parked.contains("Task two"), "{parked}");
}

/// `g > a > b > c > d`, `done g` (the base, not the top), then another push:
/// the depth advice still names the bottom of the stack, now with its mark.
#[test]
fn the_depth_advice_carries_the_bottom_mark_once_it_is_closed() {
    let c = Sandbox::new_seeded("stack-depth-mark");
    c.ok(&["push", "Base goal", "--why", "root of the branch"]);
    c.ok(&["push", "A", "--why", "first step"]);
    c.ok(&["push", "B", "--why", "second step"]);
    c.ok(&["push", "C", "--why", "third step"]);
    c.ok(&["push", "D", "--why", "fourth step"]);
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3", "t4", "t5"]);

    c.ok(&["done", "g1", "superseded already"]);

    let out = c.ok(&["push", "E", "--why", "fifth step"]);
    assert!(
        out.contains("You are 6 levels away from g1 \"Base goal\" [achieved]."),
        "{out}"
    );
}

/// `f552`: a decision that becomes superseded while still on the stack stays
/// superseded when `pop` finally reaches it, its outcome untouched, and the
/// pop line says so instead of claiming a close.
#[test]
fn a_pop_into_a_superseded_focus_leaves_it_as_it_was() {
    let c = Sandbox::new_seeded("stack-pop-superseded");
    c.ok(&["push", "Base goal", "--why", "root of the branch"]);
    c.ok(&[
        "push",
        "Old approach",
        "--why",
        "the call being made",
        "--type",
        "decision",
    ]);
    c.ok(&["push", "Follow-up", "--why", "work under the decision"]);
    c.ok(&[
        "decide",
        "New approach",
        "--reason",
        "the old one did not hold",
        "--supersedes",
        "2",
    ]);
    assert_eq!(why_json(&c, "d2")["node"]["state"], "superseded");
    assert_eq!(why_json(&c, "d2")["node"]["outcome"], "superseded by d4");

    c.ok(&["pop", "follow-up done"]);
    assert_eq!(stack_aliases(&c), vec!["g1", "d2"]);

    let out = c.ok(&["pop", "moving on"]);
    assert!(
        out.contains("d2  Old approach  -> already superseded, left as it was"),
        "{out}"
    );

    assert_eq!(why_json(&c, "d2")["node"]["state"], "superseded");
    assert_eq!(
        why_json(&c, "d2")["node"]["outcome"],
        "superseded by d4",
        "the pop overwrote the outcome the supersession wrote"
    );
}

/// `restore` rebuilds a contiguous path from the point's frozen stack: it
/// starts at the saved node nearest the root that is the deepest still-open
/// node or one of its ancestors, and reports every closed node still on that
/// path, separately from what fell off the stack entirely.
#[test]
fn restore_reports_closed_nodes_still_on_the_path_apart_from_what_left() {
    let c = Sandbox::new_seeded("stack-restore-path");
    c.ok(&["push", "Base goal", "--why", "root of the branch"]);
    c.ok(&["push", "A", "--why", "first step"]);
    c.ok(&["push", "B", "--why", "second step"]);
    c.ok(&["push", "C", "--why", "third step"]);
    let saved = c.ok(&["save", "checkpoint", "--next", "carry on"]);
    let vivac_num = saved
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    c.ok(&["done", "g1", "closed from below"]);

    let out = c.ok(&["restore", &vivac_num]);
    assert!(
        out.contains("still on the path:  g1 Base goal [achieved]"),
        "{out}"
    );
    assert!(
        !out.contains("no longer on the stack"),
        "nothing should have fallen off yet:\n{out}"
    );
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3", "t4"]);

    c.ok(&["done", "t4", "closed the tip"]);
    let out = c.ok(&["restore", &vivac_num]);
    assert!(
        out.contains("still on the path:  g1 Base goal [achieved]"),
        "{out}"
    );
    assert!(
        out.contains("no longer on the stack:  t4 C [closed]"),
        "{out}"
    );
    assert_eq!(stack_aliases(&c), vec!["g1", "t2", "t3"]);
}

// ---------------------------------------------------------------------------
// `stack --lanes` (`t594` §5.5): every lane's own stack, this folder's
// included, marked `(folder gone)` rather than dropped when the registry
// no longer finds that folder on disk.
// ---------------------------------------------------------------------------

/// A second folder, joined to `on`'s own tree as a lane named `name`,
/// with nothing pushed yet -- the same shape `brief`'s own OTHER LANES
/// gone-folder test already uses.
fn join_lane(on: &Sandbox, folder: &str, name: &str) -> Sandbox {
    let joined = Sandbox::new_empty_in(folder, on.global_home());
    joined.ok(&[
        "setup",
        "claude-code",
        "--yes",
        "--join",
        on.0.to_str().unwrap(),
        "--lane-name",
        name,
    ]);
    joined
}

/// `t594` §5.5, decision 2: a lane's folder the registry no longer finds
/// on disk is marked, never dropped -- unlike OTHER LANES, `stack
/// --lanes` exists to name every lane, not only the ones still
/// reachable.
#[test]
fn stack_lanes_marks_a_lane_whose_folder_is_gone() {
    let a = Sandbox::new_seeded("stack-lanes-gone");
    a.ok(&["push", "Track the sonar release", "--why", "seed"]);
    let b = join_lane(&a, "stack-lanes-gone-b", "sonar");
    b.ok(&["push", "Ship the sonar dashboard", "--why", "seed"]);

    // With the folder still there, `sonar` shows with no mark.
    let before = a.ok(&["stack", "--lanes", "--json"]);
    let v: Value = serde_json::from_str(&before)
        .unwrap_or_else(|e| panic!("stack --lanes --json did not print an object: {e}\n{before}"));
    let joined = v["lanes"]
        .as_array()
        .expect("lanes is an array")
        .iter()
        .find(|l| l["name"] == "sonar")
        .unwrap_or_else(|| panic!("the joined lane is missing:\n{before}"));
    assert_eq!(joined["folder_gone"], false, "{before}");
    let text = a.ok(&["stack", "--lanes"]);
    assert!(!text.contains("(folder gone)"), "{text}");

    // Gone, and it stays listed but marked -- never dropped, unlike
    // OTHER LANES (`d33`).
    std::fs::remove_dir_all(&b.0).unwrap();
    let after = a.ok(&["stack", "--lanes", "--json"]);
    let v: Value = serde_json::from_str(&after)
        .unwrap_or_else(|e| panic!("stack --lanes --json did not print an object: {e}\n{after}"));
    let joined = v["lanes"]
        .as_array()
        .expect("lanes is an array")
        .iter()
        .find(|l| l["name"] == "sonar")
        .unwrap_or_else(|| panic!("the joined lane should still be listed once gone:\n{after}"));
    assert_eq!(joined["folder_gone"], true, "{after}");

    let text = a.ok(&["stack", "--lanes"]);
    assert!(text.contains("Ship the sonar dashboard"), "{text}");
    assert!(text.contains("(folder gone)"), "{text}");
}

/// `t594` §5.5: a tree with one lane must not notice this tranche
/// happened. `stack` without `--lanes` prints exactly what it printed
/// before this task, byte for byte, and `--json` carries exactly the
/// same fields.
#[test]
fn stack_without_the_flag_prints_exactly_what_it_did() {
    let c = Sandbox::new_seeded("stack-no-lanes-flag");
    c.ok(&["push", "Base goal", "--why", "it anchors the branch"]);
    c.ok(&["push", "Task two", "--why", "the goal needs it"]);

    let out = c.ok(&["stack"]);
    assert_eq!(
        out, "\n  g1     Base goal\n    t2     Task two   <- focus\n\n",
        "a single-lane tree's stack must read exactly as it did before this tranche"
    );

    let json = c.ok(&["stack", "--json"]);
    let v: Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("stack --json did not print an object: {e}\n{json}"));
    assert_eq!(
        v.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["depth", "stack"],
        "stack --json gained a field without --lanes:\n{json}"
    );
    assert_eq!(v["depth"], 2, "{json}");
}

// ---------------------------------------------------------------------------
// `stack --lanes` names every lane the tree knows, focus or not (`f668`):
// `lanes_with_a_stack` -- and OTHER LANES, which is built on it -- keeps
// filtering out the ones with nothing on their own stack; this list stops
// doing that.
// ---------------------------------------------------------------------------

/// Plants the tree in `on` itself (`--yes`, no terminal needed) without
/// pushing anything, so `on`'s own lane is declared with an empty stack --
/// the plan `stack --lanes` needs a lane with no front of its own to name.
fn setup_with_no_front(on: &Sandbox) {
    on.ok(&["setup", "claude-code", "--yes"]);
}

/// Two lanes, neither with anything pushed: both still have to be named.
#[test]
fn stack_lanes_names_a_lane_with_nothing_pushed() {
    let a = Sandbox::new_seeded("stack-lanes-no-front-both");
    setup_with_no_front(&a);
    join_lane(&a, "stack-lanes-no-front-both-b", "sonar");

    let json = a.ok(&["stack", "--lanes", "--json"]);
    let v: Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("stack --lanes --json did not print an object: {e}\n{json}"));
    let lanes = v["lanes"].as_array().expect("lanes is an array");
    assert_eq!(lanes.len(), 2, "{json}");
    for row in lanes {
        assert!(row["focus"].is_null(), "{json}");
    }

    let text = a.ok(&["stack", "--lanes"]);
    assert!(
        !text.contains("Empty stack"),
        "a tree with lanes must not fall back to the empty-stack text:\n{text}"
    );
    for row in lanes {
        assert!(
            text.contains(row["name"].as_str().unwrap()),
            "missing lane {row} in:\n{text}"
        );
    }
}

/// One lane with a front, one without: both are listed, and the one with a
/// front sorts first -- the same order `lanes_with_a_stack`'s own rows
/// already use among themselves.
#[test]
fn stack_lanes_orders_a_lane_with_a_front_before_one_without() {
    let a = Sandbox::new_seeded("stack-lanes-mixed-front");
    setup_with_no_front(&a);
    let b = join_lane(&a, "stack-lanes-mixed-front-b", "sonar");
    b.ok(&["push", "Ship the sonar dashboard", "--why", "seed"]);

    let json = a.ok(&["stack", "--lanes", "--json"]);
    let v: Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("stack --lanes --json did not print an object: {e}\n{json}"));
    let lanes = v["lanes"].as_array().expect("lanes is an array");
    assert_eq!(lanes.len(), 2, "{json}");
    assert_eq!(lanes[0]["name"], "sonar", "{json}");
    assert!(!lanes[0]["focus"].is_null(), "{json}");
    assert!(lanes[1]["focus"].is_null(), "{json}");

    let text = a.ok(&["stack", "--lanes"]);
    let front_name = lanes[0]["name"].as_str().unwrap();
    let front_at = text
        .find(front_name)
        .unwrap_or_else(|| panic!("missing {front_name} in:\n{text}"));
    let other_name = lanes[1]["name"].as_str().unwrap();
    let other_at = text
        .find(other_name)
        .unwrap_or_else(|| panic!("missing {other_name} in:\n{text}"));
    assert!(
        front_at < other_at,
        "the lane with a front should sort before the one without:\n{text}"
    );
}

/// A tree with no lane at all -- `vivac init`, and nothing else -- names
/// `vivac setup claude-code` instead of the empty-stack text, which answers
/// a different question (`f668`).
#[test]
fn stack_lanes_on_a_tree_with_no_lane_names_setup() {
    let c = Sandbox::new_seeded("stack-lanes-no-lane-at-all");
    let text = c.ok(&["stack", "--lanes"]);
    assert!(text.contains("vivac setup claude-code"), "{text}");
    assert!(
        !text.contains("Empty stack"),
        "a tree with no lane answers a different question than an empty stack:\n{text}"
    );
}
