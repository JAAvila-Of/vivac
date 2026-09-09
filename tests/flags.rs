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
    // Held for the whole test and handed to `--port` still bound, which is
    // what makes the exit code below say something: reaching `web::serve`
    // would mean binding a port that is taken, and that is an I/O failure
    // and not a usage one -- the test below this one pins that difference.
    // This used to release the port and then assert that nothing had taken
    // it, which raced every other test in the binary for no gain (`f385`).
    let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = held.local_addr().unwrap().port();
    let (out, code) = c.run(&["web", "--port", &port.to_string(), "--no-open", "--bogus"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--bogus"), "{out}");
}

/// The premise the test above rests on: a port it cannot have is a different
/// failure from a flag it does not know, and the exit code tells them apart.
#[test]
fn a_port_already_taken_fails_as_io_and_not_as_usage() {
    let c = Sandbox::new_empty("web-taken-port");
    let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = held.local_addr().unwrap().port();
    let (out, code) = c.run(&["web", "--port", &port.to_string(), "--no-open"]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains("Input/output error"), "{out}");
}
