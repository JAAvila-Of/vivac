//! `t594` tramo 7: `why`'s plain read went back to the derived index once
//! the two facts its "born in lane" line needs -- the `seq` and the lane of
//! a node's own `node.created` -- moved off `Full` (which needed the whole
//! log folded to answer them) and onto the node itself.
//!
//! Deleting the index forces the very fallback fold this is meant to guard:
//! if the index ever persisted those two fields wrong, or dropped them, the
//! read before the delete and the read after it would disagree.

mod common;
use common::Sandbox;

/// Two lanes, each with its own declared repository and `where.changed`, and
/// one node born under each -- the shape `anchor_of` and `born_where` now
/// read straight off the node rather than by walking the log for it.
fn seed_two_lanes(c: &Sandbox) {
    let id = |n: u32| format!("{n:0>26}");
    c.append_raw_line(&format!(
        r#"{{"seq":1,"id":"{}","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{{"type":"lane.declared","lane":"main","name":"main","repos":[{{"path":"webapi"}}]}}}}"#,
        id(1)
    ));
    c.append_raw_line(&format!(
        r#"{{"seq":2,"id":"{}","ts":"2026-01-01T00:00:01Z","actor":"a_test0000000","lane":"main","payload":{{"type":"where.changed","repos":[{{"path":"webapi","branch":"develop"}}]}}}}"#,
        id(2)
    ));
    c.append_raw_line(&format!(
        r#"{{"seq":3,"id":"{}","ts":"2026-01-01T00:00:02Z","actor":"a_test0000000","lane":"main","payload":{{"type":"node.created","node":"{}","num":1,"kind":"task","title":"Born in main","why":"seed"}}}}"#,
        id(3), id(3)
    ));
    c.append_raw_line(&format!(
        r#"{{"seq":4,"id":"{}","ts":"2026-01-01T00:00:03Z","actor":"a_test0000000","lane":"sonar","payload":{{"type":"lane.declared","lane":"sonar","name":"sonar","repos":[{{"path":"service"}}]}}}}"#,
        id(4)
    ));
    c.append_raw_line(&format!(
        r#"{{"seq":5,"id":"{}","ts":"2026-01-01T00:00:04Z","actor":"a_test0000000","lane":"sonar","payload":{{"type":"where.changed","repos":[{{"path":"service","branch":"perf/sp"}}]}}}}"#,
        id(5)
    ));
    c.append_raw_line(&format!(
        r#"{{"seq":6,"id":"{}","ts":"2026-01-01T00:00:05Z","actor":"a_test0000000","lane":"sonar","payload":{{"type":"node.created","node":"{}","num":2,"kind":"task","title":"Born in sonar","why":"seed"}}}}"#,
        id(6), id(6)
    ));
}

/// The fixture actually exercises two different lanes, not one lane twice:
/// the two "born in lane" lines have to differ, or the byte-for-byte
/// comparison below would pass for the wrong reason.
#[test]
fn the_fixture_names_two_different_lanes() {
    let c = Sandbox::new_seeded("why-index-lanes-sanity");
    seed_two_lanes(&c);
    let main_out = c.ok(&["why", "1"]);
    let second_out = c.ok(&["why", "2"]);
    assert!(main_out.contains("born in lane main"), "{main_out}");
    assert!(second_out.contains("born in lane sonar"), "{second_out}");
}

/// The test that decides whether the fix is right: `why`'s plain read (no
/// `--full`) has to print the exact same bytes whether it came off a warm
/// derived index or off a fresh fold of the log, with and without `--json`.
#[test]
fn deleting_the_index_does_not_change_why_with_nodes_born_in_different_lanes() {
    let c = Sandbox::new_seeded("why-index-lanes");
    seed_two_lanes(&c);

    // A plain read persists the derived index (`LOADING.md` §4).
    c.ok(&["stack"]);
    let index_path = c.0.join(".vivac").join("index");
    assert!(index_path.exists(), "no index to delete");

    let main_before = c.ok(&["why", "1"]);
    let main_json_before = c.ok(&["why", "1", "--json"]);
    let second_before = c.ok(&["why", "2"]);
    let second_json_before = c.ok(&["why", "2", "--json"]);

    std::fs::remove_file(&index_path).unwrap();

    assert_eq!(main_before, c.ok(&["why", "1"]));
    assert_eq!(main_json_before, c.ok(&["why", "1", "--json"]));
    assert_eq!(second_before, c.ok(&["why", "2"]));
    assert_eq!(second_json_before, c.ok(&["why", "2", "--json"]));
}
