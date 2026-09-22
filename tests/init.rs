//! `f566`: `init` on a tree that already exists must not touch its config.
//!
//! `Store::create` used to run unconditionally: a second `init` regenerated
//! the config with a fresh `project_id` and `actor`, and dropped `d444`'s
//! lock back to `1` on a tree that already held a pillar or a rule.

mod common;
use common::Sandbox;

const LOCK_SENTENCE: &str =
    "this tree holds pillars and rules, and this vivac is too old to read them: update vivac";

fn config_bytes(c: &Sandbox) -> Vec<u8> {
    std::fs::read(c.0.join(".vivac").join("config")).unwrap()
}

fn log_bytes(c: &Sandbox) -> Vec<u8> {
    std::fs::read(c.0.join(".vivac").join("events")).unwrap()
}

fn is_locked(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes).contains(LOCK_SENTENCE)
}

/// `f566`, reproduced exactly: init, a rule, init again. Config and log come
/// out byte for byte the same, lock included.
#[test]
fn a_second_init_leaves_a_locked_config_and_log_untouched() {
    let c = Sandbox::new_seeded("init-twice");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    let config_before = config_bytes(&c);
    let log_before = log_bytes(&c);
    assert!(
        is_locked(&config_before),
        "setup: the rule should have locked the config"
    );

    let out = c.ok(&["init"]);
    assert!(out.contains("vivac is already planted in"), "{out}");
    assert_eq!(config_before, config_bytes(&c), "the config moved");
    assert_eq!(log_before, log_bytes(&c), "the log moved");
}

/// An empty `.vivac/` holds no tree yet, so `init` plants one there rather
/// than calling it already planted.
#[test]
fn init_over_an_empty_vivac_directory_plants_a_tree() {
    let c = Sandbox::new_empty("init-empty-dir");
    std::fs::create_dir_all(c.0.join(".vivac")).unwrap();
    let out = c.ok(&["init"]);
    assert!(out.contains("vivac planted in"), "{out}");
    assert!(c.0.join(".vivac").join("config").is_file());
    assert!(c.0.join(".vivac").join("events").is_file());
}

/// `t594` §4.9: planting a tree writes `.vivac/.gitignore` alongside the
/// config and the log, so a fresh tree is never one `git add .` away from
/// being tracked.
#[test]
fn init_keeps_the_tree_out_of_version_control() {
    let c = Sandbox::new_seeded("gitignore");
    let g = std::fs::read_to_string(c.0.join(".vivac").join(".gitignore")).unwrap();
    assert_eq!(g, "*\n");
}

/// `init` opens rather than creates when there is something to open: a
/// `.vivac/` with a log and no config regenerates through `Store::open`,
/// which locks the regenerated config if the log already holds a rule.
#[test]
fn init_over_a_log_with_no_config_regenerates_it_locked() {
    let c = Sandbox::new_seeded("init-no-config");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    std::fs::remove_file(c.0.join(".vivac").join("config")).unwrap();

    c.ok(&["init"]);
    assert!(
        is_locked(&config_bytes(&c)),
        "a config regenerated over a governed log came back unlocked"
    );
}

// ---------------------------------------------------------------------------
// `d723` piece A: `init` grows the same six flags `setup <harness>` already
// had, walking the same `tree.rs` path `setup` already walks. `mod common`
// is compiled once per test binary; `run_in`, `real_git_repo` and
// `clone_repo` are this file's own small copies of `tests/setup.rs`'s
// fixtures, the same duplication that file and `tests/lanes.rs` already
// keep between each other rather than share.
// ---------------------------------------------------------------------------

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

fn real_git_repo(at: &std::path::Path) {
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

fn clone_repo(src: &std::path::Path, destination: &std::path::Path) {
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

fn already_planted(dir: &std::path::Path) -> bool {
    dir.join(".vivac").join("config").is_file() || dir.join(".vivac").join("events").is_file()
}

/// `--dry-run` plans a plant and writes nothing, the same promise `setup`
/// already keeps: the plan names `.vivac/` itself, and nothing lands on
/// disk.
#[test]
fn init_dry_run_plans_a_plant_and_writes_nothing() {
    let c = Sandbox::new_empty("init-dry-run");
    let out = c.ok(&["init", "--dry-run"]);
    assert!(out.contains("plant the tree"), "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(!already_planted(&c.0), "a dry run must not plant");
}

/// `--yes` walks the same `tree.rs` path `setup` already walks: it plants,
/// and declares this folder's own founding lane in the same write, the one
/// piece a bare `vivac init` leaves for the first `push` to do instead
/// (`f566`).
#[test]
fn init_yes_plants_and_declares_the_founding_lane() {
    let c = Sandbox::new_empty("init-yes");
    let out = c.ok(&["init", "--yes"]);
    assert!(out.contains("Written."), "{out}");
    assert!(c.0.join(".vivac").join("config").is_file());
    let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    assert!(log.contains("\"type\":\"lane.declared\""), "{log}");
}

/// `--name` saves the product's own name to the registry, the same write
/// `setup --name` already makes (`t640`).
#[test]
fn init_name_saves_the_product_to_the_registry() {
    let c = Sandbox::new_empty("init-name");
    c.ok(&["init", "--yes", "--name", "IQuorum"]);
    let registry = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert!(registry.contains("\"name\": \"IQuorum\""), "{registry}");
}

/// `f724`: a message that splices in a name has to reach its width through
/// `render::wrap`, never a break placed by hand before the name was ever
/// typed. `--name` colliding with another project's is the one line of
/// `init`'s own plan that carries one at all (`t640`, point 10 bis), so a
/// name long enough to run past 76 columns on its own is what proves the
/// break survives it.
#[test]
fn init_name_collision_wraps_a_long_name_rather_than_running_past_the_width() {
    let c = Sandbox::new_empty("init-name-collision-width");
    let first = c.0.join("first");
    std::fs::create_dir_all(&first).unwrap();
    let long = "A Name Chosen On Purpose To Run Longer Than One Line Of The Plan Could Hold";
    run_in(&first, c.global_home(), &["init", "--yes", "--name", long]);

    let second = c.0.join("second");
    std::fs::create_dir_all(&second).unwrap();
    let (out, code) = run_in(
        &second,
        c.global_home(),
        &["init", "--dry-run", "--name", long],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("already names another project"), "{out}");
    // The header line names a path with no bound of its own, and a
    // `sub_line` -- eight spaces in -- carries a path or a command that is
    // not this test's to word-wrap either: `assert_no_plan_line_is_wider_than_the_block`
    // in `tests/setup_scenarios.rs` skips the very same two shapes, by what
    // they are rather than by their text.
    const SUB_LINE_INDENT: usize = 8;
    for line in out.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent == SUB_LINE_INDENT || trimmed.starts_with("vivac init") {
            continue;
        }
        assert!(
            line.chars().count() <= 76,
            "a plan line ran past 76 columns: {line:?}\nfull output:\n{out}"
        );
    }
}

/// `--lane-name` names the founding lane instead of falling back to the
/// folder's own name.
#[test]
fn init_lane_name_names_the_founding_lane() {
    let c = Sandbox::new_empty("init-lane-name");
    c.ok(&["init", "--yes", "--lane-name", "custom-name"]);
    let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    assert!(log.contains("\"name\":\"custom-name\""), "{log}");
}

/// `--join` declares this folder as a lane of the target tree, rather than
/// planting a second one -- the same write `setup --join` already makes.
#[test]
fn init_join_declares_a_lane_of_the_target_rather_than_planting() {
    let target = Sandbox::new_empty("init-join-target");
    target.ok(&["init", "--yes"]);

    let joiner = Sandbox::new_empty("init-join-joiner");
    let out = joiner.ok(&["init", "--join", target.0.to_str().unwrap(), "--yes"]);
    assert!(out.contains("becomes"), "{out}");
    assert!(joiner.0.join(".vivac").join("lane").is_file());
    assert!(
        !already_planted(&joiner.0),
        "a join must not plant a second tree"
    );
}

/// `--undo` takes off exactly what `--join` wrote in the joiner's own
/// folder: its lane file, and the `.vivac/` it holds nothing else beside.
/// The target tree's own log is never rolled back -- "the log only ever
/// grows" (`tree.rs`'s own words): the joiner's `lane.declared` event stays
/// in it, the same way any other event a lane once wrote would.
#[test]
fn init_undo_removes_a_joined_lane_and_leaves_the_target_log_growing_only() {
    let target = Sandbox::new_empty("init-undo-target");
    target.ok(&["init", "--yes"]);
    let target_log_before =
        std::fs::read_to_string(target.0.join(".vivac").join("events")).unwrap();

    let joiner = Sandbox::new_empty("init-undo-joiner");
    joiner.ok(&["init", "--join", target.0.to_str().unwrap(), "--yes"]);
    assert!(joiner.0.join(".vivac").join("lane").is_file());

    let dry = joiner.ok(&["init", "--undo", "--dry-run"]);
    assert!(dry.contains("remove"), "{dry}");
    assert!(
        joiner.0.join(".vivac").join("lane").is_file(),
        "dry-run undid something"
    );

    let out = joiner.ok(&["init", "--undo", "--yes"]);
    assert!(out.contains("Undone."), "{out}");
    assert!(
        !joiner.0.join(".vivac").exists(),
        "a joined .vivac/ holding only the lane should go"
    );
    let target_log_after = std::fs::read_to_string(target.0.join(".vivac").join("events")).unwrap();
    assert!(
        target_log_after.starts_with(&target_log_before),
        "the target's own pre-join history must survive the joiner's --undo untouched"
    );
    assert!(
        target_log_after.contains("\"lane.declared\""),
        "the join's own event should stay in the target's log: {target_log_after}"
    );
}

/// The second-map guard (`refuse_second_map`), reached from `init` exactly
/// as it already is from `setup`: two folders whose repository shares a
/// root commit with an already-registered project refuse rather than plant
/// a second map of it.
#[test]
fn init_refuses_a_second_map_the_same_way_setup_does() {
    let c = Sandbox::new_empty("init-second-map");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    let (setup_out, setup_code) = run_in(&first, c.global_home(), &["init", "--yes"]);
    assert_eq!(setup_code, 0, "{setup_out}");

    let second = c.0.join("Prod-fork");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("Planting another tree would give this product two maps."),
        "{out}"
    );
    assert!(
        !already_planted(&second),
        "a tree was planted despite the guard"
    );
}

/// `--new-tree` bypasses the second-map guard above, the same escape
/// `setup --new-tree` already gives.
#[test]
fn init_new_tree_bypasses_the_second_map_guard() {
    let c = Sandbox::new_empty("init-new-tree");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("Prod-fork");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--new-tree", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(already_planted(&second));
}

/// `f721`, fixed at the root: planting with `init` and planting with
/// `setup` leave the same tree -- the same set of files under `.vivac/`,
/// a founding lane declared, and the version lock set in both cases. This
/// is the equivalence piece B leans on to retire `setup`'s own copy of the
/// planting path: once this holds, only one of the two needs to keep it.
#[test]
fn planting_with_init_and_planting_with_setup_leave_the_same_tree() {
    let via_init = Sandbox::new_empty("equiv-init");
    via_init.ok(&["init", "--yes"]);

    let via_setup = Sandbox::new_empty("equiv-setup");
    via_setup.ok(&["setup", "claude-code", "--yes"]);

    let vivac_file_names = |c: &Sandbox| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(c.0.join(".vivac"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    // `setup claude-code` also writes `.claude/` and `.mcp.json` beside
    // `.vivac/`, which is exactly what this piece keeps out of `init`'s own
    // reach -- so the comparison is scoped to `.vivac/` itself, the one
    // thing both commands promise to leave the same.
    assert_eq!(
        vivac_file_names(&via_init),
        vivac_file_names(&via_setup),
        "init and setup left a different .vivac/ behind"
    );

    for c in [&via_init, &via_setup] {
        let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
        assert!(
            log.contains("\"type\":\"lane.declared\""),
            "no founding lane declared for {:?}: {log}",
            c.0
        );
        let config = std::fs::read_to_string(c.0.join(".vivac").join("config")).unwrap();
        assert!(
            config.contains("this tree holds lanes"),
            "the version lock was not set for {:?}: {config}",
            c.0
        );
        assert!(
            !c.0.join(".vivac").join("lane").is_file(),
            "the tree's own root folder should carry no lane file"
        );
    }
}

/// `--dry-run` and `--yes` contradict each other on `init` exactly as they
/// already do on `setup` (`t594`).
#[test]
fn init_dry_run_and_yes_together_is_a_usage_error() {
    let c = Sandbox::new_empty("init-dry-run-yes-exclusive");
    let (out, code) = c.run(&["init", "--dry-run", "--yes"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("there is nothing for --yes to confirm"),
        "{out}"
    );
}

/// `--join` with nothing after it must not fall through to planting
/// (`f632`): the flag is present and its value is absent, and a run that
/// only ever reads the value would plant instead of refusing.
#[test]
fn init_join_with_no_value_refuses_rather_than_planting() {
    let c = Sandbox::new_empty("init-join-no-value");
    let (out, code) = c.run(&["init", "--join"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("Without that word init plants instead of joining"),
        "{out}"
    );
    assert!(
        !already_planted(&c.0),
        "a tree was planted despite the missing --join value"
    );
}
