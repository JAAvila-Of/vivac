//! `triage` — the pruning view.
//!
//! `BRIEF-SPEC.md` §4 names it: a brief over budget must not lie by omission,
//! the signal is that the graph needs pruning. `MODEL.md` §6.1 sends it the
//! deep nodes. And `d33` sends it the rescued ones, which stay hanging off a
//! discarded parent on purpose.

mod common;
use common::Sandbox;

fn section(s: &str, title: &str) -> bool {
    s.lines().any(|l| l.trim_start().starts_with(title))
}

/// A healthy tree has nothing to prune, and says so without empty sections.
#[test]
fn nothing_to_prune() {
    let c = Sandbox::new_seeded("healthy");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let s = c.ok(&["triage"]);
    assert!(s.contains("Nothing to prune"), "{s}");
    for t in ["PARKED", "FALSE CLOSES", "SURVIVED"] {
        assert!(!s.contains(t), "it emitted an empty {t}:\n{s}");
    }
}

/// A parked node comes out with the reason it was parked for. Without it
/// nothing can be decided, which is what the view exists for.
#[test]
fn parked_nodes_come_out_with_their_reason() {
    let c = Sandbox::new_seeded("parked");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["push", "A detour", "--why", "spotted along the way"]);
    c.ok(&["park", "the backend had to be decided first"]);
    let s = c.ok(&["triage"]);
    assert!(section(&s, "PARKED"), "{s}");
    assert!(s.contains("A detour"), "{s}");
    assert!(
        s.contains("the backend had to be decided"),
        "no reason:\n{s}"
    );
    assert!(s.contains("focus <id>"), "no concrete action:\n{s}");
}

/// The close that turns false **later**, when a blocker gets hung on something
/// already closed. It is the measured case that took 26 days to spot, and the
/// only one the closure rule cannot prevent: when `done` ran, the finding did
/// not exist yet.
#[test]
fn a_false_close_comes_out_with_its_count() {
    let c = Sandbox::new_seeded("false");
    c.ok(&["push", "Permissions audit", "--why", "it is due for review"]);
    c.ok(&["pop", "report delivered"]);
    c.ok(&[
        "add",
        "Unfixed finding",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "came out of the audit, late",
    ]);
    let s = c.ok(&["triage"]);
    assert!(section(&s, "FALSE CLOSES"), "{s}");
    assert!(s.contains("Permissions audit"), "{s}");
    assert!(s.contains("1 blocker"), "it did not say how many are left: {s}");
}

/// A **forced** close does not come back here. It was a decision, it leaves a
/// trace and the tree marks it; repeating it every day would be asking for
/// what was already decided to be decided again. Same exemption `check` makes.
#[test]
fn a_forced_close_does_not_come_back_to_triage() {
    let c = Sandbox::new_seeded("forced");
    c.ok(&["push", "Permissions audit", "--why", "it is due for review"]);
    c.ok(&[
        "add",
        "Unfixed finding",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "came out of the audit",
    ]);
    c.ok(&[
        "done",
        "1",
        "closing anyway, the finding stands on its own",
        "--force",
    ]);
    let s = c.ok(&["triage"]);
    assert!(
        !s.contains("FALSE CLOSES"),
        "it insists on what was already decided: {s}"
    );

    // But the tree still marks it: it is not hidden, it just stops repeating.
    let t = c.ok(&["tree", "--all"]);
    assert!(t.contains("FALSE CLOSE"), "the tree stopped marking it: {t}");
}

/// Whatever survives an abandonment stays alive under something discarded on
/// purpose (`d33`), so it has to be looked at again. This is where.
#[test]
fn the_rescued_node_comes_back_past_the_eye() {
    let c = Sandbox::new_seeded("rescued");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["push", "Pick a backend", "--why", "the store needs one"]);
    c.ok(&[
        "add",
        "Benchmark",
        "--parent",
        "2",
        "--why",
        "it has to be measured first",
    ]);
    c.ok(&["abandon", "2", "the backend no longer applies", "--rescue", "3"]);

    let s = c.ok(&["triage"]);
    assert!(section(&s, "SURVIVED A DISCARD"), "{s}");
    assert!(s.contains("Benchmark"), "{s}");
    assert!(
        s.contains("the backend no longer applies"),
        "it does not say why its parent fell:\n{s}"
    );

    // Only the boundary, not the whole branch: what hangs off the rescued node
    // is not repeated, or the pruning view would be the tree all over again.
    c.ok(&[
        "add",
        "Grandchild of the rescued node",
        "--parent",
        "3",
        "--why",
        "it hangs there",
    ]);
    let s = c.ok(&["triage"]);
    assert!(!s.contains("Grandchild"), "it listed the whole branch:\n{s}");
}

/// `MODEL.md` §6.1: from 6 on it shows up in triage, with `promote` as the way
/// out. A deep stack is almost never lack of discipline.
#[test]
fn from_six_deep_onward() {
    let c = Sandbox::new_seeded("deep");
    for i in 1..=5 {
        c.ok(&["push", &format!("Level {i}"), "--why", "still going down"]);
    }
    let s = c.ok(&["triage"]);
    assert!(!s.contains("6 OR MORE"), "it warned at five:\n{s}");

    c.ok(&["push", "Level 6", "--why", "one more"]);
    let s = c.ok(&["triage"]);
    assert!(section(&s, "6 OR MORE FROM THE ROOT"), "{s}");
    assert!(s.contains("promote <id>"), "no way out offered:\n{s}");
    assert!(s.contains("depth 6"), "{s}");
}

/// The other half of the audience. `--json` carries the four baskets, empty
/// ones included: a consumer should not have to guess whether a key is missing
/// or the basket is empty.
#[test]
fn the_json_carries_the_four_baskets() {
    let c = Sandbox::new_seeded("json");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let s = c.ok(&["triage", "--json"]);
    for k in ["parked", "deep", "orphaned_by_discard", "false_closes"] {
        assert!(s.contains(k), "the {k} basket is missing:\n{s}");
    }
}
