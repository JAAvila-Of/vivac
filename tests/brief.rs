//! The test contract from `BRIEF-SPEC.md` §10, against the real binary.
//!
//! No dependencies: `CARGO_BIN_EXE_vivac` comes from cargo, and the store is a
//! temporary directory. Every test seeds its own tree, because a shared one
//! would make execution order matter.

mod common;
use common::Caja;

/// A tree with one of everything, so that no section comes out empty.
fn poblado(nombre: &str) -> Caja {
    let c = Caja::nueva(nombre);
    c.ok(&[
        "push",
        "Migrate authentication to OIDC",
        "--why",
        "the old provider is shutting down",
    ]);
    c.ok(&[
        "add",
        "No dependencies under a copyleft licence",
        "--parent",
        "1",
        "--type",
        "constraint",
        "--why",
        "company policy",
    ]);
    c.ok(&[
        "push",
        "Pick a cache backend",
        "--why",
        "the token store needs one",
        "--governs",
        "src/cache/**",
    ]);
    c.ok(&[
        "decide",
        "Use a distributed token store",
        "--reason",
        "a single node will not hold",
        "--alternative",
        "JWT with no revocation",
    ]);
    c.ok(&[
        "add",
        "Does the token volume fit in one node?",
        "--parent",
        "3",
        "--type",
        "question",
        "--blocks",
        "--why",
        "it decides the backend",
    ]);
    c.ok(&[
        "add",
        "Update the integration tests",
        "--parent",
        "3",
        "--why",
        "the backend changes what has to be stood up",
    ]);
    c.ok(&[
        "park",
        "6",
        "the backend had to be decided before touching the tests",
    ]);
    c.ok(&[
        "flag",
        "4",
        "suspect",
        "--why",
        "it assumed Redis, and there is no Redis in staging",
    ]);
    c.ok(&[
        "save",
        "before touching the adapter",
        "--next",
        "extract the validator",
    ]);
    c
}

fn seccion(brief: &str, titulo: &str) -> bool {
    brief.lines().any(|l| l.trim() == titulo)
}

/// §10.1 — Same log, same `--now`, two runs, same bytes.
#[test]
fn determinism() {
    let c = poblado("det");
    let a = c.ok(&["brief", "--now", "2026-09-15T10:00:00Z"]);
    let b = c.ok(&["brief", "--now", "2026-09-15T10:00:00Z"]);
    assert_eq!(a, b);
    assert!(a.contains("2026-09-15"), "--now overrides the clock:\n{a}");
}

/// §10.2 — With the budget squeezed, the spine comes out whole and says so.
///
/// It is the hardest rule in the specification: if the spine does not fit, the
/// budget is wrong, not the brief. Without it the brief does not answer
/// question 1 and has no reason to exist.
#[test]
fn the_spine_is_never_truncated() {
    let c = poblado("spine");
    let espina = |b: &str| {
        assert!(
            b.contains("Migrate authentication to OIDC"),
            "the root is missing:\n{b}"
        );
        assert!(b.contains("Pick a cache backend"), "the focus is missing:\n{b}");
        assert!(b.contains("<== HERE"), "the marker is missing:\n{b}");
    };

    // Tight but reachable: it fits by trimming, and it says so.
    let b = c.ok(&["brief", "--budget", "200", "--now", "2026-09-15T10:00:00Z"]);
    espina(&b);
    assert!(b.contains("trimmed"), "it trimmed without saying so:\n{b}");

    // Impossible: it does not fit even with everything truncatable gone. The
    // spine comes out anyway, and the warning says what is left over is tree,
    // not render.
    let b = c.ok(&["brief", "--budget", "40", "--now", "2026-09-15T10:00:00Z"]);
    espina(&b);
    assert!(b.contains("over budget"), "it did not warn:\n{b}");
}

/// §10.3 — As the budget drops, sections fall from the bottom up, never
/// skipping.
#[test]
fn truncation_order() {
    let c = poblado("trunc");
    let entero = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(seccion(&entero, "LAST VIVAC"), "{entero}");
    assert!(seccion(&entero, "DO NOT TOUCH NOW"), "{entero}");
    assert!(seccion(&entero, "FLAGGED"), "{entero}");

    // The vivac is section 9 and falls before 7 and 6.
    let apretado = c.ok(&["brief", "--budget", "150", "--now", "2026-09-15T10:00:00Z"]);
    assert!(
        !seccion(&apretado, "LAST VIVAC"),
        "it should have fallen:\n{apretado}"
    );

    // And the non-truncatable ones hold: invariants and blocking questions.
    assert!(seccion(&apretado, "INVARIANTS"), "{apretado}");
    assert!(seccion(&apretado, "BLOCKS"), "{apretado}");
}

/// §10.5 — A superseded decision is never rendered.
#[test]
fn superseded_is_absent() {
    let c = poblado("sup");
    c.ok(&[
        "decide",
        "Use database-backed sessions",
        "--reason",
        "simpler",
        "--supersedes",
        "4",
    ]);
    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(b.contains("Use database-backed sessions"), "{b}");
    assert!(
        !b.contains("Use a distributed token store"),
        "the superseded one is still there:\n{b}"
    );
}

/// §10.7 — An empty stack produces §8, never empty output.
#[test]
fn initial_state() {
    let c = Caja::nueva("initial");
    let b = c.ok(&["brief"]);
    assert!(b.contains("No active focus"), "{b}");
    assert!(b.contains("vivac push"), "no concrete action:\n{b}");

    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["park", "unfinished"]);
    let b = c.ok(&["brief"]);
    assert!(b.contains("No active focus"), "{b}");
    assert!(b.contains("OPEN GOALS") || b.contains("focus"), "{b}");
}

/// §10.8 — No empty section emits a heading.
#[test]
fn no_hollow_headings() {
    let c = Caja::nueva("hollow");
    c.ok(&["push", "Alone", "--why", "nothing hangs off it"]);
    let b = c.ok(&["brief"]);
    for t in [
        "INVARIANTS",
        "BLOCKS",
        "DO NOT TOUCH NOW",
        "FLAGGED",
        "STANDING DECISIONS",
    ] {
        assert!(!seccion(&b, t), "{t} came out empty:\n{b}");
    }
}

/// §10.9 — No flag is rendered without its reason, because it cannot be raised
/// without one.
#[test]
fn reason_is_mandatory() {
    let c = Caja::nueva("reason");
    c.ok(&["push", "Something", "--why", "it is needed"]);
    let (s, cod) = c.correr(&["flag", "1", "suspect"]);
    assert_eq!(cod, 2, "a flag with no reason has to fail:\n{s}");
    assert!(s.contains("--why"), "{s}");
}

/// §10.6 — With no version control there are no diff lines, and it says so.
#[test]
fn degradation_without_an_anchor() {
    let c = poblado("null");
    let s = c.ok(&["restore", "v1"]);
    assert!(s.contains("No anchor"), "{s}");
    assert!(!s.contains("changes since"), "it invented a diff:\n{s}");
}

/// The redaction guard holds here too: there is no back door through `decide`
/// or through `flag`.
#[test]
fn the_guard_covers_the_new_operations() {
    let c = Caja::nueva("guard");
    c.ok(&["push", "Something", "--why", "it is needed"]);
    let (_, cod) = c.correr(&[
        "decide",
        "Rotate",
        "--reason",
        "use ghp_16C7e42F292c6912E7710c838347Ae178B4a",
    ]);
    assert_eq!(cod, 3, "decide let a credential through");
    let (_, cod) = c.correr(&["flag", "1", "review", "--why", "see /home/someone/.config"]);
    assert_eq!(cod, 3, "flag let a personal path through");
}

/// `f30` — a standing decision is not a pending child. It shows up in its own
/// section and nowhere else: listing it twice fills the brief with things not
/// to do, which is the opposite of what it exists for.
#[test]
fn a_decision_is_not_a_front() {
    let c = poblado("dec");
    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert_eq!(
        b.matches("Use a distributed token store").count(),
        1,
        "the decision shows up more than once:\n{b}"
    );

    // And that single time is under STANDING DECISIONS, not BORN FROM HERE.
    let hasta = b.find("STANDING DECISIONS").expect("the section is missing");
    assert!(
        b.find("Use a distributed token store").unwrap() > hasta,
        "it shows up before its section, i.e. as a pending child:\n{b}"
    );

    let o = c.ok(&["open"]);
    assert!(
        !o.contains("Use a distributed token store"),
        "open lists it as a front:\n{o}"
    );
    assert!(
        o.contains("1 standing decision"),
        "open made it vanish without saying so:\n{o}"
    );
}

/// `q26` — closing a parent cannot make its open children invisible.
///
/// The case came out of the project's own tree: `t8` closed with `t9`, `t10`
/// and `f21` open below it, and the brief showed 3 of the 6 fronts. Listing
/// them would drag in the whole tree; counting them does not.
#[test]
fn what_is_open_under_a_closed_node_gets_counted() {
    let c = Caja::nueva("deep");
    c.ok(&["push", "The goal", "--why", "it is needed"]);
    c.ok(&["push", "A branch", "--why", "the goal needs it"]);
    c.ok(&[
        "add",
        "A finding",
        "--parent",
        "2",
        "--why",
        "spotted along the way",
    ]);
    // Closes with an open child that does not block: correct, and the case.
    c.ok(&["pop", "branch finished"]);

    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(
        b.contains("+ 1 further down"),
        "it did not warn about what was left below:\n{b}"
    );
    assert!(
        !b.contains("A finding"),
        "it listed it instead of counting it; that drags in the whole tree:\n{b}"
    );
    assert!(b.contains("vivac open"), "it counted without saying where to look:\n{b}");
}
