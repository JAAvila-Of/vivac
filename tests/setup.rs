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

#[test]
fn a_fresh_project_gets_all_four_pieces() {
    let c = Sandbox::new_empty("setup-fresh");
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("vivac setup claude-code, in"), "{out}");
    assert!(out.contains("plant the tree"), "{out}");
    assert!(out.contains("create, with two hooks"), "{out}");
    assert!(out.contains(SESSION_START), "{out}");
    assert!(out.contains(SESSION_END), "{out}");
    assert!(out.contains("create, with the server \"vivac\""), "{out}");
    assert!(out.contains("create: how an agent brings"), "{out}");
    assert!(out.contains("Written."), "{out}");
    assert!(
        out.contains("Undo:  vivac setup claude-code --undo"),
        "{out}"
    );

    assert!(c.0.join(".vivac").exists(), "the tree was not planted");

    let settings: serde_json::Value = serde_json::from_str(&read(&settings_path(&c))).unwrap();
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        SESSION_START
    );
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        SESSION_END
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
    assert!(after.contains("\r\n"), "CRLF was lost:\n{after:?}");
}

// ---------------------------------------------------------------------------
// 4. The same file, indented at four spaces.
// ---------------------------------------------------------------------------

#[test]
fn indentation_of_an_existing_file_is_kept() {
    let c = Sandbox::new_empty("setup-indent");
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
// 6 and 7. The MCP server, taken and free.
// ---------------------------------------------------------------------------

#[test]
fn a_taken_vivac_server_name_is_a_conflict_and_nothing_is_written() {
    let c = Sandbox::new_empty("setup-mcp-taken");
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
        !c.0.join(".vivac").exists(),
        "the tree was planted despite the conflict"
    );
    assert!(
        !settings_path(&c).exists(),
        "settings.json was written despite the conflict"
    );
}

#[test]
fn another_name_already_running_vivac_mcp_is_left_as_it_is() {
    let c = Sandbox::new_empty("setup-mcp-other-name");
    let mcp = serde_json::json!({
        "mcpServers": { "vivac-tree": { "command": "vivac", "args": ["mcp"] } }
    });
    std::fs::write(mcp_path(&c), serde_json::to_string_pretty(&mcp).unwrap()).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("already runs vivac mcp as \"vivac-tree\""),
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
        out.contains("replace the copy an earlier vivac wrote"),
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
    assert!(!c.0.join(".vivac").exists());
}

#[test]
fn a_json_root_that_is_not_an_object_is_a_conflict() {
    let c = Sandbox::new_empty("setup-not-object");
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
    let before: Vec<_> = std::fs::read_dir(&c.0).unwrap().collect();
    let (out, code) = c.run(&["setup", "claude-code", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    let after: Vec<_> = std::fs::read_dir(&c.0).unwrap().collect();
    assert_eq!(before.len(), after.len(), "dry-run created something");
    assert!(!c.0.join(".vivac").exists());
}

// ---------------------------------------------------------------------------
// 11. No terminal and no `--yes`.
// ---------------------------------------------------------------------------

#[test]
fn no_terminal_and_no_yes_refuses_without_a_plan() {
    let c = Sandbox::new_empty("setup-no-terminal");
    let (out, code) = c.run(&["setup", "claude-code"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("there is no terminal here to ask"), "{out}");
    assert!(out.contains("vivac setup claude-code --dry-run"), "{out}");
    assert!(out.contains("vivac setup claude-code --yes"), "{out}");
    assert!(
        !out.contains("vivac setup claude-code, in"),
        "a plan was shown:\n{out}"
    );
    assert!(!c.0.join(".vivac").exists());
}

// ---------------------------------------------------------------------------
// 12 and 13. `--undo`.
// ---------------------------------------------------------------------------

#[test]
fn undo_after_a_fresh_setup_leaves_only_the_tree() {
    let c = Sandbox::new_empty("setup-undo-fresh");
    c.ok(&["setup", "claude-code", "--yes"]);
    let vivac_before = std::fs::read(c.0.join(".vivac").join("config")).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Undone. The tree in .vivac/ is untouched."),
        "{out}"
    );
    assert!(!c.0.join(".claude").exists(), "{:?}", list(&c.0));
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
        out.contains("changed since setup wrote it; left as it is"),
        "{out}"
    );
    assert!(
        read(&settings_path(&c)).contains("C:/tools/vivac.exe session start --hook"),
        "the differently-spelled hook was removed"
    );
    assert!(skill_path(&c).exists(), "the edited skill was removed");
}

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

// ---------------------------------------------------------------------------
// 20. The registry's own folder.
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
        .args(["setup", "claude-code", "--yes"])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains("holds the registry of the trees on this machine"),
        "{text}"
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

    let expected = format!(
        "{FRONTMATTER}<!-- written by vivac setup; fingerprint {claimed:016x}; vivac setup \
         claude-code --undo removes it while the text is unchanged -->\n{BODY}"
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

// ---------------------------------------------------------------------------
// Coordinator's review: C, D, E, F.
// ---------------------------------------------------------------------------

/// C: an existing `settings.json` with neither hook says "add", not
/// "create" -- "create" is only for a file that does not exist yet.
#[test]
fn an_existing_settings_file_with_no_hooks_says_add_not_create() {
    let c = Sandbox::new_empty("setup-add-not-create-settings");
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    std::fs::write(settings_path(&c), "{\n  \"otherKey\": 1\n}\n").unwrap();

    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(out.contains("add two hooks"), "{out}");
    assert!(!out.contains("create, with two hooks"), "{out}");
}

/// C: the same for `.mcp.json` with no server of ours.
#[test]
fn an_existing_mcp_file_with_no_server_says_add_not_create() {
    let c = Sandbox::new_empty("setup-add-not-create-mcp");
    std::fs::write(mcp_path(&c), "{\n  \"mcpServers\": {}\n}\n").unwrap();

    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(out.contains("add the server \"vivac\""), "{out}");
    assert!(!out.contains("create, with the server"), "{out}");
}

/// D: `--undo --dry-run` shows the plan and writes nothing, without asking.
#[test]
fn undo_dry_run_shows_the_plan_and_writes_nothing() {
    let c = Sandbox::new_empty("setup-undo-dry-run");
    c.ok(&["setup", "claude-code", "--yes"]);
    let before = read_bytes(&settings_path(&c));

    let (out, code) = c.run(&["setup", "claude-code", "--undo", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("vivac setup claude-code --undo, in"), "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(!out.contains("Undo it?"), "{out}");
    assert_eq!(before, read_bytes(&settings_path(&c)));
    assert!(c.0.join(".vivac").exists());
}

/// E: when `.mcp.json` will be removed because it holds nothing else, the
/// undo plan wraps it the same two-line way `settings.json` already does.
#[test]
fn undo_plan_wraps_the_mcp_removal_when_the_file_would_empty_out() {
    let c = Sandbox::new_empty("setup-undo-mcp-wrap");
    c.ok(&["setup", "claude-code", "--yes"]);

    let (out, _) = c.run(&["setup", "claude-code", "--undo", "--dry-run"]);
    assert!(out.contains("remove the server \"vivac\";"), "{out}");
    assert!(out.contains("nothing else is left, so it goes"), "{out}");
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

/// (a): a subfolder of a repository with no tree of its own gets Claude
/// Code's files and a new tree right there, and the plan warns with the
/// repository's root.
#[test]
fn a_subfolder_of_a_repository_gets_its_own_files_and_tree_with_a_warning() {
    let c = Sandbox::new_empty("setup-two-roots-subfolder");
    create_git_dir(&c.0);
    let sub = c.0.join("packages").join("app");
    std::fs::create_dir_all(&sub).unwrap();

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
        sub.join(".vivac").exists(),
        "the tree was not planted in the subfolder"
    );
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
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("is inside the repository at"), "{out}");
}

/// (b), second half: `.git` as a worktree's file gets no warning either.
#[test]
fn the_root_of_a_repository_gets_no_warning_with_git_as_a_worktree_file() {
    let c = Sandbox::new_empty("setup-two-roots-git-file");
    create_git_worktree_file(&c.0);
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("is inside the repository at"), "{out}");
}

/// (c): no `.git` anywhere in the ancestry gets no warning.
#[test]
fn no_git_anywhere_gets_no_warning() {
    let c = Sandbox::new_empty("setup-two-roots-no-git");
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("is inside the repository at"), "{out}");
}

/// (d): with a tree already planted above, Claude Code's files still land
/// in the current directory, the tree row names where the tree actually is,
/// and nothing is written into the folder above.
#[test]
fn a_tree_above_keeps_claude_codes_files_below_and_names_the_tree_root() {
    let c = Sandbox::new_seeded("setup-two-roots-tree-above");
    let sub = c.0.join("workdir");
    std::fs::create_dir_all(&sub).unwrap();

    let (out, code) = run_in(&sub, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(&format!("already there, in {}", printed(&c.0).display())),
        "{out}"
    );
    assert!(sub.join(".claude").join("settings.json").exists());
    assert!(sub.join(".mcp.json").exists());
    assert!(
        !sub.join(".vivac").exists(),
        "a second tree was planted below the existing one"
    );
    assert!(
        !c.0.join(".claude").exists(),
        "Claude Code files were written above the current directory"
    );
    assert!(!c.0.join(".mcp.json").exists());
}

/// (f): `--undo` in a subfolder of a tree removes only what was written
/// there, and leaves the tree above untouched.
#[test]
fn undo_in_a_subfolder_removes_only_that_folders_files() {
    let c = Sandbox::new_seeded("setup-two-roots-undo-subfolder");
    let vivac_before = std::fs::read(c.0.join(".vivac").join("config")).unwrap();
    let sub = c.0.join("workdir");
    std::fs::create_dir_all(&sub).unwrap();
    let (setup_out, setup_code) = run_in(&sub, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(setup_code, 0, "{setup_out}");
    assert!(
        sub.join(".claude").join("settings.json").exists(),
        "setup did not write into the subfolder it ran in"
    );

    let (out, code) = run_in(
        &sub,
        c.global_home(),
        &["setup", "claude-code", "--undo", "--yes"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(!sub.join(".claude").exists());
    assert!(!sub.join(".mcp.json").exists());
    assert!(
        c.0.join(".vivac").exists(),
        "the tree above was touched by undo"
    );
    assert_eq!(
        vivac_before,
        std::fs::read(c.0.join(".vivac").join("config")).unwrap()
    );
}

/// (g): a workspace with a repository inside it -- setup at the workspace
/// root and again inside the repository leaves two sets of Claude Code
/// files and a single shared tree.
#[test]
fn a_workspace_and_a_repository_inside_it_share_one_tree_and_get_two_sets_of_files() {
    let c = Sandbox::new_empty("setup-two-roots-workspace");
    c.ok(&["setup", "claude-code", "--yes"]);
    assert!(c.0.join(".vivac").exists());

    let repository = c.0.join("service");
    std::fs::create_dir_all(&repository).unwrap();
    create_git_dir(&repository);

    let (out, code) = run_in(
        &repository,
        c.global_home(),
        &["setup", "claude-code", "--yes"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("is inside the repository at"), "{out}");
    assert!(
        out.contains(&format!("already there, in {}", printed(&c.0).display())),
        "{out}"
    );

    assert!(c.0.join(".claude").join("settings.json").exists());
    assert!(repository.join(".claude").join("settings.json").exists());
    assert!(
        !repository.join(".vivac").exists(),
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
    let (out, code) = run_with_home(
        &c.0,
        &c.0,
        c.global_home(),
        &["setup", "claude-code", "--yes"],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&home_folder_text(&printed(&c.0))), "{out}");
    assert!(
        !out.contains("vivac setup claude-code, in"),
        "a plan was shown:\n{out}"
    );
    assert!(!c.0.join(".vivac").exists());
    assert!(!c.0.join(".claude").exists());
    assert!(!c.0.join(".mcp.json").exists());
}

/// (h), second half: the same refusal holds with `--dry-run`.
#[test]
fn setup_refuses_in_the_home_folder_with_dry_run_too() {
    let c = Sandbox::new_empty("setup-two-roots-home-dry-run");
    let (out, code) = run_with_home(
        &c.0,
        &c.0,
        c.global_home(),
        &["setup", "claude-code", "--dry-run"],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&home_folder_text(&printed(&c.0))), "{out}");
    assert!(!c.0.join(".vivac").exists());
    assert!(!c.0.join(".claude").exists());
}

/// (i): a `.vivac/` that is the global store, found as the tree's own root,
/// refuses with the registry's text and that `.vivac/`'s own path.
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
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(&format!(
            "{} holds the registry of the trees on this machine, so it cannot",
            printed(&store).display()
        )),
        "{out}"
    );
    assert!(out.contains("Run setup inside a project."), "{out}");
    assert!(!c.0.join(".claude").exists());
    assert!(!c.0.join(".mcp.json").exists());
}

// ---------------------------------------------------------------------------
// t579 §5.4: upgrading from the exact SKILL.md v0.10.0 wrote.
// ---------------------------------------------------------------------------

/// The literal SKILL.md v0.10.0 wrote: its frontmatter and body
/// (`git show v0.10.0:src/setup/skill-frontmatter.md` and
/// `skill-body.md`), joined by the marker line with the fingerprint that
/// version's own `fnv1a64` computed over that text. Setup already knows how
/// to replace a copy an earlier vivac wrote; this fixture is what that copy
/// actually looked like.
const OLD_RELEASE_SKILL: &str = include_str!("data/skill-v0.10.0.md");

#[test]
fn an_old_release_skill_is_replaced_by_the_new_one() {
    let fresh = Sandbox::new_empty("setup-skill-old-release-fresh");
    fresh.ok(&["setup", "claude-code", "--yes"]);
    let expected = read(&skill_path(&fresh));

    let c = Sandbox::new_empty("setup-skill-old-release-upgrade");
    std::fs::create_dir_all(skill_path(&c).parent().unwrap()).unwrap();
    std::fs::write(skill_path(&c), OLD_RELEASE_SKILL).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("replace the copy an earlier vivac wrote"),
        "{out}"
    );
    assert_eq!(read(&skill_path(&c)), expected);
}

// ---------------------------------------------------------------------------
// t579 §5.2/§9: the skill only teaches commands that exist. Every bare
// "vivac <word>" and every "vivac_<word>" in its text is either a verb
// `vivac --help` lists or a tool `vivac mcp` serves.
// ---------------------------------------------------------------------------

/// Words that follow a bare "vivac" in the skill's text without naming a
/// command. Each entry is a real sentence read out of the text, not a
/// guess: "vivac never imports anything by itself" and "... is another
/// map." Widening this list to let a typo pass is the wrong fix; renaming
/// or rewriting the sentence is the right one.
const NOT_A_COMMAND: &[&str] = &["never", "is"];

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
