//! `check` — the `MODEL.md` §9 invariants, plus `--gates` (`d350`/`d351`).
//!
//! `STORE` and `PROJECT` are about one tree. `GATES` is about every tree
//! this machine's registry knows: the store is fine and nothing is
//! delivering it, measured off each project's own log. A project reports
//! here when every node it holds was written before the first session was
//! ever opened -- "zero openings, ever" is not the criterion: a real tree
//! had exactly one opening, arriving after all of its work, and that late
//! opening was enough to clear a "never opened" filter while the hook still
//! was not wired up for the months of work that came before it.

mod common;
use common::Sandbox;

fn project_name(c: &Sandbox) -> String {
    c.0.file_name().unwrap().to_string_lossy().into_owned()
}

/// Writes a node and registers the project, without ever opening a session.
fn seed_no_session(c: &Sandbox, title: &str, why: &str) {
    c.ok(&["push", title, "--why", why]);
    c.ok(&["stack"]);
}

/// Writes a node, then opens a session the way the hook does: the opening
/// arrives after the only node this project holds.
fn seed_then_open_session(c: &Sandbox, title: &str, why: &str) {
    c.ok(&["push", title, "--why", why]);
    c.ok(&["session", "start", "--hook"]);
}

/// Opens a session first, then writes a node: at least one node came after
/// an opening, which is the one shape `--gates` must not report.
fn open_session_then_seed(c: &Sandbox, title: &str, why: &str) {
    c.ok(&["session", "start", "--hook"]);
    c.ok(&["push", title, "--why", why]);
}

/// Registers the project with no node ever written: the only event a store
/// can hold with nothing on the stack is `session.started` itself.
fn note_project_with_no_nodes(c: &Sandbox) {
    c.ok(&["session", "start", "--hook"]);
}

#[test]
fn a_project_with_nodes_and_no_session_appears_in_gates() {
    let c = Sandbox::new_seeded("gates-reports");
    seed_no_session(&c, "Ship the release", "the tag is cut");
    let name = project_name(&c);

    let (s, code) = c.run(&["check", "--gates"]);

    assert_eq!(code, 1, "{s}");
    assert!(
        s.contains("GATES (1)  <- the store is fine; nothing delivers it"),
        "{s}"
    );
    assert!(
        s.contains(&format!(
            "{name}: 1 nodes written, and not one after a session ever opened"
        )),
        "{s}"
    );
}

/// The case that motivated the correction: a session opened, but only after
/// every node the project holds. All of its work is still before the first
/// opening, so it has to appear -- "zero openings, ever" would have missed
/// exactly this tree.
#[test]
fn a_project_with_a_session_opened_only_after_all_its_nodes_appears() {
    let c = Sandbox::new_seeded("gates-late-open");
    seed_then_open_session(&c, "Ship the release", "the tag is cut");
    let name = project_name(&c);

    let (s, code) = c.run(&["check", "--gates"]);

    assert_eq!(code, 1, "{s}");
    assert!(
        s.contains(&format!(
            "{name}: 1 nodes written, and not one after a session ever opened"
        )),
        "{s}"
    );
}

/// The other side of the same line: a node written **after** a session was
/// already open does not appear, because that node was delivered.
#[test]
fn a_project_with_a_node_written_after_a_session_does_not_appear() {
    let c = Sandbox::new_seeded("gates-early-open");
    open_session_then_seed(&c, "Ship the release", "the tag is cut");
    let name = project_name(&c);

    let s = c.ok(&["check", "--gates"]);

    assert!(
        !s.contains("GATES ("),
        "a project with work after its opening was reported:\n{s}"
    );
    assert!(!s.contains(&name), "{s}");
}

#[test]
fn a_fresh_project_with_no_nodes_does_not_appear() {
    let c = Sandbox::new_seeded("gates-empty");
    note_project_with_no_nodes(&c);
    let name = project_name(&c);

    let s = c.ok(&["check", "--gates"]);

    assert!(
        !s.contains("GATES ("),
        "an empty project was reported:\n{s}"
    );
    assert!(!s.contains(&name), "{s}");
}

/// The line format and the JSON array, against a two-project registry so the
/// count and the closing advice are both exercised.
#[test]
fn gates_json_carries_the_same_lines_as_a_key() {
    let a = Sandbox::new_seeded("gates-json-a");
    seed_no_session(&a, "Ship the release", "the tag is cut");
    let name_a = project_name(&a);
    let b = Sandbox::new_seeded_in("gates-json-b", a.global_home());
    open_session_then_seed(&b, "Guard the release notes", "the version was a hand edit");

    let (s, code) = a.run(&["check", "--gates", "--json"]);
    assert_eq!(code, 1, "{s}");
    let v: serde_json::Value = serde_json::from_str(&s).expect("check --json is not JSON");

    let gates = v["gates"].as_array().expect("gates is not an array");
    assert_eq!(gates.len(), 1, "{s}");
    assert_eq!(
        gates[0].as_str().unwrap(),
        format!("{name_a}: 1 nodes written, and not one after a session ever opened")
    );
    assert_eq!(v["ok"], serde_json::json!(false), "{s}");
}

/// The advice line closes the section, the same way `PROJECT` closes its own.
#[test]
fn gates_prints_the_advice_that_closes_the_section() {
    let c = Sandbox::new_seeded("gates-advice");
    seed_no_session(&c, "Ship the release", "the tag is cut");

    let (s, _) = c.run(&["check", "--gates"]);

    assert!(
        s.contains("A tree nobody opens is a tree nobody reads. Run  vivac hooks  inside"),
        "{s}"
    );
    assert!(s.contains("that project and paste what it prints."), "{s}");
}

/// The regression: without `--gates`, `check` never looks at the registry,
/// never mentions `GATES`, and its `ok`/exit code are exactly what they were
/// before this flag existed -- even with another project sitting in the same
/// registry that `--gates` would report.
#[test]
fn without_the_flag_check_is_unchanged_even_with_a_project_gates_flagged() {
    let a = Sandbox::new_seeded("gates-off-a");
    seed_no_session(&a, "Ship the release", "the tag is cut");
    let b = Sandbox::new_seeded_in("gates-off-b", a.global_home());

    let (text, text_code) = b.run(&["check"]);
    assert_eq!(text_code, 0, "{text}");
    assert!(text.contains("No findings. 0 nodes checked."), "{text}");
    assert!(!text.contains("GATES"), "{text}");

    let (json, json_code) = b.run(&["check", "--json"]);
    assert_eq!(json_code, 0, "{json}");
    let v: serde_json::Value = serde_json::from_str(&json).expect("check --json is not JSON");
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["ok", "project", "store"], "{json}");
}

/// `--gates` names projects by the registry's bare name, never by path: an
/// absolute path names the account and the machine, and the security pillar
/// allows neither into a result.
#[test]
fn gates_output_carries_no_absolute_path() {
    let a = Sandbox::new_seeded("gates-path-a");
    seed_no_session(&a, "Ship the release", "the tag is cut");
    let b = Sandbox::new_seeded_in("gates-path-b", a.global_home());
    seed_no_session(&b, "Guard the release notes", "the version was a hand edit");

    let (s, code) = a.run(&["check", "--gates"]);
    assert_eq!(code, 1, "{s}");

    for root in [&a.0, &b.0] {
        let full = root.to_string_lossy();
        assert!(!s.contains(full.as_ref()), "an absolute path leaked:\n{s}");
    }
}

/// A flag the parser does not know is refused, not ignored, and `--gates` is
/// the one flag `check` gained.
#[test]
fn gates_is_a_known_flag() {
    let c = Sandbox::new_seeded("gates-known");
    let (s, code) = c.run(&["check", "--gates"]);
    assert_eq!(code, 0, "--gates was refused as unknown:\n{s}");
}
