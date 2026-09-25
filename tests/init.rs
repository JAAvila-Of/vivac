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
    // `f721`: `new_seeded` already locks the config to `LANE_SENTENCE` at
    // plant time, and a tree already off `1` never moves again for a rule
    // alone (`lock_if_needed`'s own guard) -- so this only checks that
    // *some* lock is already in place, the same way the config was always
    // going to be locked well before this test's own rule.
    assert!(
        c.is_locked_any(),
        "setup: the tree should already be locked"
    );

    // `f721`: bare `init` now walks the same path `--yes` already did,
    // whose own answer for an already-set-up tree is
    // `setup::init::apply_writes`'s "Nothing to write", not the old
    // direct path's "vivac is already planted in".
    let out = c.ok(&["init", "--yes"]);
    assert!(
        out.contains("Nothing to write: this project is already set up."),
        "{out}"
    );
    assert_eq!(config_before, config_bytes(&c), "the config moved");
    assert_eq!(log_before, log_bytes(&c), "the log moved");
}

/// An empty `.vivac/` holds no tree yet, so `init` plants one there rather
/// than calling it already planted.
#[test]
fn init_over_an_empty_vivac_directory_plants_a_tree() {
    let c = Sandbox::new_empty("init-empty-dir");
    std::fs::create_dir_all(c.0.join(".vivac")).unwrap();
    // `f721`: the old direct path's "vivac planted in" is gone with it --
    // `setup::init::written_text` says "Written." and points at setting up
    // the agent, the same as any other fresh plant (`f790`).
    let out = c.ok(&["init", "--yes"]);
    assert!(out.contains("Written."), "{out}");
    assert!(out.contains("Next: set up the agent you use"), "{out}");
    assert!(!out.contains("First node"), "{out}");
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

/// `t594` §4.9, `d723` piece B: a tree planted before that rule never got
/// its own `.vivac/.gitignore`, and writing it is `init`'s job now, along
/// with the rest of the tree's own writes.
#[test]
fn init_writes_the_gitignore_a_tree_from_before_lacks() {
    let c = Sandbox::new_seeded("init-gitignore");
    std::fs::remove_file(c.0.join(".vivac").join(".gitignore")).unwrap();
    let plan = c.ok(&["init", "--dry-run"]);
    assert!(
        lane_line_containing(
            &plan,
            ".vivac/.gitignore",
            "keeps .vivac/ out of version control"
        ),
        "{plan}"
    );
    c.ok(&["init", "--yes"]);
    let g = std::fs::read_to_string(c.0.join(".vivac").join(".gitignore")).unwrap();
    assert_eq!(g, "*\n");
}

/// `t594`: a project already fully set up, and already declared as a
/// lane, whose tree still predates `t594` §4.9 -- so it never got its own
/// `.vivac/.gitignore` -- creates that file on the very next `init`, and
/// the closing message has to say so, instead of claiming the tree
/// changed nothing two lines under the plan line that names this very
/// write.
#[test]
fn init_says_it_created_the_trees_gitignore_instead_of_claiming_nothing_changed() {
    let c = Sandbox::new_empty("init-gitignore-message");
    c.ok(&["init", "--yes"]);
    std::fs::remove_file(c.0.join(".vivac").join(".gitignore")).unwrap();

    let out = c.ok(&["init", "--yes"]);
    assert!(
        out.contains("init wrote in it: its own .gitignore."),
        "{out}"
    );
    assert!(!out.contains("init changed nothing in it"), "{out}");
}

/// A `.vivac/` with a log and no config regenerates through
/// `Store::open`, which locks the regenerated config if the log already
/// holds a rule. Read by `stack` rather than `init` here (`f721`): `init`
/// is no longer a neutral trigger for this -- this tree's log carries no
/// founding lane at all, so `init` would declare one on the very same
/// run, locking the regenerated config to `LANE_SENTENCE` instead of the
/// rule's own `LOCK_SENTENCE` and proving the wrong mechanism.
#[test]
fn a_log_with_no_config_regenerates_it_locked() {
    let c = Sandbox::seeded_with_no_lane("init-no-config");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    std::fs::remove_file(c.0.join(".vivac").join("config")).unwrap();

    c.ok(&["stack"]);
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
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
        out.status.code().unwrap_or(-1),
    )
}

// `real_git_repo`, `real_git_repo_with_content`, `remove_git_fixture` and
// `clone_repo` live further down, ported alongside the tests that need
// them: `real_git_repo` is a thin wrapper over `real_git_repo_with_content`
// there, so one definition of each serves every test in this file.

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("reading {p:?}: {e}"))
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
/// an expected text around a path. See `tests/setup.rs`'s own copy for why
/// the two platforms differ.
#[cfg(unix)]
fn printed(p: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| panic!("canonicalize {p:?}: {e}"))
}
#[cfg(not(unix))]
fn printed(p: &Path) -> std::path::PathBuf {
    p.to_path_buf()
}

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

/// `refuse_home_or_global_store`'s own text, naming `here`: `init` and
/// `setup` both refuse to write in the user's home folder, and both refuse
/// with this same sentence (`d723` piece B moved the guard itself onto
/// `init`, alongside the planting and joining it used to run ahead of).
fn home_folder_text(here: &Path) -> String {
    format!(
        "  {} is your home folder. Claude Code's settings and skills here are\n  \
         yours for every project, not this one's, and setup never writes there.\n  \
         Run setup in the folder you open Claude Code in, inside a project.",
        here.display()
    )
}

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
/// `render::wrap` (`f720`) put them on, for a plain `.contains` check that
/// does not anchor on a label the way `lane_line_containing` does.
fn plan_words(out: &str) -> String {
    out.split_whitespace().collect::<Vec<_>>().join(" ")
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

fn already_planted(dir: &std::path::Path) -> bool {
    dir.join(".vivac").join("config").is_file() || dir.join(".vivac").join("events").is_file()
}

/// `--dry-run` plans a plant and writes nothing: the plan names `.vivac/`
/// itself, and nothing lands on disk.
#[test]
fn init_dry_run_plans_a_plant_and_writes_nothing() {
    let c = Sandbox::new_empty("init-dry-run");
    let out = c.ok(&["init", "--dry-run"]);
    assert!(out.contains("plant") && out.contains("the tree"), "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(!already_planted(&c.0), "a dry run must not plant");
}

/// `--yes` plants, and declares this folder's own founding lane in the
/// same write, the one piece a bare `vivac init` leaves for the first
/// `push` to do instead (`f566`).
#[test]
fn init_yes_plants_and_declares_the_founding_lane() {
    let c = Sandbox::new_empty("init-yes");
    let out = c.ok(&["init", "--yes"]);
    assert!(out.contains("Written."), "{out}");
    assert!(c.0.join(".vivac").join("config").is_file());
    let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    assert!(log.contains("\"type\":\"lane.declared\""), "{log}");
}

/// `--name` saves the product's own name to the registry (`t640`).
#[test]
fn init_name_saves_the_product_to_the_registry() {
    let c = Sandbox::new_empty("init-name");
    c.ok(&["init", "--yes", "--name", "IQuorum"]);
    let registry = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert!(registry.contains("\"name\": \"IQuorum\""), "{registry}");
}

/// `d792`: a message that splices in a name is never hand-wrapped any
/// more -- one line per paragraph, and the terminal wraps it if it has
/// to. `--name` colliding with another project's is the one line of
/// `init`'s own plan that carries one at all (`t640`, point 10 bis), so a
/// name long enough to have once run past 76 columns is what proves this
/// paragraph is not split by hand: no line starts with the old 45-space
/// continuation indent a hand-wrapped status used to land under.
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
    assert!(out.contains(long), "the name reads on one line: {out}");
    for line in out.lines() {
        assert!(
            !line.starts_with(&" ".repeat(45)),
            "a line still lands under the old hand-wrapped continuation indent: {line:?}\nfull output:\n{out}"
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

/// `f720`, carried over from `tests/setup_scenarios.rs`'s own
/// `no_plan_line_is_wider_than_the_block` (`d723` piece B: that test's own
/// join step moved here with the rest of planting), and updated for
/// `d792`: the lane-declare line names this folder's own lane, which a
/// long folder name once ran past the width just as easily as `--name`'s
/// own collision line could, and `--join` is the one path that reaches it
/// without `--lane-name` shortening it. Neither line is wrapped by hand
/// any more, so what this proves now is that one: no line lands under the
/// old 45-space continuation indent a hand-wrapped status used to need.
#[test]
fn init_join_wraps_a_long_lane_name_rather_than_running_past_the_width() {
    let c = Sandbox::new_empty("init-join-lane-name-width");
    let target =
        c.0.join("A Name Chosen On Purpose To Run Longer Than One Line, target");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let joiner =
        c.0.join("A Name Chosen On Purpose To Run Longer Than One Line, joiner");
    std::fs::create_dir_all(&joiner).unwrap();
    let (out, code) = run_in(
        &joiner,
        c.global_home(),
        &["init", "--dry-run", "--join", target.to_str().unwrap()],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("becomes"), "{out}");
    for line in out.lines() {
        assert!(
            !line.starts_with(&" ".repeat(45)),
            "a line still lands under the old hand-wrapped continuation indent: {line:?}\nfull output:\n{out}"
        );
    }
}

/// `--join --dry-run` writes nothing: `init_dry_run_plans_a_plant_and_writes_nothing`
/// covers the plant path's own `--dry-run`, and a join is a different path
/// through `apply` -- `tree::plan_join` rather than `tree::plan` -- with
/// nothing of its own proving it stays as inert as the plant path already
/// is. This is the one a person runs before deciding which tree a folder
/// belongs to, so it matters more than most.
#[test]
fn init_join_dry_run_writes_nothing() {
    let target = Sandbox::new_empty("init-join-dry-run-target");
    target.ok(&["init", "--yes"]);

    let here = Sandbox::new_empty("init-join-dry-run-here");
    let out = here.ok(&["init", "--join", target.0.to_str().unwrap(), "--dry-run"]);
    assert!(out.contains("this folder becomes"), "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(
        !here.0.join(".vivac").exists(),
        "a join dry-run must not write the lane file or anything else"
    );
}

/// `--join` declares this folder as a lane of the target tree, rather than
/// planting a second one.
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

/// `t565` §7.7 / `t594`, at the scope `init` has now (`d723` piece B):
/// `--join` writes only the tree's own side, but that side still has to
/// be all or nothing. `write_lane` writes this folder's own `.vivac/lane`
/// before it ever reaches the target's own log, and used to leave that
/// file behind uncleaned when the second half failed -- a folder claiming
/// a lane the target's log never actually received. A read-only target
/// log is the fault: the target's own `declare_lane` has to append to it,
/// and a directory in its place is caught earlier, at plan time, before
/// any write begins, so it cannot reach the write this test is about.
#[test]
fn a_join_that_fails_mid_write_leaves_the_folder_as_it_was() {
    let target = Sandbox::new_empty("init-join-rollback-target");
    target.ok(&["init", "--yes"]);
    let events = target.0.join(".vivac").join("events");
    let mut perms = std::fs::metadata(&events).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&events, perms).unwrap();

    let here = Sandbox::new_empty("init-join-rollback-here");
    let (out, code) = here.run(&["init", "--join", target.0.to_str().unwrap(), "--yes"]);

    let mut perms = std::fs::metadata(&events).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&events, perms).unwrap();

    assert_eq!(code, 5, "{out}");
    assert!(
        !here.0.join(".vivac").exists(),
        "a failed join left a half-written .vivac/ behind:\n{out}"
    );
}

/// `--undo` takes off exactly what `--join` wrote in the joiner's own
/// folder: its lane file, and the `.vivac/` it holds nothing else beside.
/// The target tree's own log is never rolled back -- "the log only ever
/// grows" (`tree.rs`'s own words): the joiner's `lane.declared` event stays
/// in it, the same way any other event a lane once wrote would.
///
/// `f719`, carried over from `tests/setup_scenarios.rs`'s own
/// `undoing_a_join_leaves_no_hollow_vivac_behind` (`d723` piece B: undoing a
/// lane is `init`'s alone now, `setup --undo` never reaches one): once the
/// hollow `.vivac/` is gone, the ordinary upward walk from the joiner's own
/// folder has to land back on the target, so `brief` from there still
/// names the same product, and the target's own folder still resolves to
/// itself.
#[test]
fn init_undo_removes_a_joined_lane_and_leaves_the_target_log_growing_only() {
    let target = Sandbox::new_empty("init-undo-target");
    target.ok(&["init", "--yes", "--name", "Upper Product"]);
    let target_log_before =
        std::fs::read_to_string(target.0.join(".vivac").join("events")).unwrap();

    // Nested under the tree it joins, so that once its own `.vivac/` is
    // gone, the ordinary upward walk lands back on `target` -- exactly the
    // shape `f719`'s own bug report measured.
    let joiner = target.0.join("nested");
    std::fs::create_dir_all(&joiner).unwrap();
    let target_str = target.0.to_str().unwrap();
    let (join_out, join_code) = run_in(
        &joiner,
        target.global_home(),
        &["init", "--join", target_str, "--yes"],
    );
    assert_eq!(join_code, 0, "{join_out}");
    assert!(joiner.join(".vivac").join("lane").is_file());

    let (dry, dry_code) = run_in(
        &joiner,
        target.global_home(),
        &["init", "--undo", "--dry-run"],
    );
    assert_eq!(dry_code, 0, "{dry}");
    assert!(dry.contains("remove"), "{dry}");
    assert!(
        joiner.join(".vivac").join("lane").is_file(),
        "dry-run undid something"
    );

    let (out, out_code) = run_in(&joiner, target.global_home(), &["init", "--undo", "--yes"]);
    assert_eq!(out_code, 0, "{out}");
    assert!(out.contains("Undone."), "{out}");
    assert!(
        !joiner.join(".vivac").exists(),
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

    let (joiner_brief, joiner_brief_code) = run_in(&joiner, target.global_home(), &["brief"]);
    assert_eq!(joiner_brief_code, 0, "{joiner_brief}");
    assert!(
        joiner_brief.contains("project: Upper Product"),
        "brief from the joined folder must name the product above, not \
         itself, once its own .vivac/ is gone:\n{joiner_brief}"
    );

    assert!(
        already_planted(&target.0),
        "the joined tree must stay intact"
    );
    let (target_brief, target_brief_code) = run_in(&target.0, target.global_home(), &["brief"]);
    assert_eq!(target_brief_code, 0, "{target_brief}");
    assert!(
        target_brief.contains("project: Upper Product"),
        "the tree's own folder must still resolve to itself:\n{target_brief}"
    );
}

/// The second-map guard (`refuse_second_map`): two folders whose
/// repository shares a root commit with an already-registered project
/// refuse rather than plant a second map of it.
#[test]
fn init_refuses_a_second_map() {
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

/// `--new-tree` bypasses the second-map guard above.
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

/// `f721`, fixed at the root: planting with `init` leaves a complete tree --
/// the founding lane declared, and the version lock set. `d723` piece B
/// retired `setup`'s own copy of the planting path this test used to check
/// `init` against, so the equivalence itself is no longer something either
/// command can be asked to prove: `setup <harness>` does not plant at all
/// any more. What is left, and still worth holding, is `init`'s own half:
/// a plant is complete on its own, with nothing left for a harness's own
/// `setup` run to add to `.vivac/`.
#[test]
fn init_alone_leaves_a_complete_tree() {
    let c = Sandbox::new_empty("equiv-init");
    c.ok(&["init", "--yes"]);

    let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    assert!(
        log.contains("\"type\":\"lane.declared\""),
        "no founding lane declared: {log}"
    );
    let config = std::fs::read_to_string(c.0.join(".vivac").join("config")).unwrap();
    assert!(
        config.contains("this tree holds lanes"),
        "the version lock was not set: {config}"
    );
    assert!(
        !c.0.join(".vivac").join("lane").is_file(),
        "the tree's own root folder should carry no lane file"
    );
}

/// `--dry-run` and `--yes` contradict each other (`t594`).
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

// ---------------------------------------------------------------------------
// `d723` piece B, `f717` dissolved: no message `tree.rs` raises names a
// harness any more, on either of its two refusals that used to. Written to
// fail if either one starts naming one again, rather than trusted to stay
// that way.
// ---------------------------------------------------------------------------

fn names_no_harness(out: &str) {
    assert!(!out.contains("claude-code"), "{out}");
    assert!(!out.contains("codex"), "{out}");
}

/// The second-map refusal (`product_registered_refusal`): reached without
/// `--join`, when this folder's own repositories already belong to a
/// product the registry tracks.
#[test]
fn the_second_map_refusal_names_no_harness() {
    let c = Sandbox::new_empty("init-guardian-second-map");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("Prod-fork");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 1, "{out}");
    names_no_harness(&out);
}

/// `plan_join`'s own "no tree yet" refusal: `--join`ing a name or path
/// that resolves, but names a folder with no tree planted there yet.
#[test]
fn join_to_a_target_with_no_tree_yet_names_no_harness() {
    let c = Sandbox::new_empty("init-guardian-join-no-tree");
    let target = c.0.join("target");
    std::fs::create_dir_all(&target).unwrap();

    let here = c.0.join("here");
    std::fs::create_dir_all(&here).unwrap();
    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", target.to_str().unwrap()],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("has no tree yet"), "{out}");
    names_no_harness(&out);
}

// ---------------------------------------------------------------------------
// `d723` piece B carried the migration nudge to `init` along with the rest
// of the tree side: `MIGRATE_PARAGRAPHS` and `JOIN_MIGRATE_PARAGRAPHS` are
// `claude_code.rs`'s own text, unchanged, shown from here now because
// planting and joining are. Ported from `tests/setup.rs`, caller changed
// from `setup claude-code` to `init` and nothing else.
// ---------------------------------------------------------------------------

/// `f678`/`d683`: a join brings this folder's own harness pieces, but not
/// this folder's own knowledge -- instruction files, the harness's memory,
/// documents -- since the tree it joins already exists and joining it
/// never reads any of that. The closing summary now says so, and points at
/// the migration skill the same way a plant's own closing summary already
/// does, in a paragraph of its own rather than the plant's.
#[test]
fn joining_an_existing_tree_points_at_migrating_this_folders_own_knowledge() {
    let c = Sandbox::new_empty("init-join-migrate");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", &target_str],
    );
    assert_eq!(code, 0, "{out}");
    // `f790`: the migrate nudge moved off `init` and onto the first
    // `setup` of a lane that has never captured anything -- `init` itself
    // only ever ends with the `Next:` block naming the two harnesses.
    assert!(
        !out.contains("This folder's own knowledge is not in the tree"),
        "the join's own migrate paragraph is setup's to show now:\n{out}"
    );
    assert!(
        !out.contains("Use the vivac-migrate skill to bring everything this project knows"),
        "{out}"
    );
    assert!(
        out.contains("Next:") && out.contains("vivac setup claude-code"),
        "{out}"
    );
}

/// The same run's own plant, right next to it: `init` never shows the
/// migrate paragraph either, on a plant or a join alike (`f790`).
#[test]
fn planting_a_fresh_tree_still_carries_the_plants_own_migrate_paragraph() {
    let c = Sandbox::new_empty("init-plant-migrate");
    let (out, code) = c.run(&["init", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        !out.contains("Nothing has been brought in from anywhere yet"),
        "the plant's own migrate paragraph is setup's to show now:\n{out}"
    );
    assert!(
        !out.contains("This folder's own knowledge is not in the tree"),
        "{out}"
    );
    assert!(
        out.contains("Next:") && out.contains("vivac setup codex"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// Ported from the pre-`d723` piece B `tests/setup.rs`: the join, second-map,
// tree-below/above, `--name`, founding-lane and `--lane-name` coverage that
// exercised `setup <harness> --join/--new-tree/--name/--lane-name` now
// exercises `vivac init` instead, since that is where those flags live.
// The caller changed; the behaviour and the assertions did not.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// `t594` §4.5, case 3: `setup` refuses to give a product a second map, in a
// folder with no tree above it at all.
// ---------------------------------------------------------------------------

fn real_git_repo(at: &Path) {
    real_git_repo_with_content(at, "x");
}

/// Like [`real_git_repo`], but `content` sets the tree hash, and so the
/// root commit itself, apart from another repository this file builds:
/// two plain `real_git_repo` calls write the same content, the same
/// author and the same message, so only the committer date -- git's own
/// clock, whatever resolution it has -- tells their root commits apart.
/// `f676`'s own test needs two repositories whose root commits are
/// provably different rather than different by luck of the clock.
fn real_git_repo_with_content(at: &Path, content: &str) {
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
    std::fs::write(at.join("f.txt"), content).unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "first"]);
}

/// `remove_dir_all`, but clears every file's read-only bit first: git
/// leaves some files inside `.git/objects` read-only, and Windows refuses
/// to delete a read-only file even through `remove_dir_all`. The one
/// fixture in this file that lives outside any `Sandbox` needs this, since
/// nothing else cleans it up if this does not (`t594`).
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
    run_in(&below, c.global_home(), &["init", "--yes"]);
    let events_before = std::fs::read_to_string(below.join(".vivac").join("events")).unwrap();

    let (out, code) = c.run(&["init", "--yes"]);
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
        out.contains("Move that tree up here, then run init again. From inside \"Backend v2\":"),
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
    run_in(&a, c.global_home(), &["init", "--yes"]);
    run_in(&b, c.global_home(), &["init", "--yes"]);

    let (out, code) = c.run(&["init", "--yes"]);
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
    let (setup_out, setup_code) = run_in(&first, c.global_home(), &["init", "--yes"]);
    assert_eq!(setup_code, 0, "{setup_out}");

    let second = c.0.join("IQuorum-v2");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
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
    assert!(out.contains("vivac init --join IQuorum"), "{out}");
    assert!(out.contains("To plant a separate tree anyway:"), "{out}");
    assert!(out.contains("vivac init --new-tree"), "{out}");
    assert!(!already_planted(&second));
}

/// Case 4: One shared repository is enough, even when the new root also has a
/// repository the registered project never had.
#[test]
fn one_shared_repository_is_enough_even_with_an_extra_one() {
    let c = Sandbox::new_empty("setup-registered-partial");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("Prod-v2");
    clone_repo(&first.join("webapi"), &second.join("webapi"));
    real_git_repo(&second.join("infra"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
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
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("Prod-v3");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        !out.contains(secret_name),
        "the withheld name leaked: {out}"
    );
    assert!(
        out.contains("Some repositories here are already tracked by another project on this"),
        "{out}"
    );
    assert!(out.contains("machine:\n      webapi\n"), "{out}");
    assert!(
        out.contains("Planting another tree would give this product two maps."),
        "{out}"
    );
    assert!(
        out.contains("To work on it from this folder, give the path to its folder:"),
        "{out}"
    );
    assert!(
        out.contains("vivac init --join <path to that folder>"),
        "{out}"
    );
    assert!(out.contains("To plant a separate tree anyway:"), "{out}");
    assert!(out.contains("vivac init --new-tree"), "{out}");
}

/// `d680`, second half: the refusal used to name only two ways out --
/// joining, or planting a separate tree -- and never the one that keeps
/// the tree already grown here: moving it into place first, then joining.
/// Reached before `--new-tree`'s own remedy, so a reader following the
/// refusal down the page meets it before the escape that gives up this
/// folder's own tree.
#[test]
fn the_refusal_names_relocate_before_new_tree() {
    let c = Sandbox::new_empty("setup-registered-relocate-remedy");
    let first = c.0.join("IQuorum");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("IQuorum-v2");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("If the tree should live here instead, run this in the folder that holds it:"),
        "{out}"
    );
    assert!(
        out.contains("vivac relocate <path to this folder>"),
        "{out}"
    );
    let relocate_at = out.find("vivac relocate").expect("relocate remedy missing");
    let new_tree_at = out
        .find("vivac init --new-tree")
        .expect("new-tree remedy missing");
    assert!(
        relocate_at < new_tree_at,
        "relocate must be named before --new-tree: {out}"
    );
}

/// The withheld-name form of the same refusal carries the same remedy, in
/// the same place.
#[test]
fn the_withheld_name_refusal_also_names_relocate_before_new_tree() {
    let secret_name = "someone@example.com";
    let c = Sandbox::new_empty("setup-registered-relocate-remedy-withheld");
    let first = c.0.join(secret_name);
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("Prod-v3");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("If the tree should live here instead, run this in the folder that holds it:"),
        "{out}"
    );
    assert!(
        out.contains("vivac relocate <path to this folder>"),
        "{out}"
    );
    let relocate_at = out.find("vivac relocate").expect("relocate remedy missing");
    let new_tree_at = out
        .find("vivac init --new-tree")
        .expect("new-tree remedy missing");
    assert!(
        relocate_at < new_tree_at,
        "relocate must be named before --new-tree: {out}"
    );
}

/// `f677`: a repository that *is* the folder itself is named "this folder
/// itself" rather than printed as a bare ".": `Repo::relative` already
/// returns "." for exactly that folder, and the old message glued it
/// straight onto the sentence's own closing period, so the line a person
/// actually read said only "..".
#[test]
fn a_repository_that_is_the_folder_itself_is_named_rather_than_a_bare_dot() {
    let c = Sandbox::new_empty("setup-registered-dot");
    let first = c.0.join("IQuorum");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("IQuorum-v2");
    clone_repo(&first.join("webapi"), &second);

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("this folder itself"), "{out}");
    assert!(
        !out.lines().any(|l| l.trim() == ".."),
        "a line read only \"..\":\n{out}"
    );
}

/// `f676`/`d682`: a plant where the registry already knows another
/// product, but this folder's own repository shares no root commit with
/// it, used to say nothing at all -- the guard above only speaks when the
/// two look like the same product, so a genuinely new product and one
/// whose repository the registry simply has not learned about yet read
/// identically. The plan now says so itself, as a warning rather than a
/// refusal: exit 0, and it shows with `--dry-run` too.
#[test]
fn planting_beside_a_registered_product_that_shares_nothing_warns_in_the_plan() {
    let c = Sandbox::new_empty("setup-second-map-hint");
    let first = c.0.join("IQuorum");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("Unrelated");
    real_git_repo_with_content(&second.join("app"), "y");

    let (out, code) = run_in(&second, c.global_home(), &["init", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("This plants a new product."), "{out}");
    assert!(
        out.contains("Nothing here shares a repository with the projects vivac already tracks"),
        "{out}"
    );
    assert!(
        out.contains("If it is, stop and use --join <name> instead."),
        "{out}"
    );
}

/// The same run against an empty registry carries none of it: there is
/// nothing yet for this folder's repository to fail to share with.
#[test]
fn planting_with_an_empty_registry_carries_no_second_map_hint() {
    let c = Sandbox::new_empty("setup-second-map-hint-empty");
    real_git_repo(&c.0.join("app"));

    let (out, code) = c.run(&["init", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("This plants a new product."), "{out}");
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
    run_in(&work, c.global_home(), &["init", "--yes"]);
    // `f721`: `init` itself now refuses to nest a second tree inside one
    // it already resolves to -- `mid` would just join `work` as a lane,
    // never planting a tree of its own -- so this fabricates `mid`'s tree
    // by hand, the shape two independently-planted, later-nested products
    // (a copy, or a tree from before `d723`) can still leave behind.
    common::plant_undeclared(&mid, "setup-above-warning-mid");

    let (out, code) = run_in(&f, c.global_home(), &["init", "--yes"]);
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

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
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
/// registry side of the setup below was disconnected entirely (`t594`).
/// The positive anchor at the end closes that: with the
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
    let (other_out, other_code) = run_in(&other_root, c.global_home(), &["init", "--yes"]);
    assert_eq!(
        other_code, 0,
        "the already-registered project never got set up: {other_out}"
    );

    clone_repo(&other_root.join("webapi"), &c.0.join("webapi"));
    let below = c.0.join("Backend v2");
    std::fs::create_dir_all(&below).unwrap();
    run_in(&below, c.global_home(), &["init", "--yes"]);

    let (out, code) = c.run(&["init", "--yes"]);
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
    let (out2, code2) = c.run(&["init", "--yes"]);
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
/// `declare_lane` with a no-op left this assertion green (`t594`).
/// The count and the joining folder's own name, neither of
/// which the pre-existing `main` declaration could satisfy, tie it to
/// this run specifically.
#[test]
fn join_by_name_declares_a_lane_and_signs_writes_with_it() {
    let c = Sandbox::new_empty("setup-join-name");
    let target = c.0.join("IQuorum");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);
    let log_at_target = std::fs::read_to_string(target.join(".vivac").join("events")).unwrap();
    let declared_before = log_at_target.matches("\"type\":\"lane.declared\"").count();

    let here = c.0.join("IQuorum-v2");
    std::fs::create_dir_all(&here).unwrap();
    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", "IQuorum"],
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

/// `t594`'s other half: `--lane-name` alongside `--join`
/// names the lane it declares in the *target* tree -- nothing exercised
/// this path before, since the existing `--lane-name` test only ran
/// against a plain plant, never against `join`.
#[test]
fn lane_name_names_the_lane_over_join_too() {
    let c = Sandbox::new_empty("setup-join-lane-name");
    let target = c.0.join("IQuorum");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("IQuorum-v2");
    std::fs::create_dir_all(&here).unwrap();
    let (out, code) = run_in(
        &here,
        c.global_home(),
        &[
            "init",
            "--yes",
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
/// the exit code and the lane file's existence (`t594`)
/// left "the same way" unproven: a path spec that resolved but never
/// actually declared anything would have passed too.
#[test]
fn join_by_path_works_the_same_way() {
    let c = Sandbox::new_empty("setup-join-path");
    let target = c.0.join("Prod");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("Prod-v2");
    std::fs::create_dir_all(&here).unwrap();
    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", &target_str],
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
        .env("TZ", "UTC")
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
    run_in(&original, c.global_home(), &["init", "--yes"]);
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
        &["init", "--yes", "--join", &copy_str],
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

/// `t594` §4.7, form 5's other half: `setup` planting into a folder that
/// is itself a copy of a tree elsewhere. It writes -- the lane file, and
/// the lane's own declaration -- so it warns on `stderr` like every other
/// write does, and it is `note_registry` that leaves the notice for
/// `warn_if_wrote` to act on.
///
/// The `--join` half has had a test since it landed; this half had none,
/// and `clippy` had nothing to say either, since `set_pending` keeps its
/// other callers. The two halves warn for the same reason and neither is
/// covered by the other's test.
#[test]
fn setup_planting_in_a_folder_that_is_a_copy_warns_on_stderr() {
    let c = Sandbox::new_empty("setup-plant-copy");
    let original = c.0.join("orig");
    std::fs::create_dir_all(&original).unwrap();
    // `init`, not `setup`: the tree has to reach the copy with no lane
    // declared yet, so the `setup` below has something real to write to it
    // -- the warning hangs off a write that happened and off nothing else
    // (`t594`), so a fixture where setup writes only the
    // harness files would prove the opposite of what it looks like.
    // `f721`: `init` alone cannot leave that shape behind any more, since
    // it declares the founding lane at plant time now -- `plant_undeclared`
    // fabricates it by hand instead.
    common::plant_undeclared(&original, "setup-plant-copy");
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
    let before = read(&copy.join(".vivac").join("events")).lines().count();

    let (stdout, stderr, code) = run_in_split(&copy, c.global_home(), &["init", "--yes"]);

    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        read(&copy.join(".vivac").join("events")).lines().count() > before,
        "the write this warning reports on never happened:\n{stdout}"
    );
    assert!(
        stderr.contains("COPY OF ANOTHER TREE"),
        "planting into a copy never warned:\n{stderr}"
    );
    assert!(
        !stdout.contains("COPY OF ANOTHER TREE"),
        "the warning leaked into stdout:\n{stdout}"
    );
}

/// Case 3: `--join` to a folder with no tree refuses, and nothing is
/// written. Only the exit code used to be checked (`t594`);
/// the text is what tells this refusal apart from any other exit-1
/// `join` can reach.
#[test]
fn join_to_a_folder_with_no_tree_refuses() {
    let c = Sandbox::new_empty("setup-join-no-tree");
    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let empty_target = c.0.join("NoTreeHere").to_string_lossy().into_owned();

    let (out, code) = run_in(&here, c.global_home(), &["init", "--join", &empty_target]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("has no tree yet, so there is nothing to join."),
        "{out}"
    );
    assert!(!here.join(".vivac").exists());
}

/// `f616`: a name the registry does not know used to fall through to
/// `Case 3`'s own text -- "that folder has no tree yet" -- which sends the
/// fix at a folder nobody typed. Nobody typed a folder here at all: the
/// spec has no separator in it, and nothing on disk answers to it either,
/// so the only honest reading is that the name itself is unknown.
#[test]
fn a_project_name_the_registry_does_not_know_is_said_to_be_unknown() {
    let c = Sandbox::new_empty("setup-join-unknown-name");
    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();

    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["init", "--join", "no-such-project"],
    );

    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("No project named no-such-project in the registry."),
        "{out}"
    );
    assert!(out.contains("vivac vivacs"), "{out}");
    assert!(
        !out.contains("has no tree yet, so there is nothing to join."),
        "the folder-shaped text is still shown to an unknown name:\n{out}"
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
    run_in(&a, c.global_home(), &["init", "--yes"]);
    let b = c.0.join("B");
    std::fs::create_dir_all(&b).unwrap();
    run_in(&b, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let (join_out, join_code) = run_in(&here, c.global_home(), &["init", "--yes", "--join", "A"]);
    assert_eq!(join_code, 0, "{join_out}");

    let (out, code) = run_in(&here, c.global_home(), &["init", "--join", "B"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("This folder is already a lane of another tree."),
        "{out}"
    );
}

/// `Failure::already_a_lane`'s own doc says it is for "a folder that
/// already carries somebody else's `.vivac/lane`". A folder with no
/// `.vivac/` at all, sitting under a tree and resolving up into it,
/// carries none: the refusal is right, and the sentence it used to give
/// was false. It gets its own, which says the thing that is actually
/// true and where to look.
#[test]
fn a_folder_under_a_tree_is_refused_for_the_tree_above_not_a_lane_it_has_not_got() {
    let c = Sandbox::new_empty("setup-join-under-a-tree");
    let above = c.0.join("Above");
    let sub = above.join("Sub");
    std::fs::create_dir_all(&sub).unwrap();
    run_in(&above, c.global_home(), &["init", "--yes"]);

    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let (out, code) = run_in(&sub, c.global_home(), &["init", "--join", "T"]);

    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("A tree sits above this folder, in \"Above\","),
        "{out}"
    );
    assert!(
        !out.contains("already a lane of another tree"),
        "this folder carries no lane of anybody's: {out}"
    );
    assert!(
        !sub.join(".vivac").exists(),
        "a refused join must write nothing here"
    );
}

/// `t594`: `--join` from the folder that holds its own
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
    run_in(&here, c.global_home(), &["init", "--yes"]);

    let other = c.0.join("Other");
    std::fs::create_dir_all(&other).unwrap();
    run_in(&other, c.global_home(), &["init", "--yes"]);

    let (out, code) = run_in(&here, c.global_home(), &["init", "--join", "Other"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("This folder holds a tree of its own"), "{out}");
    assert!(
        !out.contains("already a lane of another tree"),
        "the wrong text is still shown: {out}"
    );
}

/// `t594`: with a tree above *and* a tree below, joining
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
    run_in(&work, c.global_home(), &["init", "--yes"]);
    // `f721`: `init` refuses to nest a second tree inside one it already
    // resolves to, so `nested` is fabricated by hand rather than planted
    // through the CLI (see `a_tree_above_the_joined_one_warns_but_still_completes`).
    common::plant_undeclared(&nested, "setup-below-and-above-nested");

    let (out, code) = run_in(&f, c.global_home(), &["init", "--yes"]);
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

/// The very same guard, reached by joining instead of planting. `--join`
/// returns before `apply` ever runs and `refuse_second_map` -- which owned
/// the tree-below check -- was only ever called from `apply`, so a folder
/// with a tree inside it joined a tree elsewhere at exit 0 and said
/// nothing: the split product §6.4 exists to catch, minted by the very
/// flag §6.3 hands people as the remedy. The same structural mistake
/// `refuse_home_or_global_store` already had, and the same fix -- the
/// guard belongs in `run`, where both branches go through it.
///
/// `d626`: the guard moving to `run` fixed *that* nothing was said, but
/// what it said next was still the plant-only text -- "move that tree up
/// here, then run setup again" names a door nobody was standing in front
/// of, since a join was never going to plant one here at all. This checks
/// the sentence, not just the exit code and the write.
#[test]
fn a_join_with_another_tree_below_names_the_choice_not_the_other_remedy() {
    let c = Sandbox::new_empty("setup-below-and-join");
    let target = c.0.join("T");
    let f = c.0.join("F");
    let nested = f.join("Nested");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    // A real tree with a first event to point a lane back at, so the only
    // thing left that can refuse this join is the tree below `F`.
    run_in(&target, c.global_home(), &["init", "--yes"]);
    run_in(&nested, c.global_home(), &["init", "--yes"]);

    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(&f, c.global_home(), &["init", "--join", &target_str]);

    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("There is another product's tree below this folder:"),
        "{out}"
    );
    assert!(out.contains("Nested"), "{out}");
    assert!(
        !out.contains("There is already a tree inside this folder"),
        "the plant-only text is still shown to a join:\n{out}"
    );
    assert!(
        out.contains(&format!("cannot be a lane of \"{target_str}\"")),
        "{out}"
    );
    assert!(
        out.contains("move it up. From inside Nested:"),
        "the remedy does not name the folder to run it from:\n{out}"
    );
    assert!(
        !out.contains("vivac relocate Nested"),
        "relocate's own argument is a destination, not the tree that moves:\n{out}"
    );
    assert!(out.contains("vivac relocate .."), "{out}");
    assert!(
        !f.join(".vivac").exists(),
        "F must not have become a lane of T despite the tree below"
    );
}

/// `d626`: with more than one foreign tree below, every one of them is
/// named -- the same shape `two_trees_below_refuse_with_the_plural_text`
/// already settled for a plant, and for the same reason: whoever fixes
/// the first one and reaches this refusal again would only be learning
/// the same thing twice.
#[test]
fn several_trees_below_refuse_a_join_by_naming_all_of_them() {
    let c = Sandbox::new_empty("setup-below-several-and-join");
    let target = c.0.join("T");
    let f = c.0.join("F");
    let alpha = f.join("Alpha");
    let beta = f.join("Beta");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);
    run_in(&alpha, c.global_home(), &["init", "--yes"]);
    run_in(&beta, c.global_home(), &["init", "--yes"]);

    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(&f, c.global_home(), &["init", "--join", &target_str]);

    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("There are other products' trees below this folder:"),
        "{out}"
    );
    assert!(out.contains("    Alpha"), "{out}");
    assert!(out.contains("    Beta"), "{out}");
    assert!(
        out.contains(&format!(
            "cannot be a lane of \"{target_str}\" while any of them is there"
        )),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "Any of them that belongs to \"{target_str}\" can move up, from inside it:"
        )),
        "{out}"
    );
    assert!(out.contains("vivac relocate .."), "{out}");
    assert!(
        out.contains("For the rest, join from a folder that does not contain them."),
        "{out}"
    );
    assert!(
        !out.contains("vivac cannot merge trees"),
        "the plant-only plural text is still shown to a join:\n{out}"
    );
    assert!(!f.join(".vivac").exists());
}

/// `d626`: a tree below whose own folder name the redaction guard
/// rejects refuses a join without ever printing that name, the same
/// guarantee `a_registered_products_withheld_name_points_at_the_path_remedy`
/// already holds for a plant.
#[test]
fn a_join_with_a_withheld_tree_below_names_neither_the_folder_nor_a_count() {
    let secret_name = "someone@example.com";
    let c = Sandbox::new_empty("setup-below-withheld-and-join");
    let target = c.0.join("T");
    let f = c.0.join("F");
    let hidden = f.join(secret_name);
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&hidden).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);
    run_in(&hidden, c.global_home(), &["init", "--yes"]);

    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(&f, c.global_home(), &["init", "--join", &target_str]);

    assert_eq!(code, 1, "{out}");
    assert!(
        !out.contains(secret_name),
        "the withheld folder name leaked: {out}"
    );
    assert!(
        out.contains("There is another product's tree below this folder, under a name this tool"),
        "{out}"
    );
    assert!(out.contains("will not write down."), "{out}");
    assert!(
        out.contains(&format!(
            "cannot be a lane of \"{target_str}\" while that tree is there"
        )),
        "{out}"
    );
    assert!(
        out.contains("Join from a folder that does not contain it, or move that tree up from"),
        "{out}"
    );
    assert!(out.contains("inside it:   vivac relocate .."), "{out}");
    assert!(
        !out.contains("From inside"),
        "a folder this tool will not name cannot be pointed at: {out}"
    );
    assert!(!f.join(".vivac").exists());
}

/// The plural of `a_join_with_a_withheld_tree_below_names_neither_the_folder_nor_a_count`:
/// two trees below, both under names the redaction guard rejects, refuse a
/// join without naming either one -- checked in code and not by hand, on
/// purpose. This branch answers to the security pillar, which is the one
/// pillar with veto, and "I looked at the output" leaves nothing behind
/// that would catch somebody breaking it later without noticing.
#[test]
fn several_withheld_trees_below_refuse_a_join_naming_none_of_them() {
    let first_name = "first@example.com";
    let second_name = "second@example.com";
    let c = Sandbox::new_empty("setup-below-several-withheld-and-join");
    let target = c.0.join("T");
    let f = c.0.join("F");
    let first = f.join(first_name);
    let second = f.join(second_name);
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);
    run_in(&first, c.global_home(), &["init", "--yes"]);
    run_in(&second, c.global_home(), &["init", "--yes"]);

    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(&f, c.global_home(), &["init", "--join", &target_str]);

    assert_eq!(code, 1, "{out}");
    assert!(
        !out.contains(first_name) && !out.contains(second_name),
        "a withheld folder name leaked: {out}"
    );
    assert!(
        out.contains("There are other products' trees below this folder, under names this tool"),
        "{out}"
    );
    assert!(out.contains("will not write down."), "{out}");
    assert!(
        out.contains(&format!(
            "cannot be a lane of \"{target_str}\" while any of them is there"
        )),
        "{out}"
    );
    assert!(
        out.contains("Join from a folder that does not contain them, or move them up from"),
        "{out}"
    );
    assert!(
        out.contains("inside each one:   vivac relocate .."),
        "{out}"
    );
    assert!(!f.join(".vivac").exists());
}

/// The case nobody would have looked at by hand: one tree below is under a
/// name the guard rejects, the other is not. The one that can be shown is
/// shown whole -- this is not the plant-only refusal's "in \"X\" and
/// \"Y\"" prose, it is `d626`'s own one-route-per-line list -- and the
/// withheld one is not replaced by a placeholder or a count: saying "1 more"
/// would still be handing over information the guard exists to keep back.
#[test]
fn a_join_with_some_trees_below_withheld_lists_only_the_ones_it_can_show() {
    let secret_name = "someone@example.com";
    let c = Sandbox::new_empty("setup-below-mixed-withheld-and-join");
    let target = c.0.join("T");
    let f = c.0.join("F");
    let hidden = f.join(secret_name);
    let visible = f.join("Visible");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::create_dir_all(&visible).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);
    run_in(&hidden, c.global_home(), &["init", "--yes"]);
    run_in(&visible, c.global_home(), &["init", "--yes"]);

    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(&f, c.global_home(), &["init", "--join", &target_str]);

    assert_eq!(code, 1, "{out}");
    assert!(
        !out.contains(secret_name),
        "the withheld folder name leaked: {out}"
    );
    assert!(
        !out.contains("under names this tool will not write down"),
        "one route is visible, so this is not the all-withheld text:\n{out}"
    );
    assert!(
        out.contains(&format!(
            "cannot be a lane of \"{target_str}\" while any of them is there"
        )),
        "{out}"
    );

    // The list between the header and the blank line that ends it has to be
    // exactly the one route the guard let through -- no placeholder and no
    // count standing in for the one it withheld, since either would still
    // be handing over information the guard exists to keep back.
    let header = "There are other products' trees below this folder:\n";
    let after_header = out
        .split_once(header)
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("the plural, visible-routes header is missing:\n{out}"));
    let listing = after_header
        .split_once("\n\n")
        .map(|(list, _)| list)
        .unwrap_or_else(|| panic!("no blank line after the route list:\n{out}"));
    assert_eq!(
        listing, "    Visible",
        "the list must name only the visible route, nothing else and no count:\n{out}"
    );

    assert!(!f.join(".vivac").exists());
}

/// The defect `d626` exists to close: the remedy used to read
/// `vivac relocate <tree below>`, which is backwards twice over --
/// `relocate` takes a destination, not a source, and it has to be run
/// from inside the tree that moves, never from above it
/// (`src/relocate.rs`). With the tree below two folders deep, the same
/// route has to show up twice: once as where the tree is, and once as
/// where to stand before typing the fix.
#[test]
fn the_join_remedy_names_the_folder_to_run_it_from_not_a_destination() {
    let c = Sandbox::new_empty("setup-below-remedy-direction");
    let target = c.0.join("T");
    let f = c.0.join("F");
    let deep = f.join("Group").join("Sub");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&deep).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);
    run_in(&deep, c.global_home(), &["init", "--yes"]);

    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(&f, c.global_home(), &["init", "--join", &target_str]);

    assert_eq!(code, 1, "{out}");
    assert!(out.contains("    Group/Sub"), "{out}");
    assert!(
        out.contains("move it up. From inside Group/Sub:"),
        "the remedy does not say where to stand:\n{out}"
    );
    assert!(
        out.contains("vivac relocate .."),
        "relocate's own argument has to be the destination, not the tree below:\n{out}"
    );
    assert!(
        !out.contains("vivac relocate Group/Sub"),
        "the old remedy pointed relocate at the tree below as if it were a destination:\n{out}"
    );
    assert!(!f.join(".vivac").exists());
}

/// `t594` (critical): `registry::resolve` used to hand a
/// relative `--join` spec straight to `entry.path`, corrupting the
/// machine registry for good -- every reader of that entry resolves it
/// from a folder of its own, not from the one that typed `--join`.
#[test]
fn a_relative_join_target_is_recorded_as_an_absolute_path() {
    let c = Sandbox::new_empty("setup-join-relative-target");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("F");
    let sub = here.join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    let (out, code) = run_in(
        &sub,
        c.global_home(),
        &["init", "--yes", "--join", "../../T"],
    );
    assert_eq!(code, 0, "{out}");

    let registry = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert!(
        !registry.contains(".."),
        "a relative path leaked into the machine registry: {registry}"
    );

    // The corruption `t594` was reproduced with: `brief`
    // from a subfolder of the joined folder dying on a path nobody but
    // the original `cd` could resolve.
    let nested = sub.join("deeper");
    std::fs::create_dir_all(&nested).unwrap();
    let (brief_out, brief_code) = run_in(&nested, c.global_home(), &["brief"]);
    assert_eq!(brief_code, 0, "{brief_out}");
}

/// `t594`: `--join` returned before `apply` ever ran
/// `refuse_home_or_global_store`, so the home-folder guard lived in one
/// branch and the other had none. Same fixture as
/// `setup_refuses_in_the_home_folder`, with `--join` instead of a plain
/// setup.
#[test]
fn join_refuses_in_the_home_folder_too() {
    let c = Sandbox::new_empty("setup-join-home");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let (out, code) = run_with_home(&c.0, &c.0, c.global_home(), &["init", "--join", "T"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&home_folder_text(&printed(&c.0))), "{out}");
    assert!(!c.0.join(".vivac").join("lane").exists());
}

/// `t594`: a tree with no events yet used to answer on
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

    let (out, code) = run_in(&here, c.global_home(), &["init", "--join", &target_str]);
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

// ---------------------------------------------------------------------------
// `--join` run a second time against the tree this folder already is a lane
// of. It used to mint a fresh lane id and declare it, leaving the id the
// folder had been signing with orphaned in the log: the stack, the focus and
// the counters all hang off that id, and none of them resolve any more.
// ---------------------------------------------------------------------------

/// The whole of it, said once: nothing happened, and that is the answer.
const ALREADY_THAT_TREE: &str =
    "This folder is already a lane of that tree, and init changed nothing in it.";

/// The second line, for the one case where something was asked for and not
/// done: a flag accepted in silence is what this product does not do.
const LANE_NAME_LEFT: &str = "The lane name it already has was left as it is.";

/// Everything a second join has to leave exactly as it found it, read off
/// disk: the id this folder signs with, the target tree's whole log, and
/// the machine registry.
fn join_state(here: &Path, target: &Path, home: &Path) -> (String, String, String) {
    (
        lane_id_of(here),
        read(&target.join(".vivac").join("events")),
        read(&home.join("projects")),
    )
}

/// The one that matters. A folder joins, pushes a node, and joins the very
/// same tree again: the node used to vanish from its stack, silently and at
/// exit 0, because the second join minted an id the stack knew nothing
/// about. The stack is what makes an orphaned lane visible from outside --
/// the lane file still parses, the tree still resolves, and only the work
/// is gone.
#[test]
fn a_second_join_of_the_same_tree_leaves_the_stack_alone() {
    let c = Sandbox::new_empty("setup-join-again-stack");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let target_str = target.to_string_lossy().into_owned();
    let (first_out, first_code) = run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", &target_str],
    );
    assert_eq!(first_code, 0, "{first_out}");

    let (push_out, push_code) = run_in(
        &here,
        c.global_home(),
        &["push", "Work from the joined folder", "--why", "seed"],
    );
    assert_eq!(push_code, 0, "{push_out}");
    let (stack_before, stack_code) = run_in(&here, c.global_home(), &["stack"]);
    assert_eq!(stack_code, 0, "{stack_before}");
    assert!(
        stack_before.contains("Work from the joined folder"),
        "the fixture never had a stack to lose:\n{stack_before}"
    );

    let (out, code) = run_in(&here, c.global_home(), &["init", "--join", &target_str]);
    assert_eq!(code, 0, "{out}");

    let (stack_after, stack_code) = run_in(&here, c.global_home(), &["stack"]);
    assert_eq!(stack_code, 0, "{stack_after}");
    assert!(
        stack_after.contains("Work from the joined folder"),
        "the second join took this folder's stack with it:\n{stack_after}"
    );
}

/// The same run, read off disk instead: the lane id is the one it already
/// was, the target tree's log gains nothing at all -- no second
/// `lane.declared` under a new id -- and the machine registry is untouched
/// too. And it says so, rather than repeating the sentence it gives a
/// folder that really did just become a lane.
#[test]
fn a_second_join_of_the_same_tree_writes_nothing() {
    let c = Sandbox::new_empty("setup-join-again-writes");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let target_str = target.to_string_lossy().into_owned();
    let (first_out, first_code) = run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", &target_str],
    );
    assert_eq!(first_code, 0, "{first_out}");
    let before = join_state(&here, &target, c.global_home());

    let (out, code) = run_in(&here, c.global_home(), &["init", "--join", &target_str]);

    assert_eq!(code, 0, "{out}");
    assert!(out.contains(ALREADY_THAT_TREE), "{out}");
    assert!(
        !out.contains("This folder is now a lane"),
        "a second join still claims it just joined:\n{out}"
    );
    assert!(
        !out.contains(LANE_NAME_LEFT),
        "nobody asked for a lane name, so there is nothing to report:\n{out}"
    );

    let after = join_state(&here, &target, c.global_home());
    assert_eq!(before.0, after.0, "the lane id changed under the folder");
    assert_eq!(
        before.1, after.1,
        "the target tree's own log gained an event"
    );
    assert_eq!(before.2, after.2, "the machine registry changed");
}

/// `--lane-name` over a second join. Asking for the name the lane already
/// has changes nothing and says nothing extra; asking for a different one
/// changes nothing either, and says so -- accepting a flag and quietly
/// doing nothing with it is the mistake `t594` already
/// closed once, on the planting side.
#[test]
fn a_second_join_with_another_lane_name_changes_nothing_and_says_so() {
    let c = Sandbox::new_empty("setup-join-again-lane-name");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let target_str = target.to_string_lossy().into_owned();
    let (first_out, first_code) = run_in(
        &here,
        c.global_home(),
        &[
            "init",
            "--yes",
            "--join",
            &target_str,
            "--lane-name",
            "first-name",
        ],
    );
    assert_eq!(first_code, 0, "{first_out}");
    let before = join_state(&here, &target, c.global_home());
    assert!(
        before.1.contains("\"name\":\"first-name\""),
        "the fixture never got the name it joined under:\n{}",
        before.1
    );

    let (same_out, same_code) = run_in(
        &here,
        c.global_home(),
        &["init", "--join", &target_str, "--lane-name", "first-name"],
    );
    assert_eq!(same_code, 0, "{same_out}");
    assert!(same_out.contains(ALREADY_THAT_TREE), "{same_out}");
    assert!(
        !same_out.contains(LANE_NAME_LEFT),
        "the name asked for is the name it has, so nothing was left behind:\n{same_out}"
    );

    let (out, code) = run_in(
        &here,
        c.global_home(),
        &["init", "--join", &target_str, "--lane-name", "other-name"],
    );

    assert_eq!(code, 0, "{out}");
    assert!(out.contains(ALREADY_THAT_TREE), "{out}");
    assert!(
        out.contains(LANE_NAME_LEFT),
        "a name was asked for and not taken, in silence:\n{out}"
    );

    let after = join_state(&here, &target, c.global_home());
    assert_eq!(before.0, after.0, "the lane id changed under the folder");
    assert_eq!(before.1, after.1, "the target tree's own log changed");
    assert_eq!(before.2, after.2, "the machine registry changed");
    assert!(
        !after.1.contains("other-name"),
        "the name it was told to leave alone reached the log anyway:\n{}",
        after.1
    );
}

/// The negative of all three: the folder that is already a lane still gets
/// the refusal when the tree named is a *different* one, and a folder
/// joining for the first time still joins. Neither may reach the new
/// sentence -- it is for the one case where there really is nothing to do.
#[test]
fn a_join_to_a_different_tree_is_still_refused_and_a_first_join_still_works() {
    let c = Sandbox::new_empty("setup-join-again-negative");
    let a = c.0.join("A");
    std::fs::create_dir_all(&a).unwrap();
    run_in(&a, c.global_home(), &["init", "--yes"]);
    let b = c.0.join("B");
    std::fs::create_dir_all(&b).unwrap();
    run_in(&b, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let (first_out, first_code) = run_in(&here, c.global_home(), &["init", "--yes", "--join", "A"]);
    assert_eq!(first_code, 0, "{first_out}");
    assert!(
        first_out.contains("Written."),
        "a first join stopped saying what it did:\n{first_out}"
    );
    assert!(
        !first_out.contains(ALREADY_THAT_TREE),
        "a first join answered as though it had nothing to do:\n{first_out}"
    );
    assert!(here.join(".vivac").join("lane").exists());

    let (out, code) = run_in(&here, c.global_home(), &["init", "--join", "B"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("This folder is already a lane of another tree."),
        "{out}"
    );
    assert!(
        !out.contains(ALREADY_THAT_TREE),
        "another tree was answered as though it were the same one:\n{out}"
    );
}

/// A folder that holds the tree itself **and** carries its own
/// `.vivac/lane` -- the shape `tests/lanes.rs`'s own
/// `a_folder_whose_own_lane_file_names_main_keeps_writing` builds for the
/// folder `main` was claimed away from, still holding the tree it always
/// held. `--join` naming that very folder has to resolve through
/// `lane_carried_by`, the same as any other self-join: `l.root` and
/// `target` are the same folder here too, so this reaches the branch
/// above rather than `already_has_a_tree`, which is for a folder with a
/// tree of its own and no lane to redirect at all.
#[test]
fn a_join_of_a_folder_that_is_both_the_tree_and_its_own_lane_says_so() {
    let c = Sandbox::new_seeded("setup-join-self-lane");
    // `f721`: `new_seeded` already declares `main` for real as `seq` 1,
    // with a real, random project id -- so the lane file below has to
    // name *that* id, and the hand-crafted claim follows it as `seq` 2
    // rather than colliding with it.
    let real_project = {
        let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
        let first: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
        first["id"].as_str().unwrap().to_string()
    };
    c.append_raw_line(
        r#"{"seq":2,"id":"01SEEDSELFJOINAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.claimed","lane":"main"}}"#,
    );
    std::fs::write(
        c.0.join(".vivac").join("lane"),
        format!(r#"{{"version":1,"id":"main","project":"{real_project}"}}"#),
    )
    .unwrap();

    let here = c.0.to_string_lossy().into_owned();
    let (out, code) = c.run(&["init", "--join", &here]);

    assert_eq!(code, 0, "{out}");
    assert!(out.contains(ALREADY_THAT_TREE), "{out}");
}

/// `t594`: `--lane-name` used to be accepted and
/// silently ignored when planting fresh -- worse than either using it or
/// refusing it outright, since accepting a flag and doing nothing with it
/// leaves no trace that it was ignored.
#[test]
fn lane_name_names_main_when_planting_fresh_too() {
    let c = Sandbox::new_empty("setup-lane-name-fresh-plant");
    let (out, code) = c.run(&["init", "--yes", "--lane-name", "custom-name"]);
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
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("Prod-fork");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (refused_out, refused_code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(refused_code, 1, "{refused_out}");
    assert!(
        refused_out.contains("already tracked by project"),
        "{refused_out}"
    );

    let (out, code) = run_in(&second, c.global_home(), &["init", "--new-tree", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(already_planted(&second));
}

/// Case 6: `--join` and `--new-tree` together is a usage error. Only the
/// exit code used to be checked (`t594`); the text is
/// what tells this usage error apart from any other exit-2 `setup` can
/// give.
#[test]
fn join_and_new_tree_together_is_a_usage_error() {
    let c = Sandbox::new_empty("setup-join-new-tree-exclusive");
    let (out, code) = c.run(&["init", "--join", "X", "--new-tree"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("--join joins a tree that already exists, and --new-tree plants a"),
        "{out}"
    );
}

/// `f632`: `--join` with nothing after it must refuse, not silently plant a
/// second tree where the caller meant to join one. `has("join")` was true
/// and `opt("join")` was `None`, and `run` only ever asked for the value, so
/// this used to fall straight through to `apply` and plant.
#[test]
fn join_with_no_value_refuses_instead_of_planting() {
    let c = Sandbox::new_empty("setup-join-no-value");
    let (out, code) = c.run(&["init", "--join"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--join"), "{out}");
    assert!(
        !already_planted(&c.0),
        "it planted a tree instead of refusing:\n{out}"
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
        &["init", "--yes", "--lane-name", "custom-name"],
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
/// (`t594`), which a run that failed outright would
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
        &["init", "--yes", "--lane-name", secret],
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
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("IQ-Suite-v2");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 1, "{out}");
    let join_line = out
        .lines()
        .find(|l| l.trim_start().starts_with("vivac init --join"))
        .unwrap_or_else(|| panic!("no --join command line in the refusal:\n{out}"));
    assert!(
        join_line.contains("\"IQ Suite\""),
        "the printed command did not quote the name with a space: {join_line}"
    );
    let words = shell_split(join_line.trim());
    let mut cli_args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
    cli_args.push("--yes");

    let (join_out, join_code) = run_in(&second, c.global_home(), &cli_args);
    assert_eq!(join_code, 0, "{join_out}");
    assert!(second.join(".vivac").join("lane").exists());
}

// ---------------------------------------------------------------------------
// 9. The founding lane names itself after its own folder too (`d624`).
// ---------------------------------------------------------------------------

/// Every other lane is already named after the folder it is
/// (`claude_code.rs:837`); the founding one was the exception, and the
/// exception read as a git branch (`f611`, `d624`).
#[test]
fn setup_names_the_founding_lane_after_its_own_folder() {
    let c = Sandbox::new_empty("setup-founding-lane-folder-name");
    let here = c.0.join("webapi");
    std::fs::create_dir_all(&here).unwrap();
    let (out, code) = run_in(&here, c.global_home(), &["init", "--yes"]);
    assert_eq!(code, 0, "{out}");
    let log = std::fs::read_to_string(here.join(".vivac").join("events")).unwrap();
    assert!(
        log.contains("\"type\":\"lane.declared\",\"lane\":\"main\",\"name\":\"webapi\""),
        "{log}"
    );
}

// `a_tree_nobody_has_run_setup_in_still_says_main` used to live here: a
// tree from before `setup` ran, whose founding lane read as the fallback
// `main` rather than its own folder's name, until `setup` declared it for
// real. `f721` removed the state its whole point depended on -- `d723`
// folded declaring the founding lane into every plant, bare or not, so a
// tree that has never had it declared cannot exist any more, and there is
// no `init` left that leaves one behind. `setup_names_the_founding_lane_after_its_own_folder`,
// above, still covers the naming rule itself.

// ---------------------------------------------------------------------------
// `t640`: `--name` fixes the product's own name on purpose, rather than
// always deriving it from whichever folder holds the tree.
// ---------------------------------------------------------------------------

/// Point 6 and the header both at once: `--name` while planting saves the
/// name to the registry, and the brief it plants leads with it.
#[test]
fn name_plants_the_product_and_the_brief_shows_it() {
    let c = Sandbox::new_empty("setup-name-plants");
    let (out, code) = c.run(&["init", "--yes", "--name", "IQuorum"]);
    assert_eq!(code, 0, "{out}");

    let registry = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert!(registry.contains("\"name\": \"IQuorum\""), "{registry}");

    let brief = c.ok(&["brief"]);
    let header = brief.lines().next().unwrap_or("");
    assert!(header.contains("project: IQuorum"), "{header}");
}

/// Point 7: a name fixed on purpose keeps naming the product once the
/// folder it once came from no longer says the same thing.
#[test]
fn a_saved_name_wins_over_a_moved_folder() {
    let c = Sandbox::new_empty("setup-name-precedence");
    c.ok(&["init", "--yes", "--name", "IQuorum"]);

    let moved = c.0.parent().unwrap().join("setup-name-precedence-moved");
    std::fs::rename(&c.0, &moved).unwrap();

    let (out, code) = run_in(&moved, c.global_home(), &["brief"]);
    assert_eq!(code, 0, "{out}");
    let header = out.lines().next().unwrap_or("");
    assert!(header.contains("project: IQuorum"), "{header}");
    assert!(
        !header.contains("setup-name-precedence-moved"),
        "the folder's new name overrode the name fixed on purpose: {header}"
    );

    std::fs::remove_dir_all(&moved).ok();
}

/// Without `--name`, nothing about the registry or the brief's header
/// changes: no `name` field is written, and the header still leads with
/// the folder's own name, exactly as it did before this tranche.
#[test]
fn no_name_leaves_the_registry_and_the_header_exactly_as_before() {
    let c = Sandbox::new_empty("setup-no-name-regression");
    c.ok(&["init", "--yes"]);

    let registry = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert!(!registry.contains("\"name\""), "{registry}");

    let from_folder = c.0.file_name().unwrap().to_string_lossy().into_owned();
    let brief = c.ok(&["brief"]);
    let header = brief.lines().next().unwrap_or("");
    assert!(
        header.contains(&format!("project: {from_folder}")),
        "{header}"
    );
}

/// Point 2, first half: `--name` beside `--join` refuses by name, before
/// touching the disk.
#[test]
fn name_with_join_refuses_before_writing_anything() {
    let c = Sandbox::new_empty("setup-name-with-join");
    let before = list(&c.0);
    let (out, code) = c.run(&["init", "--yes", "--join", "somewhere", "--name", "X"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--join"), "{out}");
    assert!(out.contains("--name"), "{out}");
    assert_eq!(list(&c.0), before, "the disk changed");
    assert!(!c.0.join(".vivac").exists());
}

/// Point 2, second half: `--name` beside `--undo` refuses the same way.
#[test]
fn name_with_undo_refuses_before_writing_anything() {
    let c = Sandbox::new_empty("setup-name-with-undo");
    let before = list(&c.0);
    let (out, code) = c.run(&["init", "--undo", "--name", "X"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--undo"), "{out}");
    assert!(out.contains("--name"), "{out}");
    assert_eq!(list(&c.0), before, "the disk changed");
}

/// Point 1: `--name` given while this folder is only becoming an
/// ordinary new lane of a tree that already exists -- not planting one,
/// and not `--new-tree` either -- has no product left to fix a name on
/// for the first time.
#[test]
fn name_is_rejected_when_not_planting() {
    let c = Sandbox::new_empty("setup-name-not-planting");
    let root = c.0.join("root");
    std::fs::create_dir_all(&root).unwrap();
    run_in(&root, c.global_home(), &["init", "--yes"]);

    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let (out, code) = run_in(&sub, c.global_home(), &["init", "--yes", "--name", "X"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--name"), "{out}");
    assert!(!sub.join(".vivac").exists());
}

/// Point 3: `--name` with nothing after it is a usage error, the same
/// gap `f632` already closed for `--join`.
#[test]
fn name_with_no_value_refuses() {
    let c = Sandbox::new_empty("setup-name-no-value");
    let (out, code) = c.run(&["init", "--yes", "--name"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--name"), "{out}");
    assert!(!c.0.join(".vivac").exists());
}

/// Point 4: a name the redaction guard refuses exits 3 with its own
/// message, and nothing reaches the disk.
#[test]
fn name_the_guard_rejects_refuses_and_writes_nothing() {
    let c = Sandbox::new_empty("setup-name-guard-rejects");
    let (out, code) = c.run(&[
        "init",
        "--yes",
        "--name",
        "sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345",
    ]);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("Refused"), "{out}");
    assert!(!c.0.join(".vivac").exists());
}

/// Point 5: the minimal shape a name has to have before it ever reaches
/// the redaction guard.
#[test]
fn name_shape_is_checked_before_the_guard() {
    let c = Sandbox::new_empty("setup-name-shape");
    let (out, code) = c.run(&["init", "--yes", "--name", "   "]);
    assert_eq!(code, 2, "{out}");
    assert!(!c.0.join(".vivac").exists());

    let too_long = "x".repeat(101);
    let d = Sandbox::new_empty("setup-name-shape-long");
    let (out, code) = d.run(&["init", "--yes", "--name", &too_long]);
    assert_eq!(code, 2, "{out}");
    assert!(!d.0.join(".vivac").exists());
}

/// Point 8: a second site sharing repositories is refused with the fixed
/// name rather than the folder-derived one, and the very `--join`
/// command the refusal offers works.
#[test]
fn a_second_folder_sharing_repos_is_refused_with_the_fixed_name_and_join_works() {
    let c = Sandbox::new_empty("setup-name-sharing-refusal");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    run_in(
        &first,
        c.global_home(),
        &["init", "--yes", "--name", "IQuorum"],
    );

    let second = c.0.join("Prod-fork");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (refused_out, refused_code) = run_in(&second, c.global_home(), &["init", "--yes"]);
    assert_eq!(refused_code, 1, "{refused_out}");
    assert!(
        refused_out.contains("already tracked by project \"IQuorum\""),
        "{refused_out}"
    );
    assert!(
        refused_out.contains("vivac init --join IQuorum"),
        "{refused_out}"
    );

    let (out, code) = run_in(
        &second,
        c.global_home(),
        &["init", "--yes", "--join", "IQuorum"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(second.join(".vivac").join("lane").is_file());
}

/// Point 9, planting fresh at the tree's own root: the plan names the
/// product on the same line that names the lane.
#[test]
fn the_plan_names_the_product_when_planting_fresh() {
    let c = Sandbox::new_empty("setup-name-plan-fresh");
    let (out, code) = c.run(&["init", "--yes", "--name", "IQuorum"]);
    assert_eq!(code, 0, "{out}");
    let lane_name = c.0.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        lane_line_containing(
            &out,
            ".vivac/events",
            &format!("its log, with this folder as part \"{lane_name}\" of \"IQuorum\""),
        ),
        "{out}"
    );
}

/// Point 9, `--new-tree`: the same line, this time for a folder that
/// insists on being a separate product despite a shared root commit.
#[test]
fn the_plan_names_the_product_with_new_tree_and_name() {
    let c = Sandbox::new_empty("setup-name-plan-new-tree");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    run_in(&first, c.global_home(), &["init", "--yes"]);

    let second = c.0.join("Prod-fork");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(
        &second,
        c.global_home(),
        &["init", "--new-tree", "--yes", "--name", "Fork"],
    );
    assert_eq!(code, 0, "{out}");
    let lane_name = second.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        lane_line_containing(
            &out,
            ".vivac/events",
            &format!("its log, with this folder as part \"{lane_name}\" of \"Fork\""),
        ),
        "{out}"
    );
}

/// Point 10 bis: `--name` lets someone fix a name another project on
/// this machine already answers to. That is a warning, not a refusal --
/// a project's identity is its first event's id, never its name -- so
/// setup still writes, but the plan says so first, and `--join` by that
/// name refuses afterwards with the same ambiguity error
/// `registry::resolve` already gives two roots that share a name.
#[test]
fn name_that_collides_with_another_project_warns_in_the_plan_and_still_writes() {
    let c = Sandbox::new_empty("setup-name-collision");
    let first = c.0.join("first");
    std::fs::create_dir_all(&first).unwrap();
    run_in(
        &first,
        c.global_home(),
        &["init", "--yes", "--name", "IQuorum"],
    );

    let second = c.0.join("second");
    std::fs::create_dir_all(&second).unwrap();
    let (out, code) = run_in(
        &second,
        c.global_home(),
        &["init", "--yes", "--name", "IQuorum"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("\"IQuorum\" already names another project on this machine"),
        "{out}"
    );
    assert!(already_planted(&second));

    let registry = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert_eq!(
        registry.matches("\"name\": \"IQuorum\"").count(),
        2,
        "{registry}"
    );

    let (join_out, join_code) = run_in(
        &second,
        c.global_home(),
        &["init", "--yes", "--join", "IQuorum"],
    );
    assert_eq!(join_code, 2, "{join_out}");
    assert!(
        join_out.contains("names 2 projects on this machine"),
        "{join_out}"
    );
}

/// `f724`, updated for `d792`: a message that splices in a name is never
/// hand-wrapped any more, so it reads on one line however long the name
/// is. The collision paragraph above is the one line of the plan that
/// carries one, and `--name` accepts up to `t640`'s own `NAME_MAX_LEN`
/// (100) -- long enough on its own to have once run past 76 columns
/// without any help from a long folder, which is what makes this the
/// sibling of `tests/init.rs`'s own version of this same test rather than
/// a repeat of it.
#[test]
fn setup_name_collision_wraps_a_long_name_rather_than_running_past_the_width() {
    let c = Sandbox::new_empty("setup-name-collision-width");
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
    assert!(out.contains(long), "the name reads on one line: {out}");
    for line in out.lines() {
        assert!(
            !line.starts_with(&" ".repeat(45)),
            "a line still lands under the old hand-wrapped continuation indent: {line:?}\nfull output:\n{out}"
        );
    }
}

/// The other half of point 10 bis: with nothing to collide against, the
/// plan carries no warning at all.
#[test]
fn name_with_no_collision_shows_no_warning() {
    let c = Sandbox::new_empty("setup-name-no-collision");
    let (out, code) = c.run(&["init", "--yes", "--name", "SoloProject"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("already names another project"), "{out}");
}

// ---------------------------------------------------------------------------
// `t640`, `d674`: `--join` walks the same path planting does, minus the
// plant itself -- the plan, the confirmation, and every piece a plain
// `setup` writes, all in one commit (`f667`/`f669`).
// ---------------------------------------------------------------------------

/// Point 13: closes `f669` -- `--join` with no terminal and no `--yes`
/// used to write straight away. Now it refuses first, the same as
/// planting, and names both ways out.
#[test]
fn join_with_no_terminal_and_no_yes_refuses_without_writing() {
    let c = Sandbox::new_empty("setup-join-no-terminal");
    let target = c.0.join("T");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("F");
    std::fs::create_dir_all(&here).unwrap();
    let target_str = target.to_string_lossy().into_owned();
    let (out, code) = run_in(&here, c.global_home(), &["init", "--join", &target_str]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("there is no terminal here to ask"), "{out}");
    // `f675`: following the bare commands this message used to suggest
    // would have planted a second tree instead of joining this one, since
    // dropping `--join` is what turns them into a plant. Both lines have
    // to carry it back.
    assert!(
        out.contains(&format!("vivac init --join {target_str} --dry-run")),
        "{out}"
    );
    assert!(
        out.contains(&format!("vivac init --join {target_str} --yes")),
        "{out}"
    );
    assert!(!here.join(".vivac").exists(), "the lane was written anyway");
    assert!(
        !here.join(".claude").exists(),
        "the harness was written anyway"
    );
}

// ---------------------------------------------------------------------------
// `d680`: `--undo` and the lane file `--join` leaves behind.
// ---------------------------------------------------------------------------

/// `--join` writes `.vivac/lane`, and `--undo` used to leave it in place no
/// matter what: the only way off was deleting the folder by hand, and
/// while it stayed, `relocate` refused the folder as already holding a
/// lane. A lane that never wrote anything to the tree owns no history for
/// the file to orphan, so `--undo` can take it -- and once it does, the
/// folder no longer blocks `relocate`.
#[test]
fn undo_after_a_join_that_wrote_nothing_removes_the_lane_file_and_unblocks_relocate() {
    let c = Sandbox::new_empty("setup-undo-lane-unwritten");
    let target = c.0.join("Target");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("Joiner");
    std::fs::create_dir_all(&here).unwrap();
    run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", "Target"],
    );
    assert!(here.join(".vivac").join("lane").exists());

    let (out, code) = run_in(&here, c.global_home(), &["init", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("remove") && out.contains("this folder's lane"),
        "{out}"
    );
    assert!(
        !here.join(".vivac").join("lane").exists(),
        "the lane file must be gone once its lane never wrote"
    );

    let (relocate_out, relocate_code) = run_in(
        &target,
        c.global_home(),
        &["relocate", here.to_str().unwrap()],
    );
    assert_eq!(relocate_code, 0, "{relocate_out}");
}

/// A lane that did write something owns a piece of history no longer
/// findable through anything but its own id: `--undo` leaves its file
/// alone and says why, rather than deleting the one file that still names
/// it.
#[test]
fn undo_after_a_join_that_wrote_something_keeps_the_lane_file() {
    let c = Sandbox::new_empty("setup-undo-lane-written");
    let target = c.0.join("Target");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("Joiner");
    std::fs::create_dir_all(&here).unwrap();
    run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", "Target"],
    );
    run_in(
        &here,
        c.global_home(),
        &["push", "work from the joined folder", "--why", "seed"],
    );

    let (out, code) = run_in(&here, c.global_home(), &["init", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    // `d792`: never wrapped by hand any more, so this reads the "what"
    // whole, wherever the path column happened to end.
    assert!(
        plan_words(&out).contains(
            "this lane has written to the tree, and removing it would orphan what it wrote"
        ),
        "{out}"
    );
    assert!(
        here.join(".vivac").join("lane").exists(),
        "the lane file must stay once its lane has written"
    );
}

/// `f719`, point B, condition 3: a `.gitignore` `write_gitignore` did not
/// write in full -- someone added a line of their own -- is not this
/// tool's to erase, so `--undo` leaves it, and the folder that holds it,
/// right where they are. The lane file itself still goes: it never wrote,
/// and nothing about that changes here.
#[test]
fn undo_leaves_a_vivac_dir_whose_gitignore_was_hand_edited() {
    let c = Sandbox::new_empty("setup-undo-vivac-dir-hand-edited-gitignore");
    let target = c.0.join("Target");
    std::fs::create_dir_all(&target).unwrap();
    run_in(&target, c.global_home(), &["init", "--yes"]);

    let here = c.0.join("Joiner");
    std::fs::create_dir_all(&here).unwrap();
    run_in(
        &here,
        c.global_home(),
        &["init", "--yes", "--join", "Target"],
    );
    let gitignore = here.join(".vivac").join(".gitignore");
    let mut contents = read(&gitignore);
    contents.push_str("!keep-me\n");
    std::fs::write(&gitignore, &contents).unwrap();

    let (out, code) = run_in(&here, c.global_home(), &["init", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        plan_words(&out).contains("remove") && plan_words(&out).contains("this folder's lane"),
        "{out}"
    );
    assert!(
        plan_words(&out).contains("it holds more than this lane"),
        "{out}"
    );
    assert!(
        !here.join(".vivac").join("lane").exists(),
        "the lane file must still go once its lane never wrote"
    );
    assert!(
        here.join(".vivac").exists(),
        "the folder must stay: its .gitignore carries a line this tool never wrote"
    );
    assert_eq!(
        read(&gitignore),
        contents,
        "the hand-added line must survive"
    );
}

// ---------------------------------------------------------------------------
// `f721`: a bare `vivac init` used to fall through to the old
// `Store::create` path instead of `setup::init`'s one path (`d723` piece
// A) -- no lane declared, no repositories recorded, no version lock, and
// the guard against two trees of one product never got the chance to run
// at all, since it lives inside `tree::plan`, which the old path never
// called.
// ---------------------------------------------------------------------------

/// `f721`, the exact defect: a bare `init`, no `--yes` and no terminal to
/// ask, in an ordinary git folder with a remote. The old path never asked
/// anyone anything and planted regardless; the fix routes it through the
/// same terminal check `--yes` already had, so it refuses instead, the
/// same family `no_terminal_and_no_yes_refuses_without_a_plan`
/// (`tests/setup.rs`) already proves for `setup`.
#[test]
fn bare_init_with_no_terminal_and_no_yes_refuses_and_writes_nothing() {
    let c = Sandbox::new_empty("f721-bare-no-terminal");
    real_git_repo(&c.0);
    std::process::Command::new("git")
        .arg("-C")
        .arg(&c.0)
        .args([
            "remote",
            "add",
            "origin",
            "https://example.invalid/f721.git",
        ])
        .output()
        .unwrap();

    let (out, code) = c.run(&["init"]);
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("--yes"), "{out}");
    assert!(
        !c.0.join(".vivac").join("events").exists(),
        "a refused bare init must not write the log:\n{out}"
    );
}

/// `f721`: every test that only needs *a* tree, not a bare `init`
/// specifically, seeds one through `common::Sandbox::new_seeded`, and
/// that helper used to hand back exactly the incomplete tree this defect
/// produced -- no founding lane, no version lock -- rather than the
/// complete one `setup::init` plants (`init_alone_leaves_a_complete_tree`,
/// above, already proves what the flagged path gives). Any test relying
/// on `new_seeded` for a lane to `stack`/`push` against, or a lock to
/// check, was silently standing on the bug.
#[test]
fn the_seeded_helper_hands_back_a_complete_tree_not_a_bare_one() {
    let c = Sandbox::new_seeded("f721-seeded-helper-complete");
    let log = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    assert!(
        log.contains("\"type\":\"lane.declared\""),
        "no founding lane declared: {log}"
    );
    let config = std::fs::read_to_string(c.0.join(".vivac").join("config")).unwrap();
    assert!(
        config.contains("this tree holds lanes"),
        "the version lock was not set: {config}"
    );
}

/// `f721`'s own consequence, reproduced end to end: today, a bare `init`
/// records no repositories at all (the two tests above), so the guard
/// against two trees of one product never has anything to compare
/// against, and a second bare `init` over a clone plants a second, empty
/// tree right next to the first -- both exiting 0, nothing to tell them
/// apart. Fixed at the root (`d723`), a bare `init` walks the very same
/// `tree::plan` the flagged path already did, so the second one is
/// refused before it ever asks to proceed -- no terminal and no `--yes`
/// needed, since `refuse_second_map` runs ahead of that question.
#[test]
fn a_bare_init_on_a_clone_of_an_already_registered_product_is_refused() {
    let c = Sandbox::new_empty("f721-bare-second-map");
    let first = c.0.join("Prod");
    real_git_repo(&first.join("webapi"));
    let (setup_out, setup_code) = run_in(&first, c.global_home(), &["init", "--yes"]);
    assert_eq!(setup_code, 0, "{setup_out}");

    let second = c.0.join("Prod-fork");
    clone_repo(&first.join("webapi"), &second.join("webapi"));

    let (out, code) = run_in(&second, c.global_home(), &["init"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("Planting another tree would give this product two maps."),
        "{out}"
    );
    assert!(
        !already_planted(&second),
        "a bare init must not have planted a second tree"
    );
}

// ---------------------------------------------------------------------------
// `d784`: a bare `init` writes no lane file at all (`main`'s implicit
// shape), so `undo_lane` never had anything to say about it and
// `.vivac/` sat outside `--undo`'s reach no matter how empty it stayed.
// This is the one door `--undo` may remove a tree through.
// ---------------------------------------------------------------------------

/// The project id keyed by the log's own first line -- what the registry
/// keys its entry by, read straight off `.vivac/events` rather than
/// through any command's own output.
fn project_id(c: &Sandbox) -> String {
    let log = c.log();
    let first = log.lines().next().expect("a seeded tree has a first line");
    let v: serde_json::Value = serde_json::from_str(first).unwrap();
    v["id"].as_str().unwrap().to_string()
}

#[test]
fn undo_of_a_bare_plant_with_no_work_removes_the_tree_and_forgets_it() {
    let c = Sandbox::new_seeded("undo-bare-clean");
    let id = project_id(&c);
    let registry_before = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert!(
        registry_before.contains(&id),
        "setup: the registry should already know this project: {registry_before}"
    );

    let (out, code) = c.run(&["init", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        plan_words(&out).contains("remove")
            && plan_words(&out).contains("the tree init planted here: it holds no work yet"),
        "{out}"
    );
    assert!(
        plan_words(&out).contains("forget") && plan_words(&out).contains("this project"),
        "{out}"
    );
    assert!(
        out.contains("Undone. There is no tree here any more."),
        "{out}"
    );
    assert!(!c.0.join(".vivac").exists(), "the tree must be gone");

    let registry_after = std::fs::read_to_string(c.global_home().join("projects")).unwrap();
    assert!(
        !registry_after.contains(&id),
        "the registry must forget a project whose tree is gone: {registry_after}"
    );
}

/// `--dry-run` plans the removal and writes nothing, the tree included.
#[test]
fn undo_of_a_bare_plant_dry_run_writes_nothing() {
    let c = Sandbox::new_seeded("undo-bare-dry-run");

    let (out, code) = c.run(&["init", "--undo", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing written: --dry-run."), "{out}");
    assert!(c.0.join(".vivac").exists(), "the tree must still be there");
}

/// A capture event -- here, an ordinary `add` -- makes the tree not
/// `--undo`'s to remove any more: it refuses outright and touches
/// nothing, counting only the events `session::capture_count` (`d779`'s
/// own definition of work) actually counts.
#[test]
fn undo_of_a_bare_plant_that_already_holds_work_is_refused_and_touches_nothing() {
    let c = Sandbox::new_seeded("undo-bare-with-work");
    c.ok(&["add", "A finding", "--why", "reason"]);
    let log_before = c.log();

    let (out, code) = c.run(&["init", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "Nothing to undo: the tree in .vivac/ already holds work (1 writes), and \
             init --undo never removes a tree that does."
        ),
        "{out}"
    );
    assert!(c.0.join(".vivac").exists(), "the tree must stay");
    assert_eq!(log_before, c.log(), "a refused undo must write nothing");
}

/// `session.started` is one of `is_capture`'s own exceptions (`d779`): a
/// hook that only ever opens a session leaves a tree just as undoable as
/// one nobody has touched at all.
#[test]
fn undo_of_a_bare_plant_stays_available_after_a_session_started_hook() {
    let c = Sandbox::new_seeded("undo-bare-session-started");
    let (hook_out, hook_code) = c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"source":"startup","session_id":"s1"}"#,
    );
    assert_eq!(hook_code, 0, "{hook_out}");
    assert!(
        c.log().contains("\"session.started\""),
        "setup: the hook should have written session.started: {}",
        c.log()
    );

    let (out, code) = c.run(&["init", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Undone. There is no tree here any more."),
        "{out}"
    );
    assert!(!c.0.join(".vivac").exists());
}

// ---------------------------------------------------------------------------
