//! `vivac mcp` — the tree as tools an agent can call.
//!
//! `d100` is why this exists: engram is an MCP server, and taking its place
//! means being reachable through the same door, in the tool list, with a
//! schema. `INTEGRATION.md` §4 is why it does not claim more than that --
//! tools are voluntary, and an agent under task pressure does not call them.
//!
//! The rule the whole thing hangs from: **standard output is the protocol**.
//! One stray `println!` in a path the server touches does not read as untidy,
//! it corrupts the channel and the client hangs up. That is what
//! `nothing_but_json_rpc_reaches_standard_output` is for.

mod common;
use common::Sandbox;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

struct Server {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Server {
    fn start(c: &Sandbox) -> Server {
        let mut child = Command::new(BIN)
            .current_dir(&c.0)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Server {
            child,
            input,
            output,
        }
    }

    /// Writes a line and reads the one line that answers it.
    fn ask(&mut self, line: &str) -> Value {
        self.notify(line);
        let mut buf = String::new();
        self.output.read_line(&mut buf).unwrap();
        assert!(!buf.is_empty(), "the server closed without answering");
        serde_json::from_str(&buf).unwrap_or_else(|e| panic!("this is not JSON-RPC: {e}\n{buf}"))
    }

    /// Writes a line and expects nothing back.
    fn notify(&mut self, line: &str) {
        writeln!(self.input, "{line}").unwrap();
        self.input.flush().unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const HELLO: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;

fn hello(c: &Sandbox) -> Server {
    let mut s = Server::start(c);
    s.ask(HELLO);
    s.notify(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    s
}

fn seeded(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "the version was a hand edit",
    ]);
    c.ok(&[
        "add",
        "Guard the commit messages",
        "--why",
        "a malformed one does not count",
        "--type",
        "task",
    ]);
    c
}

fn text_of(reply: &Value) -> String {
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {reply}"))
        .to_string()
}

#[test]
fn initialize_answers_with_the_server_and_its_version() {
    let c = seeded("init");
    let mut s = Server::start(&c);
    let r = s.ask(HELLO);
    assert_eq!(r["jsonrpc"], "2.0");
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["serverInfo"]["name"], "vivac");
    assert_eq!(
        r["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(r["result"]["capabilities"]["tools"].is_object(), "{r}");
}

/// Four, and no more. Every tool costs context in every session the agent
/// ever opens, so the list is a budget and not a catalogue.
#[test]
fn the_tool_list_is_the_four_and_only_the_four() {
    let c = seeded("list");
    let mut s = hello(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = r["result"]["tools"].as_array().unwrap().clone();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        ["vivac_brief", "vivac_find", "vivac_open", "vivac_why"]
    );
    for t in &tools {
        assert!(
            t["description"]
                .as_str()
                .map(|d| d.len() > 20)
                .unwrap_or(false),
            "a tool with no description is a tool nobody calls: {t}"
        );
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
    }
}

#[test]
fn the_brief_comes_back_as_the_prose_it_is() {
    let c = seeded("brief");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"vivac_brief","arguments":{}}}"#,
    );
    let t = text_of(&r);
    assert!(t.contains("Ship the release apparatus"), "{t}");
}

#[test]
fn find_comes_back_as_the_json_the_cli_would_print() {
    let c = seeded("find");
    let cli_text = c.ok(&["find", "malformed", "--json"]);
    let cli: Value = serde_json::from_str(&cli_text).expect("the CLI payload is not JSON");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"vivac_find","arguments":{"query":"malformed"}}}"#,
    );
    let t = text_of(&r);
    let v: Value = serde_json::from_str(&t).expect("the payload is not JSON");
    assert_eq!(v, cli, "the MCP tool and `find --json` disagree:\n{t}");
}

#[test]
fn why_carries_the_path_down_from_the_goal() {
    let c = seeded("why");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"vivac_why","arguments":{"id":"t2"}}}"#,
    );
    let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert_eq!(v["node"]["title"], "Guard the commit messages");
    assert!(v["path"].as_array().unwrap().len() >= 2, "{v}");
}

#[test]
fn open_lists_the_fronts() {
    let c = seeded("open");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"vivac_open","arguments":{}}}"#,
    );
    let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert!(!v.as_array().unwrap().is_empty(), "{v}");
}

/// A refusal the model can read and act on, not a protocol error that only
/// the client ever sees. `isError` is the difference.
#[test]
fn a_node_that_does_not_exist_is_an_error_the_model_can_read() {
    let c = seeded("missing");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"vivac_why","arguments":{"id":"t999"}}}"#,
    );
    assert!(
        r["error"].is_null(),
        "it answered at the protocol level: {r}"
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(text_of(&r).contains("t999"), "{r}");
}

#[test]
fn a_search_with_no_query_is_an_error_the_model_can_read() {
    let c = seeded("noquery");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"vivac_find","arguments":{}}}"#,
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
}

#[test]
fn an_unknown_method_gets_a_json_rpc_error() {
    let c = seeded("unknown");
    let mut s = hello(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":9,"method":"resources/list"}"#);
    assert_eq!(r["error"]["code"], -32601, "{r}");
}

/// A notification has no id and gets no answer. Answering one would leave a
/// line on the wire nobody is waiting for, and everything after it would be
/// read as the reply to something else.
#[test]
fn a_notification_gets_no_answer() {
    let c = seeded("notify");
    let mut s = hello(&c);
    s.notify(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":10,"method":"tools/list"}"#);
    assert_eq!(
        r["id"], 10,
        "an answer to the notification was still in the pipe: {r}"
    );
}

/// The one that guards the rule the design hangs from.
#[test]
fn nothing_but_json_rpc_reaches_standard_output() {
    let c = seeded("clean");
    let mut s = hello(&c);
    for line in [
        r#"{"jsonrpc":"2.0","id":11,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"vivac_brief","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"vivac_open","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"vivac_why","arguments":{"id":"g1"}}}"#,
    ] {
        let r = s.ask(line);
        assert_eq!(r["jsonrpc"], "2.0", "{r}");
    }
}

/// The server outlives the calls, and something else writes the same log --
/// the agent through the CLI, another session. A tree kept from the first
/// call would answer this question with the tree from the last one.
#[test]
fn a_node_written_while_the_server_runs_is_seen_by_the_next_call() {
    let c = seeded("fresh");
    let mut s = hello(&c);
    let before = text_of(&s.ask(
        r#"{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"vivac_find","arguments":{"query":"parrot"}}}"#,
    ));
    assert!(!before.contains("parrot"), "{before}");

    c.ok(&[
        "add",
        "A parrot appeared",
        "--why",
        "written from outside the server",
        "--type",
        "finding",
    ]);

    let after = text_of(&s.ask(
        r#"{"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"vivac_find","arguments":{"query":"parrot"}}}"#,
    ));
    assert!(
        after.contains("A parrot appeared"),
        "the tree was stale:\n{after}"
    );
}

/// Every tool is a command the CLI already has.
///
/// `INTEGRATION.md` §8 listed five that no command implements -- `ask`,
/// `answer`, `assume`, `verify`, `refute` -- and a function reachable from one
/// surface only leaves half the audience outside it, which the DX pillar
/// refuses by name. The tool is the command with `vivac_` in front of it, so
/// the two surfaces cannot drift apart without this going red.
#[test]
fn every_tool_is_a_command_the_cli_already_has() {
    let c = seeded("mirror");
    let mut s = hello(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":17,"method":"tools/list"}"#);
    for t in r["result"]["tools"].as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        let command = name
            .strip_prefix("vivac_")
            .unwrap_or_else(|| panic!("a tool not named after its command: {name}"));
        let (out, _) = c.run(&[command]);
        assert!(
            !out.contains("unknown command"),
            "{name} mirrors nothing: `vivac {command}` is not a command
{out}"
        );
    }
}
