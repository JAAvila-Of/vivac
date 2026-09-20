//! `why` on the alias of a stop, not a node -- `f547`, `d651`.
//!
//! The brief prints a stop's alias in the same shape as a node's
//! (`v580 - manual - 2026-09-19`), and `why` is the verb that opens whatever
//! an alias names. Before this, `why v580` answered `No such node` even
//! though `v580` named something real: the only way to read a whole stop
//! was `vivac vivacs`, which the brief never names.

mod common;
use common::Sandbox;
use serde_json::Value;

/// Whether `out` contains `sentence`, ignoring where the wrapping put the
/// line breaks -- the same normalisation `tests/lanes.rs`'s own `says` uses,
/// needed here because the alias width changes how many spaces separate an
/// alias from a title.
fn says(out: &str, sentence: &str) -> bool {
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .contains(sentence)
}

fn vivacs_json(c: &Sandbox) -> Value {
    let s = c.ok(&["vivacs", "--json"]);
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

fn find_by_kind<'a>(list: &'a Value, kind: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} stop in {list}"))
}

/// The label, the whole intent and the frozen stack all come through --
/// `next_intent` is never clipped here, unlike the brief's own 52-character
/// cut, because the cut is exactly what sends a reader to open the stop in
/// the first place.
#[test]
fn why_on_a_stops_alias_shows_its_label_full_intent_and_stack() {
    let c = Sandbox::new_seeded("stop-alias");
    c.ok(&["push", "The goal", "--why", "it needs one"]);
    c.ok(&["push", "A step under it", "--why", "the goal needs steps"]);
    let long_intent = "Read every finding filed under the goal before touching \
        the brief again, because the fifth one changes what the fourth one assumed";
    assert!(
        long_intent.len() > 52,
        "the test needs an intent the brief would clip"
    );
    c.ok(&["save", "before lunch", "--next", long_intent]);

    let list = vivacs_json(&c);
    let manual = find_by_kind(&list, "manual");
    let alias = manual["alias"].as_str().unwrap().to_string();

    let out = c.ok(&["why", &alias]);
    assert!(out.contains(&format!("Safe stop  ->  {alias}")), "{out}");
    assert!(out.contains("\"before lunch\""), "{out}");
    assert!(
        out.contains(&format!("you were about to: {long_intent}")),
        "the intent was clipped or missing:\n{out}"
    );
    assert!(out.contains("The stack it carried"), "{out}");
    assert!(says(&out, "g1 The goal"), "{out}");
    assert!(says(&out, "t2 A step under it"), "{out}");

    // The second push froze its own stop with `node_ref` set to the node it
    // forked from, which resolves: `written at` names it.
    let push_stops: Vec<&Value> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["kind"] == "push")
        .collect();
    let forked = push_stops
        .iter()
        .find(|v| !v["node_ref"].is_null())
        .expect("the second push names where it forked from");
    let out2 = c.ok(&["why", forked["alias"].as_str().unwrap()]);
    assert!(says(&out2, "written at g1 The goal"), "{out2}");
}

/// `restore` already opens a stop from any lane of the tree, not only the
/// one in view -- `Tree::vivac` never filters by lane. `why` has to read
/// the same way: a stop is a fact of the tree, not of the lane that wrote
/// it. Fabricated the way `tests/lanes.rs` fabricates shapes no CLI path
/// writes yet, since making an actual second lane is not what this proves.
#[test]
fn a_stop_from_another_lane_still_opens() {
    let c = Sandbox::new_seeded("stop-other-lane");
    c.append_raw_line(
        r#"{"seq":1,"id":"01OTHERLANEEVENTAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"another","payload":{"type":"vivac.created","vivac":"01OTHERLANEVIVACAAAAAAAA","num":1,"kind":"manual","stack":[["g9","A goal in the other lane"]],"working_set":[],"next_intent":"finish the other lanes work","anchor":{"kind":"","id":""},"node_ref":null,"label":"stop from the other lane"}}"#,
    );

    let out = c.ok(&["why", "v1"]);
    assert!(out.contains("Safe stop  ->  v1"), "{out}");
    assert!(out.contains("\"stop from the other lane\""), "{out}");
    assert!(says(&out, "g9 A goal in the other lane"), "{out}");
}

/// A stop with no label and no intent prints neither line, and the command
/// still succeeds -- both fields are genuinely optional (`session end`
/// writes plenty of stops with no label at all).
#[test]
fn a_stop_with_no_label_and_no_intent_omits_both_lines() {
    let c = Sandbox::new_seeded("stop-bare");
    c.ok(&["push", "A goal", "--why", "it needs one"]);
    c.ok(&["save"]);

    let list = vivacs_json(&c);
    let manual = find_by_kind(&list, "manual");
    let alias = manual["alias"].as_str().unwrap().to_string();
    assert_eq!(manual["label"], "");
    assert_eq!(manual["next_intent"], "");

    let (out, code) = c.run(&["why", &alias]);
    assert_eq!(code, 0, "{out}");
    assert!(
        !out.contains('"'),
        "an empty label still printed quotes:\n{out}"
    );
    assert!(!out.contains("you were about to"), "{out}");
    assert!(out.contains("The stack it carried"), "{out}");
}

/// A stop alias that names nothing real still refuses the same way a node
/// id that names nothing real does: `f547`'s own reproduction had two real
/// stops on the tree and a third number that named neither a node nor one.
#[test]
fn a_missing_stops_alias_still_says_no_such_node() {
    let c = Sandbox::new_seeded("stop-missing");
    c.ok(&["push", "A goal", "--why", "it needs one"]);
    c.ok(&["save", "checkpoint"]);
    let list = vivacs_json(&c);
    assert_eq!(list.as_array().unwrap().len(), 2, "{list}");

    let (out, code) = c.run(&["why", "v99"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("No such node: v99."), "{out}");
}

/// `why` on a stop's alias, with `--json`, gives exactly the object
/// `vivacs --json` gives for the same stop -- loose, not inside a list --
/// because the two functions share one builder (`vivac_json`) and cannot
/// answer the same question two different ways.
#[test]
fn why_json_of_a_stop_matches_its_entry_in_vivacs_json() {
    let c = Sandbox::new_seeded("stop-json");
    c.ok(&["push", "A goal", "--why", "it needs one"]);
    c.ok(&["save", "checkpoint", "--next", "keep going"]);

    let list = vivacs_json(&c);
    let manual = find_by_kind(&list, "manual").clone();
    let alias = manual["alias"].as_str().unwrap().to_string();

    let s = c.ok(&["why", &alias, "--json"]);
    let why_json: Value = serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"));
    assert_eq!(
        why_json, manual,
        "\nwhy --json:  {why_json}\nvivacs entry: {manual}"
    );
}
