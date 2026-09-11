//! `t426` §1-§4, §6 — `--against`, `vivac declare` and the `check` finding.
//!
//! `gobierno-como-objetos.md`'s split continues here: `t411` gave a decision
//! nothing to say about what it was judged against, and vivac cannot judge
//! that on its own either. What it can hold is the declaration -- which
//! pillar or rule, and a sentence -- and tell an unrecorded judgement apart
//! from one that was skipped, which is `d445`'s whole point.

mod common;
use common::Sandbox;
use serde_json::Value;

/// `p1` (open pillar) and `r2` (open rule, under it): the minimum a
/// decision needs to be able to declare anything at all.
fn with_open_rule(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "add",
        "Security",
        "--type",
        "pillar",
        "--why",
        "vetoes on the spot, no negotiation",
    ]);
    c.ok(&[
        "add",
        "Keep the write path local",
        "--parent",
        "1",
        "--type",
        "rule",
        "--why",
        "the mechanical half of the pillar",
    ]);
    c
}

fn why_json(c: &Sandbox, id: &str) -> Value {
    let s = c.ok(&["why", id, "--json"]);
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// A ULID is 26 characters, all ASCII alphanumeric. An alias like `p1` or
/// `r2` never is: this is how a test tells the log carries the resolved
/// identifier and not the word somebody typed.
fn looks_like_a_ulid(s: &str) -> bool {
    s.len() == 26 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The `node` field of every `against` entry on the log's own last
/// `node.created` line, read as JSON. `push` writes a vivac and a
/// `stack.pushed` alongside it, so the very last line of the log is not
/// always the one that carries `against`.
fn against_nodes_in_last_creation(c: &Sandbox) -> Vec<String> {
    let log = c.log();
    let line = log
        .lines()
        .rev()
        .find(|l| l.contains("\"node.created\""))
        .expect("a node.created line was written");
    let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON: {e}\n{line}"));
    v["payload"]["against"]
        .as_array()
        .expect("the line carries an against list")
        .iter()
        .map(|e| e["node"].as_str().unwrap().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// §7.1-§7.2: `--against` at birth, on `decide`, `push` and `add`.
// ---------------------------------------------------------------------------

#[test]
fn decide_against_a_pillar_and_a_rule_writes_both_with_their_ulids() {
    let c = with_open_rule("decide-against-two");
    c.ok(&[
        "decide",
        "Keep it local",
        "--reason",
        "the pillar settles it",
        "--against",
        "p1: the write path stays local, with no socket",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);

    let nodes = against_nodes_in_last_creation(&c);
    assert_eq!(nodes.len(), 2, "{nodes:?}");
    for n in &nodes {
        assert!(looks_like_a_ulid(n), "not a ulid: {n}");
    }

    let why = c.ok(&["why", "3"]);
    assert!(
        why.contains("judged against p1: the write path stays local, with no socket"),
        "{why}"
    );
    assert!(
        why.contains("judged against r2: nothing on the write path calls the network"),
        "{why}"
    );

    let v = why_json(&c, "3");
    let against = v["node"]["against"].as_array().expect("against is a list");
    assert_eq!(against.len(), 2, "{v}");
    assert_eq!(against[0]["node"], "p1");
    assert_eq!(
        against[0]["why"],
        "the write path stays local, with no socket"
    );
    assert!(against[0]["declared"].is_null(), "{v}");
    assert_eq!(against[1]["node"], "r2");
}

#[test]
fn push_type_decision_against_a_rule_writes_the_ulid() {
    let c = with_open_rule("push-against");
    c.ok(&[
        "push",
        "Keep it local",
        "--type",
        "decision",
        "--why",
        "the pillar settles it",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    let nodes = against_nodes_in_last_creation(&c);
    assert_eq!(nodes.len(), 1, "{nodes:?}");
    assert!(looks_like_a_ulid(&nodes[0]), "not a ulid: {}", nodes[0]);
    let v = why_json(&c, "3");
    assert_eq!(v["node"]["against"][0]["node"], "r2", "{v}");
}

#[test]
fn add_type_decision_against_a_rule_writes_the_ulid() {
    let c = with_open_rule("add-against");
    c.ok(&[
        "add",
        "Keep it local",
        "--type",
        "decision",
        "--why",
        "the pillar settles it",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    let nodes = against_nodes_in_last_creation(&c);
    assert_eq!(nodes.len(), 1, "{nodes:?}");
    assert!(looks_like_a_ulid(&nodes[0]), "not a ulid: {}", nodes[0]);
    let v = why_json(&c, "3");
    assert_eq!(v["node"]["against"][0]["node"], "r2", "{v}");
}

// ---------------------------------------------------------------------------
// §7.3: E5 -- `--against` on `push`/`add` without `--type decision`.
// ---------------------------------------------------------------------------

#[test]
fn push_against_without_type_decision_is_refused_and_writes_nothing() {
    let c = with_open_rule("push-against-e5");
    c.ok(&["push", "A run", "--why", "reason"]);
    let before = c.log();
    let (out, code) = c.run(&[
        "push",
        "Just a task",
        "--why",
        "reason",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--against goes on a decision, and this is a task"),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused push still wrote:\n{out}");
}

#[test]
fn add_against_with_another_type_is_refused_and_writes_nothing() {
    let c = with_open_rule("add-against-e5");
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "A finding",
        "--type",
        "finding",
        "--why",
        "reason",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--against goes on a decision, and this is a finding"),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused add still wrote:\n{out}");
}

// ---------------------------------------------------------------------------
// §7.4: E1, E2, E3, E4 and E6, each separately, each exit 2 with the log
// untouched.
// ---------------------------------------------------------------------------

#[test]
fn the_three_malformed_entries_are_all_refused() {
    let c = with_open_rule("e1-shapes");
    let before = c.log();
    for bad in ["r2", "r2:", ": a sentence"] {
        let (out, code) = c.run(&["decide", "A call", "--reason", "because", "--against", bad]);
        assert_eq!(code, 2, "input {bad:?}:\n{out}");
        assert!(
            out.contains("--against needs an id and a sentence: --against \"r12: why it holds\""),
            "input {bad:?}:\n{out}"
        );
    }
    assert_eq!(before, c.log(), "a refused decide still wrote");
}

#[test]
fn an_id_that_does_not_resolve_is_refused() {
    let c = with_open_rule("e2-missing");
    let before = c.log();
    let (out, code) = c.run(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "r99: a sentence",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("No such node: r99."), "{out}");
    assert_eq!(before, c.log());
}

#[test]
fn a_target_that_is_not_a_pillar_or_a_rule_is_refused() {
    let c = with_open_rule("e3-wrong-kind");
    c.ok(&["decide", "An earlier call", "--reason", "because"]);
    let before = c.log();
    let (out, code) = c.run(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "d3: a sentence",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--against points at a pillar or a rule, and d3 is a decision"),
        "{out}"
    );
    assert_eq!(before, c.log());
}

#[test]
fn a_target_that_no_longer_governs_is_refused() {
    let c = with_open_rule("e4-closed");
    c.ok(&["done", "2", "retired"]);
    let before = c.log();
    let (out, code) = c.run(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "r2: a sentence",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains(
            "--against points at what still governs, and r2 is closed: vivac rules lists what does"
        ),
        "{out}"
    );
    assert_eq!(before, c.log());
}

#[test]
fn the_same_id_twice_in_one_call_is_refused() {
    let c = with_open_rule("e6-repeat");
    let before = c.log();
    let (out, code) = c.run(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "p1: first sentence",
        "--against",
        "p1: second sentence",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--against names p1 twice"), "{out}");
    assert_eq!(before, c.log());
}

// ---------------------------------------------------------------------------
// §7.5: the redaction guard covers a declared-at-birth sentence.
// ---------------------------------------------------------------------------

#[test]
fn the_redaction_guard_covers_an_against_sentence_at_birth() {
    let c = with_open_rule("against-secret");
    let before = c.log();
    let (out, code) = c.run(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "r2: curl -H sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345",
    ]);
    assert_eq!(code, 3, "{out}");
    assert_eq!(before, c.log(), "a refused decide still wrote:\n{out}");
}

// ---------------------------------------------------------------------------
// §7.6: an ungoverned tree writes exactly what it always has.
// ---------------------------------------------------------------------------

#[test]
fn decide_on_an_ungoverned_tree_writes_no_against_key_and_no_outcome_line() {
    let c = Sandbox::new_seeded("ungoverned");
    let out = c.ok(&["decide", "A call", "--reason", "because"]);
    assert!(!out.contains("no --against"), "{out}");
    let log = c.log();
    let line = log.lines().last().unwrap();
    assert!(!line.contains("\"against\""), "{line}");
}

#[test]
fn decide_with_only_a_closed_pillar_writes_no_against_key() {
    let c = Sandbox::new_seeded("closed-pillar-only");
    c.ok(&["add", "Security", "--type", "pillar", "--why", "vetoes"]);
    c.ok(&["done", "1", "retired"]);
    let out = c.ok(&["decide", "A call", "--reason", "because"]);
    assert!(!out.contains("no --against"), "{out}");
    let log = c.log();
    let line = log.lines().last().unwrap();
    assert!(!line.contains("\"against\""), "{line}");
}

// ---------------------------------------------------------------------------
// §7.7: a governed tree with no `--against` writes `"against":[]` and warns.
// ---------------------------------------------------------------------------

#[test]
fn decide_with_no_against_on_a_governed_tree_writes_the_empty_key_and_warns() {
    let c = with_open_rule("governed-empty");
    let (out, code) = c.run(&["decide", "A call", "--reason", "because"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("no --against: a pillar judged in silence reads the same as one skipped"),
        "{out}"
    );
    assert!(out.contains("vivac declare d3 adds one"), "{out}");
    let log = c.log();
    let line = log.lines().last().unwrap();
    assert!(line.contains("\"against\":[]"), "{line}");
}

// ---------------------------------------------------------------------------
// §7.8: `declare` adds, `why` shows it late, the JSON carries `declared`.
// E7, E8 (both a birth declaration and an earlier late one), E9, and the
// redaction guard.
// ---------------------------------------------------------------------------

#[test]
fn declare_adds_a_late_declaration_shown_with_its_date() {
    let c = with_open_rule("declare-basic");
    c.ok(&["decide", "A call", "--reason", "because"]); // d3, governed, empty
    let out = c.ok(&[
        "declare",
        "3",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    assert!(
        out.contains("d3  judged against r2: nothing on the write path calls the network"),
        "{out}"
    );

    let why = c.ok(&["why", "3"]);
    assert!(why.contains("(declared "), "{why}");

    let v = why_json(&c, "3");
    let against = v["node"]["against"].as_array().unwrap();
    assert_eq!(against.len(), 1, "{v}");
    assert_eq!(against[0]["node"], "r2");
    assert!(!against[0]["declared"].is_null(), "{v}");
}

#[test]
fn declare_on_something_that_is_not_a_decision_is_refused() {
    let c = with_open_rule("declare-e7");
    let before = c.log();
    let (out, code) = c.run(&["declare", "2", "--against", "p1: a sentence"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("vivac declare takes a decision, and r2 is a rule"),
        "{out}"
    );
    assert_eq!(before, c.log());
}

/// A kind named in a refusal carries its own article: `assumption` is the one
/// of the nine that takes "an".
#[test]
fn a_refusal_names_the_kind_with_its_own_article() {
    let c = with_open_rule("against-article");
    c.ok(&[
        "add",
        "It holds on Windows",
        "--type",
        "assumption",
        "--why",
        "reason",
    ]);
    let before = c.log();

    let (out, code) = c.run(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "a3: a sentence",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--against points at a pillar or a rule, and a3 is an assumption"),
        "{out}"
    );

    let (out, code) = c.run(&[
        "add",
        "It holds on Linux",
        "--type",
        "assumption",
        "--why",
        "reason",
        "--against",
        "r2: a sentence",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--against goes on a decision, and this is an assumption"),
        "{out}"
    );

    let (out, code) = c.run(&["declare", "3", "--against", "r2: a sentence"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("vivac declare takes a decision, and a3 is an assumption"),
        "{out}"
    );

    assert_eq!(before, c.log());
}

#[test]
fn declare_repeating_a_declaration_made_at_birth_is_refused() {
    let c = with_open_rule("declare-e8-birth");
    c.ok(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    let before = c.log();
    let (out, code) = c.run(&["declare", "3", "--against", "r2: a second sentence"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("d3 already declares r2"), "{out}");
    assert_eq!(before, c.log());
}

#[test]
fn declare_repeating_an_earlier_late_declaration_is_refused() {
    let c = with_open_rule("declare-e8-late");
    c.ok(&["decide", "A call", "--reason", "because"]);
    c.ok(&[
        "declare",
        "3",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    let before = c.log();
    let (out, code) = c.run(&["declare", "3", "--against", "r2: a second sentence"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("d3 already declares r2"), "{out}");
    assert_eq!(before, c.log());
}

#[test]
fn declare_with_no_decision_is_refused() {
    let c = with_open_rule("declare-e9-no-id");
    let (out, code) = c.run(&["declare", "--against", "r2: a sentence"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("usage: vivac declare <decision> --against \"r12: <why>\""),
        "{out}"
    );
}

#[test]
fn declare_with_no_against_is_refused() {
    let c = with_open_rule("declare-e9-no-against");
    c.ok(&["decide", "A call", "--reason", "because"]);
    let (out, code) = c.run(&["declare", "3"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("usage: vivac declare <decision> --against \"r12: <why>\""),
        "{out}"
    );
}

#[test]
fn the_redaction_guard_covers_a_declare_sentence() {
    let c = with_open_rule("declare-secret");
    c.ok(&["decide", "A call", "--reason", "because"]);
    let before = c.log();
    let (out, code) = c.run(&[
        "declare",
        "3",
        "--against",
        "r2: curl -H sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345",
    ]);
    assert_eq!(code, 3, "{out}");
    assert_eq!(before, c.log(), "a refused declare still wrote:\n{out}");
}

// ---------------------------------------------------------------------------
// §7.9: `declare` on a decision born without the key: `why` shows it,
// `check` does not count it.
// ---------------------------------------------------------------------------

#[test]
fn declare_on_a_decision_born_without_the_key_shows_in_why_but_check_ignores_it() {
    let c = Sandbox::new_seeded("declare-no-key-birth");
    c.ok(&["decide", "A call", "--reason", "because"]); // d1, no governance yet
    c.ok(&["add", "Security", "--type", "pillar", "--why", "vetoes"]);
    c.ok(&[
        "add",
        "Keep the write path local",
        "--parent",
        "2",
        "--type",
        "rule",
        "--why",
        "guard",
    ]);
    c.ok(&[
        "declare",
        "1",
        "--against",
        "r3: nothing on the write path calls the network",
    ]);

    let why = c.ok(&["why", "1"]);
    assert!(
        why.contains("judged against r3: nothing on the write path calls the network"),
        "{why}"
    );

    let (out, code) = c.run(&["check"]);
    assert_eq!(
        code, 0,
        "a decision born with no key must not be counted:\n{out}"
    );
    assert!(
        !out.contains("declares nothing it was judged against"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// §7.10: `check`'s finding, and its two independent footers.
// ---------------------------------------------------------------------------

#[test]
fn check_reports_an_open_decision_with_an_empty_key() {
    let c = with_open_rule("check-finding");
    c.ok(&["decide", "A call", "--reason", "because"]); // d3, governed, empty

    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(
            "d3 declares nothing it was judged against: vivac declare d3 --against \"<id>: <why>\""
        ),
        "{out}"
    );
    assert!(
        out.contains("A decision that declared nothing stays as it was written: vivac declare"),
        "{out}"
    );
    assert!(
        !out.contains("A false close is not repaired"),
        "no false close happened here:\n{out}"
    );
}

#[test]
fn check_does_not_report_a_decision_with_no_key() {
    let c = Sandbox::new_seeded("check-no-key");
    c.ok(&["decide", "A call", "--reason", "because"]);
    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("declares nothing"), "{out}");
}

#[test]
fn check_does_not_report_a_superseded_decision() {
    let c = with_open_rule("check-superseded");
    c.ok(&["decide", "First", "--reason", "because"]); // d3
    c.ok(&[
        "decide",
        "Second",
        "--reason",
        "because",
        "--supersedes",
        "3",
    ]); // d4
    let (out, code) = c.run(&["check"]);
    // d4, the new one, still declares nothing: it is still open. d3 does
    // not, because it is superseded.
    assert_eq!(code, 1, "{out}");
    assert!(!out.contains("d3 declares nothing"), "{out}");
    assert!(out.contains("d4 declares nothing"), "{out}");
}

#[test]
fn check_does_not_report_a_decision_after_it_declares() {
    let c = with_open_rule("check-declared");
    c.ok(&["decide", "A call", "--reason", "because"]); // d3
    c.ok(&[
        "declare",
        "3",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("declares nothing"), "{out}");
}

#[test]
fn each_check_footer_prints_only_for_its_own_finding() {
    // Only a false close: no undeclared decision anywhere. The blocker is
    // added **after** the node closes clean, which is how a close turns
    // false later rather than being refused on the way in.
    let c = Sandbox::new_seeded("footer-false-close-only");
    c.ok(&["push", "A run", "--why", "reason"]);
    c.ok(&["done", "1", "wrapped up"]);
    c.ok(&[
        "add",
        "A finding",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "still open",
    ]);
    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("A false close is not repaired"), "{out}");
    assert!(!out.contains("A decision that declared nothing"), "{out}");
}

#[test]
fn check_exits_zero_with_neither_finding() {
    let c = with_open_rule("check-clean");
    c.ok(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 0, "{out}");
}

// ---------------------------------------------------------------------------
// §7.11: deleting the index changes nothing about `why`, `why --json` or
// `check`, on a tree with a birth declaration and a late one.
// ---------------------------------------------------------------------------

#[test]
fn deleting_the_index_does_not_change_why_or_check_with_declarations() {
    let c = with_open_rule("against-index-guard");
    c.ok(&[
        "decide",
        "A call",
        "--reason",
        "because",
        "--against",
        "p1: the write path stays local, with no socket",
    ]); // d3
    c.ok(&["decide", "Another call", "--reason", "because"]); // d4, empty at birth
    c.ok(&[
        "declare",
        "4",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    c.ok(&["stack"]); // persists the index

    let index_path = c.0.join(".vivac").join("index");
    assert!(index_path.exists(), "no index to delete");

    let why3_before = c.ok(&["why", "3"]);
    let why3_json_before = c.ok(&["why", "3", "--json"]);
    let why4_before = c.ok(&["why", "4"]);
    let why4_json_before = c.ok(&["why", "4", "--json"]);
    let check_before = c.ok(&["check"]);

    std::fs::remove_file(&index_path).unwrap();

    assert_eq!(why3_before, c.ok(&["why", "3"]));
    assert_eq!(why3_json_before, c.ok(&["why", "3", "--json"]));
    assert_eq!(why4_before, c.ok(&["why", "4"]));
    assert_eq!(why4_json_before, c.ok(&["why", "4", "--json"]));
    assert_eq!(check_before, c.ok(&["check"]));
}

// ---------------------------------------------------------------------------
// §7.12: `changes` counts late declarations.
// ---------------------------------------------------------------------------

#[test]
fn changes_counts_a_late_declaration_in_text_and_json() {
    let c = with_open_rule("changes-declarations");
    c.ok(&["decide", "A call", "--reason", "because"]); // d3
    c.ok(&["save", "checkpoint"]);
    c.ok(&[
        "declare",
        "3",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);

    let out = c.ok(&["changes"]);
    assert!(out.contains("1 late declaration"), "{out}");

    let json_out = c.ok(&["changes", "--json"]);
    let v: Value = serde_json::from_str(&json_out).expect("changes --json is not JSON");
    assert_eq!(v["tail"]["late_declarations"], 1, "{json_out}");
}

// ---------------------------------------------------------------------------
// §1.5: the `d444` candlock still guards `against.added`.
// ---------------------------------------------------------------------------

const LOCK_SENTENCE: &str =
    "this tree holds pillars and rules, and this vivac is too old to read them: update vivac";

fn config_text(c: &Sandbox) -> String {
    std::fs::read_to_string(c.0.join(".vivac").join("config")).unwrap()
}

#[test]
fn declare_on_an_unlocked_config_with_a_rule_already_in_the_log_locks_it() {
    let c = with_open_rule("declare-locks");
    c.ok(&["decide", "A call", "--reason", "because"]); // d3
    assert!(
        config_text(&c).contains(LOCK_SENTENCE),
        "setup: the rule should have locked it already"
    );

    // Hand-mount: put the config back the way an untouched tree would have
    // it, as if the lock had never run.
    let cfg = c.0.join(".vivac").join("config");
    let raw = std::fs::read_to_string(&cfg).unwrap();
    let mut v: Value = serde_json::from_str(&raw).unwrap();
    v["version"] = Value::from(1);
    std::fs::write(&cfg, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    assert!(
        !config_text(&c).contains(LOCK_SENTENCE),
        "hand-mount did not take"
    );

    c.ok(&[
        "declare",
        "3",
        "--against",
        "r2: nothing on the write path calls the network",
    ]);
    assert!(
        config_text(&c).contains(LOCK_SENTENCE),
        "declare on a tree that already has a rule did not lock the config:\n{}",
        config_text(&c)
    );
}
