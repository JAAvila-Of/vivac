//! `d945`: what reaches the agent says what never goes in a note, and `note`
//! itself says so when a node keeps collecting them with nothing filed under
//! it (`f944`). A title that is the id of a node is refused (`f943`): the id
//! is where the node goes, never what the new one is called.

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
            .env("TZ", "UTC")
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut s = Server {
            child,
            input,
            output,
        };
        s.ask(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#);
        writeln!(
            s.input,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )
        .unwrap();
        s.input.flush().unwrap();
        s
    }

    fn ask(&mut self, line: &str) -> Value {
        writeln!(self.input, "{line}").unwrap();
        self.input.flush().unwrap();
        let mut buf = String::new();
        self.output.read_line(&mut buf).unwrap();
        assert!(!buf.is_empty(), "the server closed without answering");
        serde_json::from_str(&buf).unwrap_or_else(|e| panic!("this is not JSON-RPC: {e}\n{buf}"))
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let line = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        });
        self.ask(&line.to_string())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text_of(reply: &Value) -> String {
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {reply}"))
        .to_string()
}

/// What `outcome::to_text` rendered for a write that went through the server:
/// the reply is the outcome as JSON, with that text in its `text` field.
fn written_text(reply: &Value) -> String {
    let v: Value = serde_json::from_str(&text_of(reply)).unwrap();
    v["text"].as_str().unwrap().to_string()
}

fn block(n: u64, alias: &str) -> String {
    format!(
        "  {n} notes in a row on {alias} with nothing filed under it.\n  \
         A finding, a choice or something left to do is a node, not a note:\n    \
         vivac add \"<title>\" --type finding|task --parent {alias}\n    \
         vivac decide \"<title>\" --reason \"<why>\" --parent {alias}"
    )
}

/// The same reminder as [`block`], as it reads over MCP (`d964`): the moves
/// are tools there, not commands.
fn tools_block(n: u64, alias: &str) -> String {
    format!(
        "  {n} notes in a row on {alias} with nothing filed under it.\n  \
         A finding, a choice or something left to do is a node, not a note:\n    \
         vivac_add  title, type: finding or task, parent: {alias}\n    \
         vivac_decide  title, reason, parent: {alias}"
    )
}

fn note(c: &Sandbox, alias: &str) -> String {
    c.ok(&["note", alias, "how it went"])
}

fn notes(c: &Sandbox, alias: &str, times: u64) -> Vec<String> {
    (0..times).map(|_| note(c, alias)).collect()
}

fn ulid_of(c: &Sandbox, id: &str) -> String {
    let s = c.ok(&["why", id, "--json"]);
    let v: Value = serde_json::from_str(&s).unwrap();
    v["node"]["id"].as_str().unwrap().to_string()
}

fn log_len(c: &Sandbox) -> usize {
    c.log().lines().count()
}

fn seeded(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Goal A", "--why", "the work"]);
    c
}

#[test]
fn the_note_that_makes_eight_says_so_and_so_does_the_note_that_makes_twice_eight() {
    let c = seeded("nn-eighth");
    let outputs = notes(&c, "g1", 16);
    for (i, out) in outputs.iter().enumerate() {
        let n = i as u64 + 1;
        assert!(out.contains("  g1 noted"), "{out}");
        match n {
            8 => assert!(
                out.contains(&format!("  g1 noted\n{}", block(8, "g1"))),
                "{out}"
            ),
            16 => assert!(
                out.contains(&format!("  g1 noted\n{}", block(16, "g1"))),
                "{out}"
            ),
            _ => assert!(!out.contains("in a row"), "note {n}: {out}"),
        }
    }
}

#[test]
fn the_count_is_reset_by_a_child_under_the_node() {
    let c = seeded("nn-reset");
    for out in notes(&c, "g1", 7) {
        assert!(!out.contains("in a row"), "{out}");
    }
    c.ok(&["add", "A finding", "--type", "finding", "--parent", "g1"]);
    for out in notes(&c, "g1", 7) {
        assert!(!out.contains("in a row"), "{out}");
    }
    let out = note(&c, "g1");
    assert!(out.contains(&block(8, "g1")), "{out}");
}

#[test]
fn the_count_is_reset_by_nothing_else() {
    let c = seeded("nn-not-reset");
    c.ok(&["add", "Sibling", "--why", "elsewhere", "--root"]);
    c.ok(&["add", "Another", "--why", "elsewhere too", "--root"]);
    for out in notes(&c, "g1", 5) {
        assert!(!out.contains("in a row"), "{out}");
    }
    // A child under a different node.
    c.ok(&["add", "Under the sibling", "--parent", "g2"]);
    // Done and park on another node, and a push that hangs from another one.
    c.ok(&["push", "Detour", "--why", "elsewhere", "--parent", "g2"]);
    c.ok(&["done", "g3", "Record: nothing to do"]);
    c.ok(&["park", "g2", "not now"]);
    for out in notes(&c, "g1", 2) {
        assert!(!out.contains("in a row"), "{out}");
    }
    let out = note(&c, "g1");
    assert!(out.contains(&block(8, "g1")), "{out}");
}

#[test]
fn each_node_keeps_its_own_count() {
    let c = seeded("nn-interleaved");
    c.ok(&["add", "Second", "--why", "another node", "--root"]);
    for i in 1..=8u64 {
        let a = note(&c, "g1");
        let b = note(&c, "g2");
        let (want_a, want_b) = (i == 8, i == 8);
        assert_eq!(a.contains(&block(8, "g1")), want_a, "{a}");
        assert_eq!(b.contains(&block(8, "g2")), want_b, "{b}");
    }
    // A third note on g2 only moves g2.
    let a = note(&c, "g1");
    assert!(!a.contains("in a row"), "{a}");
}

#[test]
fn the_hint_never_changes_the_exit_code() {
    let c = seeded("nn-exit");
    notes(&c, "g1", 7);
    let (out, code) = c.run(&["note", "g1", "the eighth"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&block(8, "g1")), "{out}");
}

#[test]
fn nothing_but_note_prints_the_hint() {
    let c = seeded("nn-only-note");
    notes(&c, "g1", 7);
    let out = c.ok(&["add", "A task", "--why", "x", "--parent", "g1"]);
    assert!(!out.contains("in a row"), "{out}");
    notes(&c, "g1", 7);
    let out = c.ok(&["done", "t2", "Record: done"]);
    assert!(!out.contains("in a row"), "{out}");
}

#[test]
fn the_note_that_makes_eight_over_mcp_carries_the_block() {
    let c = seeded("nn-mcp");
    notes(&c, "g1", 7);
    let mut s = Server::start(&c);
    let r = s.call("vivac_note", json!({"id": "g1", "note": "how it went"}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let t = written_text(&r);
    assert!(
        t.contains(&format!("  g1 noted\n{}", tools_block(8, "g1"))),
        "{t}"
    );
}

fn refusal(title: &str, command: &str) -> String {
    format!(
        "  \"{title}\" is the id of a node, not a title, and a title cannot be changed later.\n  \
         To hang the new node from it:  {command}"
    )
}

#[test]
fn a_title_that_is_the_id_of_a_node_is_refused() {
    let c = seeded("nn-id-title");
    c.ok(&["add", "A task", "--why", "x", "--parent", "g1"]);
    let ulid = ulid_of(&c, "t2");
    let before_all = log_len(&c);
    for (title, alias) in [("t2", "t2"), ("  t2 ", "t2"), (ulid.as_str(), "t2")] {
        let shown = title.trim();
        let before = c.log();
        let (out, code) = c.run(&["add", title, "--why", "x"]);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains(&refusal(
                shown,
                &format!("vivac add \"<title>\" --parent {alias}")
            )),
            "{out}"
        );
        let (out, code) = c.run(&["push", title, "--why", "x"]);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains(&refusal(
                shown,
                &format!("vivac push \"<title>\" --why \"<why>\" --parent {alias}")
            )),
            "{out}"
        );
        let (out, code) = c.run(&["decide", title, "--reason", "because"]);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains(&refusal(
                shown,
                &format!("vivac decide \"<title>\" --reason \"<why>\" --parent {alias}")
            )),
            "{out}"
        );
        assert_eq!(before, c.log(), "a refused title wrote to the log");
    }
    assert_eq!(log_len(&c), before_all);
}

#[test]
fn a_title_that_only_looks_like_an_id_is_accepted() {
    let c = seeded("nn-id-lookalike");
    for title in ["t99999", "v2", "t1 follow-up", "2026"] {
        let (out, code) = c.run(&["add", title, "--why", "x"]);
        assert_eq!(code, 0, "{title}: {out}");
    }
}

#[test]
fn a_title_that_is_the_id_of_a_node_is_refused_over_mcp() {
    let c = seeded("nn-id-title-mcp");
    let before = c.log();
    let mut s = Server::start(&c);
    let r = s.call("vivac_add", json!({"title": "g1", "why": "x"}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(
        text_of(&r).contains("\"g1\" is the id of a node, not a title"),
        "{r}"
    );
    assert_eq!(before, c.log(), "a refused title wrote to the log");
}

#[test]
fn the_seams_block_carries_the_new_head_and_hints() {
    let c = Sandbox::new_seeded("nn-seams");
    let (plain, _) = c.run_stdin(&["session", "start", "--hook"], "{}");
    let head = "  Look first: vivac_find \"<words>\". Work the tree already holds is never\n  opened twice: what you find, settle or leave to do while on it is a node\n  under it, not a note.";
    assert!(plain.contains(head), "{plain}");
    assert!(!plain.contains("focus above"), "{plain}");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let (focused, _) = c.run_stdin(&["session", "start", "--hook"], "{}");
    assert!(
        focused.contains(
            "  Look first: vivac_find \"<words>\". Work the tree already holds is never
  opened twice: what you find, settle or leave to do while on it is a node
  under it, not a note. The focus above is where work was left, maybe not
  by you: hang new work from what it continues.
"
        ),
        "{focused}"
    );
    assert!(
        focused.contains(
            "  a choice is settled  vivac_decide  title, reason, alternative\n                       yours or the person's\n"
        ),
        "{focused}"
    );
    assert!(
        focused.contains(
            "                       CI, a tracker, the cloud: the tree is its only record\n                       never a finding, a choice or something left to do\n"
        ),
        "{focused}"
    );
    assert!(
        plain.contains("                       yours or the person's\n"),
        "{plain}"
    );
}

#[test]
fn the_tool_texts_carry_the_new_sentences() {
    let c = Sandbox::new_seeded("nn-descriptions");
    let mut s = Server::start(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = r["result"]["tools"].as_array().unwrap();
    let description = |name: &str| -> String {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))["description"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let note = description("vivac_note");
    for want in [
        "how the work on it went, or something that changed outside git -- CI, a tracker, the cloud -- that the tree is the only record of.",
        "A note never carries a finding, a choice (yours or the person's) or something left to do: each of those is a node under this one, filed with `vivac_add` or `vivac_decide`, so it shows up as what it is.",
        "When a node keeps collecting notes with nothing filed under it, the answer says so.",
    ] {
        assert!(note.contains(want), "{note}");
    }
    let add = description("vivac_add");
    for want in [
        "this is for what was just noticed. Something left to do that turns up while working on a node is a task filed under it, never a note on it.",
        "its outcome starting with Record:. The title is words, never an id: the node it hangs from goes in `parent`.",
    ] {
        assert!(add.contains(want), "{add}");
    }
    let decide = description("vivac_decide");
    assert!(
        decide.contains(
            "Record a decision, with the reason it was made and every alternative that lost. The choice can be yours or the person's; either way it is a decision, never a note."
        ),
        "{decide}"
    );
}

/// `d993`, `f990`: told "let's make it a rule: all code in English", an agent
/// filed it as a decision under the task it was on, because `vivac_decide`
/// said a limit the person sets is one; asked why it was not a rule, it moved
/// it to a rule, which reaches no session unless somebody asks for the rules.
/// A norm is a constraint, and the seams, `vivac_decide` and the `type` of
/// `vivac_add` and `vivac_push` all say so.
#[test]
fn a_norm_the_person_sets_is_taught_as_a_constraint() {
    let c = Sandbox::new_seeded("nn-norm-seam");
    c.ok(&["push", "A goal", "--why", "to have a focus"]);
    let (plain, _) = c.run_stdin(&["session", "start", "--hook"], "{}");
    assert!(
        plain.contains(
            "  a norm is set        vivac_add  title, type: constraint, why: their words\n                       root, if it holds for all work: every brief shows it\n                       a constraint even when they call it a rule\n"
        ),
        "{plain}"
    );
    assert_eq!(
        plain.matches("mcp__vivac__vivac_add,").count(),
        1,
        "a second row for vivac_add put it in the load list twice:\n{plain}"
    );
    let mut s = Server::start(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = r["result"]["tools"].as_array().unwrap();
    let tool = |name: &str| -> Value {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .clone()
    };
    let decide = tool("vivac_decide")["description"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!decide.contains("a limit they set"), "{decide}");
    assert!(
        decide.contains(
            "A norm the person sets for the work from now on is not a choice: file it with vivac_add as a constraint, even when they call it a rule."
        ),
        "{decide}"
    );
    for name in ["vivac_add", "vivac_push"] {
        let text = tool(name)["inputSchema"]["properties"]["type"]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} has no type"))
            .to_string();
        assert!(
            text.contains("A norm the person sets for the work from now on is a constraint, with root when it holds for all work, so every brief shows it. A rule is a line a pillar draws, read with vivac_rules when work is checked."),
            "{name}: {text}"
        );
    }
}
