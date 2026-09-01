//! `--help` against the parser.
//!
//! An option that exists and is not announced is an option nobody uses. `add`
//! took `--type`, `--ref` and `--governs` while the help listed only half of
//! what it accepts, and `--type` is the one that decides what a node *is*:
//! without it everything `add` made was born the default kind (`f72`). The
//! parser refuses a flag it does not know, so the two can be checked against
//! each other with nothing but the binary.

mod common;
use common::Sandbox;

/// The help for one command: its line, plus the indented ones under it.
fn block(help: &str, command: &str) -> String {
    let head = format!("vivac {command} ");
    let mut inside = false;
    let mut out = String::new();
    for l in help.lines() {
        if l.trim_start().starts_with("vivac ") {
            inside = l.contains(&head);
        }
        if inside {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

#[test]
fn the_help_announces_every_flag_add_accepts() {
    let c = Sandbox::new_seeded("help-add");
    c.ok(&["push", "Ship the release", "--why", "the tag is cut"]);
    // The parser rejects what it does not know, so exit 0 here is proof that
    // all six are accepted.
    c.ok(&[
        "add",
        "Write the announcement",
        "--parent",
        "1",
        "--why",
        "nobody knows yet",
        "--blocks",
        "--type",
        "task",
        "--ref",
        "docs/RELEASE.md",
        "--governs",
        "docs/",
    ]);
    let help = c.ok(&["--help"]);
    let add = block(&help, "add");
    assert!(!add.is_empty(), "the help says nothing about add:\n{help}");
    for flag in [
        "--parent",
        "--why",
        "--blocks",
        "--type",
        "--ref",
        "--governs",
    ] {
        assert!(
            add.contains(flag),
            "add accepts {flag} and the help does not say so:\n{add}"
        );
    }
}
