//! `vivac setup codex` — `t592`, tranche 1: a clean project, none of the
//! three files Codex reads there yet. Merging with a file already there,
//! `--undo` and running it twice are tranche 2 and are not this file's job
//! (`d653`).

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
// 5. A file already there: tranche 1 stops rather than merging (`t592`).
// ---------------------------------------------------------------------------

#[test]
fn a_file_already_there_stops_instead_of_guessing_how_to_merge() {
    let c = Sandbox::new_empty("setup-codex-conflict");
    std::fs::create_dir_all(c.0.join(".codex")).unwrap();
    std::fs::write(config_path(&c), "# hand-written\n").unwrap();

    let (out, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(CONFIG_LABEL), "{out}");
    assert_eq!(
        read(&config_path(&c)),
        "# hand-written\n",
        "the existing file was touched"
    );
    assert!(!hooks_path(&c).exists(), "hooks.json was written anyway");
    assert!(!skill_path(&c).exists(), "SKILL.md was written anyway");
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
// 7. A flag this harness does not know yet refuses before writing anything
//    (`t592` tranche 2): the same usage error the rest of the module gives
//    a flag it does not take, not the silence that once let `--undo` write
//    the three files it was asked to remove.
// ---------------------------------------------------------------------------

fn assert_nothing_was_written(c: &Sandbox) {
    assert!(!config_path(c).exists(), "config.toml was written anyway");
    assert!(!hooks_path(c).exists(), "hooks.json was written anyway");
    assert!(!skill_path(c).exists(), "SKILL.md was written anyway");
}

#[test]
fn undo_refuses_before_writing_anything() {
    let c = Sandbox::new_empty("setup-codex-flag-undo");
    let (out, code) = c.run(&["setup", "codex", "--yes", "--undo"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--undo"), "{out}");
    assert_nothing_was_written(&c);
}

/// `t592` tranche 2 (`d710`): `--new-tree`, `--lane-name` and `--name` are
/// the tree's own flags, not the harness's, so they stop being refused and
/// behave exactly as they do for `claude-code`. `--join` joined this
/// harness too in piece G of the same tranche (`f714`); `tests/setup_scenarios.rs`
/// covers it. `--undo` is not this tranche's (`d710` §3) and still refuses
/// above.
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
