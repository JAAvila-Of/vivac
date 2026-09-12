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
use std::collections::BTreeSet;

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

/// Strips `--full`'s three own fields from every object in `v`, wherever
/// they sit -- `node`, any step of `path`, and inside the `standing` /
/// `open_then` nodes nested under those.
fn without_full_fields(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "anchor" | "standing" | "open_then"))
                .map(|(k, v)| (k.clone(), without_full_fields(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_full_fields).collect()),
        other => other.clone(),
    }
}

/// `main.rs` loads `why` two different ways depending on `--full`, so it
/// can it use the derived index when the log is not needed -- and the two
/// paths have no business disagreeing about anything neither one is
/// responsible for. This checks that directly, rather than by field
/// presence alone: the plain read has to equal `--full`'s own output with
/// exactly the three fields it adds removed, nothing more and nothing less.
///
/// `t465` clips an ancestor step's `why`, `notes` and `outcome` when `--full`
/// is absent, so this equality only holds because `seeded`'s bodies are all
/// shorter than `ANCESTOR_CLIP` -- clipping a string that already fits is a
/// no-op. A fixture with long bodies would make the plain read and `--full`
/// disagree on purpose, and that disagreement is what the next test proves.
#[test]
fn the_plain_read_is_full_with_its_three_fields_removed() {
    let c = seeded("plain-equals-full-stripped");
    let plain: Value = serde_json::from_str(&c.ok(&["why", "4", "--json"])).unwrap();
    let full = full_json(&c, "4");
    assert_eq!(
        plain,
        without_full_fields(&full),
        "why and why --full disagree once --full's own fields are stripped:\n\
         plain={plain}\nfull={full}"
    );
}

/// `d330`: the plain, non-JSON render of `why` clips an ancestor's body so
/// that reading a deep node no longer drags eleven whole ancestor bodies
/// along with it. `--full` keeps giving the body of every step whole, and
/// the node actually asked about is never clipped, in either mode.
///
/// Each field carries its own open/close marker pair around filler long
/// enough to cross the clip length, so a bug that clips the wrong field, or
/// clips a field partway through the open marker, fails for a distinct
/// reason instead of one test standing in for three.
const ANCESTOR_PADDING: &str =
    "filler word after filler word after filler word after filler word after filler word after filler word after filler word after filler word after";
fn long_field(tag: &str) -> String {
    format!("{tag}-open {ANCESTOR_PADDING} {tag}-close")
}

/// The root carries a why, a note and an outcome that all run well past the
/// clip length used for an ancestor's body. The target is a plain child
/// added under the root by id, once the root itself has already closed, so
/// closing the root does not have to fight the stack for focus.
fn seeded_with_long_bodies(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "Root ancestor with a long body",
        "--why",
        &long_field("anc-why"),
    ]);
    c.ok(&["note", "1", &long_field("anc-note")]);
    c.ok(&["done", "1", &long_field("anc-outcome")]);
    c.ok(&[
        "add",
        "The target node",
        "--parent",
        "1",
        "--why",
        &long_field("tgt-why"),
    ]);
    c.ok(&["note", "2", &long_field("tgt-note")]);
    c.ok(&["done", "2", &long_field("tgt-outcome")]);
    c
}

#[test]
fn full_prints_the_ancestor_body_whole() {
    let c = seeded_with_long_bodies("ancestor-full");
    let s = c.ok(&["why", "2", "--full"]);
    assert!(
        s.contains("anc-why-close"),
        "--full dropped the tail of the ancestor's why:\n{s}"
    );
    assert!(
        s.contains("anc-note-close"),
        "--full dropped the tail of the ancestor's note:\n{s}"
    );
    assert!(
        s.contains("anc-outcome-close"),
        "--full dropped the tail of the ancestor's outcome:\n{s}"
    );
}

#[test]
fn without_full_the_ancestor_body_is_truncated() {
    let c = seeded_with_long_bodies("ancestor-clipped");
    let s = c.ok(&["why", "2"]);
    assert!(
        s.contains("anc-why-open"),
        "the start of the ancestor's why should still be there:\n{s}"
    );
    assert!(
        !s.contains("anc-why-close"),
        "the ancestor's why should have been clipped:\n{s}"
    );
    assert!(
        !s.contains("anc-note-close"),
        "the ancestor's note should have been clipped:\n{s}"
    );
    assert!(
        !s.contains("anc-outcome-close"),
        "the ancestor's outcome should have been clipped:\n{s}"
    );
    assert!(
        s.contains("..."),
        "a clipped body needs its own truncation mark visible:\n{s}"
    );
}

#[test]
fn the_requested_node_prints_whole_with_or_without_full() {
    let c = seeded_with_long_bodies("target-whole");
    for args in [&["why", "2"][..], &["why", "2", "--full"][..]] {
        let s = c.ok(args);
        assert!(
            s.contains("tgt-why-close"),
            "{args:?} clipped the requested node's own why:\n{s}"
        );
        assert!(
            s.contains("tgt-note-close"),
            "{args:?} clipped the requested node's own note:\n{s}"
        );
        assert!(
            s.contains("tgt-outcome-close"),
            "{args:?} clipped the requested node's own outcome:\n{s}"
        );
    }
}

/// `f389`: a second note used to overwrite the first everywhere `why` reads
/// from, with no sign anything had been dropped. Once there is more than
/// one, every one of them has to print, each with the date it was written
/// (`d390`).
#[test]
fn why_prints_every_note_with_its_date_once_there_is_more_than_one() {
    let c = Sandbox::new_seeded("two-notes");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["note", "1", "first note"]);
    c.ok(&["note", "1", "second note"]);
    let s = c.ok(&["why", "1"]);
    assert!(
        s.contains("] first note"),
        "the first note is missing:\n{s}"
    );
    assert!(
        s.contains("] second note"),
        "the second note is missing:\n{s}"
    );
    assert!(s.contains("! ["), "neither note carries a date:\n{s}");
}

/// The regression the date above can introduce: a node with exactly one
/// note has to keep reading exactly as it did before `d390`, with no date in
/// front of it -- the same argument `f186` made for the lineage's anchor,
/// applied here.
#[test]
fn why_prints_a_single_note_with_no_date() {
    let c = Sandbox::new_seeded("one-note");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["note", "1", "the only note"]);
    let s = c.ok(&["why", "1"]);
    assert!(s.contains("! the only note"), "{s}");
    assert!(
        !s.contains("! ["),
        "a single note must not carry a date:\n{s}"
    );
}

/// The JSON twin of the two tests above: `note` keeps naming the latest one,
/// nothing that reads that field breaks, and `notes` carries every one of
/// them in the order they were written (`d390`).
#[test]
fn the_json_carries_every_note_and_note_still_carries_the_last() {
    let c = Sandbox::new_seeded("json-notes");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["note", "1", "first note"]);
    c.ok(&["note", "1", "second note"]);
    let s = c.ok(&["why", "1", "--json"]);
    let v: Value = serde_json::from_str(&s).unwrap();
    let notes = v["node"]["notes"].as_array().expect("notes is an array");
    assert_eq!(notes.len(), 2, "{v}");
    assert_eq!(notes[0]["note"], "first note", "{v}");
    assert_eq!(notes[1]["note"], "second note", "{v}");
    assert_eq!(
        v["node"]["note"], "second note",
        "note should still name the latest one:\n{v}"
    );
}

/// The JSON mirror of `without_full_the_ancestor_body_is_truncated` and
/// `full_prints_the_ancestor_body_whole`: `path`'s ancestor clips its `why`,
/// each `notes[].note` and its `outcome` the same way the prose does, under
/// the same `ANCESTOR_CLIP`, and `--full` leaves them whole. `node` never
/// clips, in either mode, because it is the one thing `why` was actually
/// asked about.
#[test]
fn path_clip_matches_the_prose_and_full_leaves_it_whole() {
    let c = seeded_with_long_bodies("json-ancestor-clip");
    let plain: Value = serde_json::from_str(&c.ok(&["why", "2", "--json"])).unwrap();
    let full = full_json(&c, "2");

    let why_plain = plain["path"][0]["why"].as_str().unwrap();
    assert!(
        why_plain.contains("anc-why-open"),
        "the start of the ancestor's why should survive the clip:\n{plain}"
    );
    assert!(
        !why_plain.contains("anc-why-close"),
        "the ancestor's why should have been clipped:\n{plain}"
    );
    assert!(
        why_plain.contains("..."),
        "a clipped why needs its own truncation mark:\n{plain}"
    );
    assert!(
        !plain["path"][0]["notes"][0]["note"]
            .as_str()
            .unwrap()
            .contains("anc-note-close"),
        "the ancestor's note should have been clipped:\n{plain}"
    );
    assert!(
        !plain["path"][0]["outcome"]
            .as_str()
            .unwrap()
            .contains("anc-outcome-close"),
        "the ancestor's outcome should have been clipped:\n{plain}"
    );

    assert!(
        full["path"][0]["why"]
            .as_str()
            .unwrap()
            .contains("anc-why-close"),
        "--full should carry the ancestor's why whole:\n{full}"
    );
    assert!(
        full["path"][0]["notes"][0]["note"]
            .as_str()
            .unwrap()
            .contains("anc-note-close"),
        "--full should carry the ancestor's note whole:\n{full}"
    );
    assert!(
        full["path"][0]["outcome"]
            .as_str()
            .unwrap()
            .contains("anc-outcome-close"),
        "--full should carry the ancestor's outcome whole:\n{full}"
    );

    assert!(
        plain["node"]["why"]
            .as_str()
            .unwrap()
            .contains("tgt-why-close"),
        "the requested node's own why must never clip:\n{plain}"
    );
}

/// Every handle -- `in_parallel`, and `node`'s `standing` and `open_then`
/// under `--full` -- carries exactly `alias`, `kind`, `state` and `title`,
/// nothing more and nothing less; a `born_here` handle carries those four
/// plus `blocks`, because the prose stars the ones that keep their parent
/// from closing.
#[test]
fn every_handle_carries_exactly_its_own_fields() {
    let c = Sandbox::new_seeded("handle-keys");
    c.ok(&["push", "Root", "--why", "root reason"]);
    c.ok(&[
        "add",
        "Sibling of the target, still open",
        "--parent",
        "1",
        "--why",
        "sibling reason",
    ]);
    c.ok(&[
        "add",
        "The target node",
        "--parent",
        "1",
        "--why",
        "target reason",
    ]);
    c.ok(&[
        "add",
        "Child of the target, still open",
        "--parent",
        "3",
        "--why",
        "child reason",
        "--blocks",
    ]);
    c.ok(&[
        "decide",
        "Decision under the target",
        "--parent",
        "3",
        "--reason",
        "decision reason",
    ]);

    let v = full_json(&c, "3");

    let keys_of = |h: &Value| -> BTreeSet<String> {
        h.as_object()
            .unwrap_or_else(|| panic!("not an object: {h}"))
            .keys()
            .cloned()
            .collect()
    };
    let handle_keys: BTreeSet<String> = ["alias", "kind", "state", "title"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let born_here_keys: BTreeSet<String> = ["alias", "kind", "state", "title", "blocks"]
        .into_iter()
        .map(str::to_string)
        .collect();

    let siblings = v["in_parallel"].as_array().unwrap();
    assert!(
        !siblings.is_empty(),
        "the fixture should carry an open sibling:\n{v}"
    );
    for h in siblings {
        assert_eq!(
            keys_of(h),
            handle_keys,
            "in_parallel carries the wrong keys:\n{v}"
        );
    }

    let standing = v["node"]["standing"].as_array().unwrap();
    assert!(
        !standing.is_empty(),
        "the fixture should carry a standing decision:\n{v}"
    );
    for h in standing {
        assert_eq!(
            keys_of(h),
            handle_keys,
            "standing carries the wrong keys:\n{v}"
        );
    }

    let open_then = v["node"]["open_then"].as_array().unwrap();
    assert!(
        !open_then.is_empty(),
        "the fixture should carry an open_then sibling:\n{v}"
    );
    for h in open_then {
        assert_eq!(
            keys_of(h),
            handle_keys,
            "open_then carries the wrong keys:\n{v}"
        );
    }

    let born_here = v["born_here"].as_array().unwrap();
    assert_eq!(
        born_here.len(),
        2,
        "the fixture should carry two open children:\n{v}"
    );
    for h in born_here {
        assert_eq!(
            keys_of(h),
            born_here_keys,
            "born_here carries the wrong keys:\n{v}"
        );
    }
}

/// `t465`: `blockers` walks every step of the path **with the node itself
/// included**, using `blocking_of` rather than a flat `open_blockers` read
/// off the node alone. An ancestor further up that is still open and has its
/// own open blocker shows up here, blocker included; a step that already
/// closed answers empty, even with an open blocker left behind -- that is a
/// false close, not a debt, and `triage` is where that gets reported instead.
#[test]
fn blockers_reaches_an_open_ancestor_and_skips_a_closed_one() {
    let c = Sandbox::new_seeded("ancestor-blockers");
    c.ok(&["push", "Root", "--why", "root reason"]);
    c.ok(&[
        "add",
        "Ancestor still open",
        "--parent",
        "1",
        "--why",
        "ancestor reason",
    ]);
    c.ok(&[
        "add",
        "Blocks the open ancestor",
        "--parent",
        "2",
        "--why",
        "blocker reason",
        "--blocks",
    ]);
    c.ok(&[
        "add",
        "Closed ancestor step",
        "--parent",
        "2",
        "--why",
        "closed reason",
    ]);
    c.ok(&[
        "add",
        "Blocks the closed ancestor",
        "--parent",
        "4",
        "--why",
        "blocker reason",
        "--blocks",
    ]);
    c.ok(&["done", "4", "closed anyway", "--force"]);
    c.ok(&[
        "add",
        "The queried node",
        "--parent",
        "4",
        "--why",
        "target reason",
    ]);

    let v: Value = serde_json::from_str(&c.ok(&["why", "6", "--json"])).unwrap();
    let blockers = v["blockers"].as_array().unwrap();
    assert_eq!(blockers.len(), 1, "{v}");
    assert_eq!(blockers[0]["blocked"], "t2", "{v}");
    let until: Vec<String> = blockers[0]["until"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["alias"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(until, vec!["t3"], "{v}");
    assert!(
        blockers.iter().all(|b| b["blocked"] != "t4"),
        "a closed step must not appear in blockers, even with an open blocker of its own:\n{v}"
    );
}

/// Parses the aliases a prose block lists after its header line, up to the
/// next blank line: one alias per line, the first token once a leading `*`
/// (born here's blocking-marker) is skipped.
fn prose_aliases_after(s: &str, header: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in s.lines() {
        if line.trim_start().starts_with(header) {
            inside = true;
            continue;
        }
        if inside {
            if line.trim().is_empty() {
                break;
            }
            let mut words = line.split_whitespace();
            let mut tok = words.next().expect("a listed line names an alias");
            if tok == "*" {
                tok = words.next().expect("a starred line still names an alias");
            }
            out.push(tok.to_string());
        }
    }
    out
}

/// Parses every "`<alias>` does not close until these close (`n`):" block:
/// the blocked step's own alias, paired with the aliases listed under it.
fn prose_blockers(s: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let lines: Vec<&str> = s.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if let Some(at) = trimmed.find(" does not close until these close (") {
            let blocked = trimmed[..at].to_string();
            let mut until = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() {
                let alias = lines[i]
                    .split_whitespace()
                    .next()
                    .expect("a listed line names an alias")
                    .to_string();
                until.push(alias);
                i += 1;
            }
            out.push((blocked, until));
        }
        i += 1;
    }
    out
}

/// One tree that carries every corner `prose_and_json_name_exactly_the_same_aliases`
/// checks at once: an open sibling of the target and a closed one, an open
/// child that blocks and one that does not and one that is closed, and a
/// blocker at two different steps of the path -- the ancestor and the target
/// itself.
fn seeded_for_the_prose_and_json_check(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Root", "--why", "root reason"]);
    c.ok(&[
        "add",
        "Ancestor",
        "--parent",
        "1",
        "--why",
        "ancestor reason",
    ]);
    c.ok(&[
        "add",
        "Blocks the ancestor",
        "--parent",
        "2",
        "--why",
        "blocker reason",
        "--blocks",
    ]);
    c.ok(&[
        "add",
        "The target node",
        "--parent",
        "2",
        "--why",
        "target reason",
    ]);
    c.ok(&[
        "add",
        "Open sibling of the target",
        "--parent",
        "2",
        "--why",
        "sibling reason",
    ]);
    c.ok(&[
        "add",
        "Closed sibling of the target",
        "--parent",
        "2",
        "--why",
        "sibling reason",
    ]);
    c.ok(&["done", "6", "settled"]);
    c.ok(&[
        "add",
        "Open child of the target, blocks",
        "--parent",
        "4",
        "--why",
        "child reason",
        "--blocks",
    ]);
    c.ok(&[
        "add",
        "Open child of the target, no blocks",
        "--parent",
        "4",
        "--why",
        "child reason",
    ]);
    c.ok(&[
        "add",
        "Closed child of the target",
        "--parent",
        "4",
        "--why",
        "child reason",
    ]);
    c.ok(&["done", "9", "settled"]);
    c
}

/// The test that would have caught `blockers` answering a different question
/// than "does not close until these close" does (`t465`, `f440`): every alias
/// the prose lists under "In parallel", "Born here and still open" and each
/// "does not close until these close" has to be exactly the set of aliases
/// the JSON's `in_parallel`, `born_here` and `blockers[].until` carry.
#[test]
fn prose_and_json_name_exactly_the_same_aliases() {
    let c = seeded_for_the_prose_and_json_check("prose-json-check");
    let prose = c.ok(&["why", "4"]);
    let v: Value = serde_json::from_str(&c.ok(&["why", "4", "--json"])).unwrap();

    let siblings_prose = prose_aliases_after(&prose, "In parallel, still open (");
    let siblings_json: Vec<String> = v["in_parallel"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["alias"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(siblings_prose, siblings_json, "{prose}\n{v}");
    assert_eq!(siblings_prose, vec!["t3", "t5"], "{prose}");

    let born_here_prose = prose_aliases_after(&prose, "Born here and still open (");
    let born_here_json: Vec<String> = v["born_here"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["alias"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(born_here_prose, born_here_json, "{prose}\n{v}");
    assert_eq!(born_here_prose, vec!["t7", "t8"], "{prose}");

    let blockers_prose = prose_blockers(&prose);
    let blockers_json: Vec<(String, Vec<String>)> = v["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["blocked"].as_str().unwrap().to_string(),
                b["until"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| h["alias"].as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(blockers_prose, blockers_json, "{prose}\n{v}");
    assert_eq!(
        blockers_prose,
        vec![
            ("t2".to_string(), vec!["t3".to_string()]),
            ("t4".to_string(), vec!["t7".to_string()]),
        ],
        "{prose}"
    );
}
