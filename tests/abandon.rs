//! `abandon` with rescue — `MODEL.md` §6, with the exception recorded in `d33`.
//!
//! Rescue **does not reparent**. Invariant 11 says a thing is born in one
//! place, and the schema makes `spawns` immutable on purpose: it travels
//! inside the creation event. A rescued node stays where it was born, alive
//! under an abandoned parent. These tests are what holds that promise up.

mod common;
use common::Caja;

/// g1 > t2 > t3 > t4. Abandoning t2 puts the other three in play.
fn rama(nombre: &str) -> Caja {
    let c = Caja::nueva(nombre);
    c.ok(&["push", "Migrate authentication", "--why", "the provider is shutting down"]);
    c.ok(&["push", "Pick a cache backend", "--why", "the token store needs one"]);
    c.ok(&[
        "add",
        "Serialization benchmark",
        "--parent",
        "2",
        "--why",
        "it has to be measured first",
    ]);
    c.ok(&[
        "add",
        "Drop dead imports",
        "--parent",
        "3",
        "--why",
        "spotted along the way",
    ]);
    c
}

/// Without `--cascade` nothing falls, and the list of what would fall comes
/// out whole. Abandoning has to cost the same as `pop`, but not in silence.
#[test]
fn without_cascade_nothing_falls() {
    let c = rama("nocascade");
    let (s, cod) = c.correr(&["abandon", "2", "the backend no longer applies"]);
    assert_eq!(cod, 1, "it had to refuse:\n{s}");
    for t in ["Pick a cache backend", "benchmark", "Drop dead imports"] {
        assert!(s.to_lowercase().contains(&t.to_lowercase()), "it did not list {t}:\n{s}");
    }
    assert!(s.contains("--rescue"), "it did not offer the rescue:\n{s}");

    // And nothing moved.
    let t = c.ok(&["tree", "--all"]);
    assert!(!t.contains("[!]"), "it abandoned something without confirmation:\n{t}");
}

/// Rescuing a node rescues its descendants. Saving the parent and letting the
/// children die would be a half rescue nobody asked for.
#[test]
fn a_rescue_drags_the_descendants_along() {
    let c = rama("drags");
    let s = c.ok(&["abandon", "2", "the backend no longer applies", "--rescue", "3"]);
    assert!(s.contains("Rescued"), "{s}");

    let t = c.ok(&["tree", "--all"]);
    assert!(t.contains("[!] t2"), "t2 had to fall:\n{t}");
    assert!(t.contains("[ ] t3"), "t3 had to survive:\n{t}");
    assert!(t.contains("[ ] t4"), "t4 fell with its rescued parent:\n{t}");
}

/// If everything open gets rescued there is nothing left to confirm, and
/// `--cascade` stops being needed: only what falls unnamed is confirmed.
#[test]
fn rescuing_everything_needs_no_cascade() {
    let c = rama("all");
    let (_, cod) = c.correr(&["abandon", "2", "no longer applies", "--rescue", "3"]);
    assert_eq!(cod, 0, "it asked for a cascade with nothing to take down");
}

/// **The product's promise.** The rescued node is still born from an abandoned
/// one, and `why` says so instead of hiding it. Reparenting would have made
/// this path lie.
#[test]
fn the_rescued_node_is_still_born_where_it_was_born() {
    let c = rama("lineage");
    c.ok(&["abandon", "2", "the backend no longer applies", "--rescue", "3"]);

    let w = c.ok(&["why", "3"]);
    assert!(w.contains("Pick a cache backend"), "it erased the origin:\n{w}");
    assert!(w.contains("abandoned"), "it does not say the origin fell:\n{w}");
    assert!(
        w.contains("the backend no longer applies"),
        "it lost the reason for the abandonment:\n{w}"
    );

    // And the store stays healthy: something alive under something abandoned
    // is a legitimate shape of the tree, not corruption.
    let (_, cod) = c.correr(&["check"]);
    assert_eq!(cod, 0, "check took it for broken");
}

/// The stack is the path to the focus and it cannot cross an abandoned node.
/// The rescued node stays alive, but off the path.
#[test]
fn the_stack_does_not_cross_an_abandoned_node() {
    let c = rama("stack");
    c.ok(&["focus", "3"]);
    c.ok(&["abandon", "2", "no longer applies", "--rescue", "3"]);
    let s = c.ok(&["stack"]);
    assert!(
        !s.contains("Pick a cache backend"),
        "an abandoned node is still on the stack:\n{s}"
    );
    assert!(
        !s.contains("benchmark") && !s.contains("Benchmark"),
        "the rescued node was left on a path that no longer exists:\n{s}"
    );
}

/// Rescuing something that does not hang off the abandoned node makes no
/// sense, and saying so is cheaper than letting it through: the agent's CLI
/// ignores nothing.
#[test]
fn what_does_not_hang_off_it_cannot_be_rescued() {
    let c = rama("foreign");
    let (s, cod) = c.correr(&[
        "abandon",
        "2",
        "no longer applies",
        "--cascade",
        "--rescue",
        "1",
    ]);
    assert_eq!(cod, 2, "it let an impossible rescue through:\n{s}");
    assert!(s.contains("does not hang off"), "{s}");

    let (s, cod) = c.correr(&[
        "abandon",
        "2",
        "no longer applies",
        "--cascade",
        "--rescue",
        "2",
    ]);
    assert_eq!(cod, 2, "it rescued itself from itself:\n{s}");
}
