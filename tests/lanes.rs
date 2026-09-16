//! Resolving a working folder to the tree it belongs to and the lane it is
//! (`t594` §2.3), from outside the process: a lane folder is not something
//! any command writes yet, so these fabricate `.vivac/lane` by hand, the
//! same way `common::Sandbox::append_raw_line` fabricates log shapes no CLI
//! path writes yet either.

mod common;
use common::Sandbox;
use std::path::{Path, PathBuf};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

fn run(dir: &Path, home: &Path, args: &[&str]) -> (String, i32) {
    let o = std::process::Command::new(BIN)
        .current_dir(dir)
        .env("VIVAC_HOME", home)
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
        o.status.code().unwrap_or(-1),
    )
}

/// The `id` of line 1 of the log, which `d201` keys the registry by, read
/// the same way `tests/registry.rs` does.
fn first_event_id(c: &Sandbox) -> String {
    let log = c.0.join(".vivac").join("events");
    let text = std::fs::read_to_string(&log).expect("the log is there");
    let line = text.lines().next().expect("the log has a first line");
    let v: serde_json::Value = serde_json::from_str(line).expect("the first line parses");
    v["id"].as_str().expect("an event has an id").to_string()
}

/// A directory name nothing else in this file takes.
fn unique(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "vivac-lanes-{name}-{}-{n}-{ts}",
        std::process::id()
    ))
}

/// Writes `lane_dir/.vivac/lane` naming `project`, the shape `lane.rs`
/// writes -- fabricated rather than joined, since joining is a later task.
fn write_lane(lane_dir: &Path, project: &str) {
    let vivac_dir = lane_dir.join(".vivac");
    std::fs::create_dir_all(&vivac_dir).unwrap();
    std::fs::write(
        vivac_dir.join("lane"),
        format!(r#"{{"version":1,"id":"01LANEIDAAAAAAAAAAAAAAAAAAA","project":"{project}"}}"#),
    )
    .unwrap();
}

/// Seeds a project and hands back its id, so a lane elsewhere can name it.
/// The `push` and the `stack` after it are what enters it into the registry
/// in the first place (`f277`).
fn seed_project(name: &str) -> (Sandbox, String) {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "seed the tree",
    ]);
    c.ok(&["stack"]);
    let project = first_event_id(&c);
    (c, project)
}

#[test]
fn a_lane_outside_the_tree_reads_it_through_the_registry() {
    let (tree, project) = seed_project("lanes-open");

    let lane_dir = unique("outside");
    std::fs::create_dir_all(&lane_dir).unwrap();
    write_lane(&lane_dir, &project);
    let deep = lane_dir.join("deep").join("er");
    std::fs::create_dir_all(&deep).unwrap();

    let (s, code) = run(&deep, tree.global_home(), &["open"]);

    assert_eq!(code, 0, "{s}");
    assert!(s.contains("Ship the release apparatus"), "{s}");

    std::fs::remove_dir_all(&lane_dir).ok();
}

#[test]
fn a_lane_whose_registry_entry_is_removed_refuses_with_exit_4() {
    let (tree, project) = seed_project("lanes-removed");

    let lane_dir = unique("removed");
    std::fs::create_dir_all(&lane_dir).unwrap();
    write_lane(&lane_dir, &project);
    let deep = lane_dir.join("deep");
    std::fs::create_dir_all(&deep).unwrap();

    // The tree itself is untouched; only the machine's memory of where it
    // lives is erased, by hand, the way a moved or reimaged registry would
    // leave it.
    std::fs::remove_file(tree.global_home().join("projects")).unwrap();

    let (s, code) = run(&deep, tree.global_home(), &["open"]);

    assert_eq!(code, 4, "{s}");
    assert!(s.contains("registry does not know"), "{s}");

    std::fs::remove_dir_all(&lane_dir).ok();
}

// ---------------------------------------------------------------------------
// `t594` §4.5: `setup` actually declaring a folder a lane, rather than the
// resolution above, which only ever reads a `.vivac/lane` some other path
// already wrote. From here on, `setup` writes it.
// ---------------------------------------------------------------------------

fn setup_ok(dir: &Path, home: &Path) -> String {
    let (s, code) = run(dir, home, &["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0, "{s}");
    s
}

fn log_text(c: &Sandbox) -> String {
    std::fs::read_to_string(c.0.join(".vivac").join("events")).expect("the log is there")
}

/// (1): `setup` in a second folder of the same product joins the tree
/// above as a new lane instead of planting a second one -- the bug this
/// task fixes: `.vivac/lane` appears with a fresh id, `lane.declared`
/// lands in the tree's own log, and no `config` or `events` appears
/// under the second folder.
#[test]
fn setup_in_a_second_folder_joins_the_tree_above_as_a_new_lane() {
    let c = Sandbox::new_seeded("declare-second-folder");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();

    setup_ok(&second, c.global_home());

    assert!(
        second.join(".vivac").join("lane").exists(),
        "the second folder never became a lane"
    );
    assert!(
        !second.join(".vivac").join("config").exists(),
        "a second tree was planted"
    );
    assert!(
        log_text(&c).contains("\"type\":\"lane.declared\""),
        "no lane.declared reached the tree's own log"
    );
}

/// (2): each folder keeps its own stack and its own focus -- pushing in
/// one never moves the other's.
#[test]
fn each_folder_keeps_its_own_stack_and_focus() {
    let c = Sandbox::new_seeded("declare-own-stack");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();
    setup_ok(&second, c.global_home());

    c.ok(&["push", "Main lane work", "--why", "seed main"]);
    let (out, code) = run(
        &second,
        c.global_home(),
        &["push", "Second lane work", "--why", "seed second"],
    );
    assert_eq!(code, 0, "{out}");

    let (main_stack, code) = run(&c.0, c.global_home(), &["stack"]);
    assert_eq!(code, 0, "{main_stack}");
    assert!(main_stack.contains("Main lane work"), "{main_stack}");
    assert!(!main_stack.contains("Second lane work"), "{main_stack}");

    let (second_stack, code2) = run(&second, c.global_home(), &["stack"]);
    assert_eq!(code2, 0, "{second_stack}");
    assert!(second_stack.contains("Second lane work"), "{second_stack}");
    assert!(!second_stack.contains("Main lane work"), "{second_stack}");
}

/// (3): each folder's brief carries its own name in the header (`t594`
/// §5.1's `lane_name`), not the other's and not the bare id.
#[test]
fn each_folders_brief_names_its_own_lane_in_the_header() {
    let c = Sandbox::new_seeded("declare-brief-header");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();
    setup_ok(&second, c.global_home());

    let (main_brief, code) = run(&c.0, c.global_home(), &["brief"]);
    assert_eq!(code, 0, "{main_brief}");
    assert!(main_brief.contains("lane: main"), "{main_brief}");

    let (second_brief, code2) = run(&second, c.global_home(), &["brief"]);
    assert_eq!(code2, 0, "{second_brief}");
    assert!(second_brief.contains("lane: v2"), "{second_brief}");
}

/// (4): running `setup` again in the same folder, with nothing changed,
/// does not leave a second `lane.declared` behind (`t594` §4.5.2, case
/// (e)).
#[test]
fn running_setup_again_unchanged_does_not_write_a_second_event() {
    let c = Sandbox::new_seeded("declare-no-repeat");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();
    setup_ok(&second, c.global_home());

    let before = log_text(&c);
    let out = setup_ok(&second, c.global_home());
    assert!(
        out.contains("Nothing to write: this project is already set up."),
        "{out}"
    );
    assert_eq!(before, log_text(&c), "a second run wrote to the log");
}

/// (5): a tree of today, where `setup` had never run, gets `main`
/// declared when `setup` runs in its own folder (`t594` §4.5.2, case
/// (b)) -- and every other command answers exactly as it did before,
/// down to the byte.
#[test]
fn setup_on_an_existing_trees_own_folder_declares_main_and_changes_nothing_else() {
    let c = Sandbox::new_seeded("declare-existing-main");
    c.ok(&["push", "Some node", "--why", "seed"]);
    let (before, code) = run(&c.0, c.global_home(), &["brief"]);
    assert_eq!(code, 0, "{before}");

    setup_ok(&c.0, c.global_home());

    let (after, code2) = run(&c.0, c.global_home(), &["brief"]);
    assert_eq!(code2, 0, "{after}");
    assert_eq!(before, after, "declaring main changed what brief answers");
}
