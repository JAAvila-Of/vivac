//! What a stranger reads: the chrome the binary prints.
//!
//! This file exists because the same defect got through **four separate
//! times** during the port to English. A rename over the sources reached
//! inside string literals and swapped words for whatever the surrounding code
//! happened to call them: `vivac open` printed *"6 frente openeds"* and the
//! subtree counts came out as *"3 open_count / 8 closed_count"*. Every time it
//! compiled, every time the suite stayed green, and the only thing that ever
//! caught it was running the binary by hand against a real tree.
//!
//! So the suite runs it. Two rules over everything the tool prints:
//!
//! 1. no Spanish word, and
//! 2. no snake_case token -- an identifier that leaks into prose always
//!    arrives wearing one, which makes the second class detectable without
//!    knowing in advance which identifier it was.
//!
//! `--json` is exempt from rule 2 on purpose: there the underscore is the
//! contract, not an accident.

mod common;
use common::Sandbox;

/// Spanish words that cannot appear in the chrome.
///
/// Every one of them was in the output at some point; that is the only reason
/// it is on the list. Words English shares -- `no`, `sin`, `con` -- are left
/// out, because a guard that cries wolf gets deleted.
const SPANISH: &[&str] = &[
    "frente",
    "nodo",
    "padre",
    "hijo",
    "arbol",
    "abierto",
    "abierta",
    "cerrado",
    "cerrada",
    "razon",
    "motivo",
    "aqui",
    "pila",
    "titulo",
    "estado",
    "hallazgo",
    "camino",
    "marcado",
    "riesgo",
    "forzado",
    "bandera",
    "etiqueta",
    "tarea",
    "pregunta",
    "meta",
    "para",
    "porque",
    "cuando",
    "donde",
    "desde",
    "hasta",
    "una",
    "esta",
    "este",
    "que",
    "por",
    "los",
    "las",
    "del",
    "mas",
    "siguiente",
];

/// Splits on anything that is not a word character, keeping `_` inside the
/// token so a leaked identifier stays in one piece.
fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
}

fn assert_reads_as_english(label: &str, out: &str) {
    for w in words(out) {
        assert!(
            !SPANISH.contains(&w.to_lowercase().as_str()),
            "`vivac {label}` printed the Spanish word `{w}`:\n{out}"
        );
        // Case matters: `SONAR_TOKEN` in the redaction advice is a name being
        // quoted, not a variable that escaped.
        assert!(
            !(w.contains('_') && w.chars().all(|c| c.is_ascii_lowercase() || c == '_')),
            "`vivac {label}` printed `{w}`, which is an identifier and not a word:\n{out}"
        );
    }
}

/// A tree with something of every shape in it, so the sweep below has
/// something to print about.
fn seeded() -> Sandbox {
    let c = Sandbox::new_seeded("english");
    c.ok(&["push", "Ship the release", "--why", "the build is stale"]);
    c.ok(&["push", "Fix the cache adapter", "--why", "sessions expire early"]);
    c.ok(&[
        "push",
        "No test covers expiry",
        "--why",
        "the bug had no way to reproduce",
        "--blocks",
    ]);
    c.ok(&["pop", "reproduced: it expires at 300s"]);
    c.ok(&["pop", "adapter fixed"]);
    c.ok(&["decide", "Ship from a tag", "--reason", "the tag is the anchor"]);
    c.ok(&["add", "Write the announcement", "--why", "nobody knows yet"]);
    c.ok(&["park", "6", "the wording waits for the release"]);
    c.ok(&["flag", "4", "review", "--why", "the reason may be stale"]);
    c.ok(&["note", "2", "the adapter also owns the retry budget"]);
    c.ok(&["save", "before the release", "--next", "cut the tag"]);
    c
}

/// The sweep. Every command that prints prose, over a tree that has open
/// nodes, closed nodes, a decision, a parked node, a flag, a note and a stop.
#[test]
fn nothing_the_tool_prints_is_in_spanish() {
    let c = seeded();
    let commands: &[&[&str]] = &[
        &["brief"],
        &["why", "3"],
        &["tree"],
        &["tree", "--all"],
        &["open"],
        &["stack"],
        &["parked"],
        &["triage"],
        &["stats"],
        &["check"],
        &["vivacs"],
        &["restore", "v1"],
        &["hooks"],
        &["session", "start"],
    ];
    for cmd in commands {
        let out = c.ok(cmd);
        assert_reads_as_english(&cmd.join(" "), &out);
    }
}

/// The paths that refuse. They print the most prose of anything in the tool
/// and the least-run code, which is the combination that hides a bad string.
#[test]
fn the_refusals_read_as_english_too() {
    let c = Sandbox::new_seeded("refusals");
    c.ok(&["push", "Permissions audit", "--why", "it is due"]);
    c.ok(&["push", "Two roles ignore the guard", "--why", "found in the audit", "--blocks"]);
    c.ok(&["focus", "1"]);

    // The closure rule: the one refusal the model makes on its own.
    let (out, code) = c.run(&["done", "1"]);
    assert_eq!(code, 1, "the closure rule let it through:\n{out}");
    assert_reads_as_english("done 1", &out);

    // The redaction guard. The sample is an AWS key and not a `ghp_` one
    // because the refusal quotes the offending token back, and the rule above
    // cannot tell a quoted input from a leaked identifier -- both are just an
    // underscore in the middle of prose.
    let (out, code) = c.run(&["note", "1", "the key is AKIAIOSFODNN7EXAMPLE"]);
    assert_eq!(code, 3, "the guard let a token through:\n{out}");
    assert_reads_as_english("note (redacted)", &out);

    // Usage, and an id that is not there.
    let (out, _) = c.run(&["block"]);
    assert_reads_as_english("block", &out);
    let (out, _) = c.run(&["why", "999"]);
    assert_reads_as_english("why 999", &out);
    let (out, _) = c.run(&["nonsense"]);
    assert_reads_as_english("nonsense", &out);
}

/// `block --off` printed *"f2 ya no blocks the close of g1"*: half the
/// sentence translated, half of it not. Nothing looked at that line, which is
/// how it survived the port and shipped in `0.2.0`.
#[test]
fn the_block_line_is_a_whole_sentence() {
    let c = Sandbox::new_seeded("block");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["push", "A condition", "--why", "found along the way"]);

    let on = c.ok(&["block", "2"]);
    assert!(on.contains("blocks the close of"), "{on}");
    assert_reads_as_english("block 2", &on);

    let off = c.ok(&["block", "2", "--off"]);
    assert!(
        off.contains("no longer blocks the close of"),
        "the off branch does not read as a sentence:\n{off}"
    );
    assert_reads_as_english("block 2 --off", &off);
}
