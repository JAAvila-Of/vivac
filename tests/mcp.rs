//! `vivac mcp` — the tree as tools an agent can call.
//!
//! `d100` is why this exists: the memory store this replaces is reachable
//! over MCP, and taking its place means being reachable through the same
//! door, in the tool list, with a schema. `INTEGRATION.md` §4 is why it does not claim more than that --
//! tools are voluntary, and an agent under task pressure does not call them.
//!
//! The rule the whole thing hangs from: **standard output is the protocol**.
//! One stray `println!` in a path the server touches does not read as untidy,
//! it corrupts the channel and the client hangs up. That is what
//! `nothing_but_json_rpc_reaches_standard_output` is for.

mod common;
use common::Sandbox;
use serde_json::{json, Value};
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
            .env("VIVAC_HOME", c.global_home())
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

/// Thirteen, and no more. Every tool costs context in every session the
/// agent ever opens, so the list is a budget and not a catalogue: five reads
/// plus the eight writes `t118` and `t411` add between them, and nothing
/// past that.
#[test]
fn the_tool_list_is_the_thirteen_and_only_the_thirteen() {
    let c = seeded("list");
    let mut s = hello(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = r["result"]["tools"].as_array().unwrap().clone();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "vivac_add",
            "vivac_arm",
            "vivac_brief",
            "vivac_decide",
            "vivac_find",
            "vivac_note",
            "vivac_open",
            "vivac_park",
            "vivac_pop",
            "vivac_push",
            "vivac_rules",
            "vivac_save",
            "vivac_why",
        ]
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

/// `d422`: an agent that finds no pillar and no rule needs telling what to
/// do about it, not just told the tree is empty.
#[test]
fn vivac_rules_description_points_at_claude_md_when_nothing_governs() {
    let c = seeded("rules-desc");
    let mut s = hello(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":19,"method":"tools/list"}"#);
    let tools = r["result"]["tools"].as_array().unwrap().clone();
    let rules_tool = tools
        .iter()
        .find(|t| t["name"] == "vivac_rules")
        .expect("vivac_rules is in the tool list");
    let description = rules_tool["description"].as_str().unwrap();
    assert!(
        description.contains(
            "If it comes back with no pillar and no rule while the project keeps its \
             rules in files such as CLAUDE.md or AGENTS.md, propose which are pillars \
             and which are rules, let the person decide, and write them with vivac_add."
        ),
        "{description}"
    );
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

/// `t411`: `vivac_rules` is the same pull `rules --json` already answers,
/// through the second door.
#[test]
fn rules_comes_back_as_the_json_the_cli_would_print() {
    let c = seeded("rules");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c.ok(&[
        "add",
        "Security",
        "--type",
        "pillar",
        "--why",
        "vetoes on the spot",
    ]);
    c.ok(&[
        "add",
        "Never store a secret",
        "--parent",
        "3",
        "--type",
        "rule",
        "--arm",
        "cargo test --bin vivac redact::tests",
        "--arm-dir",
        "vivac",
        "--why",
        "the mechanical half",
    ]);
    let cli_text = c.ok(&["rules", "--json"]);
    let cli: Value = serde_json::from_str(&cli_text).expect("the CLI payload is not JSON");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"vivac_rules","arguments":{}}}"#,
    );
    let t = text_of(&r);
    let v: Value = serde_json::from_str(&t).expect("the payload is not JSON");
    assert_eq!(v, cli, "the MCP tool and `rules --json` disagree:\n{t}");
}

/// `d273`'s second half, on `vivac_find`: `everywhere` fans the search out
/// over the registry instead of the resident project alone, and it has to
/// answer exactly what `find --everywhere --json` does -- `d172`'s tie,
/// carried past one project.
#[test]
fn find_with_everywhere_returns_what_the_cli_returns() {
    let a = seeded("mcp-ew-a");
    let b = Sandbox::new_seeded_in("mcp-ew-b", a.global_home());
    b.ok(&[
        "push",
        "Guard the release notes",
        "--why",
        "the version was a hand edit",
    ]);
    b.ok(&["stack"]);

    let cli_text = b.ok(&["find", "hand edit", "--everywhere", "--json"]);
    let cli: Value = serde_json::from_str(&cli_text).expect("the CLI payload is not JSON");

    let mut s = hello(&b);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"vivac_find","arguments":{"query":"hand edit","everywhere":true}}}"#,
    );
    let t = text_of(&r);
    let v: Value = serde_json::from_str(&t).expect("the payload is not JSON");
    assert_eq!(
        v, cli,
        "the MCP tool and `find --everywhere --json` disagree:\n{t}"
    );
}

/// `t465`: `path` stopped carrying the node itself -- it already travels
/// whole in `node`, so `g1` is both the first step and the last one here,
/// never `t2`.
#[test]
fn why_carries_the_path_down_from_the_goal() {
    let c = seeded("why");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"vivac_why","arguments":{"id":"t2"}}}"#,
    );
    let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert_eq!(v["node"]["title"], "Guard the commit messages");
    let path = v["path"].as_array().unwrap();
    assert_eq!(
        path.last().unwrap()["alias"],
        "g1",
        "the last path step should be the node's own parent:\n{v}"
    );
    assert_eq!(
        path[0]["alias"], "g1",
        "the first path step should be the root:\n{v}"
    );
}

/// The MCP tool's own payload has to equal `why --json`'s, value for value,
/// not just at the fields the tests above happen to reach into.
#[test]
fn why_comes_back_as_the_json_the_cli_would_print() {
    let c = seeded("why-json");
    let cli_text = c.ok(&["why", "t2", "--json"]);
    let cli: Value = serde_json::from_str(&cli_text).expect("the CLI payload is not JSON");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"vivac_why","arguments":{"id":"t2"}}}"#,
    );
    let t = text_of(&r);
    let v: Value = serde_json::from_str(&t).expect("the payload is not JSON");
    assert_eq!(v, cli, "the MCP tool and `why --json` disagree:\n{t}");
}

/// `d273`'s second half, on `vivac_why`: `project` opens a node that lives
/// in another tree, and it has to answer exactly what `why --project --json`
/// does on the CLI.
#[test]
fn why_with_project_returns_what_the_cli_returns() {
    let a = seeded("mcp-wp-a");
    let name_a = a.0.file_name().unwrap().to_string_lossy().into_owned();
    let b = Sandbox::new_seeded_in("mcp-wp-b", a.global_home());

    let cli_text = b.ok(&["why", "t2", "--project", &name_a, "--json"]);
    let cli: Value = serde_json::from_str(&cli_text).expect("the CLI payload is not JSON");

    let mut s = hello(&b);
    let r = s.ask(&format!(
        r#"{{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{{"name":"vivac_why","arguments":{{"id":"t2","project":"{name_a}"}}}}}}"#
    ));
    let t = text_of(&r);
    let v: Value = serde_json::from_str(&t).expect("the payload is not JSON");
    assert_eq!(
        v, cli,
        "the MCP tool and `why --project --json` disagree:\n{t}"
    );
}

/// `t411` §13: a read that reaches into another project's log and finds a
/// line only a newer vivac could have written comes back as the error, not
/// a half-built answer.
#[test]
fn why_with_project_over_an_unknown_event_returns_the_error_not_a_half_answer() {
    let a = seeded("mcp-nv-a");
    let name_a = a.0.file_name().unwrap().to_string_lossy().into_owned();
    a.append_unknown_event_type();
    let b = Sandbox::new_seeded_in("mcp-nv-b", a.global_home());

    let mut s = hello(&b);
    let r = s.ask(&format!(
        r#"{{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{{"name":"vivac_why","arguments":{{"id":"t2","project":"{name_a}"}}}}}}"#
    ));
    assert!(
        r["error"].is_null(),
        "it answered at the protocol level: {r}"
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(
        text_of(&r).contains("This tree was written by a newer vivac"),
        "{r}"
    );
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

#[test]
fn open_comes_back_as_the_json_the_cli_would_print() {
    let c = seeded("open-json");
    let cli_text = c.ok(&["open", "--json"]);
    let cli: Value = serde_json::from_str(&cli_text).expect("the CLI payload is not JSON");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":17,"method":"tools/call","params":{"name":"vivac_open","arguments":{}}}"#,
    );
    let t = text_of(&r);
    let v: Value = serde_json::from_str(&t).expect("the payload is not JSON");
    assert_eq!(v, cli, "the MCP tool and `open --json` disagree:\n{t}");
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

/// The staleness check the resident write path relies on: a write from
/// outside the server, sitting between two writes the server itself does,
/// has to be picked up rather than overwritten. Silently ignoring it would
/// not just lose the CLI's node -- it would append the server's own next
/// event at a `seq` the CLI already claimed, corrupting the log for every
/// reader from then on.
#[test]
fn a_write_from_another_process_between_two_mcp_writes_is_picked_up() {
    let c = seeded("interleave");
    let mut s = hello(&c);

    let first = s.ask(
        r#"{"jsonrpc":"2.0","id":30,"method":"tools/call","params":{"name":"vivac_add","arguments":{"title":"From MCP, before","why":"first"}}}"#,
    );
    assert_eq!(first["result"]["isError"], false, "{first}");

    c.ok(&[
        "add",
        "From the CLI, in between",
        "--why",
        "written while the server was up",
    ]);

    let second = s.ask(
        r#"{"jsonrpc":"2.0","id":31,"method":"tools/call","params":{"name":"vivac_add","arguments":{"title":"From MCP, after","why":"second"}}}"#,
    );
    assert_eq!(second["result"]["isError"], false, "{second}");

    let mcp_open: Value = serde_json::from_str(&text_of(&s.ask(
        r#"{"jsonrpc":"2.0","id":32,"method":"tools/call","params":{"name":"vivac_open","arguments":{}}}"#,
    )))
    .unwrap();
    let cli_open: Value = serde_json::from_str(&c.ok(&["open", "--json"])).unwrap();
    assert_eq!(
        mcp_open, cli_open,
        "the server's resident tree disagrees with a fresh fold of the same log"
    );

    let titles: Vec<&str> = mcp_open
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap())
        .collect();
    assert!(
        titles.contains(&"From MCP, before")
            && titles.contains(&"From the CLI, in between")
            && titles.contains(&"From MCP, after"),
        "one of the three writes is missing: {titles:?}"
    );

    // All three were added with no `--parent` of their own, so all three
    // should have landed under the same focus. If the CLI's write did not,
    // the resident tree treated it as attaching somewhere else instead of
    // picking it up where it was actually written.
    let find_by_title = |title: &str| {
        mcp_open
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["title"] == title)
            .unwrap_or_else(|| panic!("{title} is not in the tree: {mcp_open}"))
            .clone()
    };
    let sibling = find_by_title("From MCP, before");
    let cli_node = find_by_title("From the CLI, in between");
    assert_eq!(
        cli_node["parent"], sibling["parent"],
        "the CLI's node did not land where it should: {cli_node}"
    );
}

/// `t192`'s own guarantee -- the resident tree never disagrees with a fresh
/// fold of its own log -- has to survive `d273`'s second half untouched.
/// `vivac_why`'s `project` argument reads a tree that is not the server's
/// own, through the local index rather than the resident `Project`, and
/// nothing about answering that foreign read may disturb the fold this
/// server actually holds.
#[test]
fn a_foreign_project_read_leaves_the_resident_tree_untouched() {
    let c = seeded("foreign-untouched");
    let foreign = Sandbox::new_seeded_in("foreign-untouched-other", c.global_home());
    foreign.ok(&[
        "push",
        "Foreign root",
        "--why",
        "lives in another tree entirely",
    ]);
    foreign.ok(&["stack"]);
    let foreign_name = foreign
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let mut s = hello(&c);

    let before = s.ask(
        r#"{"jsonrpc":"2.0","id":40,"method":"tools/call","params":{"name":"vivac_add","arguments":{"title":"Before the foreign read","why":"a"}}}"#,
    );
    assert_eq!(before["result"]["isError"], false, "{before}");

    let cross = s.ask(&format!(
        r#"{{"jsonrpc":"2.0","id":41,"method":"tools/call","params":{{"name":"vivac_why","arguments":{{"id":"1","project":"{foreign_name}"}}}}}}"#
    ));
    assert_eq!(cross["result"]["isError"], false, "{cross}");
    assert!(text_of(&cross).contains("Foreign root"), "{cross}");

    let after = s.ask(
        r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"vivac_add","arguments":{"title":"After the foreign read","why":"b"}}}"#,
    );
    assert_eq!(after["result"]["isError"], false, "{after}");

    let mcp_open: Value = serde_json::from_str(&text_of(&s.ask(
        r#"{"jsonrpc":"2.0","id":43,"method":"tools/call","params":{"name":"vivac_open","arguments":{}}}"#,
    )))
    .unwrap();
    let cli_open: Value = serde_json::from_str(&c.ok(&["open", "--json"])).unwrap();
    assert_eq!(
        mcp_open, cli_open,
        "a foreign read disturbed the resident tree's fold"
    );

    let titles: Vec<&str> = mcp_open
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap())
        .collect();
    assert!(
        titles.contains(&"Before the foreign read") && titles.contains(&"After the foreign read"),
        "one of the two resident writes went missing around the foreign read: {titles:?}"
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

/// `abandon` and `restore` never reach `tools/list`. That is a security
/// veto, not a gap left for later: `abandon` discards a node and every
/// descendant it has, and doing that from a tool call would happen with
/// nobody watching a terminal. `restore` rewrites the stack and sits on the
/// same side of that line. If this test goes red because one of the two
/// got added to `TOOLS`, that is the veto being crossed, not closed.
#[test]
fn abandon_and_restore_are_never_in_the_tool_list() {
    let c = seeded("veto");
    let mut s = hello(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":18,"method":"tools/list"}"#);
    let names: Vec<&str> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for word in ["vivac_abandon", "vivac_restore"] {
        assert!(!names.contains(&word), "{word} reached the tool list");
    }
}

/// `d436`: a pillar's title carries what it restricts, and vivac keeps no
/// menu of powers, so neither tool that writes a pillar offers one.
#[test]
fn the_add_and_push_schemas_offer_no_power() {
    let c = seeded("no-power-schema");
    let mut s = hello(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":23,"method":"tools/list"}"#);
    let tools = r["result"]["tools"].as_array().unwrap().clone();
    for name in ["vivac_add", "vivac_push"] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} is in the tool list"));
        assert!(
            tool["inputSchema"]["properties"].get("power").is_none(),
            "{name} offers power: {tool}"
        );
    }
}

/// Every tool, called with every argument its own schema declares, one
/// value per declared type. An agent sends only what a schema lists, so a
/// call that reads anything else depends on an argument nobody can see
/// (`f452`); a debug build stops on that read, and the server dies here
/// instead of answering.
#[test]
fn every_tool_answers_a_call_built_from_only_its_own_schema() {
    let c = seeded("schema-reads");
    let mut s = hello(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":30,"method":"tools/list"}"#);
    let tools = r["result"]["tools"].as_array().unwrap().clone();
    for (i, t) in tools.iter().enumerate() {
        let name = t["name"].as_str().unwrap();
        let mut arguments = serde_json::Map::new();
        for (arg_name, property) in t["inputSchema"]["properties"].as_object().unwrap() {
            let value = match property["type"].as_str().unwrap() {
                "boolean" => json!(true),
                "array" => json!(["a value"]),
                _ => json!("a value"),
            };
            arguments.insert(arg_name.clone(), value);
        }
        let id = 200 + i as i64;
        let call = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        })
        .to_string();
        s.notify(&call);
        let mut buf = String::new();
        s.output.read_line(&mut buf).unwrap();
        assert!(
            !buf.is_empty(),
            "{name} crashed the server instead of answering a call built from its own schema"
        );
        let reply: Value = serde_json::from_str(&buf)
            .unwrap_or_else(|e| panic!("{name} did not reply JSON-RPC: {e}\n{buf}"));
        assert_eq!(reply["id"], id, "{name} answered a different call: {reply}");
    }
}

/// `arm` and `arm_dir` are the pair an agent following `vivac_add`'s own
/// schema has to be able to send (`f452`).
#[test]
fn vivac_add_arms_a_rule_using_only_its_own_declared_schema() {
    let c = Sandbox::new_seeded("add-arm-schema");
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    let mut s = hello(&c);
    let call = json!({
        "jsonrpc": "2.0",
        "id": 31,
        "method": "tools/call",
        "params": {
            "name": "vivac_add",
            "arguments": {
                "title": "Never store a secret",
                "why": "guard",
                "type": "rule",
                "arm": ["cargo test"],
                "arm_dir": "."
            }
        }
    })
    .to_string();
    let r = s.ask(&call);
    assert_eq!(r["result"]["isError"], false, "{r}");
    let rules = s.ask(
        r#"{"jsonrpc":"2.0","id":32,"method":"tools/call","params":{"name":"vivac_rules","arguments":{}}}"#,
    );
    let data: Value = serde_json::from_str(&text_of(&rules)).unwrap();
    assert_eq!(data["rules"][0]["arms"][0]["dir"], ".", "{data}");
    assert_eq!(
        data["rules"][0]["arms"][0]["command"], "cargo test",
        "{data}"
    );
}

/// The redaction guard lives in the ops, not in either caller, so a write
/// through the MCP door refuses a home path exactly the way `vivac push`
/// already does (`tests/brief.rs`'s `the_guard_covers_the_new_operations`).
/// This is here to prove the door does not skip it.
#[test]
fn a_write_by_mcp_with_a_home_path_is_rejected_like_the_cli() {
    let c = seeded("guard");
    let mut s = hello(&c);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":19,"method":"tools/call","params":{"name":"vivac_push","arguments":{"title":"Rotate","why":"see /home/someone/.config"}}}"#,
    );
    assert!(
        r["error"].is_null(),
        "it answered at the protocol level: {r}"
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(
        text_of(&r).contains("personal data"),
        "the guard did not name itself: {}",
        text_of(&r)
    );
}

/// A second sandbox with the same `.vivac/` a first one already has: same
/// actor (`init` draws one at random, so a second `init` would not agree),
/// and -- when the caller seeds a node into `c` before calling this -- the
/// same id for it, because `id::ulid()` is not reproducible either. Without
/// this, comparing what the CLI wrote against what MCP wrote would only ever
/// prove that two separate trees are two separate trees.
fn twin_of(c: &Sandbox, name: &str) -> Sandbox {
    let twin = Sandbox::new_empty(name);
    let vivac_dir = twin.0.join(".vivac");
    std::fs::create_dir_all(&vivac_dir).unwrap();
    for entry in std::fs::read_dir(c.0.join(".vivac")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), vivac_dir.join(entry.file_name())).unwrap();
    }
    twin
}

/// Every value a `node.created` or `vivac.created` event in this stream
/// introduced, with the placeholder each one collapses to -- `<node:0>` for
/// the first node this stream ever created, `<node:1>` for the second, and
/// the same scheme for `<vivac:_>`. `id::ulid()` draws on the machine's own
/// randomness, so the node or vivac an operation creates never gets the same
/// id twice, and something has to stand in for it. A single flat `<node>`
/// for every node would make two *different* nodes indistinguishable once
/// collapsed, so a `parent` naming the wrong one would compare equal to a
/// `parent` naming the right one -- numbering by order of first appearance
/// is what keeps them apart. A node copied in from a shared setup step does
/// show up, in the same position in both logs, so it takes the same number on
/// both sides and still compares equal -- while staying distinguishable from
/// every other node, which is the whole point.
fn fresh_ids(events: &[Value]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut nodes = 0u32;
    let mut vivacs = 0u32;
    for e in events {
        let payload = &e["payload"];
        match payload["type"].as_str() {
            Some("node.created") => {
                if let Some(node) = payload["node"].as_str() {
                    out.push((node.to_string(), format!("<node:{nodes}>")));
                    nodes += 1;
                }
            }
            Some("vivac.created") => {
                if let Some(vivac) = payload["vivac"].as_str() {
                    out.push((vivac.to_string(), format!("<vivac:{vivacs}>")));
                    vivacs += 1;
                }
            }
            _ => {}
        }
    }
    out
}

/// Walks a `Value` end to end, replacing every string equal to `from` with
/// `to`.
fn replace_value(v: &mut Value, from: &str, to: &str) {
    match v {
        Value::String(s) if s == from => *s = to.to_string(),
        Value::Array(items) => items.iter_mut().for_each(|e| replace_value(e, from, to)),
        Value::Object(fields) => fields.values_mut().for_each(|e| replace_value(e, from, to)),
        _ => {}
    }
}

/// A sandbox's events, ready to compare across the CLI path and the MCP
/// path: `id` and `ts` dropped, and every id `fresh_ids` found collapsed to
/// its placeholder.
fn tree_events(c: &Sandbox) -> Vec<Value> {
    let mut events: Vec<Value> = c
        .log()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for e in events.iter_mut() {
        let fields = e.as_object_mut().unwrap();
        fields.remove("id");
        fields.remove("ts");
    }
    for (from, to) in fresh_ids(&events) {
        for e in events.iter_mut() {
            replace_value(e, &from, &to);
        }
    }
    events
}

/// What makes the seven parity tests above worth trusting: `tree_events`
/// has to be able to tell two trees apart when they differ only in which
/// node a `parent` points at, or a `push` (or `add`, or `decide`) that
/// latched onto the wrong node would compare equal to one that latched onto
/// the right one, and none of the seven would ever notice. This is the one
/// test in the file that proves nothing about the product on its own; it
/// proves that the other seven are not proving nothing.
#[test]
fn a_wrong_parent_is_not_the_same_event_as_the_right_one() {
    // `twin_of` first, so the only thing left free to differ between the
    // two is the one field this test is about: same actor, same First and
    // Second branch. An actor drawn twice, independently, would make every
    // event differ for a reason that has nothing to do with `parent`.
    let right = Sandbox::new_seeded("parent-right");
    right.ok(&["push", "First branch", "--why", "first"]);
    right.ok(&["pop", "done"]);
    right.ok(&["push", "Second branch", "--why", "second"]);
    let wrong = twin_of(&right, "parent-wrong");

    right.ok(&["add", "A child", "--why", "hangs off the second branch"]);
    wrong.ok(&[
        "add",
        "A child",
        "--why",
        "hangs off the second branch",
        "--parent",
        "1",
    ]);

    assert_ne!(
        tree_events(&right),
        tree_events(&wrong),
        "tree_events cannot tell two different parents apart"
    );
}

/// The criterion `t118` is built to: the same operation, done by MCP and
/// done by the CLI on two identical trees, writes exactly the same events.
/// Seven tests, one per tool, attack the real risk -- that a second write
/// path quietly diverges from the first one.
#[test]
fn push_by_mcp_writes_the_same_events_as_push_by_the_cli() {
    let cli = Sandbox::new_seeded("push-cli");
    let via_mcp = twin_of(&cli, "push-mcp");
    cli.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "the version was a hand edit",
        "--type",
        "task",
        "--ref",
        "R1",
        "--governs",
        "G1",
        "--blocks",
    ]);

    let mut s = hello(&via_mcp);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"vivac_push","arguments":{"title":"Ship the release apparatus","why":"the version was a hand edit","type":"task","ref":["R1"],"governs":["G1"],"blocks":true}}}"#,
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(tree_events(&cli), tree_events(&via_mcp));
}

#[test]
fn pop_by_mcp_writes_the_same_events_as_pop_by_the_cli() {
    let cli = Sandbox::new_seeded("pop-cli");
    cli.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "the version was a hand edit",
    ]);
    let via_mcp = twin_of(&cli, "pop-mcp");

    cli.ok(&["pop", "the release went out", "--next", "watch the metrics"]);
    let mut s = hello(&via_mcp);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"vivac_pop","arguments":{"outcome":"the release went out","next":"watch the metrics"}}}"#,
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(tree_events(&cli), tree_events(&via_mcp));
}

#[test]
fn add_by_mcp_writes_the_same_events_as_add_by_the_cli() {
    let cli = Sandbox::new_seeded("add-cli");
    let via_mcp = twin_of(&cli, "add-mcp");
    cli.ok(&[
        "add",
        "Guard the commit messages",
        "--why",
        "a malformed one does not count",
        "--type",
        "finding",
        "--ref",
        "R1",
        "--governs",
        "G1",
        "--blocks",
    ]);

    let mut s = hello(&via_mcp);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"vivac_add","arguments":{"title":"Guard the commit messages","why":"a malformed one does not count","type":"finding","ref":["R1"],"governs":["G1"],"blocks":true}}}"#,
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(tree_events(&cli), tree_events(&via_mcp));
}

#[test]
fn decide_by_mcp_writes_the_same_events_as_decide_by_the_cli() {
    let cli = Sandbox::new_seeded("decide-cli");
    let via_mcp = twin_of(&cli, "decide-mcp");
    cli.ok(&[
        "decide",
        "Rotate release keys",
        "--reason",
        "the old one is in three places",
        "--alternative",
        "keep the old one",
        "--ref",
        "R1",
        "--governs",
        "G1",
        "--blocks",
    ]);

    let mut s = hello(&via_mcp);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":"vivac_decide","arguments":{"title":"Rotate release keys","reason":"the old one is in three places","alternative":["keep the old one"],"ref":["R1"],"governs":["G1"],"blocks":true}}}"#,
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(tree_events(&cli), tree_events(&via_mcp));
}

#[test]
fn note_by_mcp_writes_the_same_events_as_note_by_the_cli() {
    let cli = Sandbox::new_seeded("note-cli");
    cli.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "the version was a hand edit",
    ]);
    let via_mcp = twin_of(&cli, "note-mcp");

    cli.ok(&["note", "the rollback plan is untested"]);
    let mut s = hello(&via_mcp);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{"name":"vivac_note","arguments":{"note":"the rollback plan is untested"}}}"#,
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(tree_events(&cli), tree_events(&via_mcp));
}

#[test]
fn park_by_mcp_writes_the_same_events_as_park_by_the_cli() {
    let cli = Sandbox::new_seeded("park-cli");
    cli.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "the version was a hand edit",
    ]);
    let via_mcp = twin_of(&cli, "park-mcp");

    cli.ok(&["park", "waiting on the security review"]);
    let mut s = hello(&via_mcp);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"vivac_park","arguments":{"reason":"waiting on the security review"}}}"#,
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(tree_events(&cli), tree_events(&via_mcp));
}

#[test]
fn save_by_mcp_writes_the_same_events_as_save_by_the_cli() {
    let cli = Sandbox::new_seeded("save-cli");
    let via_mcp = twin_of(&cli, "save-mcp");
    cli.ok(&[
        "save",
        "before the migration",
        "--next",
        "run the reconcile",
    ]);

    let mut s = hello(&via_mcp);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":26,"method":"tools/call","params":{"name":"vivac_save","arguments":{"label":"before the migration","next":"run the reconcile"}}}"#,
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(tree_events(&cli), tree_events(&via_mcp));
}
