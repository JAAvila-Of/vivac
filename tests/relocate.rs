//! `vivac relocate <destination>` -- moving a tree out of the folder it was
//! planted in, and leaving that folder as one of its lanes.

mod common;
use common::Sandbox;
use std::path::{Path, PathBuf};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// Whether the output says `sentence`, ignoring where the lines break: the
/// same helper `tests/lanes.rs` reads `relocate`'s own refusals through.
fn says(out: &str, sentence: &str) -> bool {
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .contains(sentence)
}

fn run_in(dir: &Path, home: &Path, args: &[&str]) -> (String, i32) {
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

/// The `id` of line 1 of the log, the same way `tests/lanes.rs` and
/// `tests/registry.rs` read it, so a lane fabricated by hand can name the
/// right project.
fn first_event_id(c: &Sandbox) -> String {
    let text = std::fs::read_to_string(c.0.join(".vivac").join("events")).unwrap();
    let line = text.lines().next().expect("the log has a first line");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    v["id"].as_str().unwrap().to_string()
}

/// A directory next to a `Sandbox`'s own folder, never inside it: a
/// destination has to survive the origin, and `Sandbox::drop` only ever
/// cleans up what it planted. Every caller removes its own.
fn sibling_dir(c: &Sandbox, name: &str) -> PathBuf {
    c.0.parent().unwrap().join(format!(
        "vivac-relocate-{name}-{}-{}",
        std::process::id(),
        id_seed()
    ))
}

/// A counter unique within this test binary's run, so two calls to
/// `sibling_dir` in the same process never collide even when they ask for
/// the same `name` in the same millisecond.
fn id_seed() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Writes `lane_dir/.vivac/lane` naming `project`, fabricated by hand the
/// same way `tests/lanes.rs::write_lane` does: joining a lane through the
/// CLI is a later task, not this one.
fn write_lane(lane_dir: &Path, id: &str, project: &str) {
    let vivac_dir = lane_dir.join(".vivac");
    std::fs::create_dir_all(&vivac_dir).unwrap();
    std::fs::write(
        vivac_dir.join("lane"),
        format!(r#"{{"version":1,"id":"{id}","project":"{project}"}}"#),
    )
    .unwrap();
}

#[test]
fn the_tree_moves_and_the_old_folder_stays_a_lane() {
    let c = Sandbox::new_seeded("reloc-src");
    c.ok(&["push", "a goal", "--why", "it is the first thing"]);
    let before = c.ok(&["stack"]);
    let dest = sibling_dir(&c, "moves");
    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(dest.join(".vivac").join("events").is_file());
    assert!(!c.0.join(".vivac").join("events").is_file());
    assert!(c.0.join(".vivac").join("events.relocated").is_file());
    assert!(c.0.join(".vivac").join("lane").is_file());
    assert_eq!(
        c.ok(&["stack"]),
        before,
        "the old folder keeps its own thread"
    );

    std::fs::remove_dir_all(&dest).ok();
}

#[test]
fn a_busy_destination_is_refused() {
    let c = Sandbox::new_seeded("reloc-busy-src");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = Sandbox::new_seeded("reloc-busy-dest"); // already a tree of its own

    let (out, code) = c.run(&["relocate", dest.0.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(
        c.0.join(".vivac").join("events").is_file(),
        "a refused relocate must leave the origin's own log in place"
    );
    assert!(
        !c.0.join(".vivac").join("events.relocated").exists(),
        "a refused relocate must not have renamed anything at the origin"
    );
}

/// `run`'s own byte-mismatch branch (step 5) has no trigger a black-box
/// test can reach -- the copy and the read that verifies it run back to
/// back with no window for anything else to land a write in between. What
/// this proves instead is the property that actually matters: a move that
/// cannot finish leaves the origin whole and the destination clean,
/// whichever half of step 5 stopped it.
///
/// The trigger: `.vivac/.gitignore` at the destination, already a
/// directory before `relocate` ever runs. Step 3 never looks at that file,
/// so the order goes ahead, copies `events` and `config` cleanly, and then
/// cannot write `.gitignore` where a directory already sits -- no
/// operating system allows a file to land on top of one, so this is
/// deterministic and portable rather than a race against anything.
#[test]
fn a_move_that_cannot_finish_leaves_the_source_alone() {
    let c = Sandbox::new_seeded("reloc-cannot-finish");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "cannot-finish");
    std::fs::create_dir_all(dest.join(".vivac").join(".gitignore")).unwrap();

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_ne!(code, 0, "{out}");

    assert!(
        c.0.join(".vivac").join("events").is_file(),
        "the origin's own log must survive a move that could not finish"
    );
    assert!(
        c.0.join(".vivac").join("config").is_file(),
        "the origin's own config must survive a move that could not finish"
    );
    assert!(
        !c.0.join(".vivac").join("events.relocated").exists(),
        "the origin must not be renamed for a move that never finished"
    );
    assert!(
        !c.0.join(".vivac").join("lane").exists(),
        "the origin must not gain a lane file for a move that never finished"
    );
    assert!(
        !dest.join(".vivac").join("events").exists(),
        "no events must be left at a destination whose move could not finish"
    );

    std::fs::remove_dir_all(&dest).ok();
}

#[cfg(windows)]
fn second_spelling(p: &Path) -> Option<PathBuf> {
    let name = p.file_name()?.to_str()?;
    let other: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    (other != name).then(|| p.with_file_name(other))
}

#[cfg(not(windows))]
fn second_spelling(_p: &Path) -> Option<PathBuf> {
    None
}

/// `f612`, once more: the destination and the origin, reached by two
/// different spellings of the very same folder. On a platform with no
/// second spelling to offer, this says so on `stderr` and skips rather than
/// passing in silence.
///
/// Names the sentence, not just the exit code: without `same_folder` in
/// `run`'s own step 3, the raw existence check further down still refuses
/// this (the second spelling really does already hold the tree), but with
/// the wrong reason -- "already holds a tree", when what is actually true
/// is that this *is* the tree. Removing `same_folder` and watching the
/// message change is how to see this test earn its keep.
#[test]
fn relocate_to_this_folder_by_another_spelling_is_refused() {
    let c = Sandbox::new_seeded("reloc-spelling-src");
    c.ok(&["push", "a goal", "--why", "seed"]);

    let Some(destination) = second_spelling(&c.0) else {
        eprintln!(
            "skipped: this platform offers no second spelling of the same folder to test with"
        );
        return;
    };

    let (out, code) = c.run(&["relocate", destination.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(
        says(
            &out,
            "The destination is this folder, so there is nothing to move."
        ),
        "{out}"
    );
    assert!(
        !says(&out, "already holds a tree or a lane"),
        "a destination that is this folder must not be confused with one that is busy: {out}"
    );
    assert!(
        c.0.join(".vivac").join("events").is_file(),
        "the origin's own log must survive a relocate onto a second spelling of itself"
    );
    assert!(!c.0.join(".vivac").join("events.relocated").exists());
}

#[test]
fn the_moved_tree_claims_main() {
    let c = Sandbox::new_seeded("reloc-claims-main");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "claims-main");

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");

    let log = std::fs::read_to_string(dest.join(".vivac").join("events")).unwrap();
    assert!(
        log.contains(r#""type":"lane.claimed""#),
        "the moved tree must carry a lane.claimed for main:\n{log}"
    );

    let before = c.ok(&["stack"]);
    let (push_out, push_code) = c.run(&["push", "more work", "--why", "the origin keeps going"]);
    assert_eq!(push_code, 0, "{push_out}");
    let after = c.ok(&["stack"]);
    assert_ne!(
        before, after,
        "the origin's own push must still move its own stack"
    );

    std::fs::remove_dir_all(&dest).ok();
}

#[test]
fn relocate_in_a_lane_is_refused() {
    let c = Sandbox::new_seeded("reloc-in-lane");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let project = first_event_id(&c);
    // Nested under the origin: the ancestor walk `store::locate` already
    // does finds the tree without any registry entry to lean on.
    let lane_dir = c.0.join("child-lane");
    write_lane(&lane_dir, "01LANEAAAAAAAAAAAAAAAAAAAA", &project);
    let dest = sibling_dir(&c, "in-lane-dest");

    let (out, code) = run_in(
        &lane_dir,
        c.global_home(),
        &["relocate", dest.to_str().unwrap()],
    );
    assert_eq!(code, 1, "{out}");
    assert!(
        says(
            &out,
            "Run relocate in the folder that holds the tree, not in one of its lanes."
        ),
        "{out}"
    );
    assert!(!dest.exists(), "a refused relocate created the destination");
    assert!(c.0.join(".vivac").join("events").is_file());
}

#[test]
fn the_registry_points_at_the_destination_and_a_lane_still_finds_it() {
    let c = Sandbox::new_seeded("reloc-registry");
    c.ok(&["push", "a distinctive goal", "--why", "seed"]);
    let project = first_event_id(&c);

    // A lane elsewhere, outside the origin, that can only find the tree
    // through the registry -- `d273`'s own shape, fabricated the way
    // `tests/lanes.rs::a_lane_outside_the_tree_finds_it_through_the_registry`
    // does.
    let lane_dir = sibling_dir(&c, "registry-lane");
    write_lane(&lane_dir, "01LANEAAAAAAAAAAAAAAAAAAAA", &project);
    // `find`, not `stack`: the stack and the focus are the fabricated
    // lane's own and start out empty regardless of relocate (`d595`); the
    // nodes a search finds are the knowledge every lane shares.
    let before = run_in(&lane_dir, c.global_home(), &["find", "distinctive"]).0;
    assert!(
        says(&before, "a distinctive goal"),
        "the lane must read the real tree through the registry before relocate: {before}"
    );

    let dest = sibling_dir(&c, "registry-dest");
    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");

    // `store::locate` trusts a root the registry hands back without
    // checking that it still holds a tree, so a stale entry does not fail
    // loudly here: `Store::open` just seeds a fresh, empty one on top of
    // whatever `events.relocated` left behind, and a bare exit code of 0
    // would not tell the two apart. Only the real content does.
    let after = run_in(&lane_dir, c.global_home(), &["find", "distinctive"]).0;
    assert!(
        says(&after, "a distinctive goal"),
        "a lane that found the tree through the registry must still read the \
         real one after relocate, not a fresh empty tree seeded at the stale root: {after}"
    );

    std::fs::remove_dir_all(&dest).ok();
    std::fs::remove_dir_all(&lane_dir).ok();
}

#[test]
fn relocate_with_lane_name_renames_the_lane_left_behind() {
    let c = Sandbox::new_seeded("reloc-lane-name");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "lane-name-dest");

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap(), "--lane-name", "clone"]);
    assert_eq!(code, 0, "{out}");

    let log = std::fs::read_to_string(dest.join(".vivac").join("events")).unwrap();
    assert!(
        log.contains(r#""type":"lane.declared""#) && log.contains(r#""name":"clone""#),
        "the lane left behind must be redeclared under --lane-name's own name:\n{log}"
    );

    std::fs::remove_dir_all(&dest).ok();
}
