//! `vivac setup codex` — tranche 1 (`t592`) covers a clean project, none of
//! the three files Codex reads there yet. Tranche 2 adds merging with a
//! file already there (piece B, `t592` §4), running it twice (piece D,
//! §6), and taking it off again (piece C, §5).

mod common;
use common::Sandbox;

const SESSION_START: &str = "vivac session start --hook";
const SESSION_END: &str = "vivac session end --hook";
const SESSION_PROMPT: &str = "vivac session prompt --hook";

const CONFIG_LABEL: &str = ".codex/config.toml";
const HOOKS_LABEL: &str = ".codex/hooks.json";
const SKILL_LABEL: &str = ".agents/skills/vivac-migrate/SKILL.md";

const EXPECTED_CONFIG: &str = "# added by vivac setup codex\n\
[mcp_servers.vivac]\n\
command = \"vivac\"\n\
args = [\"mcp\"]\n\
# end of what vivac setup codex added\n";

const EXPECTED_HOOKS: &str = "{\n  \
  \"hooks\": {\n    \
    \"SessionStart\": [\n      \
      {\n        \
        \"matcher\": \"startup|resume|clear|compact\",\n        \
        \"hooks\": [\n          \
          { \"type\": \"command\", \"command\": \"vivac session start --hook\" }\n        \
        ]\n      \
      }\n    \
    ],\n    \
    \"UserPromptSubmit\": [\n      \
      {\n        \
        \"hooks\": [\n          \
          { \"type\": \"command\", \"command\": \"vivac session prompt --hook\" }\n        \
        ]\n      \
      }\n    \
    ],\n    \
    \"Stop\": [\n      \
      {\n        \
        \"hooks\": [\n          \
          { \"type\": \"command\", \"command\": \"vivac session end --hook\" }\n        \
        ]\n      \
      }\n    \
    ]\n  \
  }\n\
}\n";

fn config_path(c: &Sandbox) -> std::path::PathBuf {
    c.0.join(".codex").join("config.toml")
}
fn hooks_path(c: &Sandbox) -> std::path::PathBuf {
    c.0.join(".codex").join("hooks.json")
}
fn skill_path(c: &Sandbox) -> std::path::PathBuf {
    c.0.join(".agents")
        .join("skills")
        .join("vivac-migrate")
        .join("SKILL.md")
}

fn read(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("reading {p:?}: {e}"))
}

/// The words of `out`, run together regardless of which line
/// `render::wrap` (`f720`) put them on: width wraps a long status now,
/// not a hand-picked cut, so a test that cares about the words has to
/// stop caring which line they landed on -- the same shift
/// `tests/check.rs`'s own `words` already made for `copy_notice`'s
/// prose.
fn plan_words(out: &str) -> String {
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `p` the way the binary's own `current_dir()` would print it -- the same
/// split `tests/setup.rs` already needs, and for the same reason: on
/// Windows `current_dir()` returns the path as given rather than a
/// canonicalized one.
#[cfg(unix)]
fn printed(p: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| panic!("canonicalize {p:?}: {e}"))
}
#[cfg(not(unix))]
fn printed(p: &std::path::Path) -> std::path::PathBuf {
    p.to_path_buf()
}

// ---------------------------------------------------------------------------
// 1. `--dry-run`: nothing written, the plan names the three paths.
// ---------------------------------------------------------------------------

#[test]
fn dry_run_writes_nothing_and_shows_the_three_paths() {
    let c = Sandbox::new_empty("setup-codex-dry-run");
    // `d723` piece B: `setup` never plants, so a tree has to be here
    // already, or this run refuses before it ever gets to a plan.
    c.ok(&["init", "--yes"]);
    let before: Vec<_> = std::fs::read_dir(&c.0).unwrap().collect();
    let (out, code) = c.run(&["setup", "codex", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("vivac setup codex will, in"), "{out}");
    assert!(out.contains(CONFIG_LABEL), "{out}");
    assert!(out.contains(HOOKS_LABEL), "{out}");
    assert!(out.contains(SKILL_LABEL), "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    let after: Vec<_> = std::fs::read_dir(&c.0).unwrap().collect();
    assert_eq!(before.len(), after.len(), "dry-run created something");
    assert!(!c.0.join(".codex").exists());
    assert!(!c.0.join(".agents").exists());
}

// ---------------------------------------------------------------------------
// 2. A clean project: the three files, with the exact content.
// ---------------------------------------------------------------------------

#[test]
fn a_clean_project_gets_the_three_files_with_the_exact_content() {
    let c = Sandbox::new_empty("setup-codex-fresh");
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Written."), "{out}");

    assert_eq!(read(&config_path(&c)), EXPECTED_CONFIG);
    assert_eq!(read(&hooks_path(&c)), EXPECTED_HOOKS);

    let hooks: serde_json::Value = serde_json::from_str(&read(&hooks_path(&c))).unwrap();
    assert_eq!(
        hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        SESSION_START
    );
    assert_eq!(
        hooks["hooks"]["Stop"][0]["hooks"][0]["command"],
        SESSION_END
    );
    assert_eq!(
        hooks["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
        SESSION_PROMPT
    );
    assert!(
        hooks["hooks"]["UserPromptSubmit"][0]
            .get("matcher")
            .is_none(),
        "{hooks}"
    );
}

/// The skill is the same file `claude-code` writes, at a different path: not
/// a second copy that can drift from it (`d653`).
#[test]
fn the_skill_is_byte_for_byte_the_one_claude_code_writes() {
    let c = Sandbox::new_empty("setup-codex-skill");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);

    let codex_skill = std::fs::read(skill_path(&c)).unwrap();
    let claude_code_skill = std::fs::read(
        c.0.join(".claude")
            .join("skills")
            .join("vivac-migrate")
            .join("SKILL.md"),
    )
    .unwrap();
    assert_eq!(
        codex_skill, claude_code_skill,
        "the two harnesses wrote a different skill"
    );
}

// ---------------------------------------------------------------------------
// 3. The closing summary names the two doors setup cannot cross (`d655`).
// ---------------------------------------------------------------------------

#[test]
fn the_summary_names_the_two_doors_with_the_real_project_path() {
    let c = Sandbox::new_empty("setup-codex-doors");
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");
    let path = printed(&c.0);
    // The sandbox's own path carries no single quote, so the TOML literal
    // form applies: single-quoted, and every backslash a Windows path
    // carries survives untouched rather than doubled (a double-quoted TOML
    // string would read that backslash as the start of an escape).
    assert!(
        out.contains(&format!("[projects.'{}']", path.display())),
        "{out}"
    );
    assert!(out.contains("trust_level = \"trusted\""), "{out}");
    assert!(out.contains("~/.codex/config.toml"), "{out}");
    assert!(out.contains("/hooks"), "{out}");
}

// ---------------------------------------------------------------------------
// 4. None of the three written files carries an absolute path.
// ---------------------------------------------------------------------------

#[test]
fn none_of_the_three_files_carries_an_absolute_path() {
    let c = Sandbox::new_empty("setup-codex-no-path");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    let needle = c.0.to_string_lossy().into_owned();
    for (label, text) in [
        (CONFIG_LABEL, read(&config_path(&c))),
        (HOOKS_LABEL, read(&hooks_path(&c))),
        (SKILL_LABEL, read(&skill_path(&c))),
    ] {
        assert!(
            !text.contains(&needle),
            "{label} carries the project's own path:\n{text}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. `t592` tranche 2, piece B: each file merges with what is already
//    there instead of refusing outright, and piece D: running it twice
//    writes nothing the second time.
// ---------------------------------------------------------------------------

/// Test 5 of the piece B/D specification: a foreign `config.toml` keeps its
/// own content, byte for byte, and gains our block behind it, preceded by a
/// blank line.
#[test]
fn a_foreign_config_toml_keeps_its_content_and_gains_our_block_after_it() {
    let c = Sandbox::new_empty("setup-codex-config-foreign");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    std::fs::write(config_path(&c), "# hand-written\nsomething = 1\n").unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("add") && out.contains("the \"vivac\" server"),
        "{out}"
    );
    assert_eq!(
        read(&config_path(&c)),
        format!("# hand-written\nsomething = 1\n\n{EXPECTED_CONFIG}")
    );
}

/// Test 6: `config.toml` already carrying our block is left alone -- not a
/// single byte changes, even though this run still has the other two files
/// to write.
#[test]
fn a_config_toml_with_our_block_already_there_is_left_untouched() {
    let c = Sandbox::new_empty("setup-codex-config-already");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    std::fs::write(config_path(&c), EXPECTED_CONFIG).unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("already has the \"vivac\" server"), "{out}");
    assert_eq!(read(&config_path(&c)), EXPECTED_CONFIG);
}

/// Test 7: an opening marker with no closing one refuses, names the file
/// and which marker is missing, and writes nothing anywhere -- not even the
/// other two files this run would otherwise have created.
#[test]
fn a_config_toml_with_an_opening_marker_and_no_closing_one_is_rejected() {
    let c = Sandbox::new_empty("setup-codex-config-half-marker");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    std::fs::write(
        config_path(&c),
        "# added by vivac setup codex\n[mcp_servers.vivac]\ncommand = \"vivac\"\n",
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(CONFIG_LABEL), "{out}");
    assert!(out.contains("closing"), "{out}");
    assert!(!hooks_path(&c).exists(), "{out}");
    assert!(!skill_path(&c).exists(), "{out}");
}

/// Test 8: a foreign `hooks.json` keeps its own event intact, and ours are
/// added beside it, with the matcher on `SessionStart` and none on `Stop`.
#[test]
fn a_foreign_hooks_json_keeps_its_other_event_and_gains_ours() {
    let c = Sandbox::new_empty("setup-codex-hooks-foreign");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    let foreign = serde_json::json!({
        "hooks": {
            "PreCompact": [
                { "hooks": [ { "type": "command", "command": "some-other-tool" } ] }
            ]
        }
    });
    std::fs::write(
        hooks_path(&c),
        serde_json::to_string_pretty(&foreign).unwrap(),
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");

    let after: serde_json::Value = serde_json::from_str(&read(&hooks_path(&c))).unwrap();
    assert_eq!(
        after["hooks"]["PreCompact"][0]["hooks"][0]["command"], "some-other-tool",
        "{after}"
    );
    assert_eq!(
        after["hooks"]["SessionStart"][0]["matcher"], "startup|resume|clear|compact",
        "{after}"
    );
    assert_eq!(
        after["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        SESSION_START
    );
    assert!(
        after["hooks"]["Stop"][0].get("matcher").is_none(),
        "{after}"
    );
    assert_eq!(
        after["hooks"]["Stop"][0]["hooks"][0]["command"],
        SESSION_END
    );
    assert!(
        after["hooks"]["UserPromptSubmit"][0]
            .get("matcher")
            .is_none(),
        "{after}"
    );
    assert_eq!(
        after["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
        SESSION_PROMPT
    );
}

/// `d779`: a project whose `hooks.json` already has the two older hooks
/// gains only `UserPromptSubmit`, and neither of the first two is touched.
#[test]
fn a_hooks_json_with_the_two_older_hooks_gains_only_the_third() {
    let c = Sandbox::new_empty("setup-codex-prompt-hook-add-third");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    let existing = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "startup|resume|clear|compact",
                    "hooks": [ { "type": "command", "command": SESSION_START } ]
                }
            ],
            "Stop": [
                { "hooks": [ { "type": "command", "command": SESSION_END } ] }
            ]
        }
    });
    std::fs::write(
        hooks_path(&c),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        plan_words(&out).contains("add") && plan_words(&out).contains("the UserPromptSubmit hook"),
        "{out}"
    );

    let after: serde_json::Value = serde_json::from_str(&read(&hooks_path(&c))).unwrap();
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

/// Test 9: `hooks.json` that does not parse refuses, and writes neither the
/// TOML nor the skill -- all or nothing stays all or nothing.
#[test]
fn broken_hooks_json_refuses_and_writes_nothing() {
    let c = Sandbox::new_empty("setup-codex-hooks-broken");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    std::fs::write(hooks_path(&c), "{\n  \"hooks\": ,\n}").unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(HOOKS_LABEL), "{out}");
    assert!(out.contains("not JSON setup can read"), "{out}");
    assert!(!config_path(&c).exists(), "{out}");
    assert!(!skill_path(&c).exists(), "{out}");
}

/// Test 10: a `description` at the root of `hooks.json` is not ours, and
/// stays there after this run adds its own hooks.
#[test]
fn a_hooks_json_with_a_root_description_keeps_it() {
    let c = Sandbox::new_empty("setup-codex-hooks-description");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    let foreign = serde_json::json!({ "description": "our own hooks" });
    std::fs::write(
        hooks_path(&c),
        serde_json::to_string_pretty(&foreign).unwrap(),
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");
    let after: serde_json::Value = serde_json::from_str(&read(&hooks_path(&c))).unwrap();
    assert_eq!(after["description"], "our own hooks", "{after}");
}

/// Test 11: a skill changed since setup wrote it is a conflict, and stays
/// exactly as it was found.
#[test]
fn a_hand_edited_skill_is_a_conflict_and_is_not_overwritten() {
    let c = Sandbox::new_empty("setup-codex-skill-conflict");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(skill_path(&c).parent().unwrap()).unwrap();
    std::fs::write(skill_path(&c), "# Someone else's skill\n").unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(SKILL_LABEL), "{out}");
    assert_eq!(read(&skill_path(&c)), "# Someone else's skill\n");
    assert!(!config_path(&c).exists(), "{out}");
    assert!(!hooks_path(&c).exists(), "{out}");
}

/// Test 12: a foreign `[mcp_servers.vivac]` table, with none of our
/// markers, is rejected rather than duplicated into invalid TOML.
#[test]
fn a_foreign_mcp_servers_vivac_table_is_rejected() {
    let c = Sandbox::new_empty("setup-codex-config-mcp-table");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    std::fs::write(
        config_path(&c),
        "[mcp_servers.vivac]\ncommand = \"something-else\"\n",
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(CONFIG_LABEL), "{out}");
    assert!(out.contains("[mcp_servers.vivac]"), "{out}");
    assert!(!hooks_path(&c).exists(), "{out}");
    assert!(!skill_path(&c).exists(), "{out}");
}

/// Test 13, piece D: running `setup codex --yes` twice writes nothing the
/// second time, and not one of the three files -- nor `.vivac/events` --
/// changes a single byte.
#[test]
fn a_second_run_writes_nothing() {
    let c = Sandbox::new_empty("setup-codex-idempotent");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    let config_before = std::fs::read(config_path(&c)).unwrap();
    let hooks_before = std::fs::read(hooks_path(&c)).unwrap();
    let skill_before = std::fs::read(skill_path(&c)).unwrap();
    let events_before = std::fs::read(c.0.join(".vivac").join("events")).unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Nothing to write: this project is already set up."),
        "{out}"
    );
    assert_eq!(config_before, std::fs::read(config_path(&c)).unwrap());
    assert_eq!(hooks_before, std::fs::read(hooks_path(&c)).unwrap());
    assert_eq!(skill_before, std::fs::read(skill_path(&c)).unwrap());
    assert_eq!(
        events_before,
        std::fs::read(c.0.join(".vivac").join("events")).unwrap(),
        "the second run touched the tree"
    );
}

// ---------------------------------------------------------------------------
// 6. `vivac setup` names both harnesses, with or without a valid one.
// ---------------------------------------------------------------------------

#[test]
fn setup_with_no_harness_names_both_harnesses() {
    let c = Sandbox::new_empty("setup-codex-no-harness");
    let (out, code) = c.run(&["setup"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("claude-code"), "{out}");
    assert!(out.contains("codex"), "{out}");
}

#[test]
fn setup_with_an_unknown_harness_names_both_harnesses() {
    let c = Sandbox::new_empty("setup-codex-unknown-harness");
    let (out, code) = c.run(&["setup", "loquesea"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("claude-code"), "{out}");
    assert!(out.contains("codex"), "{out}");
}

// ---------------------------------------------------------------------------
// 7. `t592` tranche 2 (`d710`) gave `--new-tree`, `--lane-name` and `--name`
//    to `setup codex` as the tree's own flags. `d723` piece B took them
//    away again, to `init` alone: each one now carries a lapida naming
//    `vivac init` instead of doing anything here. `tests/init.rs` already
//    covers what each flag does; what is left to prove here is that
//    `setup codex` sends whoever still types one to the command that
//    reads it now, rather than a plain "unknown flag" or silently
//    planting.
// ---------------------------------------------------------------------------

#[test]
fn new_tree_is_a_tombstone_pointing_at_init() {
    let c = Sandbox::new_empty("setup-codex-flag-new-tree");
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "codex", "--yes", "--new-tree"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--new-tree is vivac init's, not setup's"),
        "{out}"
    );
    assert!(out.contains("vivac init --new-tree"), "{out}");
    assert!(out.contains("vivac setup codex"), "{out}");
}

#[test]
fn lane_name_is_a_tombstone_pointing_at_init() {
    let c = Sandbox::new_empty("setup-codex-flag-lane-name");
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "codex", "--yes", "--lane-name", "custom-name"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--lane-name is vivac init's, not setup's"),
        "{out}"
    );
    assert!(out.contains("vivac init --lane-name"), "{out}");
}

/// `t640`: `--name` used to be claude-code's own too, and codex's as well.
/// `d723` piece B moved it to `init` with the rest of the tree side.
#[test]
fn name_is_a_tombstone_pointing_at_init() {
    let c = Sandbox::new_empty("setup-codex-flag-name");
    c.ok(&["init", "--yes"]);
    let (out, code) = c.run(&["setup", "codex", "--yes", "--name", "IQuorum"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--name is vivac init's, not setup's"), "{out}");
    assert!(out.contains("vivac init --name"), "{out}");
}

// ---------------------------------------------------------------------------
// 8. `t592` tranche 2 (`d710`) gave `setup` a fourth piece here: planting
// the tree the same way `claude-code` did, from the module both harnesses
// shared (`src/setup/tree.rs`). `d723` piece B took it away again --
// planting is `init`'s alone now, and neither harness's own plan says
// anything about the tree any more.
// ---------------------------------------------------------------------------

/// The tree's own lines of a plan, told apart from the harness's own --
/// `.vivac/`, `.vivac/lane`, `.vivac/events`, and the `config` lock line,
/// which was the fourth piece `t592` tranche 1 left out (`f705`), and is
/// `init`'s alone since `d723` piece B.
fn tree_lines(out: &str) -> Vec<&str> {
    out.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with(".vivac/") || t.starts_with("config")
        })
        .collect()
}

/// `f705`'s own guard used to compare the two harnesses' tree lines for
/// equality -- neither can be compared to the other that way any more,
/// since `d723` piece B leaves both with none. What is left worth holding
/// is the negative: `setup` touching the tree again, for either harness,
/// is exactly the regression `f717`/`f721` were about.
#[test]
fn dry_run_shows_no_tree_lines_for_either_harness() {
    let c = Sandbox::new_empty("setup-codex-tree-lines");
    c.ok(&["init", "--yes"]);
    let (claude_out, claude_code) = c.run(&["setup", "claude-code", "--dry-run"]);
    assert_eq!(claude_code, 0, "{claude_out}");
    let (codex_out, codex_code) = c.run(&["setup", "codex", "--dry-run"]);
    assert_eq!(codex_code, 0, "{codex_out}");

    assert!(
        tree_lines(&claude_out).is_empty(),
        "claude-code's own plan named the tree:\n{claude_out}"
    );
    assert!(
        tree_lines(&codex_out).is_empty(),
        "codex's own plan named the tree:\n{codex_out}"
    );
}

/// `setup` writes its own three pieces onto a tree `init` already planted,
/// and touches nothing under `.vivac/`: the tree, its `.gitignore`, its
/// version lock and the lane `stack --lanes` already knows about all stay
/// exactly as `init` left them.
#[test]
fn a_clean_setup_run_leaves_the_tree_as_init_left_it() {
    let c = Sandbox::new_empty("setup-codex-plants-tree");
    c.ok(&["init", "--yes"]);
    let events_before = std::fs::read(c.0.join(".vivac").join("events")).unwrap();
    let config_before = std::fs::read(c.0.join(".vivac").join("config")).unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");

    assert_eq!(
        events_before,
        std::fs::read(c.0.join(".vivac").join("events")).unwrap(),
        "setup touched the log"
    );
    assert_eq!(
        config_before,
        std::fs::read(c.0.join(".vivac").join("config")).unwrap(),
        "setup touched the config"
    );

    let stack = c.ok(&["stack", "--lanes"]);
    let folder_name = c.0.file_name().unwrap().to_string_lossy().into_owned();
    assert!(stack.contains(&folder_name), "{stack}");
}

/// Point 4 of `f705`'s own spec: with no tree, the start hook stays quiet
/// forever. Run by hand right after `init --yes` and `setup codex --yes`,
/// it now prints the brief instead.
#[test]
fn after_a_clean_plant_the_start_hook_prints_a_brief_instead_of_staying_quiet() {
    let c = Sandbox::new_empty("setup-codex-start-hook-brief");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    let (out, code) = c.run(&["session", "start", "--hook"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.starts_with("vivac · project:"), "{out}");
}

// ---------------------------------------------------------------------------
// 9. `t592` tranche 2, piece C (`d710` §5): `--undo` takes off exactly what
//    this harness wrote and leaves everything else, mirroring
//    `claude_code::undo` rather than reinventing it (`r515`).
// ---------------------------------------------------------------------------

/// Test 12: a clean plant undone removes every file setup wrote, and the
/// `vivac-migrate` folder alongside it -- `.vivac/` aside, which stays,
/// the same as `.codex/` and `.agents/` themselves: `d784` took away
/// `--undo`'s license to remove either for being empty, even though
/// setup is what created them here.
#[test]
fn undo_after_a_clean_setup_leaves_the_folder_as_it_was_except_the_tree() {
    let c = Sandbox::new_empty("setup-codex-undo-clean");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);

    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
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
    assert!(!config_path(&c).exists());
    assert!(!hooks_path(&c).exists());
    assert!(
        !skill_path(&c).parent().unwrap().exists(),
        "the vivac-migrate folder must go"
    );
    assert!(
        c.0.join(".codex").exists(),
        "setup must never remove .codex/ itself"
    );
    assert!(
        c.0.join(".agents").exists(),
        "setup must never remove .agents/ itself"
    );
    assert!(c.0.join(".vivac").exists(), "the tree must stay");
}

/// Test 13: a foreign `config.toml` from before the first run comes back
/// byte for byte once `--undo` takes our block back off.
#[test]
fn undo_over_a_foreign_config_toml_returns_it_to_the_original_bytes() {
    let c = Sandbox::new_empty("setup-codex-undo-config-foreign");
    c.ok(&["init", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    let foreign = "# hand-written\nsomething = 1\n";
    std::fs::write(config_path(&c), foreign).unwrap();

    c.ok(&["setup", "codex", "--yes"]);
    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(read(&config_path(&c)), foreign, "{out}");
}

/// Test 14: `--undo` where `setup` never ran removes nothing and says so.
#[test]
fn undo_where_setup_never_ran_removes_nothing_and_says_so() {
    // No tree either, on purpose: `d723` piece B makes `--undo` the one
    // door into `setup` that never resolves one, so this is the one test
    // in this file that plants nothing at all.
    let c = Sandbox::new_empty("setup-codex-undo-never-ran");
    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Nothing to undo: none of what setup writes is here."),
        "{out}"
    );
    assert!(!c.0.join(".codex").exists(), "{out}");
    assert!(!c.0.join(".agents").exists(), "{out}");
    assert!(!c.0.join(".vivac").exists(), "{out}");
}

/// Test 15: a foreign event in `hooks.json` survives `--undo` intact, and
/// the file itself stays -- only our two entries go.
#[test]
fn undo_over_hooks_json_with_a_foreign_event_keeps_it_and_removes_ours() {
    let c = Sandbox::new_empty("setup-codex-undo-hooks-foreign-event");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    let mut hooks: serde_json::Value = serde_json::from_str(&read(&hooks_path(&c))).unwrap();
    hooks["hooks"]["PreCompact"] = serde_json::json!([
        { "hooks": [ { "type": "command", "command": "some-other-tool" } ] }
    ]);
    std::fs::write(
        hooks_path(&c),
        serde_json::to_string_pretty(&hooks).unwrap(),
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(hooks_path(&c).exists(), "{out}");
    let after: serde_json::Value = serde_json::from_str(&read(&hooks_path(&c))).unwrap();
    assert_eq!(
        after["hooks"]["PreCompact"][0]["hooks"][0]["command"], "some-other-tool",
        "{after}"
    );
    assert!(after["hooks"].get("SessionStart").is_none(), "{after}");
    assert!(after["hooks"].get("Stop").is_none(), "{after}");
    assert!(after["hooks"].get("UserPromptSubmit").is_none(), "{after}");
}

/// Test 16: a root-level `description` in `hooks.json`, with only our own
/// hooks otherwise, survives `--undo`; `hooks` itself goes since nothing of
/// ours is left in it.
#[test]
fn undo_over_hooks_json_with_a_root_description_keeps_it_and_drops_hooks() {
    let c = Sandbox::new_empty("setup-codex-undo-hooks-description");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    let mut hooks: serde_json::Value = serde_json::from_str(&read(&hooks_path(&c))).unwrap();
    hooks["description"] = serde_json::json!("our own hooks");
    std::fs::write(
        hooks_path(&c),
        serde_json::to_string_pretty(&hooks).unwrap(),
    )
    .unwrap();

    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(hooks_path(&c).exists(), "{out}");
    let after: serde_json::Value = serde_json::from_str(&read(&hooks_path(&c))).unwrap();
    assert_eq!(after["description"], "our own hooks", "{after}");
    assert!(after.get("hooks").is_none(), "{after}");
}

/// Test 17: a skill changed since `setup` wrote it is left alone, and the
/// plan says why.
#[test]
fn undo_leaves_a_hand_edited_skill_and_says_so() {
    let c = Sandbox::new_empty("setup-codex-undo-skill-edited");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    let skill = read(&skill_path(&c));
    std::fs::write(skill_path(&c), format!("{skill}\nedited by hand\n")).unwrap();

    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        plan_words(&out).contains("changed since setup wrote it; left as it is"),
        "{out}"
    );
    assert!(skill_path(&c).exists(), "the edited skill was removed");
}

/// Test 18: a `config.toml` with the opening marker but not the closing one
/// is not `--undo`'s to guess the end of -- it names the file, says which
/// marker is missing, and leaves the file untouched, while the other two
/// files it does recognise still go.
#[test]
fn undo_leaves_a_config_toml_with_a_half_written_marker_and_says_so() {
    let c = Sandbox::new_empty("setup-codex-undo-config-half-marker");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    let broken = read(&config_path(&c)).replace("# end of what vivac setup codex added\n", "");
    std::fs::write(config_path(&c), &broken).unwrap();

    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(CONFIG_LABEL), "{out}");
    // A status line in the plan's own grid, and the missing marker under
    // it, rather than the paragraph `apply` refuses with: that one ends by
    // saying to run setup again, which is not what this reader asked for.
    assert!(
        plan_words(&out).contains("left as it is: its marker block is half written"),
        "{out}"
    );
    assert!(
        out.contains("# end of what vivac setup codex added"),
        "{out}"
    );
    assert_eq!(
        read(&config_path(&c)),
        broken,
        "config.toml must be untouched"
    );
    assert!(
        !hooks_path(&c).exists(),
        "hooks.json should have been removed"
    );
    assert!(
        !skill_path(&c).exists(),
        "the skill should have been removed"
    );
}

/// `f718`, this harness's half: `--undo` used to reach its own question
/// with nobody there to answer it, remove nothing and exit 0. The command
/// it hands back keeps the `--undo` it was given, or it would be advice
/// for the opposite run (`f675`).
#[test]
fn no_terminal_and_no_yes_refuses_an_undo_too_and_removes_nothing() {
    let c = Sandbox::new_empty("setup-codex-no-terminal-undo");
    c.ok(&["init", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    let (out, code) = c.run(&["setup", "codex", "--undo"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("there is no terminal to ask"), "{out}");
    assert!(out.contains("vivac setup codex --undo --dry-run"), "{out}");
    assert!(out.contains("vivac setup codex --undo --yes"), "{out}");
    assert!(
        config_path(&c).is_file(),
        "the run removed something it had not been allowed to confirm"
    );
    assert!(hooks_path(&c).is_file(), "{out}");
    assert!(skill_path(&c).is_file(), "{out}");
}
