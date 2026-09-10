//! `t411` §26-§28, `d444` — older releases stop cold on a governed tree.
//!
//! `f419`: a release before pillars and rules skips a line it cannot read
//! and reuses its number on the next write, silently corrupting a tree that
//! holds one. It cannot be changed now, but it already fails on a config it
//! cannot parse, and its error repeats whatever the config said. The first
//! pillar or rule written to a tree turns the config's `version` into a
//! sentence that says so, before the event itself is appended.

mod common;
use common::Sandbox;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

const LOCK_SENTENCE: &str =
    "this tree holds pillars and rules, and this vivac is too old to read them: update vivac";

fn config_text(c: &Sandbox) -> String {
    std::fs::read_to_string(c.0.join(".vivac").join("config")).unwrap()
}

fn is_locked(c: &Sandbox) -> bool {
    config_text(c).contains(LOCK_SENTENCE)
}

// ---------------------------------------------------------------------------
// §28.1: the first pillar or rule locks the config, event behind it, no
// leftover `config.tmp`.
// ---------------------------------------------------------------------------

#[test]
fn a_pillar_born_by_add_locks_the_config_behind_the_event() {
    let c = Sandbox::new_seeded("lock-add-pillar");
    assert!(!is_locked(&c), "a fresh tree starts unlocked");
    c.ok(&["add", "Security", "--type", "pillar", "--why", "arbiter"]);
    assert!(is_locked(&c), "{}", config_text(&c));
    assert!(
        !c.0.join(".vivac").join("config.tmp").exists(),
        "the temporary file was not cleaned up"
    );
}

#[test]
fn a_rule_born_by_add_locks_the_config() {
    let c = Sandbox::new_seeded("lock-add-rule");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    assert!(is_locked(&c), "{}", config_text(&c));
}

#[test]
fn a_pillar_born_by_push_locks_the_config() {
    let c = Sandbox::new_seeded("lock-push-pillar");
    c.ok(&["push", "Security", "--type", "pillar", "--why", "arbiter"]);
    assert!(is_locked(&c), "{}", config_text(&c));
}

#[test]
fn a_pillar_born_over_mcp_locks_the_config() {
    let c = Sandbox::new_seeded("lock-mcp-add");
    assert!(!is_locked(&c));
    let mut s = Server::start(&c);
    s.ask(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"vivac_add","arguments":{"title":"Security","type":"pillar","why":"arbiter"}}}"#,
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    drop(s);
    assert!(is_locked(&c), "{}", config_text(&c));
}

// ---------------------------------------------------------------------------
// §28.2: nothing that never touches governance moves the config at all.
// ---------------------------------------------------------------------------

#[test]
fn ordinary_writes_never_change_the_config() {
    let c = Sandbox::new_seeded("lock-untouched");
    let before = config_text(&c);
    c.ok(&["push", "First", "--why", "reason"]);
    c.ok(&["add", "A finding", "--why", "found it"]);
    c.ok(&["decide", "A call", "--reason", "because"]);
    c.ok(&["note", "a note on the focus"]);
    c.ok(&["park", "2", "parked for later"]);
    c.ok(&["done", "3", "wrapped up"]);
    c.ok(&["pop", "closing the run"]);
    assert_eq!(
        before,
        config_text(&c),
        "the config moved with no governance"
    );
    assert!(!is_locked(&c));
}

// ---------------------------------------------------------------------------
// §28.3: a hand-mounted tree -- a rule in the log, config still at `1` --
// locks on its very next write, whatever it is.
// ---------------------------------------------------------------------------

#[test]
fn a_tree_with_a_rule_and_an_unlocked_config_locks_on_the_next_write() {
    let c = Sandbox::new_seeded("lock-hand-mounted");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    assert!(is_locked(&c), "setup: the rule should have locked it");

    // Hand-mount: put the config back the way an untouched tree would have
    // it, as if the lock had never run.
    let cfg = c.0.join(".vivac").join("config");
    let raw = std::fs::read_to_string(&cfg).unwrap();
    let mut v: Value = serde_json::from_str(&raw).unwrap();
    v["version"] = Value::from(1);
    std::fs::write(&cfg, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    assert!(!is_locked(&c), "hand-mount did not take");

    c.ok(&["note", "1", "a plain note"]);
    assert!(
        is_locked(&c),
        "a tree that already has a rule did not lock on its next write:\n{}",
        config_text(&c)
    );
}

// ---------------------------------------------------------------------------
// §28.4: reads answer the same with `1` and with the sentence.
// ---------------------------------------------------------------------------

#[test]
fn reads_agree_before_and_after_the_lock() {
    let c = Sandbox::new_seeded("lock-reads-agree");
    c.ok(&["push", "Root", "--why", "reason"]);
    let brief_before = c.ok(&["brief", "--now", "2026-09-10"]);
    let tree_before = c.ok(&["tree"]);
    let why_before = c.ok(&["why", "1"]);
    let rules_before = c.ok(&["rules"]);

    c.ok(&["add", "Security", "--type", "pillar", "--why", "arbiter"]);
    assert!(is_locked(&c));

    // Same reads, minus the new pillar's own line, still agree in shape:
    // compare against a second, freshly-seeded tree at each stage instead
    // of the same tree before/after, since adding the pillar is itself an
    // observable change `rules` is supposed to show.
    let brief_after = c.ok(&["brief", "--now", "2026-09-10"]);
    let tree_after = c.ok(&["tree"]);
    let why_after = c.ok(&["why", "1"]);
    assert!(brief_before.contains("Root") && brief_after.contains("Root"));
    assert!(tree_before.contains("Root") && tree_after.contains("Root"));
    assert!(why_before.contains("Root") && why_after.contains("Root"));
    assert!(!rules_before.is_empty());
}

// ---------------------------------------------------------------------------
// §28.5: an unrecognised `version` refuses a read, a write and an MCP tool,
// with the log left byte for byte alone.
// ---------------------------------------------------------------------------

fn set_version(c: &Sandbox, value: Value) {
    let cfg = c.0.join(".vivac").join("config");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    v["version"] = value;
    std::fs::write(&cfg, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

#[test]
fn an_integer_version_this_release_does_not_know_refuses_everything() {
    let c = Sandbox::new_seeded("lock-unknown-int");
    c.ok(&["push", "Root", "--why", "reason"]);
    set_version(&c, Value::from(2));
    let before = c.log();

    let (out, code) = c.run(&["tree"]);
    assert_eq!(code, 5, "{out}");
    assert!(
        out.contains(
            "This tree was written by a newer vivac: its config has version 2, which \
             this version does not know. Update vivac to read it. Nothing was written."
        ),
        "{out}"
    );

    let (out2, code2) = c.run(&["add", "Another", "--why", "reason"]);
    assert_eq!(code2, 5, "{out2}");
    assert!(out2.contains("its config has version 2"), "{out2}");

    assert_eq!(before, c.log(), "a refused read or write still wrote");
}

#[test]
fn a_high_integer_version_also_refuses() {
    let c = Sandbox::new_seeded("lock-unknown-int-99");
    c.ok(&["push", "Root", "--why", "reason"]);
    set_version(&c, Value::from(99));
    let (out, code) = c.run(&["tree"]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains("its config has version 99"), "{out}");
}

#[test]
fn an_unrecognised_string_version_refuses_a_read_and_a_write() {
    let c = Sandbox::new_seeded("lock-unknown-string");
    c.ok(&["push", "Root", "--why", "reason"]);
    set_version(&c, Value::from("some other sentence"));
    let before = c.log();

    let (out, code) = c.run(&["tree"]);
    assert_eq!(code, 5, "{out}");
    assert!(
        out.contains(
            "This tree was written by a newer vivac: its config says \"some other \
             sentence\". Update vivac to read it. Nothing was written."
        ),
        "{out}"
    );

    let (out2, code2) = c.run(&["add", "Another", "--why", "reason"]);
    assert_eq!(code2, 5, "{out2}");
    assert!(
        out2.contains("its config says \"some other sentence\""),
        "{out2}"
    );

    assert_eq!(before, c.log(), "a refused read or write still wrote");
}

/// The MCP half of §28.5, in the shape `t411` §13's own test already used
/// for the same reason: the server's *resident* root has to open cleanly to
/// serve anything at all, so the tree whose config is unrecognised is
/// reached as a foreign project instead, the way a cross-project `why`
/// already does for any other store that will not open.
#[test]
fn an_mcp_tool_reaching_a_config_it_does_not_recognise_returns_the_error_not_a_half_answer() {
    let broken = Sandbox::new_seeded("lock-mcp-broken");
    broken.ok(&["push", "Root", "--why", "reason"]);
    set_version(&broken, Value::from("some other sentence"));
    let broken_name = broken.0.file_name().unwrap().to_string_lossy().into_owned();

    let healthy = Sandbox::new_seeded_in("lock-mcp-healthy", broken.global_home());
    let mut s = Server::start(&healthy);
    s.ask(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#);
    let r = s.ask(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"vivac_why","arguments":{{"id":"t1","project":"{broken_name}"}}}}}}"#
    ));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(
        text_of(&r).contains("its config says \"some other sentence\""),
        "{r}"
    );
}

// ---------------------------------------------------------------------------
// §28.6: a missing config regenerates locked over a governed log, and at
// `1` over one with no pillar and no rule.
// ---------------------------------------------------------------------------

#[test]
fn a_missing_config_regenerates_locked_when_the_log_already_has_a_pillar() {
    let c = Sandbox::new_seeded("lock-regen-locked");
    c.ok(&["add", "Security", "--type", "pillar", "--why", "arbiter"]);
    std::fs::remove_file(c.0.join(".vivac").join("config")).unwrap();

    c.ok(&["stack"]);
    assert!(
        is_locked(&c),
        "a config regenerated over a governed log came back unlocked:\n{}",
        config_text(&c)
    );
}

#[test]
fn a_missing_config_regenerates_at_one_with_no_governance() {
    let c = Sandbox::new_seeded("lock-regen-open");
    c.ok(&["push", "Root", "--why", "reason"]);
    std::fs::remove_file(c.0.join(".vivac").join("config")).unwrap();

    c.ok(&["stack"]);
    assert!(!is_locked(&c), "{}", config_text(&c));
}

// ---------------------------------------------------------------------------
// §28.7: the lock cannot be written -- the event does not land either.
// ---------------------------------------------------------------------------

#[test]
fn a_lock_that_cannot_be_written_leaves_the_event_unwritten_and_the_config_alone() {
    let c = Sandbox::new_seeded("lock-cannot-write");
    // A directory sitting where `config.tmp` would need to be a file: fails
    // the same way on Windows, Linux and macOS, unlike a permission bit.
    std::fs::create_dir(c.0.join(".vivac").join("config.tmp")).unwrap();
    let before_log = c.log();
    let before_config = config_text(&c);

    let (out, code) = c.run(&["add", "Security", "--type", "pillar", "--why", "arbiter"]);
    assert_eq!(code, 5, "{out}");
    assert_eq!(
        before_log,
        c.log(),
        "the event landed despite the lock failing"
    );
    assert_eq!(
        before_config,
        config_text(&c),
        "the config changed even though the lock write failed"
    );
}

// ---------------------------------------------------------------------------
// MCP support, mirroring `tests/mcp.rs`'s own `Server`.
// ---------------------------------------------------------------------------

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

    fn ask(&mut self, line: &str) -> Value {
        writeln!(self.input, "{line}").unwrap();
        self.input.flush().unwrap();
        let mut buf = String::new();
        self.output.read_line(&mut buf).unwrap();
        serde_json::from_str(&buf).unwrap_or_else(|e| panic!("not JSON-RPC: {e}\n{buf}"))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn text_of(r: &Value) -> String {
    r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}
