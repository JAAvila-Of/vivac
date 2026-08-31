//! Logs written before the tool moved to English still read.
//!
//! The store is append-only and the log is the source of truth, so a rename
//! that could not read backwards would not be a rename, it would be data loss.
//! Every payload field carries a `serde(alias)` with its old Spanish name; new
//! events are written in English.
//!
//! **The fixture lives in `tests/data/`, not in this file, and that is the
//! point.** A global rename over the sources translated those old names three
//! separate times -- inside the flag alias table, inside the `serde`
//! attributes, and inside this test's own fixture -- and each time the suite
//! stayed green while the thing it was meant to protect was gone. Spanish that
//! is load bearing does not belong in a `.rs` file.

mod common;
use common::Sandbox;

const OLD_LOG: &str = include_str!("data/legacy-spanish.jsonl");

fn with_old_log(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    std::fs::write(c.0.join(".vivac").join("events"), OLD_LOG).unwrap();
    c
}

/// The fixture is genuinely in the old shape. If a rename ever reaches it,
/// every other test here would pass for the wrong reason.
#[test]
fn the_fixture_is_still_spanish() {
    for k in [
        "\"nodo\"",
        "\"tipo\"",
        "\"titulo\"",
        "\"por\"",
        "\"padre\"",
        "\"bloquea\"",
        "\"estado\"",
        "\"resultado\"",
        "\"forzado\"",
        "\"nota\"",
        "\"bandera\"",
        "\"motivo\"",
        "\"pila\"",
        "\"etiqueta\"",
    ] {
        assert!(
            OLD_LOG.contains(k),
            "the fixture lost {k}: it proves nothing now"
        );
    }
}

/// Every field of every payload comes back, with its meaning intact.
#[test]
fn a_spanish_log_still_reads_whole() {
    let c = with_old_log("oldlog");

    // Nothing was skipped on the way in.
    let s = c.ok(&["stats"]);
    assert!(!s.contains("broken lines"), "a line did not parse:\n{s}");
    assert!(s.contains("nodes          3"), "a node got lost:\n{s}");

    let t = c.ok(&["tree", "--all"]);
    assert!(t.contains("La meta"), "the root got lost:\n{t}");
    assert!(t.contains("Una pregunta"), "the child got lost:\n{t}");

    // `padre` -> parent: the provenance edge survived the rename.
    let w = c.ok(&["why", "2"]);
    assert!(w.contains("La meta"), "the lineage got lost:\n{w}");
    assert!(w.contains("decide el backend"), "`por` got lost:\n{w}");

    // `bloquea` -> blocks: the closure rule still refuses.
    let (s, code) = c.run(&["done", "1"]);
    assert_eq!(code, 1, "the blocking edge got lost:\n{s}");

    // `estado`, `resultado` and `forzado`: a forced close keeps its trace, so
    // it stays marked in the tree and stays out of triage.
    assert!(
        c.ok(&["why", "3"]).contains("terminada a la fuerza"),
        "the outcome of a forced close got lost"
    );
    assert!(
        !c.ok(&["triage"]).contains("FALSE CLOSES"),
        "the forced mark got lost, so it came back to triage"
    );

    // `nota`, `bandera` and `motivo`.
    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(b.contains("asumia Redis"), "the flag reason got lost:\n{b}");

    // `pila` and `etiqueta` inside the vivac.
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("antes del adaptador"),
        "the label got lost:\n{v}"
    );
    let r = c.ok(&["restore", "v1"]);
    assert!(r.contains("seguir por aqui"), "the next_intent got lost:\n{r}");
}

/// And what gets appended from now on is English, in the same file, without
/// the reader having to care which half it is looking at.
#[test]
fn what_is_written_from_now_on_is_english() {
    let c = with_old_log("mixed");
    c.ok(&["note", "1", "a new note"]);

    let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    let last_line = log.lines().last().unwrap();
    assert!(
        last_line.contains(r#""node":"#),
        "it did not write the new key:\n{last_line}"
    );
    assert!(
        !last_line.contains(r#""nodo":"#),
        "it wrote the old key:\n{last_line}"
    );
    assert!(
        last_line.contains(r#""note":"#),
        "it did not write the new key:\n{last_line}"
    );

    // And the mixed file still folds into one tree.
    let (s, code) = c.run(&["check"]);
    assert_eq!(code, 0, "a mixed log broke the fold:\n{s}");
    assert!(c.ok(&["tree", "--all"]).contains("La meta"));
}
