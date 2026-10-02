//! `d994`: `--supersedes` on `add` and `push` as well as `decide`, one check
//! shared by the three, and `done`'s refusal on a standing decision saying how
//! to correct a node filed with the wrong type.

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

/// The alias a write printed: the first word of its first line.
fn alias_of(out: &str) -> String {
    out.split_whitespace().next().unwrap().to_string()
}

/// A tree with a goal open and nothing else.
fn seeded(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Goal A", "--why", "the work"]);
    c
}

fn decision(c: &Sandbox, title: &str) -> String {
    alias_of(&c.ok(&["decide", title, "--reason", "because", "--root"]))
}

fn node(c: &Sandbox, alias: &str) -> Value {
    let v: Value = serde_json::from_str(&c.ok(&["why", alias, "--json"])).unwrap();
    v["node"].clone()
}

/// A refusal: the exact message, exit code 1, and a log that did not move.
fn refused(c: &Sandbox, args: &[&str], message: &str) {
    let before = c.log();
    let (out, code) = c.run(args);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(message), "wanted:\n{message}\ngot:\n{out}");
    assert_eq!(before, c.log(), "a refused call wrote to the log");
}

#[test]
fn add_supersedes_a_standing_decision_with_a_constraint() {
    let c = seeded("sup-add-constraint");
    let d = decision(&c, "Everything goes through the queue");
    let out = c.ok(&[
        "add",
        "Everything goes through the queue, always",
        "--type",
        "constraint",
        "--root",
        "--why",
        "it was a norm all along",
        "--supersedes",
        &d,
    ]);
    let new = alias_of(&out);
    assert!(new.starts_with('c'), "{out}");
    assert!(
        out.contains(&format!("        {d} becomes superseded")),
        "{out}"
    );

    let old = node(&c, &d);
    assert_eq!(old["state"], "superseded", "{old}");
    assert!(
        c.log().contains(&format!("superseded by {new}")),
        "{}",
        c.log()
    );
    assert_eq!(node(&c, &new)["state"], "active");

    let brief = c.ok(&["brief"]);
    let (invariants, standing) = brief
        .split_once("STANDING DECISIONS")
        .unwrap_or((&brief, ""));
    assert!(invariants.contains("INVARIANTS"), "{brief}");
    assert!(
        invariants.contains(&format!("{new} ")),
        "the constraint is not listed under INVARIANTS:\n{brief}"
    );
    assert!(
        !standing.contains(&format!("{d} ")),
        "the decision is still listed:\n{brief}"
    );
}

#[test]
fn push_supersedes_a_rule_with_a_rule() {
    let c = seeded("sup-push-rule");
    let old = alias_of(&c.ok(&["add", "Old rule", "--type", "rule", "--root", "--why", "w"]));
    let out = c.ok(&[
        "push",
        "New rule",
        "--type",
        "rule",
        "--root",
        "--why",
        "it replaces the old one",
        "--supersedes",
        &old,
    ]);
    let new = alias_of(&out);
    assert!(new.starts_with('r'), "{out}");
    assert!(
        out.contains(&format!("        {old} becomes superseded")),
        "{out}"
    );
    assert_eq!(node(&c, &old)["state"], "superseded");
    assert!(c.log().contains(&format!("superseded by {new}")));
}

#[test]
fn a_pillar_is_replaced_by_a_rule_and_a_rule_by_a_decision() {
    let c = seeded("sup-pillar-rule-decision");
    let pillar = alias_of(&c.ok(&[
        "add",
        "Old pillar",
        "--type",
        "pillar",
        "--root",
        "--why",
        "w",
    ]));
    let rule = alias_of(&c.ok(&[
        "add",
        "Rule in its place",
        "--type",
        "rule",
        "--root",
        "--why",
        "w",
        "--supersedes",
        &pillar,
    ]));
    assert_eq!(node(&c, &pillar)["state"], "superseded");
    assert!(c.log().contains(&format!("superseded by {rule}")));

    let out = c.ok(&[
        "decide",
        "A decision after all",
        "--reason",
        "because",
        "--root",
        "--supersedes",
        &rule,
    ]);
    let decision = alias_of(&out);
    assert_eq!(node(&c, &rule)["state"], "superseded");
    assert!(c.log().contains(&format!("superseded by {decision}")));
}

#[test]
fn superseding_a_task_or_a_finding_is_refused() {
    let c = seeded("sup-not-governing");
    let task = alias_of(&c.ok(&["add", "A task", "--parent", "g1"]));
    let finding = alias_of(&c.ok(&["add", "A finding", "--type", "finding", "--parent", "g1"]));
    for old in [&task, &finding] {
        let message = format!(
            "  {old} cannot be replaced: only a decision, a constraint, a rule or a pillar is. Close it with vivac done instead."
        );
        refused(
            &c,
            &[
                "add",
                "N",
                "--type",
                "rule",
                "--root",
                "--why",
                "w",
                "--supersedes",
                old,
            ],
            &message,
        );
        refused(
            &c,
            &[
                "push",
                "N",
                "--type",
                "rule",
                "--root",
                "--why",
                "w",
                "--supersedes",
                old,
            ],
            &message,
        );
        refused(
            &c,
            &[
                "decide",
                "N",
                "--reason",
                "r",
                "--root",
                "--supersedes",
                old,
            ],
            &message,
        );
    }
}

#[test]
fn superseding_something_no_longer_in_force_is_refused() {
    let c = seeded("sup-not-open");
    let rule = alias_of(&c.ok(&["add", "Old rule", "--type", "rule", "--root", "--why", "w"]));
    c.ok(&[
        "add",
        "Its replacement",
        "--type",
        "rule",
        "--root",
        "--why",
        "w",
        "--supersedes",
        &rule,
    ]);
    refused(
        &c,
        &[
            "add",
            "Again",
            "--type",
            "rule",
            "--root",
            "--why",
            "w",
            "--supersedes",
            &rule,
        ],
        &format!("  {rule} is superseded already: only one still in force is replaced."),
    );

    let closed = decision(&c, "Soon abandoned");
    c.ok(&["abandon", &closed, "never mind"]);
    refused(
        &c,
        &[
            "decide",
            "Late",
            "--reason",
            "r",
            "--root",
            "--supersedes",
            &closed,
        ],
        &format!("  {closed} is abandoned already: only one still in force is replaced."),
    );
    refused(
        &c,
        &[
            "push",
            "Late",
            "--type",
            "decision",
            "--root",
            "--why",
            "w",
            "--supersedes",
            &closed,
        ],
        &format!("  {closed} is abandoned already: only one still in force is replaced."),
    );
}

#[test]
fn the_new_node_has_to_be_a_governing_kind_too() {
    let c = seeded("sup-new-kind");
    let d = decision(&c, "A standing decision");
    // No `--type` and a parent: a task.
    refused(
        &c,
        &["add", "A plain add", "--parent", "g1", "--supersedes", &d],
        "  --supersedes is only for a decision, a constraint, a rule or a pillar, and this one's type is task: give it one of those with --type.",
    );
    refused(
        &c,
        &["push", "A finding", "--type", "finding", "--why", "w", "--supersedes", &d],
        "  --supersedes is only for a decision, a constraint, a rule or a pillar, and this one's type is finding: give it one of those with --type.",
    );
    // No parent and no type: a goal.
    refused(
        &c,
        &["add", "A goal", "--root", "--supersedes", &d],
        "  --supersedes is only for a decision, a constraint, a rule or a pillar, and this one's type is goal: give it one of those with --type.",
    );
}

#[test]
fn the_old_node_is_checked_before_the_new_kind() {
    let c = seeded("sup-order");
    let task = alias_of(&c.ok(&["add", "A task", "--parent", "g1"]));
    refused(
        &c,
        &["add", "A plain add", "--parent", "g1", "--supersedes", &task],
        &format!(
            "  {task} cannot be replaced: only a decision, a constraint, a rule or a pillar is. Close it with vivac done instead."
        ),
    );
}

#[test]
fn nothing_is_reparented() {
    let c = seeded("sup-parent");
    let d = decision(&c, "Standing, at the root");
    let out = c.ok(&[
        "add",
        "Replacement",
        "--type",
        "constraint",
        "--parent",
        "g1",
        "--why",
        "w",
        "--supersedes",
        &d,
    ]);
    assert!(out.contains("under g1"), "{out}");
    assert_eq!(node(&c, &d)["parent"], Value::Null, "{}", node(&c, &d));
}

#[test]
fn the_mcp_answer_carries_the_superseded_node_or_null() {
    let c = seeded("sup-json");
    let d = decision(&c, "Standing");
    let mut s = Server::start(&c);
    let r = s.call(
        "vivac_add",
        json!({"title": "Replacement", "type": "rule", "root": true, "why": "w", "supersedes": d}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    let with: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert_eq!(with["superseded"], json!({ "alias": d }), "{with}");
    let r = s.call(
        "vivac_push",
        json!({"title": "Plain", "root": true, "why": "w"}),
    );
    let without: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert!(without["superseded"].is_null(), "{without}");
    assert!(without.get("superseded").is_some(), "{without}");
}

#[test]
fn done_on_a_standing_decision_says_what_to_do_if_it_was_never_one() {
    let c = seeded("sup-done");
    let d = decision(&c, "Standing");
    let (out, code) = c.run(&["done", &d]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(&format!(
            "  It stops standing when another decision replaces it:\n    \
             vivac decide \"<what replaces it>\" --reason \"<why>\" --supersedes {d}\n  \
             If it was never a decision, write what it should have been in its place:\n    \
             vivac add \"<title>\" --type <constraint, rule or pillar> --supersedes {d}"
        )),
        "{out}"
    );
}

/// Events with `id` and `ts` dropped and the node the call created collapsed
/// to a placeholder: the only things that differ between two runs.
fn events(c: &Sandbox) -> Vec<Value> {
    fn swap(v: &mut Value, from: &str) {
        match v {
            Value::String(s) if s == from => *s = "<new>".to_string(),
            Value::Array(a) => a.iter_mut().for_each(|e| swap(e, from)),
            Value::Object(o) => o.values_mut().for_each(|e| swap(e, from)),
            _ => {}
        }
    }
    let mut all: Vec<Value> = c
        .log()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let last_created = all
        .iter()
        .rev()
        .find(|e| e["payload"]["type"] == "node.created")
        .and_then(|e| e["payload"]["node"].as_str().map(str::to_string))
        .unwrap();
    for e in all.iter_mut() {
        let f = e.as_object_mut().unwrap();
        f.remove("id");
        f.remove("ts");
        swap(e, &last_created);
    }
    all
}

#[test]
fn mcp_add_with_supersedes_writes_what_the_cli_writes() {
    let cli = seeded("sup-mcp-cli");
    let d = decision(&cli, "Standing");
    let twin = Sandbox::new_empty("sup-mcp-twin");
    let dir = twin.0.join(".vivac");
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(cli.0.join(".vivac")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }

    cli.ok(&[
        "add",
        "Replacement",
        "--type",
        "constraint",
        "--root",
        "--why",
        "w",
        "--supersedes",
        &d,
    ]);
    let mut s = Server::start(&twin);
    let r = s.call(
        "vivac_add",
        json!({"title": "Replacement", "type": "constraint", "root": true, "why": "w", "supersedes": d}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(events(&cli), events(&twin));
}

#[test]
fn mcp_refusals_use_the_argument_names_and_write_nothing() {
    let c = seeded("sup-mcp-refuse");
    let d = decision(&c, "Standing");
    let before = c.log();
    let mut s = Server::start(&c);
    let r = s.call(
        "vivac_add",
        json!({"title": "Plain", "parent": "1", "why": "w", "supersedes": d}),
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(
        text_of(&r).contains(
            "supersedes is only for a decision, a constraint, a rule or a pillar, and this one's type is task: give it one of those in type."
        ),
        "{r}"
    );
    let r = s.call(
        "vivac_push",
        json!({"title": "Plain", "type": "finding", "why": "w", "supersedes": d}),
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(
        text_of(&r).contains("this one's type is finding: give it one of those in type."),
        "{r}"
    );
    assert_eq!(before, c.log());
}

#[test]
fn the_add_and_push_tools_declare_supersedes() {
    let c = seeded("sup-mcp-schema");
    let mut s = Server::start(&c);
    let r = s.ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = r["result"]["tools"].as_array().unwrap().clone();
    let described = "An earlier decision, constraint, rule or pillar this one replaces, while it is still in force: it becomes superseded and keeps its history. For a node filed with the wrong type, write the one it should have been and name the old one here. This node has to be one of those four types too.";
    for name in ["vivac_add", "vivac_push"] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        let p = &tool["inputSchema"]["properties"]["supersedes"];
        assert_eq!(p["type"], "string", "{name}: {tool}");
        assert_eq!(p["description"], described, "{name}");
    }
    let decide = tools.iter().find(|t| t["name"] == "vivac_decide").unwrap();
    assert_eq!(
        decide["inputSchema"]["properties"]["supersedes"]["description"],
        "An earlier decision, constraint, rule or pillar this one retires, while it is still in force."
    );
}
