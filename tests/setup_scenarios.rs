//! Scenario tests for `vivac setup`: situations the per-harness suites
//! never assembled because each of them only ever ran one harness at a
//! time (`t592` tranche 2, piece G, `f714`). These mount the whole
//! situation and look at the result, not at one function: both harnesses
//! in the same folder, in either order; one product spread across two
//! folders, one harness per folder, joined across them; and the
//! second-map refusal read from each harness in turn, to prove it
//! proposes a command that harness actually takes.

mod common;
use common::Sandbox;
use std::path::Path;

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

fn ok_in(dir: &Path, home: &Path, args: &[&str]) -> String {
    let (out, code) = run_in(dir, home, args);
    assert_eq!(
        code,
        0,
        "`vivac {}` failed with {code}:\n{out}",
        args.join(" ")
    );
    out
}

/// `store::already_planted`'s own definition, copied here rather than
/// asked of the crate because an integration test has no `pub` path to it
/// -- the same reason `tests/setup.rs` and `tests/lanes.rs` each carry
/// their own copy.
fn already_planted(dir: &Path) -> bool {
    dir.join(".vivac").join("config").is_file() || dir.join(".vivac").join("events").is_file()
}

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("reading {p:?}: {e}"))
}

fn claude_code_skill(dir: &Path) -> std::path::PathBuf {
    dir.join(".claude")
        .join("skills")
        .join("vivac-migrate")
        .join("SKILL.md")
}

fn codex_skill(dir: &Path) -> std::path::PathBuf {
    dir.join(".agents")
        .join("skills")
        .join("vivac-migrate")
        .join("SKILL.md")
}

// ---------------------------------------------------------------------------
// 1-2: both harnesses set up in the same folder, in either order, land on
// the same tree and the same lane -- the only existing test that mixed
// harnesses at all checked the skill file and nothing else.
// ---------------------------------------------------------------------------

fn assert_both_harnesses_share_one_tree_and_one_lane(c: &Sandbox) {
    assert!(
        c.0.join(".vivac").join("events").is_file(),
        "no tree was planted"
    );
    assert!(c.0.join(".claude").join("settings.json").is_file());
    assert!(c.0.join(".mcp.json").is_file());
    assert!(claude_code_skill(&c.0).is_file());
    assert!(c.0.join(".codex").join("config.toml").is_file());
    assert!(c.0.join(".codex").join("hooks.json").is_file());
    assert!(codex_skill(&c.0).is_file());

    let lanes_json = c.ok(&["stack", "--lanes", "--json"]);
    let lanes: serde_json::Value = serde_json::from_str(&lanes_json).unwrap();
    let count = lanes["lanes"].as_array().unwrap().len();
    assert_eq!(count, 1, "expected exactly one lane, in:\n{lanes_json}");

    assert_eq!(
        read(&claude_code_skill(&c.0)),
        read(&codex_skill(&c.0)),
        "the two harnesses wrote a different skill"
    );
}

#[test]
fn both_harnesses_in_one_folder_share_one_tree_and_one_lane() {
    let c = Sandbox::new_empty("scenarios-both-cc-first");
    c.ok(&["setup", "claude-code", "--yes"]);
    c.ok(&["setup", "codex", "--yes"]);
    assert_both_harnesses_share_one_tree_and_one_lane(&c);
}

#[test]
fn the_other_order_lands_in_the_same_place() {
    let c = Sandbox::new_empty("scenarios-both-codex-first");
    c.ok(&["setup", "codex", "--yes"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    assert_both_harnesses_share_one_tree_and_one_lane(&c);
}

// ---------------------------------------------------------------------------
// 3: one product, two folders, one harness each, joined across them
// (`--join` in Codex, piece G's own point 2). Also covers point 5: this
// join is not refused by name any more.
// ---------------------------------------------------------------------------

#[test]
fn a_product_in_two_folders_takes_one_harness_each() {
    let c = Sandbox::new_empty("scenarios-two-folders-one-product");
    let a = c.0.join("FolderA");
    std::fs::create_dir_all(&a).unwrap();
    let (setup_out, setup_code) = run_in(
        &a,
        c.global_home(),
        &["setup", "claude-code", "--yes", "--name", "Producto"],
    );
    assert_eq!(setup_code, 0, "{setup_out}");

    let b = c.0.join("FolderB");
    std::fs::create_dir_all(&b).unwrap();
    let a_str = a.to_string_lossy().into_owned();
    let (join_out, join_code) = run_in(
        &b,
        c.global_home(),
        &["setup", "codex", "--yes", "--join", &a_str],
    );
    assert_eq!(join_code, 0, "{join_out}");
    // Point 5: `setup codex --join` used to refuse by name before ever
    // reaching the tree.
    assert!(!join_out.contains("does not take --join yet"), "{join_out}");

    assert!(a.join(".vivac").join("events").is_file(), "no tree at A");
    assert!(!already_planted(&b), "B grew a tree of its own");
    // A join used to leave the harness's own files undone (`f667`/`f669`),
    // which is the whole reason it walks the write path at all: the folder
    // that joined still needs the hooks, the server and the skill waiting
    // for the moment a session opens in it.
    assert!(
        b.join(".codex").join("config.toml").is_file(),
        "the join left B without the server"
    );
    assert!(
        b.join(".codex").join("hooks.json").is_file(),
        "the join left B without the hooks"
    );
    assert!(
        codex_skill(&b).is_file(),
        "the join left B without the skill"
    );

    let lanes_json = ok_in(&a, c.global_home(), &["stack", "--lanes", "--json"]);
    let lanes: serde_json::Value = serde_json::from_str(&lanes_json).unwrap();
    let names: Vec<&str> = lanes["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"FolderA"), "{lanes_json}");
    assert!(names.contains(&"FolderB"), "{lanes_json}");

    let a_brief = ok_in(&a, c.global_home(), &["brief"]);
    assert!(a_brief.contains("lane: FolderA"), "{a_brief}");
    let b_brief = ok_in(&b, c.global_home(), &["brief"]);
    assert!(b_brief.contains("lane: FolderB"), "{b_brief}");
}

// ---------------------------------------------------------------------------
// 4: the second-map refusal proposes commands of the harness it was asked
// as, not always `claude-code` (`f714`, the defect this piece fixes).
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

/// A second working copy of `src` that shares its root commit -- the same
/// clue the second-map guard reads to recognise two folders as the same
/// product.
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

#[test]
fn the_second_map_refusal_proposes_commands_of_the_harness_it_was_asked_as() {
    let c = Sandbox::new_empty("scenarios-second-map-harness-word");
    let a = c.0.join("FolderA");
    real_git_repo(&a.join("repo"));
    let (setup_out, setup_code) = run_in(
        &a,
        c.global_home(),
        &["setup", "claude-code", "--yes", "--name", "Producto"],
    );
    assert_eq!(setup_code, 0, "{setup_out}");

    let b = c.0.join("FolderB");
    clone_repo(&a.join("repo"), &b.join("repo"));

    let (codex_out, codex_code) = run_in(&b, c.global_home(), &["setup", "codex"]);
    assert_eq!(codex_code, 1, "{codex_out}");
    assert!(
        codex_out.contains("vivac setup codex --join"),
        "{codex_out}"
    );
    assert!(
        codex_out.contains("vivac setup codex --new-tree"),
        "{codex_out}"
    );
    assert!(!codex_out.contains("setup claude-code"), "{codex_out}");

    let (claude_code_out, claude_code_code) =
        run_in(&b, c.global_home(), &["setup", "claude-code"]);
    assert_eq!(claude_code_code, 1, "{claude_code_out}");
    assert!(
        claude_code_out.contains("vivac setup claude-code --join"),
        "{claude_code_out}"
    );
    assert!(
        claude_code_out.contains("vivac setup claude-code --new-tree"),
        "{claude_code_out}"
    );
    assert!(
        !claude_code_out.contains("setup codex"),
        "{claude_code_out}"
    );
}

// ---------------------------------------------------------------------------
// 5: `f720` -- no line of a printed plan is wider than the two-column block
// itself draws, for either harness, across the shapes that plan takes.
// ---------------------------------------------------------------------------

/// A lane's name comes straight from its folder's own name and a
/// product's from whoever runs `--name` (up to 100 characters,
/// `tree::NAME_MAX_LEN`), and neither one has a bound of its own -- the
/// whole reason `f720` wraps a status instead of trusting it to fit. A
/// short fixture would not tell the two apart: the record and lock lines
/// `f720`'s own spec measured already overflowed at names four
/// characters long, so what this is actually proving is that the wrap
/// holds once a name is long enough to span several lines of its own,
/// not just spill one word past the label. Built from several words
/// rather than one long one so the words -- not this test -- decide
/// where the wrap breaks.
fn long_name(tag: &str) -> String {
    format!("A Name Chosen On Purpose To Run Longer Than One Line Of The Plan Could Hold, {tag}")
}

/// Every printed line of `out` fits in the block `piece_line` draws,
/// except the two shapes `f720`'s own spec says stay whole: the header
/// line, which names a path with no bound of its own either, and a
/// `sub_line`, a command or a path that is not this test's to word-wrap.
/// Skipped by what they are -- the first line, and eight-space indent --
/// not by their text.
fn assert_no_plan_line_is_wider_than_the_block(out: &str) {
    const SUB_LINE_INDENT: usize = 8;
    for line in out.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent == SUB_LINE_INDENT || trimmed.starts_with("vivac setup") {
            continue;
        }
        assert!(
            line.chars().count() <= 76,
            "a plan line ran past 76 columns: {line:?}\nfull output:\n{out}"
        );
    }
}

/// `--dry-run` prints exactly the plan `f720`'s own spec measured, with
/// nothing written and nothing appended after it -- the shape this test
/// checks, isolated from the harness-specific prose a real write adds
/// afterward (Codex's own trust instructions, for one), which `f720`
/// never claimed to bound.
fn dry_run_plan(dir: &Path, home: &Path, args: &[&str]) -> String {
    let mut full = vec!["setup"];
    full.extend_from_slice(args);
    full.push("--dry-run");
    let (out, code) = run_in(dir, home, &full);
    assert_eq!(code, 0, "`vivac {}` failed:\n{out}", full.join(" "));
    assert_no_plan_line_is_wider_than_the_block(&out);
    out
}

#[test]
fn no_plan_line_is_wider_than_the_block() {
    for harness in ["claude-code", "codex"] {
        let c = Sandbox::new_empty(&format!("scenarios-plan-width-{harness}"));

        // Plant: a fresh tree, in a folder named long and declared under
        // a product name just as long.
        let planted = c.0.join(long_name("plant"));
        std::fs::create_dir_all(&planted).unwrap();
        let product = long_name("plant product");
        dry_run_plan(&planted, c.global_home(), &[harness, "--name", &product]);
        ok_in(
            &planted,
            c.global_home(),
            &["setup", harness, "--yes", "--name", &product],
        );

        // The same folder again: nothing left to write.
        dry_run_plan(&planted, c.global_home(), &[harness]);

        // `--join`: a second, long-named folder joining the tree above.
        let joining = c.0.join(long_name("join"));
        std::fs::create_dir_all(&joining).unwrap();
        let planted_str = planted.to_string_lossy().into_owned();
        dry_run_plan(
            &joining,
            c.global_home(),
            &[harness, "--join", &planted_str],
        );

        // A file setup never wrote, already foreign to the plan.
        let foreign = c.0.join(long_name("merge"));
        std::fs::create_dir_all(&foreign).unwrap();
        seed_foreign_file(harness, &foreign);
        dry_run_plan(
            &foreign,
            c.global_home(),
            &[harness, "--name", &long_name("merge product")],
        );

        // `--undo`, on the folder that actually planted.
        dry_run_plan(&planted, c.global_home(), &[harness, "--undo"]);
    }
}

/// A file setup did not write, already in place before it ever runs, so
/// the plan reports a merge ("add") rather than a plant ("create").
fn seed_foreign_file(harness: &str, dir: &Path) {
    match harness {
        "claude-code" => {
            std::fs::create_dir_all(dir.join(".claude")).unwrap();
            std::fs::write(
                dir.join(".claude").join("settings.json"),
                "{\n  \"otherKey\": true\n}\n",
            )
            .unwrap();
        }
        "codex" => {
            std::fs::create_dir_all(dir.join(".codex")).unwrap();
            std::fs::write(
                dir.join(".codex").join("config.toml"),
                "# hand-written\nsomething = 1\n",
            )
            .unwrap();
        }
        other => unreachable!("no third harness: {other}"),
    }
}
