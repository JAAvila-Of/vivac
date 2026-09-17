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

/// `run_in`, with `stdout` and `stderr` kept apart: proving the copy
/// warning lands on the stream the agent's own parsing does not touch
/// needs the two kept separate, the same reason `tests/registry.rs`'s own
/// `run_split` exists.
fn run_in_split(dir: &Path, home: &Path, args: &[&str]) -> (String, String, i32) {
    let o = std::process::Command::new(BIN)
        .current_dir(dir)
        .env("VIVAC_HOME", home)
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
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

/// `t594`: `relocate` used to sit outside `may_append`,
/// so the ambient sighting the generic dispatch already computes for the
/// origin -- correctly `Noted::Copy` here -- never reached `stderr`. The
/// origin writes its own `.vivac/lane` as part of the move (`t594` §4.6,
/// step 8), which is exactly the write `warn_if_wrote` hangs off.
#[test]
fn relocate_from_a_copy_warns_on_stderr() {
    let original = Sandbox::new_seeded("reloc-copy-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);

    let copy_src = sibling_dir(&original, "copy-src");
    std::fs::create_dir_all(copy_src.join(".vivac")).unwrap();
    std::fs::copy(
        original.0.join(".vivac").join("events"),
        copy_src.join(".vivac").join("events"),
    )
    .unwrap();

    let dest = sibling_dir(&original, "copy-dest");
    let (stdout, stderr, code) = run_in_split(
        &copy_src,
        original.global_home(),
        &["relocate", dest.to_str().unwrap()],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stderr.contains("COPY OF ANOTHER TREE"),
        "relocate from a copy never warned:\n{stderr}"
    );
    assert!(
        !stdout.contains("COPY OF ANOTHER TREE"),
        "the warning leaked into stdout:\n{stdout}"
    );

    std::fs::remove_dir_all(&copy_src).ok();
    std::fs::remove_dir_all(&dest).ok();
}

/// `t594` fix-3: the exact failure this module exists to close, reached
/// with nobody dying at all. `relocate`'s own step 8 renames `config`
/// away first and `events` second; a read landing in between finds
/// `events` still there and `config` missing, and `Store::open` mints a
/// fresh, empty config right there -- the one the folder was never
/// supposed to have. That orphaned config outlives the second rename,
/// left over once `events` is gone too, and a folder with a lane file and
/// nothing else to check against it used to read as its own, empty tree.
///
/// Fabricated by hand rather than raced for: a genuine race against a
/// window of two file renames is not something a black-box test can
/// reliably win, and the state on disk answers the same question either
/// way. Reconstructed in the order the real window actually opens it:
/// `events` is put back so a read has something real to regenerate a
/// config *against* (config alone, with `events` already gone, reads as
/// no tree at all and never reaches the bug), and only then is `events`
/// taken away again, the way the second rename would have -- leaving the
/// orphaned config the first read minted as the only thing behind.
#[test]
fn a_reader_landing_between_the_two_renames_does_not_see_an_empty_tree() {
    let c = Sandbox::new_seeded("reloc-orphan-window");
    c.ok(&["push", "a distinctive goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "orphan-window-dest");

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");

    let vivac_dir = c.0.join(".vivac");
    std::fs::rename(vivac_dir.join("events.relocated"), vivac_dir.join("events")).unwrap();
    let (stack_out, stack_code) = c.run(&["stack"]);
    assert_eq!(stack_code, 0, "{stack_out}");
    assert!(
        vivac_dir.join("config").is_file(),
        "the read itself must have regenerated a config here, config missing and \
         events present is exactly the window relocate's own step 8 opens"
    );
    std::fs::rename(vivac_dir.join("events"), vivac_dir.join("events.relocated")).unwrap();
    // The folder is now exactly what a reader landing in the real window
    // would see: a lane file, an orphaned config with no events behind
    // it, and nothing else.

    let (after_out, after_code) = c.run(&["find", "distinctive"]);
    assert_eq!(after_code, 0, "{after_out}");
    assert!(
        says(&after_out, "a distinctive goal"),
        "a reader here must still find the real tree at the destination, not an \
         empty one seeded on top of an orphaned config: {after_out}"
    );

    std::fs::remove_dir_all(&dest).ok();
}

#[test]
fn a_busy_destination_is_refused() {
    let c = Sandbox::new_seeded("reloc-busy-src");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = Sandbox::new_seeded("reloc-busy-dest"); // already a tree of its own
    let dest_before = std::fs::read(dest.0.join(".vivac").join("events")).unwrap();

    let (out, code) = c.run(&["relocate", dest.0.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(says(&out, "already holds a tree or a lane"), "{out}");
    assert!(
        c.0.join(".vivac").join("events").is_file(),
        "a refused relocate must leave the origin's own log in place"
    );
    assert!(
        !c.0.join(".vivac").join("events.relocated").exists(),
        "a refused relocate must not have renamed anything at the origin"
    );
    assert_eq!(
        std::fs::read(dest.0.join(".vivac").join("events")).unwrap(),
        dest_before,
        "a refused relocate must leave the destination's own tree exactly as it was"
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

/// `t594` fix-2, finding M1: an earlier round of this task tore down the
/// destination's whole `.vivac/` on any failure, taking files this run
/// never wrote along with it -- measured by the reviewer as an unrelated
/// `notes.txt` and an `events.relocated` from an earlier move, lost
/// alongside the copy that was actually being cleaned up.
#[test]
fn a_failed_move_never_touches_files_the_destination_already_had() {
    let c = Sandbox::new_seeded("reloc-preexisting");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "preexisting");
    std::fs::create_dir_all(dest.join(".vivac")).unwrap();
    std::fs::write(dest.join(".vivac").join("notes.txt"), b"keep me").unwrap();
    // Forces step 6 to fail after the destination's own `.vivac/` --
    // already there before this run -- is reused rather than created.
    std::fs::create_dir_all(dest.join(".vivac").join(".gitignore")).unwrap();

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_ne!(code, 0, "{out}");

    assert_eq!(
        std::fs::read(dest.join(".vivac").join("notes.txt")).unwrap(),
        b"keep me",
        "a file the destination already had, and its content, must survive a failed move"
    );
    assert!(
        dest.join(".vivac").is_dir(),
        "a .vivac/ that already existed before this run must survive a failed move"
    );
    assert!(!dest.join(".vivac").join("events").exists());

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
        log.contains(r#""type":"lane.claimed","lane":"main""#),
        "the moved tree must carry a lane.claimed naming main itself, not merely \
         an event of that type:\n{log}"
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

#[test]
fn relocate_with_an_empty_destination_is_a_usage_error() {
    let c = Sandbox::new_seeded("reloc-empty-arg");
    let (out, code) = c.run(&["relocate", ""]);
    assert_eq!(code, 2, "{out}");
}

/// `t594` fix-2, finding 6: a tree with nothing in it yet has no first
/// event and so no identity (`d201`) for the registry or the origin's own
/// lane file to be keyed by. Refusing is cheaper and honester than moving
/// it and cementing a made-up one.
#[test]
fn relocate_refuses_a_tree_with_no_events_yet() {
    let c = Sandbox::new_seeded("reloc-no-events");
    let dest = sibling_dir(&c, "no-events-dest");

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(
        says(
            &out,
            "This tree has no events yet, so nothing points at it and nothing would \
             break: move the folder yourself."
        ),
        "{out}"
    );
    assert!(
        !dest.exists(),
        "a refused relocate must not create the destination"
    );
    assert!(c.0.join(".vivac").join("config").is_file());
}

/// `t594` fix-2, finding 10: `relocate ..` is the very thing another text
/// in `t594` §6.4 recommends, so the destination sitting *above* the
/// origin has to keep working. What is refused is the opposite direction.
#[test]
fn relocate_into_a_subfolder_of_the_origin_is_refused() {
    let c = Sandbox::new_seeded("reloc-inside");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = c.0.join("nested-dest");

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(
        says(
            &out,
            "The destination is inside this folder. Moving the tree into itself \
             would leave it with two."
        ),
        "{out}"
    );
    assert!(
        !dest.exists(),
        "a refused relocate must not create the destination"
    );
    assert!(c.0.join(".vivac").join("events").is_file());
}

/// `t594` fix-2, finding 4: the destination is the tree now, but it is not
/// a lane yet, and the success text has to say so up front rather than
/// leaving that for the first refused write to explain.
#[test]
fn the_success_text_says_the_new_folder_is_not_a_lane_yet() {
    let c = Sandbox::new_seeded("reloc-print");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "print");

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(says(
        &out,
        "The new folder holds the tree but is not a lane yet. To work there:"
    ));
    assert!(says(&out, "vivac setup claude-code"));

    std::fs::remove_dir_all(&dest).ok();
}

/// `t594` fix-2, finding A1: `path_disagrees`-free or not, the registry
/// still has to be keyed by an *absolute* root -- a relative one only
/// ever resolves correctly against the `cwd` that typed it, and every
/// other lane resolves it against its own.
#[test]
fn relocate_with_a_relative_destination_still_lets_another_lane_find_it() {
    let c = Sandbox::new_seeded("reloc-relative");
    c.ok(&["push", "a distinctive goal", "--why", "seed"]);
    let project = first_event_id(&c);
    let dest_name = format!(
        "vivac-relocate-relative-dest-{}-{}",
        std::process::id(),
        id_seed()
    );
    let dest = c.0.parent().unwrap().join(&dest_name);
    let relative = format!("../{dest_name}");

    let (out, code) = c.run(&["relocate", &relative]);
    assert_eq!(code, 0, "{out}");
    assert!(dest.join(".vivac").join("events").is_file());

    // A lane elsewhere resolves the registry's own `path` against *its*
    // `cwd`, not the one relocate ran from. This one is deliberately at a
    // different depth, not a sibling of the origin -- a sibling would
    // still resolve "../<name>" to the same real folder by coincidence of
    // sharing a parent, which is exactly the kind of accident that would
    // let a raw relative string in the registry go unnoticed here.
    let lane_container = unique_dir("relative-lane-container");
    let lane_dir = lane_container.join("nested").join("deeper");
    std::fs::create_dir_all(&lane_dir).unwrap();
    write_lane(&lane_dir, "01LANEAAAAAAAAAAAAAAAAAAAA", &project);
    let (find_out, find_code) = run_in(&lane_dir, c.global_home(), &["find", "distinctive"]);
    assert_eq!(find_code, 0, "{find_out}");
    assert!(says(&find_out, "a distinctive goal"), "{find_out}");

    std::fs::remove_dir_all(&dest).ok();
    std::fs::remove_dir_all(&lane_container).ok();
}

/// `t594` fix-2, finding 3: `relocate ..` by name, the exact command this
/// crate's own advice recommends in `t594` §6.4, moving the tree up out
/// of a nested clone into the folder that holds the product.
#[test]
fn relocate_dot_dot_moves_the_tree_up_one_level() {
    let home = unique_dir("dotdot-home");
    let container = unique_dir("dotdot-container");
    let clone = container.join("clone");
    std::fs::create_dir_all(&clone).unwrap();

    let (init_out, init_code) = run_in(&clone, &home, &["init"]);
    assert_eq!(init_code, 0, "{init_out}");
    let (push_out, push_code) = run_in(&clone, &home, &["push", "a goal", "--why", "seed"]);
    assert_eq!(push_code, 0, "{push_out}");

    let (out, code) = run_in(&clone, &home, &["relocate", ".."]);
    assert_eq!(code, 0, "{out}");
    assert!(
        container.join(".vivac").join("events").is_file(),
        "the tree must land at the parent, `..`, exactly what setup's own advice means"
    );
    assert!(clone.join(".vivac").join("lane").is_file());

    std::fs::remove_dir_all(&container).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// `t594` fix-3, finding N1: `to_absolute` only joined, and `relocate ..`
/// left the registry holding the raw `…\clone\..`. `Path::file_name` of a
/// path ending in `..` is `None`, so `render::project_name` read that back
/// as the bare word `"-"`, and every project relocated this way would have
/// collided on that one name in `find --everywhere` and refused
/// `--project` outright.
#[test]
fn relocate_dot_dot_normalizes_the_path_the_registry_keeps() {
    let home = unique_dir("dotdot-normalize-home");
    let container = unique_dir("dotdot-normalize-container");
    let clone = container.join("clone");
    std::fs::create_dir_all(&clone).unwrap();

    let (init_out, init_code) = run_in(&clone, &home, &["init"]);
    assert_eq!(init_code, 0, "{init_out}");
    let (push_out, push_code) = run_in(&clone, &home, &["push", "a goal", "--why", "seed"]);
    assert_eq!(push_code, 0, "{push_out}");

    let (out, code) = run_in(&clone, &home, &["relocate", ".."]);
    assert_eq!(code, 0, "{out}");

    let registry = std::fs::read_to_string(home.join("projects")).unwrap();
    assert!(
        !registry.contains(".."),
        "the registry must never keep an unresolved .. in a path: {registry}"
    );

    let project_name = container.file_name().unwrap().to_str().unwrap();
    let (why_out, why_code) = run_in(&clone, &home, &["why", "g1", "--project", project_name]);
    assert_eq!(
        why_code, 0,
        "a project name built from the real folder must resolve: {why_out}"
    );

    std::fs::remove_dir_all(&container).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// `t594` fix-3, finding 4 (B1): `relocate` used to pass from any
/// subfolder of the tree, since `store::locate`'s own upward walk makes
/// `located.root == located.lane_dir` true from there too, and then print
/// a lane and a log neither one is actually in.
#[test]
fn relocate_from_a_subfolder_of_the_tree_is_refused() {
    let c = Sandbox::new_seeded("reloc-subfolder");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let deep = c.0.join("src").join("deep");
    std::fs::create_dir_all(&deep).unwrap();
    let dest = sibling_dir(&c, "subfolder-dest");

    let (out, code) = run_in(
        &deep,
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
    assert!(
        !dest.exists(),
        "a refused relocate must not create the destination"
    );
    assert!(c.0.join(".vivac").join("events").is_file());
}

/// `t594` fix-3, finding N3: the rollback used to copy over a `.gitignore`
/// the destination already had and then track the result as its own,
/// losing whatever line someone had added to it. `M1`'s own test never
/// saw this because it makes `.gitignore` a directory, the one case a
/// copy cannot land on top of at all.
#[test]
fn a_failed_move_never_overwrites_a_gitignore_the_destination_already_had() {
    let c = Sandbox::new_seeded("reloc-gitignore-preexisting");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "gitignore-preexisting");
    std::fs::create_dir_all(dest.join(".vivac")).unwrap();
    std::fs::write(dest.join(".vivac").join(".gitignore"), b"*\n!keep-this\n").unwrap();
    // Forces step 7 to fail, well after step 6 would already have reused
    // the destination's own `.gitignore` rather than writing over it.
    std::fs::remove_dir_all(c.global_home()).ok();
    std::fs::write(c.global_home(), b"not a directory").unwrap();

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_ne!(code, 0, "{out}");

    assert_eq!(
        std::fs::read(dest.join(".vivac").join(".gitignore")).unwrap(),
        b"*\n!keep-this\n",
        "a .gitignore the destination already had, and its content, must survive a \
         failed move"
    );

    std::fs::remove_file(c.global_home()).ok();
    std::fs::remove_dir_all(&dest).ok();
}

/// `t594` fix-4, finding N9: the same rule N3 gave `.gitignore` applies to
/// `lock` -- `commit_copy` used to `File::create` the destination's `lock`
/// unconditionally and track it for rollback regardless of whether one was
/// already there, so a failed move could delete a `lock` the destination
/// brought with it.
#[test]
fn a_failed_move_never_overwrites_a_lock_the_destination_already_had() {
    let c = Sandbox::new_seeded("reloc-lock-preexisting");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "lock-preexisting");
    std::fs::create_dir_all(dest.join(".vivac")).unwrap();
    std::fs::write(
        dest.join(".vivac").join("lock"),
        b"pre-existing-lock-marker",
    )
    .unwrap();
    // Forces step 7 to fail, well after step 6 would already have reused
    // the destination's own `lock` rather than writing over it.
    std::fs::remove_dir_all(c.global_home()).ok();
    std::fs::write(c.global_home(), b"not a directory").unwrap();

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_ne!(code, 0, "{out}");

    assert_eq!(
        std::fs::read(dest.join(".vivac").join("lock")).unwrap(),
        b"pre-existing-lock-marker",
        "a lock the destination already had, and its content, must survive a \
         failed move"
    );

    std::fs::remove_file(c.global_home()).ok();
    std::fs::remove_dir_all(&dest).ok();
}

/// `t594` fix-3, finding 6 (N4): a `.vivac/` this run created and left
/// empty must not survive its own rollback -- if it did, the folder would
/// still read as busy the next time step 5 checked it, blocking the very
/// retry a failed move should always allow.
#[test]
fn a_destination_left_empty_by_a_failed_move_can_be_retried() {
    let c = Sandbox::new_seeded("reloc-retry-empty");
    c.ok(&["push", "a goal", "--why", "seed"]);
    let dest = sibling_dir(&c, "retry-empty-dest");
    // The `.vivac/` this run creates in step 6 has nothing foreign inside
    // it, so a step 7 failure has to roll it back outright.
    std::fs::remove_dir_all(c.global_home()).ok();
    std::fs::write(c.global_home(), b"not a directory").unwrap();

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_ne!(code, 0, "{out}");
    assert!(
        !dest.join(".vivac").exists(),
        "a .vivac/ this run created and left empty must not survive its own rollback"
    );
    std::fs::remove_file(c.global_home()).ok();

    let (retry_out, retry_code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_eq!(
        retry_code, 0,
        "a retry must not be blocked by the failed attempt: {retry_out}"
    );
    assert!(dest.join(".vivac").join("events").is_file());

    std::fs::remove_dir_all(&dest).ok();
}

/// `t594` fix-2, finding 1: step 7's own rollback. `record_move` -- unlike
/// `note`, which never fails its caller -- fails the whole operation when
/// the registry cannot be written, and the origin has to come back whole.
#[test]
fn a_registry_that_cannot_be_written_leaves_the_source_alone() {
    let c = Sandbox::new_seeded("reloc-registry-blocked");
    c.ok(&["push", "a goal", "--why", "seed"]);
    // The seed push above already created `VIVAC_HOME` as a directory,
    // writing the registry into it. Replaced with a plain file so
    // `record_move`'s own `create_dir_all` cannot make a directory where
    // a file already sits.
    std::fs::remove_dir_all(c.global_home()).ok();
    std::fs::write(c.global_home(), b"not a directory").unwrap();
    let dest = sibling_dir(&c, "registry-blocked");

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_ne!(code, 0, "{out}");

    assert!(
        c.0.join(".vivac").join("events").is_file(),
        "a registry failure must leave the origin's own log in place"
    );
    assert!(!c.0.join(".vivac").join("events.relocated").exists());
    assert!(
        !c.0.join(".vivac").join("lane").exists(),
        "the origin must not gain a lane file before the registry landed"
    );
    assert!(
        !dest.join(".vivac").join("events").exists(),
        "no events must be left at a destination whose registry write failed"
    );

    std::fs::remove_file(c.global_home()).ok();
}

/// `t594` fix-2, finding 2: a failure *inside* step 8 -- after the origin's
/// own `.vivac/lane` already landed -- still leaves the origin a working
/// tree, because that file is written before anything is renamed, not
/// after.
#[test]
fn a_blocked_rename_at_the_origin_still_leaves_it_a_working_tree() {
    let c = Sandbox::new_seeded("reloc-origin-blocked");
    c.ok(&["push", "a distinctive goal", "--why", "seed"]);
    let before = c.ok(&["find", "distinctive"]);
    // `config.relocated` already exists as a directory: step 8's own
    // rename of `config` onto that name cannot land, even though the
    // copy, the registry and the origin's own lane file already have.
    std::fs::create_dir_all(c.0.join(".vivac").join("config.relocated")).unwrap();
    let dest = sibling_dir(&c, "origin-blocked");

    let (out, code) = c.run(&["relocate", dest.to_str().unwrap()]);
    assert_ne!(code, 0, "{out}");
    assert!(
        says(
            &out,
            "Something failed while updating this folder's own bookkeeping."
        ),
        "a step 8 failure must say what to check, not just the bare IO error: {out}"
    );

    assert!(
        c.0.join(".vivac").join("lane").is_file(),
        "the lane file is written before the rename that failed"
    );
    assert!(
        c.0.join(".vivac").join("events").is_file(),
        "the origin's own log must still be there: its own rename never ran"
    );
    assert!(
        c.0.join(".vivac").join("config").is_file(),
        "the origin's own config must still be there: its own rename is what failed"
    );
    // `already_planted` is still true here, so the folder still answers
    // from its own real tree without ever having to ask the registry.
    assert_eq!(c.ok(&["find", "distinctive"]), before);

    std::fs::remove_dir_all(&dest).ok();
}

/// A directory nothing else in this file or its `Sandbox`s owns, for the
/// tests that need a layout `Sandbox::new_seeded` cannot give them: a
/// tree planted somewhere other than its own home's obvious sibling.
fn unique_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "vivac-relocate-{name}-{}-{}",
        std::process::id(),
        id_seed()
    ))
}
