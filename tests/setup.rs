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
        after.contains("# Bringing another memory into vivac"),
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
