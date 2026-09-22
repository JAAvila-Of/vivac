//! `vivac setup codex` — tranche 1 (`t592`) covers a clean project, none of
//! the three files Codex reads there yet. Tranche 2 adds merging with a
//! file already there (piece B, `t592` §4), running it twice (piece D,
//! §6), and taking it off again (piece C, §5).

mod common;
use common::Sandbox;

const SESSION_START: &str = "vivac session start --hook";
const SESSION_END: &str = "vivac session end --hook";

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
    let before: Vec<_> = std::fs::read_dir(&c.0).unwrap().collect();
    let (out, code) = c.run(&["setup", "codex", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("vivac setup codex, in"), "{out}");
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
}

/// The skill is the same file `claude-code` writes, at a different path: not
/// a second copy that can drift from it (`d653`).
#[test]
fn the_skill_is_byte_for_byte_the_one_claude_code_writes() {
    let c = Sandbox::new_empty("setup-codex-skill");
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
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    std::fs::write(config_path(&c), "# hand-written\nsomething = 1\n").unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("add: the \"vivac\" server"), "{out}");
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
}

/// Test 9: `hooks.json` that does not parse refuses, and writes neither the
/// TOML nor the skill -- all or nothing stays all or nothing.
#[test]
fn broken_hooks_json_refuses_and_writes_nothing() {
    let c = Sandbox::new_empty("setup-codex-hooks-broken");
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
// 7. `t592` tranche 2 (`d710`): `--new-tree`, `--lane-name` and `--name` are
//    the tree's own flags, not the harness's, so they behave exactly as
//    they do for `claude-code` instead of being refused.
// ---------------------------------------------------------------------------

/// `t592` tranche 2 (`d710`): `--new-tree`, `--lane-name` and `--name` are
/// the tree's own flags, not the harness's, so they stop being refused and
/// behave exactly as they do for `claude-code`. `--join` joined this
/// harness too in piece G of the same tranche (`f714`); `tests/setup_scenarios.rs`
/// covers it.
#[test]
fn new_tree_is_accepted_and_plants_the_tree() {
    let c = Sandbox::new_empty("setup-codex-flag-new-tree");
    let (out, code) = c.run(&["setup", "codex", "--yes", "--new-tree"]);
    assert_eq!(code, 0, "{out}");
    assert!(c.0.join(".vivac").exists(), "the tree was not planted");
}

#[test]
fn lane_name_names_the_lane_it_declares() {
    let c = Sandbox::new_empty("setup-codex-flag-lane-name");
    let (out, code) = c.run(&["setup", "codex", "--yes", "--lane-name", "custom-name"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        read(&c.0.join(".vivac").join("events")).contains("\"name\":\"custom-name\""),
        "{out}"
    );
}

/// `t640`: `--name` is claude-code's own too, and now this harness's as
/// well -- it saves the product's name the same way, rather than being
/// refused or silently ignored.
#[test]
fn name_names_the_product_it_plants() {
    let c = Sandbox::new_empty("setup-codex-flag-name");
    let (out, code) = c.run(&["setup", "codex", "--yes", "--name", "IQuorum"]);
    assert_eq!(code, 0, "{out}");
    let registry = read(&c.global_home().join("projects"));
    assert!(registry.contains("\"name\": \"IQuorum\""), "{registry}");
}

// ---------------------------------------------------------------------------
// 8. `t592` tranche 2 (`d710`): the fourth piece -- planting the tree the
//    same way `claude-code` does, from the module both harnesses share
//    (`src/setup/tree.rs`).
// ---------------------------------------------------------------------------

/// The tree's own lines of a plan, told apart from the harness's own --
/// `.vivac/`, `.vivac/lane`, `.vivac/events`, and the `config` lock line,
/// which is the fourth piece `t592` tranche 1 left out (`f705`).
fn tree_lines(out: &str) -> Vec<&str> {
    out.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with(".vivac/") || t.starts_with("config")
        })
        .collect()
}

/// `f705`: `setup codex --dry-run` used to show three lines where
/// `claude-code` showed six, because it never touched the tree at all.
/// Point 2 of `d710` §3: both plans now name the same tree, in the same
/// words -- checked on the tree's own lines, not the whole output, since
/// the two harnesses' own pieces are not the same and were never meant to
/// be.
#[test]
fn dry_run_shows_the_same_tree_lines_claude_code_does() {
    let c = Sandbox::new_empty("setup-codex-tree-lines");
    let (claude_out, claude_code) = c.run(&["setup", "claude-code", "--dry-run"]);
    assert_eq!(claude_code, 0, "{claude_out}");
    let (codex_out, codex_code) = c.run(&["setup", "codex", "--dry-run"]);
    assert_eq!(codex_code, 0, "{codex_out}");

    let claude_tree = tree_lines(&claude_out);
    let codex_tree = tree_lines(&codex_out);
    assert!(!claude_tree.is_empty(), "{claude_out}");
    assert_eq!(
        claude_tree, codex_tree,
        "codex's own tree lines drifted from claude-code's:\n\
         claude-code: {claude_tree:?}\ncodex: {codex_tree:?}"
    );

    // And where they sit, not only that they are there: the same lines in
    // the same order still read as an afterthought if all three arrive
    // after this harness's own pieces, which is what they were until
    // `f705`. The ground is named first and what the run records about it
    // last, exactly as `claude-code` has always placed them.
    let at = |needle: &str| {
        codex_out
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} is missing from the plan:\n{codex_out}"))
    };
    assert!(
        at(".vivac/ ") < at(".codex/config.toml"),
        "planting the tree is announced after the files that need it:\n{codex_out}"
    );
    assert!(
        at(".agents/skills") < at(".vivac/events"),
        "what the run records about the tree is announced before the pieces:\n{codex_out}"
    );
}

/// Point 3: a clean plant leaves the tree exactly as `claude-code` would --
/// planted, its own `.gitignore`, its own version lock, and a lane `stack
/// --lanes` already knows about.
#[test]
fn a_clean_plant_also_plants_the_tree() {
    let c = Sandbox::new_empty("setup-codex-plants-tree");
    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{out}");

    assert!(c.0.join(".vivac").join("events").is_file(), "{out}");
    assert!(c.0.join(".vivac").join(".gitignore").is_file(), "{out}");
    assert!(c.0.join(".vivac").join("config").is_file(), "{out}");

    let stack = c.ok(&["stack", "--lanes"]);
    let folder_name = c.0.file_name().unwrap().to_string_lossy().into_owned();
    assert!(stack.contains(&folder_name), "{stack}");
}

/// Point 4: the one thing `f705` actually cost -- with no tree, the start
/// hook stays quiet forever. Run by hand right after `setup codex --yes`,
/// it now prints the brief instead.
#[test]
fn after_a_clean_plant_the_start_hook_prints_a_brief_instead_of_staying_quiet() {
    let c = Sandbox::new_empty("setup-codex-start-hook-brief");
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

/// The folder's own top-level entries, `.vivac/` aside: enough to prove
/// `--undo` leaves nothing of its own behind, the same shallow listing
/// `tests/setup.rs`'s own `list` takes for the same reason.
fn list_without_the_tree(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != ".vivac")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Test 12: a clean plant undone leaves the folder exactly as it was --
/// `.vivac/` aside, which stays -- checked against the folder's entire
/// listing rather than just the three files `setup` itself knows about.
#[test]
fn undo_after_a_clean_setup_leaves_the_folder_as_it_was_except_the_tree() {
    let c = Sandbox::new_empty("setup-codex-undo-clean");
    let before = list_without_the_tree(&c.0);
    c.ok(&["setup", "codex", "--yes"]);

    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Undone. The tree in .vivac/ is untouched."),
        "{out}"
    );
    assert_eq!(list_without_the_tree(&c.0), before, "{out}");
    assert!(c.0.join(".vivac").exists(), "the tree must stay");
}

/// Test 13: a foreign `config.toml` from before the first run comes back
/// byte for byte once `--undo` takes our block back off.
#[test]
fn undo_over_a_foreign_config_toml_returns_it_to_the_original_bytes() {
    let c = Sandbox::new_empty("setup-codex-undo-config-foreign");
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
}

/// Test 16: a root-level `description` in `hooks.json`, with only our own
/// hooks otherwise, survives `--undo`; `hooks` itself goes since nothing of
/// ours is left in it.
#[test]
fn undo_over_hooks_json_with_a_root_description_keeps_it_and_drops_hooks() {
    let c = Sandbox::new_empty("setup-codex-undo-hooks-description");
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
    c.ok(&["setup", "codex", "--yes"]);
    let skill = read(&skill_path(&c));
    std::fs::write(skill_path(&c), format!("{skill}\nedited by hand\n")).unwrap();

    let (out, code) = c.run(&["setup", "codex", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("changed since setup wrote it; left as it is"),
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
        out.contains("left as it is: its marker block is half written"),
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
