//! `t411` §1-§7, §11-§13 bis, `d436` §15-§16 — the two types, `arms` and the
//! pull: `vivac rules`.
//!
//! `gobierno-como-objetos.md` §2: vivac cannot judge whether work complies
//! with a pillar, only whether the declaration exists. This file is the
//! mechanical half of that split -- the two new types, what they refuse to
//! be born without, the fold of a rule's arms, and the read that hands all
//! of it back on demand. A pillar carries no power field: what it restricts
//! is its title, in the project's own words, and vivac keeps no menu of it
//! (`d436`).

mod common;
use common::Sandbox;

/// A pillar and a rule under it, plus a rule with no pillar and an
/// invariant, seeded in creation order so `rules_view`'s own sort by
/// number is what puts them back in order and not something else.
///
/// Numbering, since several tests resolve by bare number: `p1` DX, `p2`
/// Security, `r3` "Never store a secret" (armed, under `p2`), `r4` "Do not
/// negotiate the veto" (judged, under `p2`), `r5` "Write a commit message in
/// English" (no pillar), `c6` the invariant.
fn governed(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c.ok(&[
        "add",
        "DX",
        "--type",
        "pillar",
        "--why",
        "chooses between what already passed the others",
    ]);
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
        "Never store a secret",
        "--parent",
        "2",
        "--type",
        "rule",
        "--arm",
        "cargo test --bin vivac redact::tests",
        "--arm-dir",
        "vivac",
        "--why",
        "the mechanical half of the security pillar",
    ]);
    c.ok(&[
        "add",
        "Do not negotiate the veto",
        "--parent",
        "2",
        "--type",
        "rule",
        "--why",
        "no command can judge a negotiation",
    ]);
    c.ok(&[
        "add",
        "Write a commit message in English",
        "--type",
        "rule",
        "--why",
        "project convention, not any one pillar's",
    ]);
    c.ok(&[
        "add",
        "Never pass raw prose through a shell",
        "--type",
        "constraint",
        "--why",
        "shell injection",
    ]);
    c
}

// ---------------------------------------------------------------------------
// §1: the two types and their aliases.
// ---------------------------------------------------------------------------

#[test]
fn a_pillar_and_a_rule_get_their_own_prefixes_and_resolve_by_alias() {
    let c = Sandbox::new_seeded("prefixes");
    let out = c.ok(&["add", "Security", "--type", "pillar", "--why", "arbiter"]);
    assert!(out.contains("p1"), "{out}");
    let out2 = c.ok(&[
        "add",
        "Never store a secret",
        "--parent",
        "1",
        "--type",
        "rule",
        "--why",
        "guard",
    ]);
    assert!(out2.contains("r2"), "{out2}");

    let why_rule = c.ok(&["why", "r2"]);
    assert!(
        why_rule.contains("judged: no command verifies it"),
        "{why_rule}"
    );
}

// ---------------------------------------------------------------------------
// §3: `--arm` at birth, `arm`/`arm --off`, and the guards on both.
// ---------------------------------------------------------------------------

#[test]
fn an_arm_on_a_non_rule_is_refused_at_birth_and_writes_nothing() {
    let c = Sandbox::new_seeded("arm-wrong-type");
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "Some task",
        "--type",
        "task",
        "--arm",
        "cargo test",
        "--why",
        "reason",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("Only a rule has an arm; this would be a task."),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused add still wrote:\n{out}");
}

#[test]
fn arm_on_something_that_is_not_a_rule_is_refused_and_writes_nothing() {
    let c = Sandbox::new_seeded("arm-not-a-rule");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let before = c.log();
    let (out, code) = c.run(&["arm", "1", "some command"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("Only a rule has an arm; g1 is a goal."),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused arm still wrote:\n{out}");
}

/// `assumption` is the one type whose word needs "an" rather than "a"
/// (`d453`), so this is the case `g1 is a goal` above cannot catch.
#[test]
fn arm_on_an_assumption_is_refused_with_the_indefinite_article() {
    let c = Sandbox::new_seeded("arm-not-a-rule-assumption");
    c.ok(&[
        "add",
        "Caching helps here",
        "--type",
        "assumption",
        "--why",
        "reason",
    ]);
    let before = c.log();
    let (out, code) = c.run(&["arm", "1", "some command"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("Only a rule has an arm; a1 is an assumption."),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused arm still wrote:\n{out}");
}

/// The birth-time refusal forms the same "a" plus a type's word, so it
/// needs the same fix as the one above (`d453`).
#[test]
fn an_arm_on_an_assumption_at_birth_is_refused_with_the_indefinite_article() {
    let c = Sandbox::new_seeded("arm-wrong-type-assumption");
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "Caching helps here",
        "--type",
        "assumption",
        "--arm",
        "cargo test",
        "--why",
        "reason",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("Only a rule has an arm; this would be an assumption."),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused add still wrote:\n{out}");
}

/// The fold: births, plus what was added, minus what was removed, in the
/// order the log carries them -- not the order they happen to sort in.
#[test]
fn arms_fold_in_log_order_births_then_additions_minus_removals() {
    let c = Sandbox::new_seeded("arm-fold-order");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c.ok(&[
        "add",
        "Never store a secret",
        "--type",
        "rule",
        "--arm",
        "check A",
        "--arm",
        "check B",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    c.ok(&["arm", "1", "check C", "--dir", "vivac"]);
    c.ok(&["arm", "1", "check A", "--dir", "vivac", "--off"]);

    let out = c.ok(&["rules", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("rules --json is not JSON");
    let arms: Vec<&str> = v["rules"][0]["arms"]
        .as_array()
        .expect("a rule's arms")
        .iter()
        .map(|x| x["command"].as_str().unwrap())
        .collect();
    assert_eq!(arms, vec!["check B", "check C"], "{out}");
}

/// Unlike a flag, which folds into a set and so shrugs off a repeat, arms
/// fold into a list: a second copy would show twice in `rules`.
#[test]
fn an_arm_the_rule_already_has_is_refused_and_writes_nothing() {
    let c = Sandbox::new_seeded("arm-repeated");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c.ok(&[
        "add",
        "Never store a secret",
        "--type",
        "rule",
        "--arm",
        "check A",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    let before = c.log();
    let (out, code) = c.run(&["arm", "1", "check A", "--dir", "vivac"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("r1 already has that arm."), "{out}");
    assert_eq!(before, c.log(), "a repeated arm still wrote:\n{out}");
}

/// The same repeat, at birth.
#[test]
fn the_same_arm_twice_at_birth_is_refused_and_writes_nothing() {
    let c = Sandbox::new_seeded("arm-twice-at-birth");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "Never store a secret",
        "--type",
        "rule",
        "--arm",
        "check A",
        "--arm",
        "check A",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("The same arm is given twice: check A"),
        "{out}"
    );
    assert_eq!(before, c.log(), "a doubled arm still wrote:\n{out}");
}

/// Removing an arm the rule does not have would write an event that folds
/// into nothing: a line in the log that changes no answer.
#[test]
fn removing_an_arm_the_rule_does_not_have_is_refused_and_writes_nothing() {
    let c = Sandbox::new_seeded("arm-absent-off");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c.ok(&[
        "add",
        "Never store a secret",
        "--type",
        "rule",
        "--why",
        "guard",
    ]);
    let before = c.log();
    let (out, code) = c.run(&["arm", "1", "check A", "--dir", "vivac", "--off"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("r1 has no such arm; vivac why r1 lists the ones it has."),
        "{out}"
    );
    assert_eq!(
        before,
        c.log(),
        "removing an absent arm still wrote:\n{out}"
    );
}

#[test]
fn the_redaction_guard_covers_an_arm_given_after_birth() {
    let c = Sandbox::new_seeded("arm-secret");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c.ok(&["add", "Guard secrets", "--type", "rule", "--why", "guard"]);
    let before = c.log();
    let (out, code) = c.run(&[
        "arm",
        "1",
        "curl -H sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345",
        "--dir",
        "vivac",
    ]);
    assert_eq!(code, 3, "{out}");
    assert_eq!(before, c.log(), "a refused arm still wrote:\n{out}");
}

#[test]
fn the_redaction_guard_covers_an_arm_given_at_birth() {
    let c = Sandbox::new_seeded("arm-secret-birth");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "Guard secrets",
        "--type",
        "rule",
        "--arm",
        "sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    assert_eq!(code, 3, "{out}");
    assert_eq!(before, c.log(), "a refused add still wrote:\n{out}");
}

#[test]
fn an_empty_arm_is_refused_and_writes_nothing() {
    let c = Sandbox::new_seeded("arm-empty");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c.ok(&["add", "Guard secrets", "--type", "rule", "--why", "guard"]);
    let before = c.log();
    let (out, code) = c.run(&["arm", "1", "   ", "--dir", "vivac"]);
    assert_eq!(code, 2, "{out}");
    assert_eq!(before, c.log(), "a refused arm still wrote:\n{out}");
}

// ---------------------------------------------------------------------------
// §4: `is_front()` -- covered in `tests/constraints.rs`, which this file
// does not repeat.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// §5: `vivac rules`.
// ---------------------------------------------------------------------------

#[test]
fn rules_orders_pillars_by_number_and_groups_correctly() {
    let c = governed("order");
    let out = c.ok(&["rules"]);

    assert!(out.contains("PILLARS"), "{out}");
    let dx_at = out.find("DX").expect("DX should print");
    let security_at = out.find("Security").expect("Security should print");
    assert!(
        dx_at < security_at,
        "the pillar created first did not sort first:\n{out}"
    );
    assert!(
        out.contains("armed in vivac/: cargo test --bin vivac redact::tests"),
        "{out}"
    );
    assert!(out.contains("RULES WITHOUT A PILLAR"), "{out}");
    assert!(out.contains("Write a commit message in English"), "{out}");
    assert!(out.contains("INVARIANTS"), "{out}");
    assert!(
        out.contains("Never pass raw prose through a shell"),
        "{out}"
    );
    assert!(
        out.contains("2 pillars \u{b7} 3 rules: 1 armed, 2 judged \u{b7} 1 invariant"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// `d421`: `rules` only ever shows a rule's arms, never that it is judged.
// ---------------------------------------------------------------------------

#[test]
fn rules_prints_no_second_line_for_a_judged_rule() {
    let c = governed("no-judged-line");
    let out = c.ok(&["rules"]);

    assert!(
        !out.contains("judged: no command verifies it"),
        "a judged rule kept the line `rules` is meant to drop:\n{out}"
    );
    // The judged rule's own title still prints, right where its arm would
    // have gone if it had one.
    assert!(out.contains("Do not negotiate the veto"), "{out}");
    let title_at = out
        .find("Do not negotiate the veto")
        .expect("the judged rule's title");
    let after_title = &out[title_at..];
    let next_line = after_title.lines().nth(1).unwrap_or_default();
    assert!(
        !next_line.trim_start().starts_with("judged:"),
        "the line right under a judged rule still names it judged:\n{out}"
    );
}

#[test]
fn why_of_a_judged_rule_still_names_it_judged() {
    // `d421` only changes `rules`'s own text; `why` keeps the one-line
    // answer, because there it is the only line and it does inform.
    let c = governed("why-still-judged");
    let out = c.ok(&["why", "r4"]);
    assert!(out.contains("judged: no command verifies it"), "{out}");
}

// ---------------------------------------------------------------------------
// `d422`: the pointer to the second map, when nothing governs.
// ---------------------------------------------------------------------------

const SECOND_MAP_HINT: [&str; 2] = [
    "  Rules kept in CLAUDE.md, AGENTS.md or a memory file are a second map, and",
    "  vivac never reads them: bring them in with vivac add --type pillar|rule.",
];

#[test]
fn rules_hints_at_the_second_map_when_nothing_governs() {
    let c = Sandbox::new_seeded("hint-empty");
    let out = c.ok(&["rules"]);
    assert!(
        out.contains("Nothing governs this project yet: no pillars, rules or invariants."),
        "{out}"
    );
    for line in SECOND_MAP_HINT {
        assert!(out.contains(line), "{out}");
    }
}

#[test]
fn rules_hints_at_the_second_map_when_only_invariants_exist() {
    let c = Sandbox::new_seeded("hint-invariants-only");
    c.ok(&[
        "add",
        "Never pass raw prose through a shell",
        "--type",
        "constraint",
        "--why",
        "shell injection",
    ]);
    let out = c.ok(&["rules"]);
    assert!(out.contains("INVARIANTS"), "{out}");
    for line in SECOND_MAP_HINT {
        assert!(out.contains(line), "{out}");
    }
}

#[test]
fn rules_does_not_hint_at_the_second_map_once_something_governs() {
    let c = governed("hint-absent-when-governed");
    let out = c.ok(&["rules"]);
    for line in SECOND_MAP_HINT {
        assert!(
            !out.contains(line),
            "the second-map hint showed even though a pillar and a rule are open:\n{out}"
        );
    }
}

#[test]
fn rules_json_carries_no_second_map_hint() {
    let c = Sandbox::new_seeded("hint-json-silent");
    let out = c.ok(&["rules", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("rules --json is not JSON");
    assert_eq!(v["pillars"], serde_json::json!([]), "{out}");
    assert_eq!(v["rules"], serde_json::json!([]), "{out}");
    assert_eq!(v["invariants"], serde_json::json!([]), "{out}");
}

#[test]
fn a_rule_answers_to_its_nearest_pillar_ancestor_not_the_root() {
    let c = Sandbox::new_seeded("nearest");
    c.ok(&["add", "Security", "--type", "pillar", "--why", "vetoes"]);
    c.ok(&[
        "add",
        "Some unrelated task",
        "--parent",
        "1",
        "--why",
        "just a task",
    ]);
    c.ok(&[
        "add",
        "Never store a secret",
        "--parent",
        "2",
        "--type",
        "rule",
        "--why",
        "guard",
    ]);

    let out = c.ok(&["rules"]);
    assert!(
        !out.contains("RULES WITHOUT A PILLAR"),
        "a rule two hops under its pillar landed without one:\n{out}"
    );
    let pillars_at = out.find("PILLARS").unwrap();
    let rule_at = out.find("Never store a secret").unwrap();
    assert!(rule_at > pillars_at, "{out}");
}

#[test]
fn rules_lists_every_open_invariant_regardless_of_focus() {
    let c = Sandbox::new_seeded("invariant-everywhere");
    c.ok(&["push", "Root goal", "--why", "root"]);
    c.ok(&[
        "add",
        "Never log a secret",
        "--parent",
        "1",
        "--type",
        "constraint",
        "--why",
        "mirrors c321",
    ]);
    c.ok(&[
        "push",
        "Unrelated detour",
        "--why",
        "somewhere else entirely",
    ]);

    let out = c.ok(&["rules"]);
    assert!(
        out.contains("Never log a secret"),
        "an invariant vanished once the focus moved off its branch:\n{out}"
    );
}

#[test]
fn rules_with_nothing_to_govern_says_so_and_exits_zero() {
    let c = Sandbox::new_seeded("empty-rules");
    let (out, code) = c.run(&["rules"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Nothing governs this project yet: no pillars, rules or invariants."),
        "{out}"
    );
}

#[test]
fn rules_json_matches_the_documented_shape() {
    let c = governed("json-shape");
    let out = c.ok(&["rules", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("rules --json is not JSON");

    assert_eq!(v["pillars"][0]["title"], "DX", "{out}");
    assert_eq!(v["pillars"][1]["title"], "Security", "{out}");

    let security_rules = v["pillars"][1]["rules"].as_array().unwrap();
    let judged = security_rules
        .iter()
        .find(|r| r["title"] == "Do not negotiate the veto")
        .expect("the judged rule is listed under Security");
    assert_eq!(judged["arms"], serde_json::json!([]), "{out}");

    assert_eq!(v["rules"][0]["title"], "Write a commit message in English");
    assert_eq!(
        v["invariants"][0]["title"],
        "Never pass raw prose through a shell"
    );
}

/// `t411` §5's own performance note is not measured here -- it is measured
/// by reading the code, not by a stopwatch in a test. What this proves is
/// only that deleting the derived index changes none of `rules`'s answers.
#[test]
fn deleting_the_index_does_not_change_rules_or_why_of_a_pillar_and_a_rule() {
    let c = governed("index-guard");
    c.ok(&["stack"]); // a plain read persists the derived index
    let index_path = c.0.join(".vivac").join("index");
    assert!(index_path.exists(), "no index to delete");

    let rules_before = c.ok(&["rules"]);
    let rules_json_before = c.ok(&["rules", "--json"]);
    let why_pillar_before = c.ok(&["why", "p2"]);
    let why_rule_before = c.ok(&["why", "r3"]);

    std::fs::remove_file(&index_path).unwrap();

    assert_eq!(rules_before, c.ok(&["rules"]));
    assert_eq!(rules_json_before, c.ok(&["rules", "--json"]));
    assert_eq!(why_pillar_before, c.ok(&["why", "p2"]));
    assert_eq!(why_rule_before, c.ok(&["why", "r3"]));
}

// ---------------------------------------------------------------------------
// The `node.created` of an existing type gains no keys.
// ---------------------------------------------------------------------------

#[test]
fn an_existing_type_gains_no_keys_in_its_node_created() {
    let c = Sandbox::new_seeded("byte-identity");
    c.ok(&[
        "add",
        "Just a task",
        "--type",
        "task",
        "--why",
        "ordinary work",
    ]);
    let log = c.log();
    let line = log.lines().last().expect("a line was written");
    assert!(line.contains("\"node.created\""), "{line}");
    assert!(
        !line.contains("\"power\"") && !line.contains("\"arms\""),
        "an existing type's node.created gained a key it never had:\n{line}"
    );
}

/// `d436`: a pillar's `node.created` carries no key of its own any more --
/// only a rule's arms still do.
#[test]
fn an_armed_rule_gains_its_own_key_but_a_pillar_gains_none() {
    let c = Sandbox::new_seeded("byte-positive");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c.ok(&["add", "Security", "--type", "pillar", "--why", "arbiter"]);
    c.ok(&[
        "add",
        "Never store a secret",
        "--parent",
        "1",
        "--type",
        "rule",
        "--arm",
        "cargo test",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    let log = c.log();
    let mut lines = log.lines();
    let pillar_line = lines.next().expect("the pillar's own line");
    let rule_line = lines.next().expect("the rule's own line");
    assert!(!pillar_line.contains("\"power\""), "{pillar_line}");
    assert!(!pillar_line.contains("\"arms\""), "{pillar_line}");
    assert!(
        rule_line.contains("\"arms\":[{\"dir\":\"vivac\",\"command\":\"cargo test\"}]"),
        "{rule_line}"
    );
    assert!(!rule_line.contains("\"power\""), "{rule_line}");
}

// ---------------------------------------------------------------------------
// `d436`: the pillar loses `power`. What a pillar restricts is its title,
// in the project's own words, and vivac keeps no menu of it.
// ---------------------------------------------------------------------------

#[test]
fn a_pillar_needs_no_power_to_be_born() {
    let c = Sandbox::new_seeded("no-power-birth");
    let (out, code) = c.run(&["add", "Security", "--type", "pillar", "--why", "arbiter"]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn power_is_an_unknown_flag_on_add_and_writes_nothing() {
    let c = Sandbox::new_seeded("power-unknown-add");
    let before = c.log();
    let (out, code) = c.run(&[
        "add", "Security", "--type", "pillar", "--power", "veto", "--why", "arbiter",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("does not take --power"), "{out}");
    assert_eq!(before, c.log(), "a refused add still wrote:\n{out}");
}

#[test]
fn power_is_an_unknown_flag_on_push_and_writes_nothing() {
    let c = Sandbox::new_seeded("power-unknown-push");
    let before = c.log();
    let (out, code) = c.run(&[
        "push", "Security", "--type", "pillar", "--power", "veto", "--why", "arbiter",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("does not take --power"), "{out}");
    assert_eq!(before, c.log(), "a refused push still wrote:\n{out}");
}

#[test]
fn rules_lists_open_pillars_by_number_the_one_created_later_comes_later() {
    let c = Sandbox::new_seeded("pillars-by-number");
    c.ok(&["add", "DX", "--type", "pillar", "--why", "judges last"]);
    c.ok(&["add", "Security", "--type", "pillar", "--why", "vetoes"]);
    let out = c.ok(&["rules"]);
    let dx_at = out.find("DX").expect("DX should print");
    let security_at = out.find("Security").expect("Security should print");
    assert!(
        dx_at < security_at,
        "the pillar created first did not print first:\n{out}"
    );
}

#[test]
fn rules_json_and_why_json_of_a_pillar_carry_no_power_key() {
    let c = Sandbox::new_seeded("no-power-json");
    c.ok(&["add", "Security", "--type", "pillar", "--why", "arbiter"]);

    let rules_out = c.ok(&["rules", "--json"]);
    let rules_v: serde_json::Value =
        serde_json::from_str(&rules_out).expect("rules --json is not JSON");
    assert!(rules_v["pillars"][0].get("power").is_none(), "{rules_out}");

    let why_out = c.ok(&["why", "p1", "--json"]);
    let why_v: serde_json::Value = serde_json::from_str(&why_out).expect("why --json is not JSON");
    assert!(why_v["node"].get("power").is_none(), "{why_out}");
}
