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

/// Whether `dir` holds a tree of its own: `store::already_planted`'s own
/// definition, `config` **or** `events`, copied here rather than asked of
/// the crate because an integration test has no `pub` path to it. `t594`
/// fix-1, finding 4: three tests used to check `config` alone, which a
/// regression that planted only an `events` file would have slipped
/// straight past.
fn already_planted(dir: &Path) -> bool {
    dir.join(".vivac").join("config").is_file() || dir.join(".vivac").join("events").is_file()
}

/// The `id` a lane's own `.vivac/lane` names, the same field
/// `tests/lanes.rs`'s own `lane_id_of` reads.
fn lane_id_of(lane_dir: &Path) -> String {
    let text =
        std::fs::read_to_string(lane_dir.join(".vivac").join("lane")).expect("the lane file reads");
    let v: serde_json::Value = serde_json::from_str(&text).expect("the lane file parses");
    v["id"]
        .as_str()
        .expect("a lane file names an id")
        .to_string()
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
// `t594` §4.5.2, §6.1: the lane plan lines, for a folder that becomes a
// brand new lane of the tree above it.
// ---------------------------------------------------------------------------

fn lane_line_containing(out: &str, label: &str, rest: &str) -> bool {
    out.lines()
        .any(|l| l.trim_start().starts_with(label) && l.contains(rest))
}

#[test]
fn the_plan_for_a_new_lane_shows_the_three_lines_the_spec_gives() {
    let c = Sandbox::new_seeded("setup-lane-plan-lines");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();

    let (out, code) = run_in(&second, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");

    assert!(
        lane_line_containing(
            &out,
            ".vivac/lane",
            "create: this folder becomes lane \"v2\" of the tree above",
        ),
        "{out}"
    );
    assert!(
        lane_line_containing(
            &out,
            ".vivac/.gitignore",
            "create: keeps .vivac/ out of version control",
        ),
        "{out}"
    );
    assert!(
        lane_line_containing(
            &out,
            "config",
            "lock: from now on this tree needs vivac 0.12 or newer",
        ),
        "{out}"
    );
}

/// The config warning is part of the *plan*, not of what gets printed
/// after writing: `--dry-run` never writes anything, and it still shows
/// the line, exactly where the plan showed it above.
#[test]
fn the_lane_config_warning_shows_up_before_anything_is_written() {
    let c = Sandbox::new_seeded("setup-lane-plan-dry-run");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();

    let (out, code) = run_in(
        &second,
        c.global_home(),
        &["setup", "claude-code", "--dry-run"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(
        lane_line_containing(
            &out,
            "config",
            "lock: from now on this tree needs vivac 0.12 or newer",
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
    let (out, code) = run_in(
        &second,
        c.global_home(),
        &["setup", "claude-code", "--dry-run"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(
        !c.0.join(".vivac").join("config").exists(),
        "--dry-run regenerated the tree's config:\n{out}"
    );
}

/// `t594` fix-2, finding 2: an already-set-up project runs into
/// `nothing_to_write` before it ever reaches `--dry-run`'s own check, and
/// that branch notes the machine's registry (`note_registry`) -- a write
/// `--dry-run` must never make, in the registry or anywhere else. The
/// registry is deleted first, so its own directory reappearing is exactly
/// the write this catches.
#[test]
fn dry_run_never_writes_the_machine_registry_either() {
    let c = Sandbox::new_empty("setup-dry-run-no-registry");
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
// 23. `.vivac/.gitignore` (`t594` §4.9).
// ---------------------------------------------------------------------------

#[test]
fn setup_writes_the_gitignore_a_tree_from_before_lacks() {
    let c = Sandbox::new_seeded("setup-gitignore");
    std::fs::remove_file(c.0.join(".vivac").join(".gitignore")).unwrap();
    let plan = c.ok(&["setup", "claude-code", "--dry-run"]);
    assert!(
        plan.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with(".vivac/.gitignore")
                && l.contains("create: keeps .vivac/ out of version control")
        }),
        "{plan}"
    );
    c.ok(&["setup", "claude-code", "--yes"]);
    let g = std::fs::read_to_string(c.0.join(".vivac").join(".gitignore")).unwrap();
    assert_eq!(g, "*\n");
}

/// `t594` fix-3, finding 1: a project already fully set up, and already
/// declared as a lane, whose tree still predates `t594` §4.9 -- so it
/// never got its own `.vivac/.gitignore` -- creates that file on the very
/// next `setup`, and the closing message has to say so, instead of
/// claiming the tree changed nothing two lines under the plan line that
/// names this very write.
#[test]
fn setup_says_it_created_the_trees_gitignore_instead_of_claiming_nothing_changed() {
    let c = Sandbox::new_empty("setup-gitignore-message");
    c.ok(&["setup", "claude-code", "--yes"]);
    std::fs::remove_file(c.0.join(".vivac").join(".gitignore")).unwrap();

    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(
        out.contains("setup wrote in it: its own .gitignore."),
        "{out}"
    );
    assert!(!out.contains("setup changed nothing in it"), "{out}");
}

#[test]
fn setup_no_longer_says_to_commit_the_tree() {
    let c = Sandbox::new_empty("setup-files");
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

/// (f): `--undo` in a subfolder of a tree removes only what was written
/// there, and leaves the tree above exactly as the earlier `setup`
/// (which joined it as a lane, `t594` §4.5) left it: `--undo` never
/// touches the log, so it has nothing to say about the lane that call
/// already declared.
#[test]
fn undo_in_a_subfolder_removes_only_that_folders_files() {
    let c = Sandbox::new_seeded("setup-two-roots-undo-subfolder");
    let sub = c.0.join("workdir");
    std::fs::create_dir_all(&sub).unwrap();
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
    assert!(!sub.join(".claude").exists());
    assert!(!sub.join(".mcp.json").exists());
    assert!(
        sub.join(".vivac").join("lane").exists(),
        "undo removed the lane file, which is not one of the four pieces it undoes"
    );
    assert!(
        c.0.join(".vivac").exists(),
        "the tree above was touched by undo"
    );
    // `undo` never touches the log, checked rather than only claimed
    // (`t594` fix-1, finding 8): both `config` and `events` stay exactly
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
const EARLIER_RELEASE_SKILLS: [(&str, &str); 4] = [
    ("v0.10.0", include_str!("data/skill-v0.10.0.md")),
    ("v0.11.0", include_str!("data/skill-v0.11.0.md")),
    ("v0.11.1", include_str!("data/skill-v0.11.1.md")),
    ("v0.11.2", include_str!("data/skill-v0.11.2.md")),
];

#[test]
fn every_earlier_release_skill_is_replaced_by_the_new_one() {
    let fresh = Sandbox::new_empty("setup-skill-fresh");
    fresh.ok(&["setup", "claude-code", "--yes"]);
    let expected = read(&skill_path(&fresh));

    // An upgrade: setup ran with that release, so everything else is
    // already there and only the skill is behind.
    for (release, old) in EARLIER_RELEASE_SKILLS {
        let c = Sandbox::new_empty(&format!("setup-skill-{release}-upgrade"));
        c.ok(&["setup", "claude-code", "--yes"]);
        std::fs::write(skill_path(&c), old).unwrap();

        let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
        assert_eq!(code, 0, "{release}: {out}");
        assert!(
            out.contains("replace the copy an earlier vivac wrote"),
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
// t579 §6, §14.3 and §15.5: the written message, golden, from "  Written."
// to the end. It only says what is true of the run that printed it.
// ---------------------------------------------------------------------------

fn written_part(out: &str) -> &str {
    let idx = out
        .find("  Written.")
        .unwrap_or_else(|| panic!("no \"Written.\" in the output:\n{out}"));
    &out[idx..]
}

const WRITTEN_MESSAGE: &str = "  Written.\n\n  Open a new Claude Code session in this folder. The brief arrives on its\n  own when it starts. If Claude Code asks whether to use the \"vivac\" server\n  from .mcp.json, say yes: it is what lets the agent write to the tree.\n\n  Nothing has been brought in from anywhere yet. To bring in what this\n  project already knows, from another memory system, the harness's own\n  memory, instruction files or its documents, ask the agent:\n\n      Use the vivac-migrate skill to bring everything this project knows\n      into vivac.\n\n  It shows you a plan before writing anything, checks what it wrote, and\n  offers to retire the other maps one at a time, only if you say yes.\n\n  Until then, another memory system you use keeps talking to the agent as\n  before, and may tell it to use that system first. That is expected: the\n  skill only reads from it.\n\n  The hooks, the server and the skill are plain files in this project:\n  commit them if everyone who works here uses vivac, and keep them out of\n  version control if only you do. .vivac/ is never committed: it is this\n  machine's record, and a copy of it in every clone would diverge from the\n  others. Its own .gitignore keeps it out.\n\n  Undo:  vivac setup claude-code --undo\n";

#[test]
fn a_fresh_setup_prints_the_written_message_verbatim() {
    let c = Sandbox::new_empty("setup-written-golden");
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert_eq!(written_part(&out), WRITTEN_MESSAGE);
}

/// §15.5 (b): a folder of its own under a tree that was already there, which
/// is what step 6 of the skill offers in a workspace. It joins that tree as
/// a new lane (`t594`), which also closes the tree's lanes lock in the same
/// write -- a fresh tree's config starts at `1` -- so this run did change
/// the tree in two ways at once, unlike a folder that merely gains the
/// missing Claude Code pieces over an unrelated part of it
/// (`SKILL_REPLACED_MESSAGE`, `SERVER_ADDED_MESSAGE`, below), which do not.
const NEW_FOLDER_MESSAGE: &str = "  Written.\n\n  Open a new Claude Code session in this folder. The brief arrives on its\n  own when it starts. If Claude Code asks whether to use the \"vivac\" server\n  from .mcp.json, say yes: it is what lets the agent write to the tree.\n\n  The tree was already there, and setup wrote in it: this folder's own\n  thread and the sentence that stops an older vivac from reading it.\n\n  The hooks, the server and the skill are plain files in this project:\n  commit them if everyone who works here uses vivac, and keep them out of\n  version control if only you do. .vivac/ is never committed: it is this\n  machine's record, and a copy of it in every clone would diverge from the\n  others. Its own .gitignore keeps it out.\n\n  Undo:  vivac setup claude-code --undo\n";

/// §15.5 (c): only the skill, which is what an upgrade writes.
const SKILL_REPLACED_MESSAGE: &str = "  Written.\n\n  The vivac-migrate skill is now the one this version of vivac ships.\n  Sessions opened from now on use it.\n\n  The tree was already there, and setup changed nothing in it.\n\n  The hooks, the server and the skill are plain files in this project:\n  commit them if everyone who works here uses vivac, and keep them out of\n  version control if only you do. .vivac/ is never committed: it is this\n  machine's record, and a copy of it in every clone would diverge from the\n  others. Its own .gitignore keeps it out.\n";

/// §15.5 (d): only the server. `--undo` would take the hooks and the skill
/// as well, so it is not offered.
const SERVER_ADDED_MESSAGE: &str = "  Written.\n\n  Open a new Claude Code session in this folder. The brief arrives on its\n  own when it starts. If Claude Code asks whether to use the \"vivac\" server\n  from .mcp.json, say yes: it is what lets the agent write to the tree.\n\n  The tree was already there, and setup changed nothing in it.\n\n  The hooks, the server and the skill are plain files in this project:\n  commit them if everyone who works here uses vivac, and keep them out of\n  version control if only you do. .vivac/ is never committed: it is this\n  machine's record, and a copy of it in every clone would diverge from the\n  others. Its own .gitignore keeps it out.\n";

/// §15.5 (e): only the tree. Nothing the harness reads changed, so there is
/// no session to open and nothing to undo.
const TREE_PLANTED_MESSAGE: &str = "  Written.\n\n  Nothing has been brought in from anywhere yet. To bring in what this\n  project already knows, from another memory system, the harness's own\n  memory, instruction files or its documents, ask the agent:\n\n      Use the vivac-migrate skill to bring everything this project knows\n      into vivac.\n\n  It shows you a plan before writing anything, checks what it wrote, and\n  offers to retire the other maps one at a time, only if you say yes.\n\n  Until then, another memory system you use keeps talking to the agent as\n  before, and may tell it to use that system first. That is expected: the\n  skill only reads from it.\n\n  The hooks, the server and the skill are plain files in this project:\n  commit them if everyone who works here uses vivac, and keep them out of\n  version control if only you do. .vivac/ is never committed: it is this\n  machine's record, and a copy of it in every clone would diverge from the\n  others. Its own .gitignore keeps it out.\n";

#[test]
fn a_new_folder_under_a_tree_is_told_the_tree_was_already_there() {
    let c = Sandbox::new_seeded("setup-written-new-folder");
    let sub = c.0.join("workdir");
    std::fs::create_dir_all(&sub).unwrap();
    let (out, code) = run_in(&sub, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(written_part(&out), NEW_FOLDER_MESSAGE);
}

#[test]
fn a_run_that_adds_only_the_server_offers_no_undo() {
    let c = Sandbox::new_empty("setup-written-server-only");
    c.ok(&["setup", "claude-code", "--yes"]);
    std::fs::remove_file(c.0.join(".mcp.json")).unwrap();
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert_eq!(written_part(&out), SERVER_ADDED_MESSAGE);
}

#[test]
fn a_run_that_plants_only_the_tree_invites_a_migration() {
    let c = Sandbox::new_empty("setup-written-tree-only");
    c.ok(&["setup", "claude-code", "--yes"]);
    std::fs::remove_dir_all(c.0.join(".vivac")).unwrap();
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert_eq!(written_part(&out), TREE_PLANTED_MESSAGE);
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
/// never carried it.
#[test]
fn dry_run_warns_about_a_tracked_log() {
    let c = Sandbox::new_seeded("tracked-dry-run");
    track_the_log(&c);
    let (out, code) = c.run(&["setup", "claude-code", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(
        out.contains(".vivac/events is tracked by git here"),
        "{out}"
    );
}

/// And it used to sit after "nothing to write" as well, so whoever was
/// already set up -- the one person who never reaches a run that writes
/// something -- never saw it at all.
#[test]
fn an_already_set_up_project_still_warns_about_a_tracked_log() {
    let c = Sandbox::new_seeded("tracked-nothing-to-write");
    // Before the first `setup`, not after: tracking the log with `git
    // init` also turns this folder into a repository of its own, and a
    // repository appearing *between* two runs is a real change for the
    // lane to redeclare, not nothing to write.
    track_the_log(&c);
    c.ok(&["setup", "claude-code", "--yes"]);
    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
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
// `t594` §4.5, case 3: `setup` refuses to give a product a second map, in a
// folder with no tree above it at all.
// ---------------------------------------------------------------------------

fn real_git_repo(at: &Path) {
    std::fs::create_dir_all(at).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(at)
            .args(args)
            .output()
            .unwrap();
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(at.join("f.txt"), "x").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "first"]);
}

/// `remove_dir_all`, but clears every file's read-only bit first: git
/// leaves some files inside `.git/objects` read-only, and Windows refuses
/// to delete a read-only file even through `remove_dir_all`. The one
/// fixture in this file that lives outside any `Sandbox` needs this, since
/// nothing else cleans it up if this does not (`t594` fix-1, finding 12).
///
/// Windows only: elsewhere `readonly` is the Unix write-permission bit
/// clippy's `permissions_set_readonly_false` warns about clearing, but
/// here it is the plain Windows attribute the same call clears for real,
/// with no such risk.
#[cfg(windows)]
fn remove_git_fixture(dir: &Path) {
    fn clear_read_only(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                clear_read_only(&path);
            } else if let Ok(metadata) = std::fs::metadata(&path) {
                let mut perms = metadata.permissions();
                if perms.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perms.set_readonly(false);
                    let _ = std::fs::set_permissions(&path, perms);
                }
            }
        }
    }
    clear_read_only(dir);
    std::fs::remove_dir_all(dir).ok();
}

#[cfg(not(windows))]
fn remove_git_fixture(dir: &Path) {
    std::fs::remove_dir_all(dir).ok();
}

/// A second working copy of `src` that shares its root commit -- the same
/// clue `d597` reads to recognise two folders as the same product.
fn clone_repo(src: &Path, destination: &Path) {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let status = std::process::Command::new("git")
        .args(["clone", "-q"])
        .arg(src)
        .arg(destination)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "git clone of {src:?} into {destination:?} failed"
    );
}

/// Case 1: A tree directly below refuses with the exact text, and writes
/// nothing -- not a tree above, and not another event in the one below.
#[test]
fn a_tree_directly_below_refuses_and_writes_nothing() {
    let c = Sandbox::new_empty("setup-below-one");
    let below = c.0.join("Backend v2");
    std::fs::create_dir_all(&below).unwrap();
    run_in(&below, c.global_home(), &["init"]);
    let events_before = std::fs::read_to_string(below.join(".vivac").join("events")).unwrap();

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("There is already a tree inside this folder, in \"Backend v2\"."),
        "{out}"
    );
    assert!(
        out.contains("Planting another one here would split this project: sessions opened in"),
        "{out}"
    );
    assert!(
        out.contains("\"Backend v2\" would use that one, and the rest this one."),
        "{out}"
    );
    assert!(
        out.contains("Move that tree up here, then run setup again. From inside \"Backend v2\":"),
        "{out}"
    );
    assert!(out.contains("vivac relocate .."), "{out}");
    assert!(!c.0.join(".vivac").exists(), "a tree was planted above");
    assert_eq!(
        events_before,
        std::fs::read_to_string(below.join(".vivac").join("events")).unwrap(),
        "the tree below gained another event"
    );
}

/// Case 2: Two trees below refuse with the plural text, naming both.
#[test]
fn two_trees_below_refuse_with_the_plural_text() {
    let c = Sandbox::new_empty("setup-below-two");
    let a = c.0.join("Backend v2");
    let b = c.0.join("Web Ova");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    run_in(&a, c.global_home(), &["init"]);
    run_in(&b, c.global_home(), &["init"]);

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("There are trees inside this folder, in \"Backend v2\" and \"Web Ova\"."),
        "{out}"
    );
    assert!(
        out.contains("vivac cannot merge trees: keep one per product, move it up here with"),
        "{out}"
    );
    assert!(
        out.contains("vivac relocate, and leave the others as they are."),
        "{out}"
    );
    assert!(!c.0.join(".vivac").exists());
}

/// Case 3: A second root sharing a repository's root commit with an
/// already-registered project refuses, naming this folder's *own*
/// repositories -- not the other project's.
#[test]
fn a_shared_root_commit_refuses_naming_this_folders_own_repos() {
    let c = Sandbox::new_empty("setup-registered-basic");
    let first = c.0.join("IQuorum");
    real_git_repo(&first.join("webapi"));
    let (setup_out, setup_code) =
        run_in(&first, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(setup_code, 0, "{setup_out}");

    let second = c.0.join("IQuorum-v2");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("Some repositories here are already tracked by project \"IQuorum\":"),
        "{out}"
    );
    assert!(out.contains("webapi"), "{out}");
    assert!(
        out.contains("Planting another tree would give this product two maps."),
        "{out}"
    );
    assert!(
        out.contains("To work on IQuorum from this folder:"),
        "{out}"
    );
    assert!(
        out.contains("vivac setup claude-code --join IQuorum"),
        "{out}"
    );
    assert!(out.contains("To plant a separate tree anyway:"), "{out}");
    assert!(out.contains("vivac setup claude-code --new-tree"), "{out}");
    assert!(!already_planted(&second));
}

/// Case 4: One shared repository is enough, even when the new root also has a
/// repository the registered project never had.
#[test]
fn one_shared_repository_is_enough_even_with_an_extra_one() {
    let c = Sandbox::new_empty("setup-registered-partial");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["setup", "claude-code", "--yes"]);

    let second = c.0.join("Prod-v2");
    clone_repo(&first.join("webapi"), &second.join("webapi"));
    real_git_repo(&second.join("infra"));

    let (out, code) = run_in(&second, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("Some repositories here are already tracked by project \"Prod\":"),
        "{out}"
    );
    assert!(!already_planted(&second));
}

/// Case 5: A registered project whose own folder name the redaction guard
/// rejects is withheld -- the second form of the text -- and it points at
/// the path remedy instead of a name. The same literal folder name
/// (`someone@example.com`) is pinned by `registry.rs`'s own
/// `a_copy_whose_folder_name_the_guard_rejects_is_not_named`, which
/// affirms directly that the guard rejects it; this is that same guarantee
/// reached through `setup` instead of `note`.
#[test]
fn a_registered_products_withheld_name_points_at_the_path_remedy() {
    let secret_name = "someone@example.com";
    let c = Sandbox::new_empty("setup-registered-withheld");
    let first = c.0.join(secret_name);
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["setup", "claude-code", "--yes"]);

    let second = c.0.join("Prod-v3");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        !out.contains(secret_name),
        "the withheld name leaked: {out}"
    );
    assert!(
        out.contains("Some repositories here are already tracked by another project on this"),
        "{out}"
    );
    assert!(out.contains("machine: webapi."), "{out}");
    assert!(
        out.contains("Planting another tree would give this product two maps."),
        "{out}"
    );
    assert!(
        out.contains("To work on it from this folder, give the path to its folder:"),
        "{out}"
    );
    assert!(
        out.contains("vivac setup claude-code --join <path to that folder>"),
        "{out}"
    );
    assert!(out.contains("To plant a separate tree anyway:"), "{out}");
    assert!(out.contains("vivac setup claude-code --new-tree"), "{out}");
}

/// Case 6: A tree that itself sits inside another one warns, but still
/// completes -- exit 0, and this folder still joins the closer tree.
#[test]
fn a_tree_above_the_joined_one_warns_but_still_completes() {
    let c = Sandbox::new_empty("setup-above-warning");
    let work = c.0.join("Work");
    let mid = work.join("T");
    let f = mid.join("sub");
    std::fs::create_dir_all(&f).unwrap();
    run_in(&work, c.global_home(), &["init"]);
    run_in(&mid, c.global_home(), &["init"]);

    let (out, code) = run_in(&f, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("This tree sits inside another one, in folder \"Work\"."),
        "{out}"
    );
    assert!(
        out.contains("above this folder use that one: keep one tree per product."),
        "{out}"
    );
    assert!(
        f.join(".vivac").join("lane").exists(),
        "the subfolder never joined the closer tree"
    );
}

/// Case 7: A known limit, written down rather than left to be discovered: a
/// product nobody ever ran `setup` on with this version left no trace in
/// the registry, so a second root of it is not recognised and just plants.
#[test]
fn a_product_the_registry_never_learned_about_is_not_recognized() {
    let c = Sandbox::new_empty("setup-registered-cold-start");
    let first = c.0.join("Untouched");
    real_git_repo(&first.join("webapi"));
    // No `setup` ever ran at `first`: the registry knows nothing about it.

    let second = c.0.join("Untouched-v2");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        already_planted(&second),
        "the known limitation: a fresh tree still gets planted"
    );
}

/// Property: with a tree below *and* a registered product at the same
/// time, the refusal about the tree below wins -- `t594` §4.5.1 describes
/// a state of the disk that has to be fixed before the product question
/// means anything.
///
/// The negative assertion alone (`!out.contains("already tracked by
/// project")`) does not tell "the tree-below refusal won" apart from "the
/// registry had nothing to say regardless" -- it stayed green when the
/// registry side of the setup below was disconnected entirely (`t594`
/// fix-1, finding 5). The positive anchor at the end closes that: with the
/// tree below out of the way, this very root does get the registry's own
/// refusal, so the first assertion is proven to distinguish the two.
#[test]
fn a_tree_below_wins_over_a_registered_product() {
    let c = Sandbox::new_empty("setup-order");
    // Outside `c.0` on purpose: this folder must not itself turn up as a
    // tree below `c.0`, only as the already-registered project whose
    // repository `c.0` also happens to hold.
    let other_root = std::env::temp_dir().join(format!(
        "vivac-setup-order-other-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    real_git_repo(&other_root.join("webapi"));
    let (other_out, other_code) = run_in(
        &other_root,
        c.global_home(),
        &["setup", "claude-code", "--yes"],
    );
    assert_eq!(
        other_code, 0,
        "the already-registered project never got set up: {other_out}"
    );

    clone_repo(&other_root.join("webapi"), &c.0.join("webapi"));
    let below = c.0.join("Backend v2");
    std::fs::create_dir_all(&below).unwrap();
    run_in(&below, c.global_home(), &["init"]);

    let (out, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("There is already a tree inside this folder, in \"Backend v2\"."),
        "{out}"
    );
    assert!(
        !out.contains("already tracked by project"),
        "the product-registered refusal must not win here: {out}"
    );

    // Positive anchor: with the tree below removed, this same root does
    // get the registry's own refusal instead of exiting 0.
    std::fs::remove_dir_all(below.join(".vivac")).unwrap();
    let (out2, code2) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code2, 1, "{out2}");
    assert!(out2.contains("already tracked by project"), "{out2}");

    remove_git_fixture(&other_root);
}

// ---------------------------------------------------------------------------
// `t594` §4.5's own escapes: `--join`, `--new-tree`, `--lane-name`.
// ---------------------------------------------------------------------------

/// A command line, split the way a shell would: whitespace-separated,
/// except inside a pair of double quotes. Just enough to run the exact
/// command a refusal just printed back at it, quotes included.
fn shell_split(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Case 1: `--join <name>` over a registered project: the folder gains
/// `.vivac/lane`, the tree gains a `lane.declared`, and a write from
/// there signs with that lane -- never with `main`.
///
/// The tree already carries one `lane.declared` before this run, from the
/// target's own `setup` declaring `main` -- so `log_before.contains(...)`
/// alone proves nothing about *this* run's own call: replacing
/// `declare_lane` with a no-op left this assertion green (`t594` fix-1,
/// finding 3). The count and the joining folder's own name, neither of
/// which the pre-existing `main` declaration could satisfy, tie it to
/// this run specifically.
#[test]
fn join_by_name_declares_a_lane_and_signs_writes_with_it() {
    let c = Sandbox::new_empty("setup-join-name");
    let target = c.0.join("IQuorum");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["setup", "claude-code", "--yes"]);
    let log_at_target = std::fs::read_to_string(target.join(".vivac").join("events")).unwrap();
    let declared_before = log_at_target.matches("\"type\":\"lane.declared\"").count();

    let here = c.0.join("IQuorum-v2");
    std::fs::create_dir_all(&here).unwrap();
    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", "IQuorum"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        here.join(".vivac").join("lane").exists(),
        "no lane file appeared in the joining folder"
    );

    let log_before = std::fs::read_to_string(target.join(".vivac").join("events")).unwrap();
    assert_eq!(
        log_before.matches("\"type\":\"lane.declared\"").count(),
        declared_before + 1,
        "the join did not add a second lane.declared of its own: {log_before}"
    );
    assert!(
        log_before.contains("\"name\":\"IQuorum-v2\""),
        "the joining folder's own lane was never declared under its own name: {log_before}"
    );

    let (push_out, push_code) = run_in(
        &here,
        c.global_home(),
        &["push", "Work from the joined folder", "--why", "seed"],
    );
    assert_eq!(push_code, 0, "{push_out}");

    let lane_id = lane_id_of(&here);
    assert_ne!(lane_id, "main", "the joined folder signs as main itself");
    let log_after = std::fs::read_to_string(target.join(".vivac").join("events")).unwrap();
    assert!(
        log_after.contains(&format!("\"lane\":\"{lane_id}\"")),
        "the write did not sign with the joined lane:\n{log_after}"
    );
}

/// `t594` fix-1, finding 3's other half: `--lane-name` alongside `--join`
/// names the lane it declares in the *target* tree -- nothing exercised
/// this path before, since the existing `--lane-name` test only ran
/// against a plain plant, never against `join`.
#[test]
fn lane_name_names_the_lane_over_join_too() {
    let c = Sandbox::new_empty("setup-join-lane-name");
    let target = c.0.join("IQuorum");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["setup", "claude-code", "--yes"]);

    let here = c.0.join("IQuorum-v2");
    std::fs::create_dir_all(&here).unwrap();
    let (out, code) = run_in(
        &here,
        c.global_home(),
        &[
            "setup",
            "claude-code",
            "--join",
            "IQuorum",
            "--lane-name",
            "custom-lane",
        ],
    );
    assert_eq!(code, 0, "{out}");

    let log = std::fs::read_to_string(target.join(".vivac").join("events")).unwrap();
    assert!(log.contains("\"name\":\"custom-lane\""), "{log}");
}

/// Case 2: `--join <path>` works the same way -- meaning what case 1
/// proves for a name: the tree gains this folder's own `lane.declared`,
/// and a write from here signs with it, never with `main`. Checking only
/// the exit code and the lane file's existence (`t594` fix-1, finding 13)
/// left "the same way" unproven: a path spec that resolved but never
/// actually declared anything would have passed too.
#[test]
fn join_by_path_works_the_same_way() {
    let c = Sandbox::new_empty("setup-join-path");
    let target = c.0.join("Prod");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["setup", "claude-code", "--yes"]);

    let here = c.0.join("Prod-v2");
    std::fs::create_dir_all(&here).unwrap();
    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", &target_str],
    );
    assert_eq!(code, 0, "{out}");
    assert!(here.join(".vivac").join("lane").exists());

    let (push_out, push_code) = run_in(
        &here,
        c.global_home(),
        &["push", "Work from the path-joined folder", "--why", "seed"],
    );
    assert_eq!(push_code, 0, "{push_out}");

    let lane_id = lane_id_of(&here);
    assert_ne!(lane_id, "main", "the joined folder signs as main itself");
    let log_after = std::fs::read_to_string(target.join(".vivac").join("events")).unwrap();
    assert!(
        log_after.contains(&format!("\"lane\":\"{lane_id}\"")),
        "the write did not sign with the joined lane:\n{log_after}"
    );
}

/// `run_in`, with `stdout` and `stderr` kept apart: proving the copy
/// warning lands on the stream the agent's own parsing does not touch
/// needs the two kept separate, the same reason `tests/registry.rs`'s own
/// `run_split` exists.
fn run_in_split(dir: &Path, home: &Path, args: &[&str]) -> (String, String, i32) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(dir)
        .env("VIVAC_HOME", home)
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// `t594`: `--join` used to note the registry and throw
/// the answer away (`let _ = registry::note(...)`), so joining a folder
/// that is itself a known copy -- exactly the mistake the warning exists
/// to catch -- passed in silence. `original` keeps the registry's `path`,
/// alive; `copy` is registered as one of its copies (a plain `brief` run
/// there first); a third folder joins `copy` itself.
#[test]
fn join_to_a_folder_that_is_itself_a_copy_warns_on_stderr() {
    let c = Sandbox::new_empty("setup-join-copy");
    let original = c.0.join("orig");
    std::fs::create_dir_all(&original).unwrap();
    run_in(
        &original,
        c.global_home(),
        &["setup", "claude-code", "--yes"],
    );
    run_in(
        &original,
        c.global_home(),
        &["push", "a goal", "--why", "so the log has a first event"],
    );

    let copy = c.0.join("copy");
    std::fs::create_dir_all(copy.join(".vivac")).unwrap();
    std::fs::copy(
        original.join(".vivac").join("events"),
        copy.join(".vivac").join("events"),
    )
    .unwrap();
    // Registers `copy` in `orig`'s own `copies`, before the join this test
    // is about ever runs.
    run_in(&copy, c.global_home(), &["brief"]);

    let here = c.0.join("third");
    std::fs::create_dir_all(&here).unwrap();
    let copy_str = copy.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_in_split(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", &copy_str],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stderr.contains("COPY OF ANOTHER TREE"),
        "join to a known copy never warned:\n{stderr}"
    );
    assert!(
        !stdout.contains("COPY OF ANOTHER TREE"),
        "the warning leaked into stdout:\n{stdout}"
    );
}

/// Case 3: `--join` to a folder with no tree refuses, and nothing is
/// written. Only the exit code used to be checked (`t594` fix-1, finding
/// 13); the text is what tells this refusal apart from any other exit-1
/// `join` can reach.
#[test]
fn join_to_a_folder_with_no_tree_refuses() {
    let c = Sandbox::new_empty("setup-join-no-tree");
    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let empty_target = c.0.join("NoTreeHere").to_string_lossy().into_owned();

    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", &empty_target],
    );
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("has no tree yet, so there is nothing to join."),
        "{out}"
    );
    assert!(!here.join(".vivac").exists());
}

/// Case 4: `--join` from a folder that is already a lane of another tree
/// refuses.
#[test]
fn join_from_a_folder_already_a_lane_of_another_tree_refuses() {
    let c = Sandbox::new_empty("setup-join-already-lane");
    let a = c.0.join("A");
    std::fs::create_dir_all(&a).unwrap();
    run_in(&a, c.global_home(), &["setup", "claude-code", "--yes"]);
    let b = c.0.join("B");
    std::fs::create_dir_all(&b).unwrap();
    run_in(&b, c.global_home(), &["setup", "claude-code", "--yes"]);

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let (join_out, join_code) = run_in(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", "A"],
    );
    assert_eq!(join_code, 0, "{join_out}");

    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", "B"],
    );
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("This folder is already a lane of another tree."),
        "{out}"
    );
}

/// `t594` fix-1, finding 9: `--join` from the folder that holds its own
/// tree used to answer with the "already a lane of another tree" text --
/// wrong, since this folder carries no lane at all, it carries the tree.
#[test]
fn join_from_the_folder_that_holds_its_own_tree_names_it_correctly() {
    // Two sibling folders, neither nested inside the other: `here` must
    // hold a tree of its own, not be a lane `store::locate` resolves up
    // into some other tree -- which is exactly what nesting it inside a
    // seeded sandbox would have done.
    let c = Sandbox::new_empty("setup-join-own-tree");
    let here = c.0.join("HasATree");
    std::fs::create_dir_all(&here).unwrap();
    run_in(&here, c.global_home(), &["setup", "claude-code", "--yes"]);

    let other = c.0.join("Other");
    std::fs::create_dir_all(&other).unwrap();
    run_in(&other, c.global_home(), &["setup", "claude-code", "--yes"]);

    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", "Other"],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("This folder holds a tree of its own"), "{out}");
    assert!(
        !out.contains("already a lane of another tree"),
        "the wrong text is still shown: {out}"
    );
}

/// `t594` fix-1, finding 6: with a tree above *and* a tree below, joining
/// the one above used to skip the tree-below check entirely -- `setup` in
/// `Work/F` joined `Work` and said nothing about `Work/F/Nested`, exactly
/// the split product §6.4 exists to catch.
#[test]
fn a_tree_below_refuses_even_when_there_is_one_above() {
    let c = Sandbox::new_empty("setup-below-and-above");
    let work = c.0.join("Work");
    let f = work.join("F");
    let nested = f.join("Nested");
    std::fs::create_dir_all(&nested).unwrap();
    run_in(&work, c.global_home(), &["init"]);
    run_in(&nested, c.global_home(), &["init"]);

    let (out, code) = run_in(&f, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("There is already a tree inside this folder, in \"Nested\"."),
        "{out}"
    );
    assert!(
        !f.join(".vivac").exists(),
        "F must not have joined Work despite the tree below"
    );
}

/// `t594` fix-1, finding 1 (critical): `registry::resolve` used to hand a
/// relative `--join` spec straight to `entry.path`, corrupting the
/// machine registry for good -- every reader of that entry resolves it
/// from a folder of its own, not from the one that typed `--join`.
#[test]
fn a_relative_join_target_is_recorded_as_an_absolute_path() {
    let c = Sandbox::new_empty("setup-join-relative-target");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["setup", "claude-code", "--yes"]);

    let here = c.0.join("F");
    let sub = here.join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    let (out, code) = run_in(
        &sub,
        c.global_home(),
        &["setup", "claude-code", "--join", "../../T"],
    );
    assert_eq!(code, 0, "{out}");

    let registry = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert!(
        !registry.contains(".."),
        "a relative path leaked into the machine registry: {registry}"
    );

    // The corruption `t594` fix-1 finding 1 was reproduced with: `brief`
    // from a subfolder of the joined folder dying on a path nobody but
    // the original `cd` could resolve.
    let nested = sub.join("deeper");
    std::fs::create_dir_all(&nested).unwrap();
    let (brief_out, brief_code) = run_in(&nested, c.global_home(), &["brief"]);
    assert_eq!(brief_code, 0, "{brief_out}");
}

/// `t594` fix-1, finding 2: `--join` returned before `apply` ever ran
/// `refuse_home_or_global_store`, so the home-folder guard lived in one
/// branch and the other had none. Same fixture as
/// `setup_refuses_in_the_home_folder`, with `--join` instead of a plain
/// setup.
#[test]
fn join_refuses_in_the_home_folder_too() {
    let c = Sandbox::new_empty("setup-join-home");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["setup", "claude-code", "--yes"]);

    let (out, code) = run_with_home(
        &c.0,
        &c.0,
        c.global_home(),
        &["setup", "claude-code", "--join", "T"],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&home_folder_text(&printed(&c.0))), "{out}");
    assert!(!c.0.join(".vivac").join("lane").exists());
}

/// `t594` fix-1, finding 4: `--join --dry-run` used to write the lane
/// file, declare the lane in the target tree and note the machine
/// registry anyway -- `--dry-run` promises nothing is written by any
/// path, and `apply`'s own promise (`t594` fix-2, finding 2) does not
/// cover a path it never runs through.
#[test]
fn join_dry_run_writes_nothing() {
    let c = Sandbox::new_empty("setup-join-dry-run");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["setup", "claude-code", "--yes"]);
    let log_before = std::fs::read_to_string(target.join(".vivac").join("events")).unwrap();
    let registry_before = std::fs::read_to_string(c.global_home().join("projects")).unwrap();

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();

    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", "T", "--dry-run"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(!here.join(".vivac").exists(), "the lane file was written");

    let log_after = std::fs::read_to_string(target.join(".vivac").join("events")).unwrap();
    assert_eq!(log_before, log_after, "the target tree's own log changed");
    let registry_after = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert_eq!(
        registry_before, registry_after,
        "the machine registry changed"
    );
}

/// `t594` fix-1, finding 10: a tree with no events yet used to answer on
/// one long, unwrapped line, and echoed back a path this run resolved --
/// neither is true any more.
#[test]
fn join_to_a_tree_with_no_events_yet_wraps_and_names_nothing() {
    let c = Sandbox::new_empty("setup-join-no-events");
    let target = c.0.join("Empty");
    std::fs::create_dir_all(target.join(".vivac")).unwrap();
    std::fs::write(target.join(".vivac").join("events"), "").unwrap();

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let target_str = target.to_string_lossy().into_owned();

    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["setup", "claude-code", "--join", &target_str],
    );
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("That tree has no events yet, so there is nothing to join:"),
        "{out}"
    );
    assert!(
        !out.contains(&target_str),
        "the path was echoed back into the message: {out}"
    );
    assert!(!here.join(".vivac").exists());
}

/// `t594` fix-1, finding 8: `--lane-name` used to be accepted and
/// silently ignored when planting fresh -- worse than either using it or
/// refusing it outright, since accepting a flag and doing nothing with it
/// leaves no trace that it was ignored.
#[test]
fn lane_name_names_main_when_planting_fresh_too() {
    let c = Sandbox::new_empty("setup-lane-name-fresh-plant");
    let (out, code) = c.run(&[
        "setup",
        "claude-code",
        "--yes",
        "--lane-name",
        "custom-name",
    ]);
    assert_eq!(code, 0, "{out}");
    let log = c.log();
    assert!(log.contains("\"name\":\"custom-name\""), "{log}");
}

/// Case 5: `--new-tree` plants despite a shared root commit -- the
/// negative of 1.3 shown first, without it.
#[test]
fn new_tree_plants_despite_a_shared_root_commit() {
    let c = Sandbox::new_empty("setup-new-tree");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["setup", "claude-code", "--yes"]);

    let second = c.0.join("Prod-fork");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (refused_out, refused_code) =
        run_in(&second, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(refused_code, 1, "{refused_out}");
    assert!(
        refused_out.contains("already tracked by project"),
        "{refused_out}"
    );

    let (out, code) = run_in(
        &second,
        c.global_home(),
        &["setup", "claude-code", "--new-tree", "--yes"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(already_planted(&second));
}

/// Case 6: `--join` and `--new-tree` together is a usage error. Only the
/// exit code used to be checked (`t594` fix-1, finding 13); the text is
/// what tells this usage error apart from any other exit-2 `setup` can
/// give.
#[test]
fn join_and_new_tree_together_is_a_usage_error() {
    let c = Sandbox::new_empty("setup-join-new-tree-exclusive");
    let (out, code) = c.run(&["setup", "claude-code", "--join", "X", "--new-tree"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--join joins a tree that already exists, and --new-tree plants a"),
        "{out}"
    );
}

/// Case 7, first half: `--lane-name` names the lane.
#[test]
fn lane_name_names_the_lane() {
    let c = Sandbox::new_seeded("setup-lane-name-ok");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();

    let (out, code) = run_in(
        &second,
        c.global_home(),
        &[
            "setup",
            "claude-code",
            "--yes",
            "--lane-name",
            "custom-name",
        ],
    );
    assert_eq!(code, 0, "{out}");
    let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    assert!(log.contains("\"name\":\"custom-name\""), "{log}");
}

/// Case 7, second half: a name the guard rejects falls back without
/// failing the operation, and neither the name nor a fragment of it
/// reaches the log. The same literal secret is pinned directly against
/// the guard by `relocate.rs`'s own
/// `a_lane_name_the_guard_rejects_falls_back_without_failing`.
///
/// "Falls back" was never checked -- only that the secret did not leak
/// (`t594` fix-1, finding 13), which a run that failed outright would
/// also have satisfied. The reserve name `lane::name_for` actually writes
/// is what proves a fallback happened rather than nothing at all.
#[test]
fn a_lane_name_the_guard_rejects_falls_back_without_failing() {
    let secret = "ghp_16C7e42F292c6912E7710c838347Ae178B4a";
    let c = Sandbox::new_seeded("setup-lane-name-guard");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();

    let (out, code) = run_in(
        &second,
        c.global_home(),
        &["setup", "claude-code", "--yes", "--lane-name", secret],
    );
    assert_eq!(code, 0, "{out}");
    let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    assert!(!log.contains(secret), "the secret leaked whole: {log}");
    assert!(
        !log.contains("16C7e42F292c6912E7710c838347Ae178B4a"),
        "a fragment of the secret leaked: {log}"
    );

    let lane_id = lane_id_of(&second);
    let reserve_name = format!("lane-{}", &lane_id[..lane_id.len().min(4)]);
    assert!(
        log.contains(&format!("\"name\":\"{reserve_name}\"")),
        "the reserve name never appeared, so no fallback is proven: {log}"
    );
}

/// Case 8, the one that closes the circle: a refusal from 1.3 prints a
/// `--join` command naming a project whose folder has a space in it, so
/// the printed command quotes it -- and running that exact command,
/// quotes respected, actually works.
#[test]
fn the_join_command_printed_by_the_refusal_actually_works() {
    let c = Sandbox::new_empty("setup-close-the-loop");
    let first = c.0.join("IQ Suite");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["setup", "claude-code", "--yes"]);

    let second = c.0.join("IQ-Suite-v2");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 1, "{out}");
    let join_line = out
        .lines()
        .find(|l| l.trim_start().starts_with("vivac setup claude-code --join"))
        .unwrap_or_else(|| panic!("no --join command line in the refusal:\n{out}"));
    assert!(
        join_line.contains("\"IQ Suite\""),
        "the printed command did not quote the name with a space: {join_line}"
    );
    let words = shell_split(join_line.trim());
    let cli_args: Vec<&str> = words[1..].iter().map(String::as_str).collect();

    let (join_out, join_code) = run_in(&second, c.global_home(), &cli_args);
    assert_eq!(join_code, 0, "{join_out}");
    assert!(second.join(".vivac").join("lane").exists());
}
