//! `push` — the depth advice, the one thing it prints beyond the node it opened.
//!
//! `MODEL.md` §6.1: intervene from four deep, never block. The number it
//! reports measures the **stack** and not the lineage (`f156`), so the node it
//! names has to be the one that stack was opened from. Naming anything else
//! gives a true number pointing at the wrong goal, and sends `promote` -- the
//! remedy, which cuts the stack -- somewhere it cannot help.

mod common;
use common::Sandbox;

/// Opens `n` levels and returns what the last `push` printed.
fn descend(c: &Sandbox, goal: &str, n: usize) -> String {
    let mut last = c.ok(&["push", goal, "--why", "the goal itself"]);
    for i in 1..n {
        last = c.ok(&[
            "push",
            &format!("Detour {i}"),
            "--why",
            "spotted along the way",
        ]);
    }
    last
}

/// Three deep is still a detour and says nothing. The threshold is the whole
/// design: a warning that fires early is a warning people learn to skip.
#[test]
fn under_four_deep_there_is_no_advice() {
    let c = Sandbox::new_seeded("shallow");
    let s = descend(&c, "A goal", 3);
    assert!(!s.contains("levels away"), "it warned at three deep:\n{s}");
}

/// Four deep is where it starts, and it names where the stack began.
#[test]
fn at_four_deep_the_advice_names_where_the_stack_began() {
    let c = Sandbox::new_seeded("deep");
    let s = descend(&c, "A goal", 4);
    assert!(
        s.contains("You are 4 levels away from g1 \"A goal\"."),
        "{s}"
    );
    assert!(s.contains("did the real goal move?"), "{s}");
    assert!(
        s.contains("vivac promote"),
        "no way out of the warning:\n{s}"
    );
}

/// `f331`: with more than one root -- which is exactly what `promote` exists
/// to make -- the advice used to name whichever root was written first, even
/// a closed one on a lineage you were nowhere near.
#[test]
fn with_several_roots_the_advice_names_your_own() {
    let c = Sandbox::new_seeded("roots");
    c.ok(&["push", "First goal", "--why", "it came first"]);
    c.ok(&["pop", "done"]);
    let s = descend(&c, "Second goal", 4);
    assert!(
        s.contains("You are 4 levels away from g2 \"Second goal\"."),
        "it named a root that is not the one this stack came from:\n{s}"
    );
    assert!(!s.contains("First goal"), "{s}");
}
