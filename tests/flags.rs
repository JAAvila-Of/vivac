//! Every command passes the unknown-flag check before it does anything.
//!
//! `init`, `hooks` and `mcp` all return before `ops::Ctx::load`, so a check
//! gated on that load never saw them: `vivac init --bogus` planted a tree,
//! `vivac hooks --bogus` printed the hooks, and `vivac mcp --bogus` went on
//! to serve. Each one ignored the flag it did not understand instead of
//! refusing it -- the exact failure `f51` describes for the commands that do
//! load a store (`f150`).
//!
//! `stack --bogus` already worked, which is how the gap showed: the table
//! existed, it just sat below the early returns instead of above them.

mod common;
use common::Sandbox;

#[test]
fn init_rejects_an_unknown_flag_and_plants_no_store() {
    let c = Sandbox::new_empty("init-bogus");
    let (out, code) = c.run(&["init", "--bogus"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--bogus"), "{out}");
    assert!(
        !c.0.join(".vivac").exists(),
        "it planted a tree before noticing the flag:\n{out}"
    );
}

#[test]
fn hooks_rejects_an_unknown_flag_and_prints_nothing() {
    let c = Sandbox::new_seeded("hooks-bogus");
    let (out, code) = c.run(&["hooks", "--bogus"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--bogus"), "{out}");
    assert!(
        !out.contains("Paste this into"),
        "it printed the hooks anyway:\n{out}"
    );
}

#[test]
fn mcp_rejects_an_unknown_flag_without_waiting_on_standard_input() {
    // No `.vivac/` at all: the check runs before `find_root`, and if it did
    // not, this would hang reading a request that never comes rather than
    // failing outright.
    let c = Sandbox::new_empty("mcp-bogus");
    let (out, code) = c.run(&["mcp", "--bogus"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--bogus"), "{out}");
}
