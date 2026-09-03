//! `why --full` — `t164`: the payload per step of the path that `WEB.md` 3.2
//! needs and the plain `why` never had to carry.
//!
//! The tree the three tests below share means "the ordinary tree" and its
//! shape does not change between them: what changes is only which node they
//! call `why` on.
//!
//! Everything a sandbox does happens inside the same second on the real
//! clock, which is exactly the trap the model warns about: two siblings can
//! open and close on the same calendar day, in an order only the log's `seq`
//! remembers. `siblings_open_and_closed_the_same_day_are_told_apart_by_seq`
//! is the test that would pass for the wrong reason on any implementation
//! that reached for `closed` instead.

mod common;
use common::Sandbox;
use serde_json::Value;

/// `g1` (root), two siblings under it -- one closed before `t4` is born, one
/// closed after -- and `t4` itself, the node every test below asks about.
/// Two decisions are added last, one superseding the other, so their order
/// never lands inside `t4`'s own `num < n.num` window.
fn seeded(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Root of the siblings", "--why", "it needs one"]);
    c.ok(&["add", "Closed before the target was born", "--why", "a"]);
    c.ok(&["add", "Closed after the target was born", "--why", "b"]);
    c.ok(&["done", "2", "settled early"]);
    c.ok(&["add", "The target node", "--why", "c"]);
    c.ok(&["done", "3", "settled late"]);
    c.ok(&["decide", "First call", "--reason", "chosen for x"]);
    c.ok(&[
        "decide",
        "Second call replaces the first",
        "--reason",
        "chosen for y",
        "--supersedes",
        "5",
    ]);
    c
}

fn full_json(c: &Sandbox, id: &str) -> Value {
    let s = c.ok(&["why", id, "--full", "--json"]);
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// The trap the model calls out by name: a date ties two stops on the same
/// day, so "still open when the target was born" has to come from the log's
/// `seq` and not from `closed`. `t2` settled before `t4` existed and must
/// drop out; `t3` settled after and must stay in, even though both `closed`
/// dates read identical to `t4`'s own `opened` date.
#[test]
fn siblings_open_and_closed_the_same_day_are_told_apart_by_seq() {
    let c = seeded("open-then-seq");
    let v = full_json(&c, "4");
    let aliases: Vec<String> = v["node"]["open_then"]
        .as_array()
        .expect("open_then is an array")
        .iter()
        .map(|n| n["alias"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        aliases,
        vec!["t3"],
        "t3 was still open when t4 was born and t2 was not:\n{v}"
    );
}

/// The three fields `--full` adds land on `node` and on every step of
/// `path`, not only on one of the two.
#[test]
fn full_adds_its_three_fields_to_node_and_to_every_step_of_the_path() {
    let c = seeded("full-fields");
    let v = full_json(&c, "4");
    for step in [&v["node"]]
        .into_iter()
        .chain(v["path"].as_array().unwrap())
    {
        for field in ["anchor", "standing", "open_then"] {
            assert!(
                !step[field].is_null(),
                "{field} is missing from a step:\n{step}"
            );
        }
    }
}

/// A superseded decision stops standing; the one that superseded it does.
#[test]
fn a_superseded_decision_drops_out_of_standing_and_its_successor_stands() {
    let c = seeded("standing");
    let v = full_json(&c, "4");
    let root = &v["path"][0];
    assert_eq!(root["alias"], "g1", "the root moved:\n{v}");
    let standing: Vec<String> = root["standing"]
        .as_array()
        .expect("standing is an array")
        .iter()
        .map(|n| n["alias"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !standing.contains(&"d5".to_string()),
        "d5 was superseded and still stands:\n{v}"
    );
    assert!(
        standing.contains(&"d6".to_string()),
        "d6 supersedes d5 and does not stand:\n{v}"
    );
}

/// No git here at all: `anchor` reads empty, and nothing panics on the way.
#[test]
fn anchor_is_empty_and_does_not_panic_with_no_git() {
    let c = seeded("no-git-anchor");
    let (s, code) = c.run(&["why", "4", "--full", "--json"]);
    assert_eq!(code, 0, "it should not have failed:\n{s}");
    let v: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(
        v["node"]["anchor"]["id"], "",
        "anchor should read empty:\n{v}"
    );
}

/// The detached-HEAD shape `src/anchor.rs`'s own
/// `head_is_read_without_spawning_git` reads: `.git/HEAD` holding a sha
/// directly, no ref and no `git` process involved.
fn set_head(c: &Sandbox, sha: &str) {
    let git_dir = c.0.join(".git");
    std::fs::create_dir_all(&git_dir).unwrap();
    std::fs::write(git_dir.join("HEAD"), sha).unwrap();
}

/// `anchor_is_empty_and_does_not_panic_with_no_git` only proves `anchor`
/// does not crash; it would stay green even if `anchor_of` always handed
/// back an empty value. This is the one that looks at the value itself:
/// with a real `.git/HEAD`, the anchor carries that real sha.
#[test]
fn anchor_carries_the_real_sha_from_a_detached_head() {
    let c = Sandbox::new_seeded("anchor-sha");
    let sha = "a".repeat(40);
    set_head(&c, &sha);
    c.ok(&["push", "Node A", "--why", "a"]);
    let v = full_json(&c, "1");
    assert_eq!(
        v["node"]["anchor"]["id"], sha,
        "the anchor did not carry the real sha:\n{v}"
    );
}

/// The spec's own wording: "the anchor in force when that node was born",
/// not the one `HEAD` points to now. `A` is born under one sha and keeps it
/// even after `HEAD` moves on to a second one for `B`.
#[test]
fn anchor_is_the_one_in_force_when_the_node_was_born_not_now() {
    let c = Sandbox::new_seeded("anchor-moment");
    let first = "a".repeat(40);
    let second = "b".repeat(40);
    set_head(&c, &first);
    c.ok(&["push", "Node A", "--why", "a"]);
    set_head(&c, &second);
    c.ok(&["push", "Node B", "--why", "b"]);
    let v = full_json(&c, "2");
    assert_eq!(
        v["path"][0]["anchor"]["id"], first,
        "A should have kept the sha it was born under:\n{v}"
    );
    assert_eq!(
        v["node"]["anchor"]["id"], second,
        "B should carry the sha HEAD moved to:\n{v}"
    );
}

/// `why` without `--full` is untouched: none of the three fields appear,
/// on `node` or anywhere in `path`. This is what keeps the default read from
/// growing heavier for a payload most callers never asked for.
#[test]
fn without_full_the_three_fields_are_absent() {
    let c = seeded("no-full");
    let s = c.ok(&["why", "4", "--json"]);
    let v: Value = serde_json::from_str(&s).unwrap();
    for step in [&v["node"]]
        .into_iter()
        .chain(v["path"].as_array().unwrap())
    {
        for field in ["anchor", "standing", "open_then"] {
            assert!(
                step.get(field).is_none(),
                "{field} leaked into the plain read:\n{v}"
            );
        }
    }
}
