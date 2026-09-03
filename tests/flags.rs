//! Every command passes the unknown-flag check before it does anything.
//!
//! `init`, `hooks`, `mcp` and `web` all return before `ops::Ctx::load`, so a
//! check gated on that load never saw them: `vivac init --bogus` planted a
//! tree, `vivac hooks --bogus` printed the hooks, and `vivac mcp --bogus` and
//! `vivac web --bogus` would have gone on to serve. Each one ignored the
//! flag it did not understand instead of refusing it -- the exact failure
//! `f51` describes for the commands that do load a store (`f150`).
//!
//! `stack --bogus` already worked, which is how the gap showed: the table
//! existed, it just sat below four early returns instead of above them.

mod common;
use common::Sandbox;
use std::net::TcpListener;

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

#[test]
fn web_rejects_an_unknown_flag_without_binding_a_port() {
    let c = Sandbox::new_empty("web-bogus");
    let port = {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let (out, code) = c.run(&["web", "--port", &port.to_string(), "--no-open", "--bogus"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--bogus"), "{out}");
    // The command returned before ever calling `web::serve`, so the port it
    // would have bound is still free.
    assert!(
        TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "something is still holding the port:\n{out}"
    );
}
