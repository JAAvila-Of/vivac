//! `t411` §19-§22, `d441` — the folder an arm runs in.
//!
//! An arm used to be a command with nowhere written down to run it from, and
//! the folder the tree lives in can hold more than one repository: the
//! wrong one gives a false green. `d441` makes the folder part of the arm's
//! identity, checked and normalized the same way everywhere it is written.

mod common;
use common::Sandbox;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

const MSG_NEEDS_ARM_DIR: &str = "An arm needs --arm-dir: the folder it runs in, relative to the \
     one that holds .vivac. Use . for that folder itself.";
const MSG_ARM_DIR_WITHOUT_ARM: &str = "--arm-dir says where an arm runs, and no --arm was given.";
const MSG_NEEDS_DIR: &str = "An arm needs --dir: the folder it runs in, relative to the \
     one that holds .vivac. Use . for that folder itself.";
const MSG_ABSOLUTE: &str = "An arm's folder is relative to the one that holds .vivac: an \
     absolute path would write this machine's layout into the log.";
const MSG_DOTDOT: &str = "An arm's folder has to be inside the one that holds .vivac.";

fn rule_sandbox(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    std::fs::create_dir(c.0.join("vivac")).unwrap();
    c
}

// ---------------------------------------------------------------------------
// §22.1: the folder round-trips through the log, at birth.
// ---------------------------------------------------------------------------

#[test]
fn add_with_an_arm_and_its_folder_writes_the_pair() {
    let c = rule_sandbox("dir-add");
    let (out, code) = c.run(&[
        "add",
        "Never store a secret",
        "--type",
        "rule",
        "--arm",
        "X",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(
        c.log()
            .contains(r#""arms":[{"dir":"vivac","command":"X"}]"#),
        "{}",
        c.log()
    );
}

#[test]
fn push_with_an_arm_and_its_folder_writes_the_pair() {
    let c = rule_sandbox("dir-push");
    let (out, code) = c.run(&[
        "push",
        "Never store a secret",
        "--type",
        "rule",
        "--arm",
        "X",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(
        c.log()
            .contains(r#""arms":[{"dir":"vivac","command":"X"}]"#),
        "{}",
        c.log()
    );
}

// ---------------------------------------------------------------------------
// §22.2: the three missing-folder messages, in add, push and arm.
// ---------------------------------------------------------------------------

#[test]
fn arm_without_arm_dir_is_refused_and_writes_nothing() {
    let c = rule_sandbox("dir-missing-add");
    let before = c.log();
    let (out, code) = c.run(&[
        "add", "A rule", "--type", "rule", "--arm", "X", "--why", "guard",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains(MSG_NEEDS_ARM_DIR), "{out}");
    assert_eq!(before, c.log(), "{out}");

    let (out2, code2) = c.run(&[
        "push", "A rule", "--type", "rule", "--arm", "X", "--why", "guard",
    ]);
    assert_eq!(code2, 2, "{out2}");
    assert!(out2.contains(MSG_NEEDS_ARM_DIR), "{out2}");
    assert_eq!(before, c.log(), "{out2}");
}

#[test]
fn arm_dir_without_an_arm_is_refused_and_writes_nothing() {
    let c = rule_sandbox("dir-without-arm");
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "A rule",
        "--type",
        "rule",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains(MSG_ARM_DIR_WITHOUT_ARM), "{out}");
    assert_eq!(before, c.log(), "{out}");
}

#[test]
fn arm_command_without_dir_is_refused_and_writes_nothing() {
    let c = rule_sandbox("dir-missing-arm-cmd");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    let before = c.log();
    let (out, code) = c.run(&["arm", "1", "X"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains(MSG_NEEDS_DIR), "{out}");
    assert_eq!(before, c.log(), "{out}");

    let (out2, code2) = c.run(&["arm", "1", "X", "--off"]);
    assert_eq!(code2, 2, "{out2}");
    assert!(out2.contains(MSG_NEEDS_DIR), "{out2}");
    assert_eq!(before, c.log(), "{out2}");
}

// ---------------------------------------------------------------------------
// §22.3: absolute folders, rejected on every system.
// ---------------------------------------------------------------------------

#[test]
fn absolute_folders_are_rejected_on_every_system() {
    let c = rule_sandbox("dir-absolute");
    for bad in [
        "/tmp",
        "C:\\x",
        "C:/x",
        "C:x",
        "\\\\server\\share",
        "\\x",
        "~",
        "~/x",
    ] {
        let before = c.log();
        let (out, code) = c.run(&[
            "add",
            "A rule",
            "--type",
            "rule",
            "--arm",
            "X",
            "--arm-dir",
            bad,
            "--why",
            "guard",
        ]);
        assert_eq!(code, 2, "folder {bad:?}: {out}");
        assert!(out.contains(MSG_ABSOLUTE), "folder {bad:?}: {out}");
        assert_eq!(before, c.log(), "folder {bad:?} still wrote:\n{out}");
    }
}

// ---------------------------------------------------------------------------
// §22.4: a `..` component, anywhere in the path.
// ---------------------------------------------------------------------------

#[test]
fn a_dotdot_component_is_rejected_wherever_it_sits() {
    let c = rule_sandbox("dir-dotdot");
    for bad in ["../x", "vivac/../..", "vivac/../vivac"] {
        let before = c.log();
        let (out, code) = c.run(&[
            "add",
            "A rule",
            "--type",
            "rule",
            "--arm",
            "X",
            "--arm-dir",
            bad,
            "--why",
            "guard",
        ]);
        assert_eq!(code, 2, "folder {bad:?}: {out}");
        assert!(out.contains(MSG_DOTDOT), "folder {bad:?}: {out}");
        assert_eq!(before, c.log(), "folder {bad:?} still wrote:\n{out}");
    }
}

// ---------------------------------------------------------------------------
// §22.5: a folder that does not exist, and a file where one should be.
// ---------------------------------------------------------------------------

#[test]
fn a_folder_that_does_not_exist_is_refused_and_writes_nothing() {
    let c = rule_sandbox("dir-absent");
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "A rule",
        "--type",
        "rule",
        "--arm",
        "X",
        "--arm-dir",
        "nope",
        "--why",
        "guard",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("There is no folder nope inside the one that holds .vivac."),
        "{out}"
    );
    assert_eq!(before, c.log(), "{out}");
}

#[test]
fn a_file_instead_of_a_folder_is_refused_and_writes_nothing() {
    let c = rule_sandbox("dir-is-a-file");
    std::fs::write(c.0.join("plain.txt"), "hi").unwrap();
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "A rule",
        "--type",
        "rule",
        "--arm",
        "X",
        "--arm-dir",
        "plain.txt",
        "--why",
        "guard",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("There is no folder plain.txt inside the one that holds .vivac."),
        "{out}"
    );
    assert_eq!(before, c.log(), "{out}");
}

// ---------------------------------------------------------------------------
// §22.6: normalization.
// ---------------------------------------------------------------------------

#[test]
fn the_folder_is_normalized_before_it_is_written() {
    let c = rule_sandbox("dir-normalize");
    std::fs::create_dir(c.0.join("vivac").join("src")).unwrap();

    c.ok(&[
        "add",
        "R1",
        "--type",
        "rule",
        "--arm",
        "X",
        "--arm-dir",
        "./vivac/",
        "--why",
        "guard",
    ]);
    assert!(
        c.log().contains(r#""dir":"vivac""#),
        "./vivac/ did not normalize to vivac:\n{}",
        c.log()
    );

    c.ok(&[
        "add",
        "R2",
        "--type",
        "rule",
        "--arm",
        "Y",
        "--arm-dir",
        "vivac\\src",
        "--why",
        "guard",
    ]);
    assert!(
        c.log().contains(r#""dir":"vivac/src""#),
        "vivac\\src did not normalize to vivac/src:\n{}",
        c.log()
    );

    c.ok(&[
        "add",
        "R3",
        "--type",
        "rule",
        "--arm",
        "Z",
        "--arm-dir",
        "./",
        "--why",
        "guard",
    ]);
    assert!(
        c.log().contains(r#""dir":".""#),
        "./ did not normalize to .:\n{}",
        c.log()
    );
}

// ---------------------------------------------------------------------------
// §22.7: resolved against the tree's own root, never the process cwd.
// ---------------------------------------------------------------------------

#[test]
fn the_folder_resolves_against_the_tree_root_not_the_process_cwd() {
    let c = rule_sandbox("dir-from-subdir");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    let sub = c.0.join("vivac");
    let out = Command::new(BIN)
        .current_dir(&sub)
        .env("VIVAC_HOME", c.global_home())
        .args(["arm", "1", "X", "--dir", "vivac"])
        .output()
        .unwrap();
    let code = out.status.code().unwrap_or(-1);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(code, 0, "{text}");
    assert!(
        c.log().contains(r#""dir":"vivac","command":"X""#),
        "{}",
        c.log()
    );
}

// ---------------------------------------------------------------------------
// §22.8: identity is the pair.
// ---------------------------------------------------------------------------

#[test]
fn the_same_command_in_two_folders_are_two_different_arms() {
    let c = rule_sandbox("dir-identity");
    c.ok(&[
        "add",
        "A rule",
        "--type",
        "rule",
        "--arm",
        "X",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    let (out, code) = c.run(&["arm", "1", "X", "--dir", "."]);
    assert_eq!(
        code, 0,
        "the same command in a different folder was refused:\n{out}"
    );
}

#[test]
fn off_with_the_right_command_and_the_wrong_folder_finds_nothing_to_remove() {
    let c = rule_sandbox("dir-off-wrong-dir");
    c.ok(&[
        "add",
        "A rule",
        "--type",
        "rule",
        "--arm",
        "X",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    let before = c.log();
    let (out, code) = c.run(&["arm", "1", "X", "--dir", ".", "--off"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("r1 has no such arm; vivac why r1 lists the ones it has."),
        "{out}"
    );
    assert_eq!(before, c.log(), "{out}");
}

// ---------------------------------------------------------------------------
// §22.9: the redaction guard covers the folder, too.
// ---------------------------------------------------------------------------

#[test]
fn the_redaction_guard_covers_the_folder() {
    let c = rule_sandbox("dir-secret");
    std::fs::create_dir(c.0.join("sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345")).ok();
    let before = c.log();
    let (out, code) = c.run(&[
        "add",
        "A rule",
        "--type",
        "rule",
        "--arm",
        "X",
        "--arm-dir",
        "sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345",
        "--why",
        "guard",
    ]);
    assert_eq!(code, 3, "{out}");
    assert_eq!(before, c.log(), "{out}");
}

// ---------------------------------------------------------------------------
// §22.10: `rules` and `why` show the folder; JSON carries it; the index
// changes nothing.
// ---------------------------------------------------------------------------

#[test]
fn rules_and_why_show_the_folder_the_arm_runs_in() {
    let c = rule_sandbox("dir-display");
    c.ok(&[
        "add",
        "Never store a secret",
        "--type",
        "rule",
        "--arm",
        "cargo test --bin vivac redact::tests",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    c.ok(&["arm", "1", "check root", "--dir", "."]);
    let rules_out = c.ok(&["rules"]);
    assert!(
        rules_out.contains("armed in vivac/: cargo test --bin vivac redact::tests"),
        "{rules_out}"
    );
    assert!(rules_out.contains("armed in ./: check root"), "{rules_out}");

    let why_out = c.ok(&["why", "r1"]);
    assert!(
        why_out.contains("armed in vivac/: cargo test --bin vivac redact::tests"),
        "{why_out}"
    );

    let json_out = c.ok(&["rules", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let arms = v["rules"][0]["arms"].as_array().unwrap();
    assert!(
        arms.contains(
            &serde_json::json!({"dir": "vivac", "command": "cargo test --bin vivac redact::tests"})
        ),
        "{json_out}"
    );
}

// ---------------------------------------------------------------------------
// `t411` §32, hole 2: `vivac arm`'s own confirmation shows the folder too,
// in both its shapes -- a pair with no folder is exactly what `d441` retired.
// ---------------------------------------------------------------------------

#[test]
fn arm_confirms_with_the_folder_when_added_and_when_removed() {
    let c = rule_sandbox("dir-confirm-named");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);

    let added = c.ok(&["arm", "1", "check A", "--dir", "vivac"]);
    assert!(added.contains("r1  armed in vivac/: check A"), "{added}");

    let removed = c.ok(&["arm", "1", "check A", "--dir", "vivac", "--off"]);
    assert!(
        removed.contains("r1  no longer armed in vivac/: check A"),
        "{removed}"
    );
}

#[test]
fn arm_confirms_the_tree_root_folder_as_dot() {
    let c = rule_sandbox("dir-confirm-dot");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);

    let added = c.ok(&["arm", "1", "check root", "--dir", "."]);
    assert!(added.contains("r1  armed in ./: check root"), "{added}");
}

#[test]
fn deleting_the_index_does_not_change_rules_or_why_of_an_armed_rule() {
    let c = rule_sandbox("dir-index-guard");
    c.ok(&[
        "add",
        "Never store a secret",
        "--type",
        "rule",
        "--arm",
        "cargo test --bin vivac redact::tests",
        "--arm-dir",
        "vivac",
        "--why",
        "guard",
    ]);
    c.ok(&["stack"]);
    let index_path = c.0.join(".vivac").join("index");
    assert!(index_path.exists(), "no index to delete");

    let rules_before = c.ok(&["rules"]);
    let rules_json_before = c.ok(&["rules", "--json"]);
    let why_before = c.ok(&["why", "r1"]);

    std::fs::remove_file(&index_path).unwrap();

    assert_eq!(rules_before, c.ok(&["rules"]));
    assert_eq!(rules_json_before, c.ok(&["rules", "--json"]));
    assert_eq!(why_before, c.ok(&["why", "r1"]));
}

// ---------------------------------------------------------------------------
// §22.11: MCP uses its own vocabulary for the same two messages.
// ---------------------------------------------------------------------------

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

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

#[test]
fn vivac_add_with_an_arm_and_no_arm_dir_reports_the_mcp_form_and_writes_nothing() {
    let c = rule_sandbox("dir-mcp-add");
    let before = c.log();
    let mut s = Server::start(&c);
    s.ask(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"vivac_add","arguments":{"title":"A rule","type":"rule","why":"guard","arm":["X"]}}}"#,
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    let text = text_of(&r);
    assert!(
        text.contains("An arm needs arm_dir: the folder it runs in, relative to the one that holds .vivac. Use . for that folder itself."),
        "{text}"
    );
    assert_eq!(before, c.log(), "{text}");
}

#[test]
fn vivac_arm_without_dir_reports_the_mcp_form_and_writes_nothing() {
    let c = rule_sandbox("dir-mcp-arm");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    let before = c.log();
    let mut s = Server::start(&c);
    s.ask(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#);
    let r = s.ask(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"vivac_arm","arguments":{"id":"r1","command":"X"}}}"#,
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    let text = text_of(&r);
    assert!(
        text.contains("An arm needs dir: the folder it runs in, relative to the one that holds .vivac. Use . for that folder itself."),
        "{text}"
    );
    assert_eq!(before, c.log(), "{text}");
}
