//! `vivac setup claude-code` — `t565` §7 and §11.
//!
//! Every test runs in a temporary sandbox with its own `VIVAC_HOME`: no test
//! here ever touches a real project. `--yes` stands in for a terminal
//! everywhere except the test that proves there is none.

mod common;
use common::Sandbox;
use std::path::Path;

const SESSION_START: &str = "vivac session start --hook";
const SESSION_END: &str = "vivac session end --hook";
const SESSION_PROMPT: &str = "vivac session prompt --hook";

fn settings_path(c: &Sandbox) -> std::path::PathBuf {
    c.0.join(".claude").join("settings.json")
}
fn mcp_path(c: &Sandbox) -> std::path::PathBuf {
    c.0.join(".mcp.json")
}
fn skill_path(c: &Sandbox) -> std::path::PathBuf {
    c.0.join(".claude")
        .join("skills")
        .join("vivac-migrate")
        .join("SKILL.md")
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("reading {p:?}: {e}"))
}
fn read_bytes(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("reading {p:?}: {e}"))
}

/// Whether `dir` holds a tree of its own: `store::already_planted`'s own
/// definition, `config` **or** `events`, copied here rather than asked of
/// the crate because an integration test has no `pub` path to it.
/// `t594`: three tests used to check `config` alone, which a
/// regression that planted only an `events` file would have slipped
/// straight past.
fn already_planted(dir: &Path) -> bool {
    dir.join(".vivac").join("config").is_file() || dir.join(".vivac").join("events").is_file()
}

// `unlocked` used to live here: `d734` moved it to `common::Sandbox` once
// the same function existed, word for word bar the JSON read, in this
// file and two others (`f724`'s own lesson about two hand copies
// drifting apart). The two tests below are about the lock line a plan
// shows the moment a tree is not locked to `store::LANE_SENTENCE` yet,
// which a freshly seeded tree no longer is -- `Sandbox::unlocked` puts it
// back.

/// `p` the way the binary's own `current_dir()` would print it, for building
/// an expected text around a path.
///
/// On Unix, `current_dir()` returns the *physical* path: on macOS, a
/// temporary directory under `/var` comes back under `/private/var`, which
/// `canonicalize` also resolves to. On Windows it is the opposite:
/// `current_dir()` returns the path as given, short 8.3 names included (a
/// GitHub runner's `TEMP` is `C:\Users\RUNNER~1\...`), and `canonicalize`
/// would both expand those and prepend `\\?\`, breaking the very paths this
/// is meant to match.
#[cfg(unix)]
fn printed(p: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| panic!("canonicalize {p:?}: {e}"))
}
#[cfg(not(unix))]
fn printed(p: &std::path::Path) -> std::path::PathBuf {
    p.to_path_buf()
}

// ---------------------------------------------------------------------------
// 1. A fresh project.
// ---------------------------------------------------------------------------

/// `d723` piece B: `setup` no longer plants, so this folder needs a tree
/// of its own before `setup claude-code` writes its three pieces onto it --
/// this used to be one run that left four pieces behind, `.vivac/` among
/// them; now it is two, and what this test proves is the second one alone.
#[test]
fn a_fresh_project_gets_the_three_harness_pieces() {
    let c = Sandbox::new_empty("setup-fresh");
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("vivac setup claude-code will, in"), "{out}");
    assert!(
        out.contains("create") && out.contains("three hooks"),
        "{out}"
    );
    assert!(out.contains(SESSION_START), "{out}");
    assert!(out.contains(SESSION_END), "{out}");
    assert!(out.contains(SESSION_PROMPT), "{out}");
    assert!(
        out.contains("create") && out.contains("the \"vivac\" server"),
        "{out}"
    );
    assert!(
        out.contains("create") && out.contains("how an agent brings"),
        "{out}"
    );
    assert!(out.contains("Written."), "{out}");
    assert!(
        out.contains("Undo: vivac setup claude-code --undo"),
        "{out}"
    );

    assert!(c.0.join(".vivac").exists(), "the tree must stay planted");

    let settings: serde_json::Value = serde_json::from_str(&read(&settings_path(&c))).unwrap();
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        SESSION_START
    );
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        SESSION_END
    );
    assert_eq!(
        settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
        SESSION_PROMPT
    );

    let mcp: serde_json::Value = serde_json::from_str(&read(&mcp_path(&c))).unwrap();
    assert_eq!(mcp["mcpServers"]["vivac"]["command"], "vivac");
    assert_eq!(mcp["mcpServers"]["vivac"]["args"][0], "mcp");

    let skill = read(&skill_path(&c));
    assert!(skill.starts_with("---\nname: vivac-migrate\n"), "{skill}");
    assert!(
        skill.contains("written by vivac setup; fingerprint "),
        "{skill}"
    );
}

// ---------------------------------------------------------------------------
// 2. Idempotent.
// ---------------------------------------------------------------------------

#[test]
fn a_second_run_writes_nothing() {
    let c = Sandbox::new_empty("setup-idempotent");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    let settings_before = read_bytes(&settings_path(&c));
    let mcp_before = read_bytes(&mcp_path(&c));
    let skill_before = read_bytes(&skill_path(&c));
    let log_before = c.log();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Nothing to write: this project is already set up."),
        "{out}"
    );
    assert_eq!(settings_before, read_bytes(&settings_path(&c)));
    assert_eq!(mcp_before, read_bytes(&mcp_path(&c)));
    assert_eq!(skill_before, read_bytes(&skill_path(&c)));
    assert_eq!(log_before, c.log(), "the second run touched the tree");
}

// ---------------------------------------------------------------------------
// 3. A settings.json with its own content, out of order, CRLF.
// ---------------------------------------------------------------------------

const FOREIGN_SETTINGS: &str = "{\r\n  \"otherKey\": \"z\",\r\n  \"env\": {\r\n    \"SOME_TOKEN\": \"shh\"\r\n  },\r\n  \"hooks\": {\r\n    \"SessionStart\": [\r\n      {\r\n        \"matcher\": \"*\",\r\n        \"hooks\": [\r\n          {\r\n            \"type\": \"command\",\r\n            \"command\": \"some-other-tool\"\r\n          }\r\n        ]\r\n      }\r\n    ],\r\n    \"Stop\": [\r\n      {\r\n        \"hooks\": [\r\n          {\r\n            \"type\": \"command\",\r\n            \"command\": \"another-tool\"\r\n          }\r\n        ]\r\n      }\r\n    ]\r\n  },\r\n  \"aKey\": 1\r\n}\r\n";

const FOREIGN_SETTINGS_FOUR_SPACE: &str = "{\r\n    \"otherKey\": \"z\",\r\n    \"env\": {\r\n        \"SOME_TOKEN\": \"shh\"\r\n    },\r\n    \"hooks\": {\r\n        \"SessionStart\": [\r\n            {\r\n                \"matcher\": \"*\",\r\n                \"hooks\": [\r\n                    {\r\n                        \"type\": \"command\",\r\n                        \"command\": \"some-other-tool\"\r\n                    }\r\n                ]\r\n            }\r\n        ],\r\n        \"Stop\": [\r\n            {\r\n                \"hooks\": [\r\n                    {\r\n                        \"type\": \"command\",\r\n                        \"command\": \"another-tool\"\r\n                    }\r\n                ]\r\n            }\r\n        ]\r\n    },\r\n    \"aKey\": 1\r\n}\r\n";

#[test]
fn a_settings_file_with_its_own_content_keeps_it_and_gains_ours_at_the_end() {
    let c = Sandbox::new_empty("setup-foreign-settings");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    std::fs::write(settings_path(&c), FOREIGN_SETTINGS).unwrap();

    c.ok(&["setup", "claude-code", "--yes"]);
    let after = read(&settings_path(&c));

    // Order: the four original top-level keys, in their original order.
    let order: Vec<usize> = ["otherKey", "env", "hooks", "aKey"]
        .iter()
        .map(|k| after.find(&format!("\"{k}\"")).unwrap())
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{after}");

    assert!(
        after.contains("SOME_TOKEN"),
        "the env block was lost:\n{after}"
    );
    assert!(after.contains("shh"), "{after}");
    assert!(
        after.contains("some-other-tool"),
        "the foreign SessionStart hook was lost:\n{after}"
    );
    assert!(
        after.contains("another-tool"),
        "the foreign Stop hook was lost:\n{after}"
    );
    assert!(after.contains(SESSION_START), "{after}");
    assert!(after.contains(SESSION_END), "{after}");
    assert!(after.contains(SESSION_PROMPT), "{after}");
    assert!(after.contains("\r\n"), "CRLF was lost:\n{after:?}");
}

// ---------------------------------------------------------------------------
// 4. The same file, indented at four spaces.
// ---------------------------------------------------------------------------

#[test]
fn indentation_of_an_existing_file_is_kept() {
    let c = Sandbox::new_empty("setup-indent");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    std::fs::write(settings_path(&c), FOREIGN_SETTINGS_FOUR_SPACE).unwrap();

    c.ok(&["setup", "claude-code", "--yes"]);
    let after = read(&settings_path(&c));
    assert!(after.contains("\r\n    \"otherKey\""), "{after}");
    assert!(after.contains("\r\n    \"hooks\": {"), "{after}");
}

// ---------------------------------------------------------------------------
// 5. A SessionStart already running vivac, spelled differently.
// ---------------------------------------------------------------------------

#[test]
fn a_differently_spelled_session_start_is_left_alone_and_copied_into_the_plan() {
    let c = Sandbox::new_empty("setup-different-spelling");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": [
                { "hooks": [ { "type": "command", "command": "C:/tools/vivac.exe session start --hook" } ] }
            ]
        }
    });
    std::fs::write(
        settings_path(&c),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("already runs  C:/tools/vivac.exe session start --hook"),
        "{out}"
    );

    let after: serde_json::Value = serde_json::from_str(&read(&settings_path(&c))).unwrap();
    let start = after["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(
        start.len(),
        1,
        "a second SessionStart entry was added:\n{after}"
    );
}

// ---------------------------------------------------------------------------
// `d779`: a project that already has the two older hooks gains only the
// third, and neither of the first two is touched or duplicated.
// ---------------------------------------------------------------------------

#[test]
fn a_project_with_the_two_older_hooks_gains_only_the_third() {
    let c = Sandbox::new_empty("setup-prompt-hook-add-third");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": [
                { "hooks": [ { "type": "command", "command": SESSION_START } ] }
            ],
            "Stop": [
                { "hooks": [ { "type": "command", "command": SESSION_END } ] }
            ]
        }
    });
    std::fs::write(
        settings_path(&c),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("add") && out.contains("the UserPromptSubmit hook"),
        "{out}"
    );
    assert!(!out.contains("already has all three hooks"), "{out}");

    let after: serde_json::Value = serde_json::from_str(&read(&settings_path(&c))).unwrap();
    assert_eq!(
        after["hooks"]["SessionStart"].as_array().unwrap().len(),
        1,
        "the existing SessionStart hook was duplicated:\n{after}"
    );
    assert_eq!(
        after["hooks"]["Stop"].as_array().unwrap().len(),
        1,
        "the existing Stop hook was duplicated:\n{after}"
    );
    assert_eq!(
        after["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
        SESSION_PROMPT
    );
}

// ---------------------------------------------------------------------------
// 6 and 7. The MCP server, taken and free.
// ---------------------------------------------------------------------------

#[test]
fn a_taken_vivac_server_name_is_a_conflict_and_nothing_is_written() {
    let c = Sandbox::new_empty("setup-mcp-taken");
    c.ok(&["init", "--yes"]);
    let mcp = serde_json::json!({
        "mcpServers": { "vivac": { "command": "something-else", "args": ["run"] } }
    });
    std::fs::write(mcp_path(&c), serde_json::to_string_pretty(&mcp).unwrap()).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("already has a server called \"vivac\""),
        "{out}"
    );
    assert!(out.contains("something-else run"), "{out}");
    assert!(out.contains("Nothing written."), "{out}");
    assert!(
        !settings_path(&c).exists(),
        "settings.json was written despite the conflict"
    );
}

#[test]
fn another_name_already_running_vivac_mcp_is_left_as_it_is() {
    let c = Sandbox::new_empty("setup-mcp-other-name");
    c.ok(&["init", "--yes"]);
    let mcp = serde_json::json!({
        "mcpServers": { "vivac-tree": { "command": "vivac", "args": ["mcp"] } }
    });
    std::fs::write(mcp_path(&c), serde_json::to_string_pretty(&mcp).unwrap()).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        plan_words(&out).contains("already runs vivac mcp as \"vivac-tree\""),
        "{out}"
    );

    let after: serde_json::Value = serde_json::from_str(&read(&mcp_path(&c))).unwrap();
    assert!(after["mcpServers"].get("vivac").is_none(), "{after}");
    assert_eq!(after["mcpServers"]["vivac-tree"]["command"], "vivac");
}

// ---------------------------------------------------------------------------
// 8. The skill's three non-missing states.
// ---------------------------------------------------------------------------

#[test]
fn a_skill_with_no_marker_is_a_conflict() {
    let c = Sandbox::new_empty("setup-skill-no-marker");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(skill_path(&c).parent().unwrap()).unwrap();
    std::fs::write(skill_path(&c), "# Someone else's skill\n").unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("SKILL.md is already there"), "{out}");
    assert!(out.contains("Nothing written."), "{out}");
}

#[test]
fn a_skill_with_a_broken_fingerprint_is_a_conflict() {
    let c = Sandbox::new_empty("setup-skill-bad-fingerprint");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(skill_path(&c).parent().unwrap()).unwrap();
    std::fs::write(
        skill_path(&c),
        "---\nname: vivac-migrate\ndescription: x\n---\n<!-- written by vivac setup; fingerprint 0000000000000000; vivac setup claude-code --undo removes it while the text is unchanged -->\n\nedited\n",
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("SKILL.md is already there"), "{out}");
}

#[test]
fn a_skill_an_earlier_vivac_wrote_is_replaced() {
    let c = Sandbox::new_empty("setup-skill-replaceable");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(skill_path(&c).parent().unwrap()).unwrap();
    // A valid marker over old body text: the fingerprint has to be the real
    // FNV-1a of the content that follows, or this would read as a conflict
    // instead.
    let old_body = "---\nname: vivac-migrate\ndescription: an older description\n---\n";
    let rest = "\n# An older vivac-migrate\n\nOlder text.\n";
    let fp = fnv1a64(format!("{old_body}{rest}").as_bytes());
    let text = format!(
        "{old_body}<!-- written by vivac setup; fingerprint {fp:016x}; vivac setup claude-code --undo removes it while the text is unchanged -->\n{rest}"
    );
    std::fs::write(skill_path(&c), &text).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        plan_words(&out).contains("replace")
            && plan_words(&out).contains("the copy an earlier vivac wrote"),
        "{out}"
    );
    let after = read(&skill_path(&c));
    assert!(
        after.contains("# Bringing what a project knows into vivac"),
        "{after}"
    );
    assert!(!after.contains("An older vivac-migrate"), "{after}");
}

// ---------------------------------------------------------------------------
// 9. Broken JSON, and JSON whose root is not an object.
// ---------------------------------------------------------------------------

#[test]
fn broken_json_refuses_with_line_and_column_and_writes_nothing() {
    let c = Sandbox::new_empty("setup-broken-json");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    std::fs::write(settings_path(&c), "{\n  \"a\": ,\n}").unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(".claude/settings.json is not JSON setup can read"),
        "{out}"
    );
    assert!(out.contains("line 2"), "{out}");
    assert!(out.contains("Nothing written."), "{out}");
}

#[test]
fn a_json_root_that_is_not_an_object_is_a_conflict() {
    let c = Sandbox::new_empty("setup-not-object");
    c.ok(&["init", "--yes"]);
    std::fs::write(mcp_path(&c), "[1, 2, 3]").unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(".mcp.json holds JSON whose top level is not an object"),
        "{out}"
    );
    assert!(out.contains("Nothing written."), "{out}");
}

// ---------------------------------------------------------------------------
// 10. `--dry-run`.
// ---------------------------------------------------------------------------

#[test]
fn dry_run_writes_nothing() {
    let c = Sandbox::new_empty("setup-dry-run");
    c.ok(&["init", "--yes"]);
    let before: Vec<_> = std::fs::read_dir(&c.0).unwrap().collect();
    let (out, code) = c.run(&["setup", "claude-code", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    let after: Vec<_> = std::fs::read_dir(&c.0).unwrap().collect();
    assert_eq!(before.len(), after.len(), "dry-run created something");
    assert!(!c.0.join(".claude").exists());
}

// ---------------------------------------------------------------------------
// 11. No terminal and no `--yes`.
// ---------------------------------------------------------------------------

/// `f718`: the same guard on the other door. `--undo` reached its own
/// question without ever passing the check, so a run with nobody to answer
/// printed the question anyway, removed nothing and exited 0 -- and 0 with
/// nothing done is what a script reads as done. The command it hands back
/// keeps the `--undo` it was given, for the reason `f675` already gave:
/// without it, the advice is advice for the opposite run.
#[test]
fn no_terminal_and_no_yes_refuses_an_undo_too_and_removes_nothing() {
    let c = Sandbox::new_empty("setup-no-terminal-undo");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    let (out, code) = c.run(&["setup", "claude-code", "--undo"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("there is no terminal to ask"), "{out}");
    assert!(
        out.contains("vivac setup claude-code --undo --dry-run"),
        "{out}"
    );
    assert!(
        out.contains("vivac setup claude-code --undo --yes"),
        "{out}"
    );
    assert!(
        c.0.join(".claude").join("settings.json").is_file(),
        "the run removed something it had not been allowed to confirm"
    );
    assert!(c.0.join(".mcp.json").is_file(), "{out}");
}

#[test]
fn no_terminal_and_no_yes_refuses_without_a_plan() {
    let c = Sandbox::new_empty("setup-no-terminal");
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "claude-code"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("there is no terminal here to ask"), "{out}");
    assert!(out.contains("vivac setup claude-code --dry-run"), "{out}");
    assert!(out.contains("vivac setup claude-code --yes"), "{out}");
    assert!(
        !out.contains("vivac setup claude-code will, in"),
        "a plan was shown:\n{out}"
    );
    assert!(!c.0.join(".claude").exists());
}

// ---------------------------------------------------------------------------
// `t594` §4.5.2, §6.1: the lane plan lines, for a folder that becomes a
// brand new lane of the tree above it.
// ---------------------------------------------------------------------------

/// Whether the plan shows a line naming `label` whose own text also
/// contains `rest`: `d792` never wraps a plan item across more than one
/// line, so both the path and its "what" always sit on the very same
/// line now, with no continuation to put back together.
fn lane_line_containing(out: &str, label: &str, rest: &str) -> bool {
    out.lines().any(|l| {
        let trimmed = l.trim_start();
        trimmed.contains(label) && trimmed.contains(rest)
    })
}

/// The words of `out`, run together regardless of which line
/// `render::wrap` (`f720`) put them on -- for a plain `.contains` check
/// that does not anchor on a label the way `lane_line_containing` does.
/// The same shift `tests/check.rs`'s own `words` already made for
/// `copy_notice`'s prose, for the same reason: width wraps a status now,
/// not a hand-picked cut.
fn plan_words(out: &str) -> String {
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// `d723` piece B: showing the lane's own plan lines is `init`'s job now --
// `setup` never resolves into a folder that is not yet one of the tree's
// own lanes (`Failure::not_a_lane_yet`), so `init --yes` is what these
// three tests run instead of `setup claude-code --yes`.
#[test]
fn the_plan_for_a_new_lane_shows_the_three_lines_the_spec_gives() {
    let c = Sandbox::unlocked("setup-lane-plan-lines");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 0, "{out}");

    // `t640`, point 9: the plan names the product on this line too,
    // derived here since nobody fixed one with `--name`.
    let product = c.0.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        lane_line_containing(
            &out,
            ".vivac/lane",
            &format!("this folder becomes lane \"v2\" of \"{product}\""),
        ),
        "{out}"
    );
    assert!(
        lane_line_containing(
            &out,
            ".vivac/.gitignore",
            "keeps .vivac/ out of version control"
        ),
        "{out}"
    );
    assert!(
        lane_line_containing(
            &out,
            ".vivac/config",
            "the minimum version to open it: vivac 0.12"
        ),
        "{out}"
    );
}

/// The config warning is part of the *plan*, not of what gets printed
/// after writing: `--dry-run` never writes anything, and it still shows
/// the line, exactly where the plan showed it above.
#[test]
fn the_lane_config_warning_shows_up_before_anything_is_written() {
    let c = Sandbox::unlocked("setup-lane-plan-dry-run");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();

    let (out, code) = run_in(&second, c.global_home(), &["init", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(
        lane_line_containing(
            &out,
            ".vivac/config",
            "the minimum version to open it: vivac 0.12"
        ),
        "{out}"
    );
    assert!(!second.join(".vivac").exists(), "dry-run wrote something");
}

/// Finding 6 (baja): `plan_lane` used to read the tree's config through
/// `Store::open`, which fills a missing one back in on its own -- a write
/// `--dry-run` must never cause, even one this indirect, and even over a
/// tree it is not planting.
#[test]
fn dry_run_never_regenerates_a_missing_config() {
    let c = Sandbox::new_seeded("setup-dry-run-no-config");
    std::fs::remove_file(c.0.join(".vivac").join("config")).unwrap();
    assert!(!c.0.join(".vivac").join("config").exists());

    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();
    let (out, code) = run_in(&second, c.global_home(), &["init", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(
        !c.0.join(".vivac").join("config").exists(),
        "--dry-run regenerated the tree's config:\n{out}"
    );
}

/// `t594`: an already-set-up project runs into
/// `nothing_to_write` before it ever reaches `--dry-run`'s own check, and
/// that branch notes the machine's registry (`note_registry`) -- a write
/// `--dry-run` must never make, in the registry or anywhere else. The
/// registry is deleted first, so its own directory reappearing is exactly
/// the write this catches.
#[test]
fn dry_run_never_writes_the_machine_registry_either() {
    let c = Sandbox::new_empty("setup-dry-run-no-registry");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    std::fs::remove_dir_all(c.global_home()).ok();
    assert!(!c.global_home().exists());

    let (out, code) = c.run(&["setup", "claude-code", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(
        !c.global_home().exists(),
        "--dry-run wrote to the machine's registry:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// 12 and 13. `--undo`.
// ---------------------------------------------------------------------------

#[test]
fn undo_after_a_fresh_setup_leaves_only_the_tree() {
    let c = Sandbox::new_empty("setup-undo-fresh");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    let vivac_before = std::fs::read(c.0.join(".vivac").join("config")).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        plan_words(&out).contains("remove")
            && plan_words(&out)
                .contains("the three hooks setup wrote; nothing else is left, so it goes"),
        "{out}"
    );
    assert!(
        out.contains("Undone. The tree in .vivac/ is untouched."),
        "{out}"
    );
    // `d784`: `--undo` removes no file setup did not write, and no
    // directory but `vivac-migrate` itself -- `.claude/` stays, empty or
    // not, the same as it would have if setup had found it already there.
    assert!(
        c.0.join(".claude").exists(),
        "setup must never remove .claude/ itself: {:?}",
        list(&c.0)
    );
    assert!(!settings_path(&c).exists());
    assert!(
        !skill_path(&c).parent().unwrap().exists(),
        "{:?}",
        list(&c.0.join(".claude").join("skills"))
    );
    assert!(!mcp_path(&c).exists());
    assert!(c.0.join(".vivac").exists());
    assert_eq!(
        vivac_before,
        std::fs::read(c.0.join(".vivac").join("config")).unwrap()
    );
}

#[test]
fn undo_over_a_file_with_its_own_content_returns_it_to_the_original_text() {
    let c = Sandbox::new_empty("setup-undo-restore");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    std::fs::write(settings_path(&c), FOREIGN_SETTINGS).unwrap();

    c.ok(&["setup", "claude-code", "--yes"]);
    let (out, code) = c.run(&["setup", "claude-code", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(read(&settings_path(&c)), FOREIGN_SETTINGS, "{out}");
}

// ---------------------------------------------------------------------------
// 14. `--undo` leaves what it does not recognise, and says so.
// ---------------------------------------------------------------------------

#[test]
fn undo_leaves_a_differently_spelled_hook_and_an_edited_skill() {
    let c = Sandbox::new_empty("setup-undo-leaves");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": [
                { "hooks": [ { "type": "command", "command": "C:/tools/vivac.exe session start --hook" } ] }
            ]
        }
    });
    std::fs::write(
        settings_path(&c),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    c.ok(&["setup", "claude-code", "--yes"]);
    // Edit the skill after setup wrote it, so its fingerprint no longer
    // matches: the way an edited file looks to `--undo`.
    let skill = read(&skill_path(&c));
    std::fs::write(skill_path(&c), format!("{skill}\nedited by hand\n")).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("runs vivac another way; left as it is"),
        "{out}"
    );
    assert!(
        plan_words(&out).contains("changed since setup wrote it; left as it is"),
        "{out}"
    );
    assert!(
        read(&settings_path(&c)).contains("C:/tools/vivac.exe session start --hook"),
        "the differently-spelled hook was removed"
    );
    assert!(skill_path(&c).exists(), "the edited skill was removed");
}

// ---------------------------------------------------------------------------
// `d680` used to live here: `--undo` and the lane file `--join` leaves
// behind. `d723` piece B moved both `--join` and `--undo`'s own reach into
// `.vivac/lane` to `init` -- `setup --undo` never touches the lane at all
// any more, so none of the four scenarios this section covered (an
// unwritten lane removed and unblocking `relocate`, a written lane kept,
// a lane removed even with nothing else left to undo, a hand-edited
// `.gitignore` kept) has anything left to say about `setup`. `init.rs`'s
// own `init_undo_removes_a_joined_lane_and_leaves_the_target_log_growing_only`
// covers the first case through `init --undo`; the other three are not
// ported there yet -- a gap, not a decision, and named as one in the
// report this piece ends with.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 19. The `hooks` tombstone.
// ---------------------------------------------------------------------------

#[test]
fn hooks_is_a_tombstone() {
    let c = Sandbox::new_empty("setup-hooks-tombstone");
    let (out, code) = c.run(&["hooks"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("vivac hooks is gone: vivac setup claude-code writes the hooks itself,"),
        "{out}"
    );
    assert!(!out.contains("Paste this into"), "{out}");
}

/// `d792`: the plainest door to `Failure::SetupNoTree` -- a folder with no
/// `.vivac/` anywhere above it -- prints the new block on stderr and
/// exits the same 4 it always has.
#[test]
fn setup_with_no_tree_at_all_prints_the_new_failure_and_keeps_its_exit_code() {
    let c = Sandbox::new_empty("setup-no-tree-at-all");
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 4, "{out}");
    assert!(
        out.contains("There is no tree here for setup to connect, so nothing was written."),
        "{out}"
    );
    assert!(
        out.contains("Next:") && out.contains("vivac init --join"),
        "{out}"
    );
    assert!(!c.0.join(".claude").exists());
}

// ---------------------------------------------------------------------------
// 20. The registry's own folder. `d723` piece B: `setup` finds no tree to
// resolve from inside the registry's own folder at all -- nothing marks it
// as one -- so it refuses with `Failure::SetupNoTree`, the generic answer,
// before it ever reaches the registry-specific one. Following that
// refusal's own advice (`vivac init`) is what actually meets the
// registry-specific text: `resolve_roots`, unchanged, still catches the
// tree root resolving to the registry's own folder.
// ---------------------------------------------------------------------------

#[test]
fn setup_refuses_inside_the_registry_folder() {
    let c = Sandbox::new_empty("setup-registry-root");
    // Marks `global_home()` as the registry the way `registry::note` does,
    // without needing a second project to trigger it for real.
    std::fs::create_dir_all(c.global_home()).unwrap();
    std::fs::write(c.global_home().join("projects"), "{}").unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(c.global_home())
        .env("VIVAC_HOME", c.global_home())
        .env("TZ", "UTC")
        .args(["setup", "claude-code", "--yes"])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(4), "{text}");
    assert!(
        text.contains("There is no tree here for setup to connect"),
        "{text}"
    );

    let init_out = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(c.global_home())
        .env("VIVAC_HOME", c.global_home())
        .env("TZ", "UTC")
        .args(["init", "--yes"])
        .output()
        .unwrap();
    let init_text = String::from_utf8_lossy(&init_out.stdout).into_owned()
        + &String::from_utf8_lossy(&init_out.stderr);
    assert_eq!(init_out.status.code(), Some(1), "{init_text}");
    assert!(
        init_text.contains("holds the registry of the trees on this machine"),
        "{init_text}"
    );
}

// ---------------------------------------------------------------------------
// 21. The skill, golden, and its fingerprint.
// ---------------------------------------------------------------------------

const FRONTMATTER: &str = include_str!("../src/setup/skill-frontmatter.md");
const BODY: &str = include_str!("../src/setup/skill-body.md");

fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[test]
fn the_skill_is_golden_and_its_fingerprint_is_the_real_hash_of_its_text() {
    let c = Sandbox::new_empty("setup-skill-golden");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    let skill = read(&skill_path(&c));

    let mut lines: Vec<&str> = skill.split('\n').collect();
    let marker = lines[4];
    let fp_hex = marker
        .strip_prefix("<!-- written by vivac setup; fingerprint ")
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let claimed = u64::from_str_radix(fp_hex, 16).unwrap();
    lines.remove(4);
    let without_marker = lines.join("\n");
    assert_eq!(
        claimed,
        fnv1a64(without_marker.as_bytes()),
        "the fingerprint is not the FNV-1a of the text it claims to cover"
    );

    // The marker names no harness. It named `claude-code` until `f714`,
    // and this is the same file byte for byte wherever setup writes it, so
    // a project set up with Codex was handed a line proposing a command for
    // the other harness.
    let expected = format!(
        "{FRONTMATTER}<!-- written by vivac setup; fingerprint {claimed:016x}; setup removes \
         it with --undo while the text is unchanged -->\n{BODY}"
    );
    assert_eq!(skill, expected, "the skill drifted from the literal text");
}

// ---------------------------------------------------------------------------
// 22. Help names setup, not hooks.
// ---------------------------------------------------------------------------

#[test]
fn help_names_setup_and_not_hooks() {
    let c = Sandbox::new_empty("setup-help");
    let help = c.ok(&["--help"]);
    assert!(help.contains("vivac setup claude-code"), "{help}");
    assert!(!help.contains("vivac hooks"), "{help}");
}

#[test]
fn setup_no_longer_says_to_commit_the_tree() {
    let c = Sandbox::new_empty("setup-files");
    c.ok(&["init", "--yes"]);
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(out.contains(".vivac/ is never committed"), "{out}");
}

// ---------------------------------------------------------------------------
// Coordinator's review: C, D, E, F.
// ---------------------------------------------------------------------------

/// C: an existing `settings.json` with neither hook says "add", not
/// "create" -- "create" is only for a file that does not exist yet.
#[test]
fn an_existing_settings_file_with_no_hooks_says_add_not_create() {
    let c = Sandbox::new_empty("setup-add-not-create-settings");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    std::fs::write(settings_path(&c), "{\n  \"otherKey\": 1\n}\n").unwrap();

    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(out.contains("add") && out.contains("three hooks"), "{out}");
    assert!(!out.contains("create, with three hooks"), "{out}");
}

/// C: the same for `.mcp.json` with no server of ours.
#[test]
fn an_existing_mcp_file_with_no_server_says_add_not_create() {
    let c = Sandbox::new_empty("setup-add-not-create-mcp");
    c.ok(&["init", "--yes"]);
    std::fs::write(mcp_path(&c), "{\n  \"mcpServers\": {}\n}\n").unwrap();

    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(
        out.contains("add") && out.contains("the \"vivac\" server"),
        "{out}"
    );
    assert!(!out.contains("create, with the server"), "{out}");
}

/// D: `--undo --dry-run` shows the plan and writes nothing, without asking.
#[test]
fn undo_dry_run_shows_the_plan_and_writes_nothing() {
    let c = Sandbox::new_empty("setup-undo-dry-run");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    let before = read_bytes(&settings_path(&c));

    let (out, code) = c.run(&["setup", "claude-code", "--undo", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("vivac setup claude-code --undo will, in"),
        "{out}"
    );
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(!out.contains("Undo it?"), "{out}");
    assert_eq!(before, read_bytes(&settings_path(&c)));
    assert!(c.0.join(".vivac").exists());
}

/// E: when `.mcp.json` will be removed because it holds nothing else, the
/// undo plan says so in the one sentence `settings.json`'s own removal
/// already uses -- wrapped by width now (`f720`), not by a hand-picked
/// cut, so this checks the sentence and not which line it landed on.
#[test]
fn undo_plan_wraps_the_mcp_removal_when_the_file_would_empty_out() {
    let c = Sandbox::new_empty("setup-undo-mcp-wrap");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);

    let (out, _) = c.run(&["setup", "claude-code", "--undo", "--dry-run"]);
    assert!(
        plan_words(&out).contains("remove")
            && plan_words(&out).contains("the \"vivac\" server; nothing else is left, so it goes"),
        "{out}"
    );
}

/// F: `--dry-run` with `--yes` is a usage error with the literal text, and
/// touches nothing.
#[test]
fn dry_run_with_yes_is_a_usage_error() {
    let c = Sandbox::new_empty("setup-dry-run-yes");
    let (out, code) = c.run(&["setup", "claude-code", "--dry-run", "--yes"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--dry-run writes nothing, so there is nothing for --yes to confirm."),
        "{out}"
    );
    assert!(out.contains("Give one or the other."), "{out}");
    assert!(!c.0.join(".vivac").exists());
}

fn list(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// t579 §4/§9: two roots -- Claude Code's own files always in the current
// directory, the tree wherever it is found walking up.
// ---------------------------------------------------------------------------

fn run_in(dir: &Path, home: &Path, args: &[&str]) -> (String, i32) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(dir)
        .env("VIVAC_HOME", home)
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
        out.status.code().unwrap_or(-1),
    )
}

fn create_git_dir(at: &Path) {
    std::fs::create_dir_all(at.join(".git")).unwrap();
}

fn create_git_worktree_file(at: &Path) {
    std::fs::write(at.join(".git"), "gitdir: ../elsewhere/.git/worktrees/x\n").unwrap();
}

/// (a): a subfolder of a repository, its own tree already planted there
/// (`d723` piece B: `setup` no longer plants it, `init` already has), still
/// gets Claude Code's files, and the plan still warns with the
/// repository's root.
#[test]
fn a_subfolder_of_a_repository_gets_its_own_files_with_a_warning() {
    let c = Sandbox::new_empty("setup-two-roots-subfolder");
    create_git_dir(&c.0);
    let sub = c.0.join("packages").join("app");
    std::fs::create_dir_all(&sub).unwrap();
    run_in(&sub, c.global_home(), &["init", "--yes"]);

    let (out, code) = run_in(&sub, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(&format!(
            "This folder is inside the repository at {}, not at its root.",
            printed(&c.0).display()
        )),
        "{out}"
    );
    assert!(
        out.contains("Claude Code reads these files only from the folder it is opened in: if"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "you open it at {}, run setup there instead.",
            printed(&c.0).display()
        )),
        "{out}"
    );

    assert!(sub.join(".claude").join("settings.json").exists());
    assert!(sub.join(".mcp.json").exists());
    assert!(
        !c.0.join(".claude").exists(),
        "Claude Code files leaked into the repository root"
    );
    assert!(
        !c.0.join(".vivac").exists(),
        "a tree was planted above the current directory"
    );
}

/// (b), first half: `.git` as a folder, right at the current directory,
/// gets no warning.
#[test]
fn the_root_of_a_repository_gets_no_warning_with_git_as_a_folder() {
    let c = Sandbox::new_empty("setup-two-roots-git-folder");
    create_git_dir(&c.0);
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("is inside the repository at"), "{out}");
}

/// (b), second half: `.git` as a worktree's file gets no warning either.
#[test]
fn the_root_of_a_repository_gets_no_warning_with_git_as_a_worktree_file() {
    let c = Sandbox::new_empty("setup-two-roots-git-file");
    create_git_worktree_file(&c.0);
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("is inside the repository at"), "{out}");
}

/// (c): no `.git` anywhere in the ancestry gets no warning.
#[test]
fn no_git_anywhere_gets_no_warning() {
    let c = Sandbox::new_empty("setup-two-roots-no-git");
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("is inside the repository at"), "{out}");
}

/// (d): with a tree already planted above, and this folder already one of
/// its lanes, Claude Code's files still land in the current directory, and
/// nothing is written into the folder above.
#[test]
fn a_tree_above_keeps_claude_codes_files_in_the_lane_below() {
    let c = Sandbox::new_seeded("setup-two-roots-tree-above");
    let sub = c.0.join("workdir");
    std::fs::create_dir_all(&sub).unwrap();
    // `d723` piece B: `sub` has to be one of the tree's own lanes before
    // `setup` will write into it -- `init --yes` is what declares it one
    // now, the same join `setup` used to do implicitly by itself.
    run_in(&sub, c.global_home(), &["init", "--yes"]);

    let (out, code) = run_in(&sub, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(sub.join(".claude").join("settings.json").exists());
    assert!(sub.join(".mcp.json").exists());
    assert!(
        sub.join(".vivac").join("lane").exists(),
        "the subfolder never joined the tree above as a lane"
    );
    assert!(
        !already_planted(&sub),
        "a second tree was planted below the existing one"
    );
    assert!(
        !c.0.join(".claude").exists(),
        "Claude Code files were written above the current directory"
    );
    assert!(!c.0.join(".mcp.json").exists());
}

/// (f): `--undo` in a subfolder of a tree removes only what `setup` wrote
/// there, and leaves the tree above -- and the lane `init --join` declared
/// for this folder, `d723` piece B moved that far out of `setup --undo`'s
/// own reach -- exactly as they were: `--undo` never touches the log, or
/// the lane, any more.
#[test]
fn undo_in_a_subfolder_removes_only_that_folders_files() {
    let c = Sandbox::new_seeded("setup-two-roots-undo-subfolder");
    let sub = c.0.join("workdir");
    std::fs::create_dir_all(&sub).unwrap();
    run_in(&sub, c.global_home(), &["init", "--yes"]);
    let (setup_out, setup_code) = run_in(&sub, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(setup_code, 0, "{setup_out}");
    assert!(
        sub.join(".claude").join("settings.json").exists(),
        "setup did not write into the subfolder it ran in"
    );
    let config_after_setup = std::fs::read(c.0.join(".vivac").join("config")).unwrap();
    let events_after_setup = std::fs::read(c.0.join(".vivac").join("events")).unwrap();

    let (out, code) = run_in(
        &sub,
        c.global_home(),
        &["setup", "claude-code", "--undo", "--yes"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        sub.join(".claude").exists(),
        "setup must never remove .claude/ itself"
    );
    assert!(!sub.join(".claude").join("settings.json").exists());
    assert!(!sub.join(".mcp.json").exists());
    assert!(
        sub.join(".vivac").join("lane").exists(),
        "setup --undo took the lane, which is init --undo's alone since d723 piece B"
    );
    assert!(
        c.0.join(".vivac").exists(),
        "the tree above was touched by undo"
    );
    // `undo` never touches the log, checked rather than only claimed
    // (`t594`): both `config` and `events` stay exactly
    // as the earlier `setup` left them.
    assert_eq!(
        config_after_setup,
        std::fs::read(c.0.join(".vivac").join("config")).unwrap(),
        "undo changed the tree's config"
    );
    assert_eq!(
        events_after_setup,
        std::fs::read(c.0.join(".vivac").join("events")).unwrap(),
        "undo changed the tree's log"
    );
}

/// (g): a workspace with a repository inside it -- setup at the workspace
/// root and again inside the repository leaves two sets of Claude Code
/// files and a single shared tree.
#[test]
fn a_workspace_and_a_repository_inside_it_share_one_tree_and_get_two_sets_of_files() {
    let c = Sandbox::new_empty("setup-two-roots-workspace");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    assert!(c.0.join(".vivac").exists());

    let repository = c.0.join("service");
    std::fs::create_dir_all(&repository).unwrap();
    create_git_dir(&repository);
    run_in(&repository, c.global_home(), &["init", "--yes"]);

    let (out, code) = run_in(
        &repository,
        c.global_home(),
        &["setup", "claude-code", "--yes"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("is inside the repository at"), "{out}");

    assert!(c.0.join(".claude").join("settings.json").exists());
    assert!(repository.join(".claude").join("settings.json").exists());
    assert!(
        repository.join(".vivac").join("lane").exists(),
        "the repository never joined the shared tree as a lane"
    );
    assert!(
        !already_planted(&repository),
        "a second tree was planted inside the repository"
    );
}

// ---------------------------------------------------------------------------
// t579 §4.1/§9: the user's home folder, and a `.vivac/` that is the global
// store. Both are refused before the plan, and only for `apply`.
// ---------------------------------------------------------------------------

fn run_with_home(dir: &Path, home: &Path, vivac_home: &Path, args: &[&str]) -> (String, i32) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(dir)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("VIVAC_HOME", vivac_home)
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
        out.status.code().unwrap_or(-1),
    )
}

fn home_folder_text(here: &Path) -> String {
    format!(
        "  {} is your home folder. Claude Code's settings and skills here are\n  \
         yours for every project, not this one's, and setup never writes there.\n  \
         Run setup in the folder you open Claude Code in, inside a project.",
        here.display()
    )
}

/// (h), first half: setup in the user's home folder refuses before showing a
/// plan, and writes nothing.
#[test]
fn setup_refuses_in_the_home_folder() {
    let c = Sandbox::new_empty("setup-two-roots-home");
    // `d723` piece B: `setup` refuses with no tree at all before it ever
    // reaches the home-folder guard, so a tree has to be here for that
    // guard to be the one this run actually meets.
    c.ok(&["init", "--yes"]);
    let (out, code) = run_with_home(
        &c.0,
        &c.0,
        c.global_home(),
        &["setup", "claude-code", "--yes"],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&home_folder_text(&printed(&c.0))), "{out}");
    assert!(
        !out.contains("vivac setup claude-code will, in"),
        "a plan was shown:\n{out}"
    );
    assert!(!c.0.join(".claude").exists());
    assert!(!c.0.join(".mcp.json").exists());
}

/// (h), second half: the same refusal holds with `--dry-run`.
#[test]
fn setup_refuses_in_the_home_folder_with_dry_run_too() {
    let c = Sandbox::new_empty("setup-two-roots-home-dry-run");
    c.ok(&["init", "--yes"]);
    let (out, code) = run_with_home(
        &c.0,
        &c.0,
        c.global_home(),
        &["setup", "claude-code", "--dry-run"],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&home_folder_text(&printed(&c.0))), "{out}");
    assert!(!c.0.join(".claude").exists());
}

/// (i): a `.vivac/` that is the global store, found as the tree's own root.
/// `d723` piece B: `find_root` already skips a folder the registry marks as
/// its own, the same way it always has, so `setup` never resolves a tree
/// here at all -- what used to be the registry's own refusal is
/// `Failure::SetupNoTree` now, the same as any other folder with no tree
/// to configure. The protection this guarded -- a project's tree never
/// living inside the registry's own folder -- is still whole: it is
/// `init`'s own `resolve_roots`, unchanged, that a person following
/// `SetupNoTree`'s own advice (`vivac init`) would actually reach.
#[test]
fn setup_refuses_when_the_trees_own_vivac_is_the_global_store() {
    let c = Sandbox::new_empty("setup-two-roots-store-is-tree");
    let store = c.0.join(".vivac");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("projects"), "{}").unwrap();

    let (out, code) = run_with_home(
        &c.0,
        c.global_home(),
        &store,
        &["setup", "claude-code", "--yes"],
    );
    assert_eq!(code, 4, "{out}");
    assert!(
        out.contains("There is no tree here for setup to connect"),
        "{out}"
    );
    assert!(!c.0.join(".claude").exists());
    assert!(!c.0.join(".mcp.json").exists());
}

// ---------------------------------------------------------------------------
// t579 §5.4, §14.7 and §15.6: upgrading from the exact SKILL.md each earlier
// release wrote.
// ---------------------------------------------------------------------------

/// The literal SKILL.md each earlier release wrote: its frontmatter and body
/// (`git show <tag>:src/setup/skill-frontmatter.md` and `skill-body.md`),
/// joined by the marker line with the fingerprint that version's own
/// `fnv1a64` computed over that text. Setup already knows how to replace a
/// copy an earlier vivac wrote; these fixtures are what those copies actually
/// looked like. The ones from v0.11.0 on are byte for byte the copies those
/// releases wrote into real projects.
const EARLIER_RELEASE_SKILLS: [(&str, &str); 7] = [
    ("v0.10.0", include_str!("data/skill-v0.10.0.md")),
    ("v0.11.0", include_str!("data/skill-v0.11.0.md")),
    ("v0.11.1", include_str!("data/skill-v0.11.1.md")),
    ("v0.11.2", include_str!("data/skill-v0.11.2.md")),
    ("v0.15.0", include_str!("data/skill-v0.15.0.md")),
    ("v0.15.2", include_str!("data/skill-v0.15.2.md")),
    ("v0.15.4", include_str!("data/skill-v0.15.4.md")),
];

#[test]
fn every_earlier_release_skill_is_replaced_by_the_new_one() {
    let fresh = Sandbox::new_empty("setup-skill-fresh");
    fresh.ok(&["init", "--yes"]);
    fresh.ok(&["setup", "claude-code", "--yes"]);
    let expected = read(&skill_path(&fresh));

    // An upgrade: setup ran with that release, so everything else is
    // already there and only the skill is behind.
    for (release, old) in EARLIER_RELEASE_SKILLS {
        let c = Sandbox::new_empty(&format!("setup-skill-{release}-upgrade"));
        c.ok(&["init", "--yes"]);
        c.ok(&["setup", "claude-code", "--yes"]);
        std::fs::write(skill_path(&c), old).unwrap();

        let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
        assert_eq!(code, 0, "{release}: {out}");
        assert!(
            plan_words(&out).contains("replace")
                && plan_words(&out).contains("the copy an earlier vivac wrote"),
            "{release}: {out}"
        );
        assert_eq!(read(&skill_path(&c)), expected, "{release}");
        assert_eq!(written_part(&out), SKILL_REPLACED_MESSAGE, "{release}");
    }
}

// ---------------------------------------------------------------------------
// t579 §5.2/§9: the skill only teaches commands that exist. Every bare
// "vivac <word>" and every "vivac_<word>" in its text is either a verb
// `vivac --help` lists or a tool `vivac mcp` serves.
// ---------------------------------------------------------------------------

/// Words that follow a bare "vivac" in the skill's text without naming a
/// command. Each entry is a real sentence read out of the text, not a
/// guess: "vivac never imports anything by itself", "... is another
/// map." and "offer to set vivac up there". Widening this list to let a
/// typo pass is the wrong fix; renaming or rewriting the sentence is the
/// right one.
const NOT_A_COMMAND: &[&str] = &["never", "is", "up"];

/// Every verb `--help` lists on its own line, four spaces in.
fn help_commands(help: &str) -> std::collections::BTreeSet<String> {
    help.lines()
        .filter_map(|l| l.strip_prefix("    vivac "))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

const MCP_HELLO: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
const MCP_INITIALIZE_DONE: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
const MCP_TOOLS_LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

/// The tool names `vivac mcp` actually serves, asked over the wire the same
/// way `tests/mcp.rs` does, so a tool the skill names and the server does
/// not have cannot pass by assumption.
fn mcp_tool_names(c: &Sandbox) -> std::collections::BTreeSet<String> {
    use std::io::{BufRead, BufReader, Write};
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .env("TZ", "UTC")
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    writeln!(stdin, "{MCP_HELLO}").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();

    writeln!(stdin, "{MCP_INITIALIZE_DONE}").unwrap();
    stdin.flush().unwrap();

    writeln!(stdin, "{MCP_TOOLS_LIST}").unwrap();
    stdin.flush().unwrap();
    let mut answer = String::new();
    stdout.read_line(&mut answer).unwrap();

    let response: serde_json::Value = serde_json::from_str(&answer).unwrap();
    let names = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    let _ = child.kill();
    let _ = child.wait();
    names
}

/// Every "vivac <word>" and "vivac_<word>" the text names.
///
/// Split into paragraphs first, so a heading's trailing "vivac" is never
/// paired with the next paragraph's opening word, and only a bare "vivac"
/// -- nothing glued to it, like "vivac's" or the "vivac," of a mid-sentence
/// pause -- is read as naming a command.
fn command_words_in(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for paragraph in text.split("\n\n") {
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if let Some(name) = word.strip_prefix("vivac_") {
                let clean: String = name
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                found.push(format!("vivac_{clean}"));
            } else if *word == "vivac" {
                if let Some(next) = words.get(i + 1) {
                    let clean: String = next
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric())
                        .collect();
                    if !clean.is_empty() {
                        found.push(clean);
                    }
                }
            }
        }
    }
    found
}

#[test]
fn the_skill_only_names_commands_that_exist() {
    let help_sandbox = Sandbox::new_empty("setup-skill-commands-help");
    let commands = help_commands(&help_sandbox.ok(&["--help"]));

    let mcp_sandbox = Sandbox::new_seeded("setup-skill-commands-mcp");
    let tools = mcp_tool_names(&mcp_sandbox);

    let mut total = 0;
    for text in [FRONTMATTER, BODY] {
        for word in command_words_in(text) {
            total += 1;
            if let Some(rest) = word.strip_prefix("vivac_") {
                assert!(
                    tools.contains(&word),
                    "the skill names the tool \"vivac_{rest}\", and vivac mcp serves no such tool"
                );
                continue;
            }
            if NOT_A_COMMAND.contains(&word.as_str()) {
                continue;
            }
            assert!(
                commands.contains(&word),
                "the skill says \"vivac {word}\", and the help lists no such command"
            );
        }
    }
    assert!(
        total >= 10,
        "only {total} command word(s) found: what broke is the parsing, not the skill"
    );
}

/// (j): `--undo` in the home folder is never refused, and removes only the
/// two hooks setup would have written, leaving every other key as it was.
#[test]
fn undo_in_the_home_folder_removes_only_the_hooks_setup_wrote() {
    let c = Sandbox::new_empty("setup-two-roots-undo-home");
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "otherKey": "z",
        "hooks": {
            "SessionStart": [
                { "hooks": [ { "type": "command", "command": SESSION_START } ] }
            ],
            "Stop": [
                { "hooks": [ { "type": "command", "command": SESSION_END } ] }
            ]
        }
    });
    std::fs::write(
        settings_path(&c),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();

    let (out, code) = run_with_home(
        &c.0,
        &c.0,
        c.global_home(),
        &["setup", "claude-code", "--undo", "--yes"],
    );
    assert_eq!(code, 0, "{out}");
    let after: serde_json::Value = serde_json::from_str(&read(&settings_path(&c))).unwrap();
    assert!(after.get("hooks").is_none(), "{after}");
    assert_eq!(after["otherKey"], "z");
}

// ---------------------------------------------------------------------------
// t579 §6, §14.3 and §15.5: the written message, golden, from "Written."
// to the end. It only says what is true of the run that printed it.
// ---------------------------------------------------------------------------

fn written_part(out: &str) -> &str {
    let idx = out
        .find("Written.")
        .unwrap_or_else(|| panic!("no \"Written.\" in the output:\n{out}"));
    &out[idx..]
}

// `d723` piece B: `setup` no longer touches the tree, so this no longer
// carries `tree_paragraph`'s sentence or the migration nudge that used to
// follow it -- both moved to `init` with the rest of the tree's own
// writes. Captured from a real run rather than hand-edited from the
// pre-piece-B text, the same discipline this section always held itself to.
const WRITTEN_MESSAGE: &str = "Written.\n\nOpen a new Claude Code session in this folder. The brief arrives on its own when it starts. If Claude Code asks whether to use the \"vivac\" server from .mcp.json, say yes: it is what lets the agent write to the tree.\n\nThe hooks, the server and the skill are plain files in this project: commit them if everyone who works here uses vivac, and keep them out of version control if only you do. .vivac/ is never committed: it is this machine's record, and a copy of it in every clone would diverge from the others. Its own .gitignore keeps it out.\n\nUndo: vivac setup claude-code --undo\n\nNext: bring in what this project already knows. Ask the agent:\n\n  Use the vivac-migrate skill to bring everything this project knows into vivac.\n\nIt shows you a plan before writing anything, checks what it wrote, and offers to retire the other maps one at a time, only if you say yes.\n\nUntil then, another memory system you use keeps talking to the agent as before, and may tell it to use that system first. That is expected: the skill only reads from it.\n";

#[test]
fn a_fresh_setup_prints_the_written_message_verbatim() {
    let c = Sandbox::new_empty("setup-written-golden");
    c.ok(&["init", "--yes"]);
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert_eq!(written_part(&out), WRITTEN_MESSAGE);
}

// `d723` piece B removed this section's own (b): a folder joining a tree
// above it by merely resolving into it. Resolving into a tree this folder
// is not yet a lane of is `Failure::not_a_lane_yet` now; joining it is
// `init --join`'s alone.

/// (c): only the skill, which is what an upgrade writes.
const SKILL_REPLACED_MESSAGE: &str = "Written.\n\nThe vivac-migrate skill is now the one this version of vivac ships. Sessions opened from now on use it.\n\nThe hooks, the server and the skill are plain files in this project: commit them if everyone who works here uses vivac, and keep them out of version control if only you do. .vivac/ is never committed: it is this machine's record, and a copy of it in every clone would diverge from the others. Its own .gitignore keeps it out.\n\nNext: bring in what this project already knows. Ask the agent:\n\n  Use the vivac-migrate skill to bring everything this project knows into vivac.\n\nIt shows you a plan before writing anything, checks what it wrote, and offers to retire the other maps one at a time, only if you say yes.\n\nUntil then, another memory system you use keeps talking to the agent as before, and may tell it to use that system first. That is expected: the skill only reads from it.\n";

/// (d): only the server. `--undo` would take the hooks and the skill as
/// well, so it is not offered.
///
/// `f638`, `d641`, `t789`: the hand-registered paragraph follows the
/// session paragraph only when the server is the one piece this run adds
/// and the tree already holds work. This tree holds none, so the message
/// carries no such paragraph.
const SERVER_ADDED_MESSAGE: &str = "Written.\n\nOpen a new Claude Code session in this folder. The brief arrives on its own when it starts. If Claude Code asks whether to use the \"vivac\" server from .mcp.json, say yes: it is what lets the agent write to the tree.\n\nThe hooks, the server and the skill are plain files in this project: commit them if everyone who works here uses vivac, and keep them out of version control if only you do. .vivac/ is never committed: it is this machine's record, and a copy of it in every clone would diverge from the others. Its own .gitignore keeps it out.\n\nNext: bring in what this project already knows. Ask the agent:\n\n  Use the vivac-migrate skill to bring everything this project knows into vivac.\n\nIt shows you a plan before writing anything, checks what it wrote, and offers to retire the other maps one at a time, only if you say yes.\n\nUntil then, another memory system you use keeps talking to the agent as before, and may tell it to use that system first. That is expected: the skill only reads from it.\n";

// (e) used to live here: a run that only plants the tree. `d723` piece B
// removed it -- `setup` never plants, so there is no "only the tree" case
// left for it to print a message about.

#[test]
fn a_run_that_adds_only_the_server_offers_no_undo() {
    let c = Sandbox::new_empty("setup-written-server-only");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    std::fs::remove_file(c.0.join(".mcp.json")).unwrap();
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert_eq!(written_part(&out), SERVER_ADDED_MESSAGE);
}

/// `f790`: `setup` on a fresh plant, before anything has ever been
/// captured, ends with the migrate `Next:` block -- `init` itself no
/// longer shows it.
#[test]
fn setup_on_a_fresh_plant_ends_with_the_migrate_next_block() {
    let c = Sandbox::new_empty("setup-migrate-fresh");
    c.ok(&["init", "--yes"]);
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(
        out.contains("Next: bring in what this project already knows"),
        "{out}"
    );
    assert!(
        out.contains("Use the vivac-migrate skill to bring everything this project knows"),
        "{out}"
    );
}

/// The other half: once this lane has captured something of its own, a
/// later `setup` -- run to add a piece that was missing, the server here
/// -- has nothing left to nudge about.
#[test]
fn setup_on_a_lane_that_already_captured_something_ends_with_no_migrate_block() {
    let c = Sandbox::new_empty("setup-migrate-already-captured");
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "Some real work", "--why", "seed"]);
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(
        !out.contains("bring in what this project already knows"),
        "a lane with a capture of its own still got the migrate nudge:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// `f638`, `d641`: the hand-registered-server paragraph.
//
// Before `setup` existed, the README told people to run
// `claude mcp add vivac -- vivac mcp`, which registers the server in Claude
// Code's local scope. `setup claude-code` only ever looks at the project's
// own `.mcp.json`, so for someone who did that, a later `setup` run adds a
// second, unused entry there -- and Claude Code never asks to approve it,
// contradicting the session paragraph's own "say yes" just above it.
//
// `setup` never opens a harness's personal configuration to check for a
// hand-made registration directly: for Claude Code that file also holds the
// sign-in session, and the security pillar vetoes opening it. So this is
// inferred from the project instead, and only when both hold: the tree
// already holds work (`t789`), and this run is the one adding the "vivac"
// server to `.mcp.json`.
// ---------------------------------------------------------------------------

const HAND_REGISTERED_MARKER: &str = "The tree was here before this server was.";

/// The session paragraph's own last line, unique to it: the one text this
/// paragraph is required to follow immediately.
const SESSION_PARAGRAPH_END: &str =
    "from .mcp.json, say yes: it is what lets the agent write to the tree.";

/// Whether `out` carries the hand-registered paragraph exactly once, right
/// after the session paragraph and nowhere else.
fn hand_registered_paragraph_is_right_after_session(out: &str) -> bool {
    if out.matches(HAND_REGISTERED_MARKER).count() != 1 {
        return false;
    }
    let Some(session_end) = out.find(SESSION_PARAGRAPH_END) else {
        return false;
    };
    out[session_end + SESSION_PARAGRAPH_END.len()..]
        .starts_with(&format!("\n\n{HAND_REGISTERED_MARKER}"))
}

#[test]
fn a_tree_planted_before_this_run_with_no_mcp_json_gets_the_hand_registered_paragraph() {
    let c = Sandbox::new_seeded("setup-hand-registered-fresh-mcp");
    c.ok(&["push", "Some real work", "--why", "seed"]);
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(
        hand_registered_paragraph_is_right_after_session(&out),
        "{out}"
    );
}

#[test]
fn a_tree_planted_before_this_run_with_a_different_mcp_server_gets_the_hand_registered_paragraph() {
    let c = Sandbox::new_seeded("setup-hand-registered-other-server");
    c.ok(&["push", "Some real work", "--why", "seed"]);
    std::fs::write(
        mcp_path(&c),
        "{\n  \"mcpServers\": {\n    \"other\": {\n      \"type\": \"stdio\",\n      \
         \"command\": \"other-tool\"\n    }\n  }\n}\n",
    )
    .unwrap();
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(
        hand_registered_paragraph_is_right_after_session(&out),
        "{out}"
    );
}

/// `t789`: since `init` plants on its own, every tree predates the server
/// `setup` adds, so that alone no longer tells a hand registration apart
/// from a fresh plant. A hand registration is something done to a tree in
/// use, so a tree `init` just planted, with no work in it yet, does not get
/// the paragraph: it would only warn a newcomer about something they never
/// did.
#[test]
fn a_tree_init_just_planted_does_not_get_the_hand_registered_paragraph() {
    let c = Sandbox::new_empty("setup-hand-registered-fresh-plant");
    c.ok(&["init", "--yes"]);
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(!out.contains(HAND_REGISTERED_MARKER), "{out}");
}

#[test]
fn a_tree_that_already_has_the_vivac_server_never_gets_the_hand_registered_paragraph() {
    let c = Sandbox::new_seeded("setup-hand-registered-server-present");
    std::fs::write(
        mcp_path(&c),
        "{\n  \"mcpServers\": {\n    \"vivac\": {\n      \"type\": \"stdio\",\n      \
         \"command\": \"vivac\",\n      \"args\": [\"mcp\"]\n    }\n  }\n}\n",
    )
    .unwrap();
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(!out.contains(HAND_REGISTERED_MARKER), "{out}");
}

#[test]
fn dry_run_never_prints_the_hand_registered_paragraph() {
    let c = Sandbox::new_seeded("setup-hand-registered-dry-run");
    let (out, code) = c.run(&["setup", "claude-code", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains(HAND_REGISTERED_MARKER), "{out}");
}

// ---------------------------------------------------------------------------
// `t594`: the tracked-log warning, in all three runs.
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

fn track_the_log(c: &Sandbox) {
    git(&c.0, &["init", "-q"]);
    git(&c.0, &["add", "-f", ".vivac/events"]);
    git(&c.0, &["commit", "-q", "-m", "track the log by mistake"]);
}

/// The warning used to sit after `--dry-run`'s own early exit, so a plan
/// never carried it. `d723` piece B: showing it is `init`'s job now, since
/// `setup` no longer builds a plan of the tree side at all.
#[test]
fn dry_run_warns_about_a_tracked_log() {
    let c = Sandbox::new_seeded("tracked-dry-run");
    track_the_log(&c);
    let (out, code) = c.run(&["init", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(
        out.contains(".vivac/events is tracked by git here"),
        "{out}"
    );
}

/// And it used to sit after "nothing to write" as well, so whoever was
/// already set up -- the one person who never reaches a run that writes
/// something -- never saw it at all. `d723` piece B: `init`'s own.
#[test]
fn an_already_set_up_project_still_warns_about_a_tracked_log() {
    let c = Sandbox::new_seeded("tracked-nothing-to-write");
    // Before the first `init --yes`, not after: tracking the log with `git
    // init` also turns this folder into a repository of its own, and a
    // repository appearing *between* two runs is a real change for the
    // lane to redeclare, not nothing to write.
    track_the_log(&c);
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["init", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Nothing to write: this project is already set up."),
        "{out}"
    );
    assert!(
        out.contains(".vivac/events is tracked by git here"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// A planted tree never shows up in `git status`: `.vivac/.gitignore` is the
// whole promise, checked against real git rather than assumed.
// ---------------------------------------------------------------------------

/// A repository that already has something to report keeps reporting it,
/// and nothing about the tree planted beside it joins that report.
#[test]
fn git_status_shows_nothing_of_a_planted_tree() {
    let c = Sandbox::new_empty("git-status-hides-vivac");
    git(&c.0, &["init", "-q"]);
    std::fs::write(c.0.join("tracked.txt"), "kept").unwrap();
    git(&c.0, &["add", "tracked.txt"]);
    git(
        &c.0,
        &["commit", "-q", "-m", "a file the repository already has"],
    );
    std::fs::write(c.0.join("untracked.txt"), "new").unwrap();

    c.ok(&["init", "--yes"]);

    let out = std::process::Command::new("git")
        .current_dir(&c.0)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(out.status.success(), "git status failed to run");
    let status = String::from_utf8_lossy(&out.stdout);
    assert!(
        status.contains("untracked.txt"),
        "git status stopped reporting a real change: {status}"
    );
    assert!(
        !status.contains(".vivac"),
        "git status mentioned the planted tree: {status}"
    );
}

// ---------------------------------------------------------------------------
// `d723` piece B split four of the original `t594`/`t640` tests in two:
// each asserted a single `setup` run doing both the tree's own write and
// the harness's, which no longer happens in one command. Neither half was
// dropped; each is covered by name, one line per retired test:
//
//   join_yes_in_a_clean_folder_writes_the_harness_and_the_lane
//     tree half: tests/init.rs::init_join_declares_a_lane_of_the_target_rather_than_planting
//     harness half: a_fresh_project_gets_the_three_harness_pieces (above)
//   a_new_folder_under_a_tree_is_told_the_tree_was_already_there
//     tree half: tests/init.rs::init_join_declares_a_lane_of_the_target_rather_than_planting
//     harness half: a_fresh_setup_prints_the_written_message_verbatim (above)
//   a_run_that_plants_only_the_tree_invites_a_migration
//     tree half: tests/init.rs::planting_a_fresh_tree_still_carries_the_plants_own_migrate_paragraph
//     harness half: a_fresh_setup_prints_the_written_message_verbatim (above)
//   undo_removes_an_unwritten_lane_file_even_when_nothing_else_is_left
//     covered whole by the three `d680` tests tests/init.rs already ports:
//     undo_after_a_join_that_wrote_nothing_removes_the_lane_file_and_unblocks_relocate,
//     undo_after_a_join_that_wrote_something_keeps_the_lane_file,
//     undo_leaves_a_vivac_dir_whose_gitignore_was_hand_edited
// ---------------------------------------------------------------------------
