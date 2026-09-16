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
