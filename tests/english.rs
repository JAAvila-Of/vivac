//! What a stranger reads: the chrome the binary prints.
//!
//! This file exists because output that is not English got through **five
//! separate times** during the port. Twice a rename reached inside string
//! literals and swapped words for whatever the surrounding code happened to
//! call them -- `vivac open` printed *"6 frente openeds"*, subtree counts came
//! out as *"3 open_count / 8 closed_count"* -- and twice the prose pass simply
//! never opened the line. Every time it compiled, every time the suite stayed
//! green, and the only thing that ever caught one was running the binary by
//! hand.
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
//!
//! **The word list is derived, not remembered, and that is the point.** The
//! first version of this file carried a list of the Spanish words that had
//! already been caught, which is a list of the bugs somebody had already
//! found. It went green over `main.rs` still answering *"Comando
//! desconocido"* and *"push no acepta --bogus"*, because nobody had thought
//! to write down `comando` or `acepta`.
//!
//! `tests/data/spanish-vocabulary.txt` is instead every word the binary
//! printed while it was Spanish -- lifted from the string literals of commit
//! `4846499`, the last one before the port -- minus every word it prints
//! today, plus the Spanish that is deliberately kept as **data**: the flag
//! alias table, the `serde` aliases and the Spanish spellings `Kind::parse`
//! still accepts. A word leaves the list by being spoken in English, which is
//! the only way out that is not a guess.
//!
//! `tools/spanish-vocabulary.py` regenerates it. Run it **after** fixing a
//! string this test caught, never before: regenerating first subtracts the
//! very word that is still wrong, which is how `en_paralelo` -- a key in
//! `why --json` since the port -- stayed out of the list for one commit after
//! the list existed.

mod common;
use common::Sandbox;
use std::collections::HashSet;

/// Every word the tool used to print in Spanish and does not print now. See
/// the note above for how it is derived; it is a file and not a `const`
/// because Spanish that carries weight does not belong in a `.rs`, which this
/// project learned the hard way.
const SPANISH_VOCABULARY: &str = include_str!("data/spanish-vocabulary.txt");

fn spanish() -> HashSet<&'static str> {
    SPANISH_VOCABULARY
        .lines()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .collect()
}

/// Splits on anything that is not a word character, keeping `_` inside the
/// token so a leaked identifier stays in one piece.
fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
}

/// Rule 1 on its own. `--json` gets only this one: there the underscore is
/// the contract, not an accident.
fn assert_no_spanish(label: &str, out: &str) {
    let spanish = spanish();
    // Split on `_` as well: a JSON key is one token to `words`, and
    // `en_paralelo` slipped past exactly there.
    for w in words(out).flat_map(|w| w.split('_')) {
        assert!(
            !spanish.contains(w.to_lowercase().as_str()),
            "`vivac {label}` printed the Spanish word `{w}`:
{out}"
        );
    }
}

fn assert_reads_as_english(label: &str, out: &str) {
    assert_no_spanish(label, out);
    for w in words(out) {
        // Case matters: `SONAR_TOKEN` in the redaction advice is a name being
        // quoted, not a variable that escaped.
        assert!(
            !(w.contains('_') && w.chars().all(|c| c.is_ascii_lowercase() || c == '_')),
            "`vivac {label}` printed `{w}`, which is an identifier and not a word:
{out}"
        );
    }
}

/// A tree with something of every shape in it, so the sweep below has
/// something to print about.
fn seeded() -> Sandbox {
    let c = Sandbox::new_seeded("english");
    c.ok(&["push", "Ship the release", "--why", "the build is stale"]);
    c.ok(&[
        "push",
        "Fix the cache adapter",
        "--why",
        "sessions expire early",
    ]);
    c.ok(&[
        "push",
        "No test covers expiry",
        "--why",
        "the bug had no way to reproduce",
        "--blocks",
    ]);
    c.ok(&["pop", "reproduced: it expires at 300s"]);
    c.ok(&["pop", "adapter fixed"]);
    c.ok(&[
        "decide",
        "Ship from a tag",
        "--reason",
        "the tag is the anchor",
    ]);
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
        &["reconcile"],
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

/// The other half of the audience. The keys of `--json` are the agent's
/// contract and just as public as the prose; `en_paralelo` sat inside
/// `why --json` through two releases because no test had ever asked for JSON.
#[test]
fn the_json_is_english_too() {
    let c = seeded();
    let commands: &[&[&str]] = &[
        &["brief", "--json"],
        &["why", "3", "--json"],
        &["tree", "--json"],
        &["tree", "--all", "--json"],
        &["open", "--json"],
        &["stack", "--json"],
        &["parked", "--json"],
        &["triage", "--json"],
        &["stats", "--json"],
        &["vivacs", "--json"],
        &["reconcile", "--json"],
    ];
    for cmd in commands {
        let out = c.ok(cmd);
        assert_no_spanish(&cmd.join(" "), &out);
    }
}

/// The paths that refuse. They print the most prose of anything in the tool
/// and the least-run code, which is the combination that hides a bad string.
#[test]
fn the_refusals_read_as_english_too() {
    let c = Sandbox::new_seeded("refusals");
    c.ok(&["push", "Permissions audit", "--why", "it is due"]);
    c.ok(&[
        "push",
        "Two roles ignore the guard",
        "--why",
        "found in the audit",
        "--blocks",
    ]);
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

    // Rejecting an option. Both branches: a command with a list of its own,
    // and one that takes nothing but `--json`. This is where the first
    // version of the guard was blind.
    let (out, _) = c.run(&["push", "x", "--why", "y", "--bogus"]);
    assert!(out.contains("does not take --bogus"), "{out}");
    assert_reads_as_english("push --bogus", &out);
    let (out, _) = c.run(&["park", "--bogus"]);
    assert!(out.contains("It takes: none"), "{out}");
    assert_reads_as_english("park --bogus", &out);

    let (out, _) = c.run(&["--help"]);
    assert_reads_as_english("--help", &out);
}

/// The list is only worth what it still contains. If a bad regenerate empties
/// it, or a rename translates it, every assertion above passes over nothing.
#[test]
fn the_word_list_is_still_a_word_list() {
    let v = spanish();
    assert!(v.len() > 400, "the vocabulary shrank to {}", v.len());
    for w in ["comando", "acepta", "frente", "nodo", "camino", "forzado"] {
        assert!(v.contains(w), "`{w}` fell out of the vocabulary");
    }
    for w in ["push", "brief", "stack", "anchor", "manual"] {
        assert!(!v.contains(w), "`{w}` is English and cannot be banned");
    }
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
