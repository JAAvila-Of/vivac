//! Resolving a working folder to the tree it belongs to and the lane it is
//! (`t594` §2.3), from outside the process: a lane folder is not something
//! any command writes yet, so these fabricate `.vivac/lane` by hand, the
//! same way `common::Sandbox::append_raw_line` fabricates log shapes no CLI
//! path writes yet either.

mod common;
use common::Sandbox;
use std::path::{Path, PathBuf};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// Whether the output says `sentence`, ignoring where the lines break.
/// Paragraphs are wrapped to a fixed width, so a sentence lands across two
/// lines as often as not, and a test that compares the raw text fails on a
/// rewrap while claiming the wording changed.
fn says(out: &str, sentence: &str) -> bool {
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .contains(sentence)
}

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

/// Same as `run`, with a payload on stdin -- the way a hook is called.
/// `common::Sandbox::run_stdin` does the same over a `Sandbox`; this file
/// builds its worktree roots by hand instead.
fn run_stdin(dir: &Path, home: &Path, args: &[&str], stdin: &str) -> (String, i32) {
    use std::io::Write;
    let mut child = std::process::Command::new(BIN)
        .current_dir(dir)
        .env("VIVAC_HOME", home)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let o = child.wait_with_output().unwrap();
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

/// Whether `dir` holds a tree of its own: `store::already_planted`'s own
/// definition, `config` **or** `events` (`t594` fix-1, finding 4).
fn already_planted(dir: &Path) -> bool {
    dir.join(".vivac").join("config").is_file() || dir.join(".vivac").join("events").is_file()
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
    assert!(!already_planted(&second), "a second tree was planted");
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

// ---------------------------------------------------------------------------
// `t594` fix-1, ronda 1.
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with one commit at a fresh `root`, a linked worktree of
/// it at `root`-`feature`, and a fresh `VIVAC_HOME` with the root's tree
/// already `init`ed. Both round 1 and round 2 of the worktree finding
/// need exactly this to reach the point where `setup` runs in `feature`.
fn worktree_fixture(prefix: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = unique(&format!("worktree-{prefix}-root"));
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    std::fs::write(root.join("f.txt"), "x").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "first"]);

    let home = unique(&format!("worktree-{prefix}-home"));
    let (init_out, init_code) = run(&root, &home, &["init"]);
    assert_eq!(init_code, 0, "{init_out}");

    let feature = root.parent().unwrap().join(format!(
        "{}-feature",
        root.file_name().unwrap().to_string_lossy()
    ));
    git(&root, &["worktree", "add", &feature.display().to_string()]);

    (root, feature, home)
}

/// Finding 1 (high): a linked worktree sits *beside* the tree's own
/// folder, not above it, so the upward walk never finds it and the only
/// way back is the registry. `setup` used to be the one command that
/// never noted one, which left the worktree unable to read its own
/// `brief` ever again once it declared a lane there. `setup` now notes
/// the tree it joins, the same as any other command that writes to it.
#[test]
fn setup_in_a_linked_worktree_registers_the_tree_it_joins() {
    let (root, feature, home) = worktree_fixture("registers");

    let (setup_out, setup_code) = run(&feature, &home, &["setup", "claude-code", "--yes"]);
    assert_eq!(setup_code, 0, "{setup_out}");

    let (brief_out, brief_code) = run(&feature, &home, &["brief"]);
    assert_eq!(brief_code, 0, "{brief_out}");
    assert!(
        !brief_out.contains("--join"),
        "the worktree came back unusable:\n{brief_out}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&feature).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// `t594` fix-2, finding 1: `registry::note` swallows its own errors by
/// design (`f603`), so noting the tree on its own cannot be what keeps a
/// linked worktree usable -- a `VIVAC_HOME` that cannot be written to
/// would leave it in exit 4 just the same, silently. What actually fixes
/// it is `resolve_lane` trying the worktree's main copy on its own, the
/// same retry `locate_from` already does for a folder with no lane file
/// at all. The registry is deleted entirely here, not just left unable
/// to write, to prove that path is not what this depends on any more.
#[test]
fn setup_in_a_linked_worktree_still_works_with_the_registry_gone() {
    let (root, feature, home) = worktree_fixture("noreg");

    let (setup_out, setup_code) = run(&feature, &home, &["setup", "claude-code", "--yes"]);
    assert_eq!(setup_code, 0, "{setup_out}");

    std::fs::remove_dir_all(&home).ok();
    assert!(!home.exists(), "the registry survived its own deletion");

    let (brief_out, brief_code) = run(&feature, &home, &["brief"]);
    assert_eq!(brief_code, 0, "{brief_out}");
    assert!(
        !brief_out.contains("--join"),
        "the worktree came back unusable with the registry gone:\n{brief_out}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&feature).ok();
}

/// Finding 5 (media-baja): `unchanged` used to decide `needs_lock` too, so
/// a config whose lanes sentence was removed by hand -- or by an older
/// `Store::open` regenerating one that went missing before it knew a lane
/// event counts -- never got relocked by a later `setup` that had nothing
/// new to declare.
#[test]
fn setup_relocks_the_config_when_its_lanes_sentence_was_removed_by_hand() {
    let c = Sandbox::new_seeded("declare-relock");
    setup_ok(&c.0, c.global_home());

    let config_path = c.0.join(".vivac").join("config");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    cfg["version"] = serde_json::json!(1);
    std::fs::write(
        &config_path,
        format!("{}\n", serde_json::to_string_pretty(&cfg).unwrap()),
    )
    .unwrap();

    let out = setup_ok(&c.0, c.global_home());
    let after = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        after.contains("this tree holds lanes"),
        "the sentence did not come back:\n{after}\n\n{out}"
    );
    // `t594` fix-3: this run recorded no thread at all, only closed the
    // lock again, and the message has to say that rather than the
    // sentence a real declaration earns.
    assert!(
        says(
            &out,
            "setup wrote in it: the sentence that stops an older vivac"
        ),
        "{out}"
    );
    assert!(!says(&out, "this folder's own thread"), "{out}");
}

// ---------------------------------------------------------------------------
// `t594` task 8: a linked worktree that is nobody's lane yet joins the tree
// the first time anything writes from it, and a folder whose `main` was
// claimed elsewhere can read but not write.
// ---------------------------------------------------------------------------

/// A repository with one commit at a fresh `root`, and a linked worktree of
/// it *inside* `root` at `root/feature` -- `git worktree add feature`, run
/// from `root` itself, the shape a harness that keeps its worktrees beside
/// the checkout leaves behind.
fn worktree_inside_fixture(prefix: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = unique(&format!("worktree-in-{prefix}-root"));
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    std::fs::write(root.join("f.txt"), "x").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "first"]);

    let home = unique(&format!("worktree-in-{prefix}-home"));
    let (init_out, init_code) = run(&root, &home, &["init"]);
    assert_eq!(init_code, 0, "{init_out}");

    let feature = root.join("feature");
    git(&root, &["worktree", "add", "feature"]);

    (root, feature, home)
}

/// Appends a line straight to `tree_root/.vivac/events`: the same escape
/// hatch `common::Sandbox::append_raw_line` gives the tests in this crate's
/// other files, for a tree this file builds by hand rather than through a
/// `Sandbox`.
fn append_raw_line(tree_root: &Path, line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(tree_root.join(".vivac").join("events"))
        .unwrap();
    writeln!(f, "{line}").unwrap();
}

/// Locks `tree_root`'s config to the lanes sentence by hand, the same
/// change `Store::lock_lanes_in_config` makes, so a test that seeds a
/// `lane.declared` with `append_raw_line` -- which never touches the
/// config -- still leaves the tree looking like one where a real `setup`
/// or a real auto-join already ran. `t594` branch-fix-1 #2 gates joining a
/// worktree on nothing less than this.
fn seed_lanes_config(tree_root: &Path) {
    let path = tree_root.join(".vivac").join("config");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    cfg["version"] = serde_json::json!(
        "this tree holds lanes, and this vivac is too old to read them: update vivac"
    );
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&cfg).unwrap()),
    )
    .unwrap();
}

/// The `id` a `.vivac/lane` file names.
fn lane_id_of(lane_dir: &Path) -> String {
    let text =
        std::fs::read_to_string(lane_dir.join(".vivac").join("lane")).expect("the lane file reads");
    let v: serde_json::Value = serde_json::from_str(&text).expect("the lane file parses");
    v["id"]
        .as_str()
        .expect("a lane file names an id")
        .to_string()
}

/// (1): a worktree nested inside the lane's own folder. Before it writes,
/// its brief has no `HERE` -- nothing is on its stack yet, because it reads
/// from a lane nobody has written to. After a `push`, it has its own
/// `.vivac/lane` naming a fresh id, a `lane.declared` in the tree's own
/// log, and its own stack, kept apart from `root`'s.
#[test]
fn a_worktree_inside_the_lanes_folder_joins_on_its_first_write() {
    let (root, feature, home) = worktree_inside_fixture("joins");
    // `t594` branch-fix-1 #2: joining on its own requires the tree to
    // already have a lane declared somewhere, so this stands in for a
    // `setup` that ran before `feature` ever existed.
    append_raw_line(
        &root,
        r#"{"seq":1,"id":"01SEEDMAINAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.declared","lane":"main","name":"main","repos":[{"path":"."}]}}"#,
    );
    seed_lanes_config(&root);

    let (before, code) = run(&feature, &home, &["brief"]);
    assert_eq!(code, 0, "{before}");
    assert!(!before.contains("<== HERE"), "{before}");

    let (out, code) = run(&feature, &home, &["push", "Feature work", "--why", "seed"]);
    assert_eq!(code, 0, "{out}");

    assert!(
        feature.join(".vivac").join("lane").exists(),
        "no lane file appeared in the worktree"
    );
    let lane_id = lane_id_of(&feature);
    assert_ne!(
        lane_id, "main",
        "the worktree joined signing as main itself"
    );

    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    assert!(
        log.contains("\"type\":\"lane.declared\""),
        "no lane.declared reached the tree's own log:\n{log}"
    );
    assert!(
        log.contains(&lane_id),
        "the log's lane.declared does not name the id the worktree just minted:\n{log}"
    );

    let (root_stack, code) = run(&root, &home, &["stack"]);
    assert_eq!(code, 0, "{root_stack}");
    assert!(!root_stack.contains("Feature work"), "{root_stack}");

    let (feature_stack, code) = run(&feature, &home, &["stack"]);
    assert_eq!(code, 0, "{feature_stack}");
    assert!(feature_stack.contains("Feature work"), "{feature_stack}");

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// (2): `git worktree add ../feature`, outside `root`'s own folder. It
/// joins exactly the same way, found through its main copy the same way
/// `setup` already was (`t594` fix-1).
#[test]
fn a_worktree_outside_the_folder_joins_through_its_main_copy() {
    let (root, feature, home) = worktree_fixture("outside-joins");
    // `t594` branch-fix-1 #2: joining on its own requires the tree to
    // already have a lane declared somewhere. `feature` sits outside
    // `root` entirely, so seeding this after it exists cannot make
    // `repos::scan` find it and change what this test is proving.
    append_raw_line(
        &root,
        r#"{"seq":1,"id":"01SEEDMAINAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.declared","lane":"main","name":"main","repos":[{"path":"."}]}}"#,
    );
    seed_lanes_config(&root);

    let (out, code) = run(&feature, &home, &["add", "Outside work", "--why", "seed"]);
    assert_eq!(code, 0, "{out}");

    assert!(
        feature.join(".vivac").join("lane").exists(),
        "no lane file appeared in the worktree"
    );
    let lane_id = lane_id_of(&feature);

    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    assert!(
        log.contains(&lane_id) && log.contains("\"type\":\"lane.declared\""),
        "no lane.declared naming the worktree's id reached the tree's own log:\n{log}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&feature).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// (3): the worktree's own repository inherits the root commit `main`
/// already declared for it, and never asks git for one of its own -- the
/// seeded value is not one `git rev-list` could ever produce, so if
/// anything here called out to git, the value the new lane declares would
/// not match it.
#[test]
fn a_pending_worktree_inherits_the_declared_root_commit_without_git() {
    let (root, feature, home) = worktree_inside_fixture("inherits-root");
    append_raw_line(
        &root,
        r#"{"seq":1,"id":"01SEEDMAINAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.declared","lane":"main","name":"main","repos":[{"path":".","root":"01NOTARELCOMMITAAAAAAAAAAA"}]}}"#,
    );
    // `t594` branch-fix-1 #2: joining on its own requires the tree to
    // already have a lane declared somewhere.
    seed_lanes_config(&root);

    let (out, code) = run(&feature, &home, &["add", "Inherits root", "--why", "seed"]);
    assert_eq!(code, 0, "{out}");

    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    // Line 1 is the seed above, unchanged; line 2 is the worktree's own
    // join, and it is that second line -- not the seed still sitting
    // there -- that has to carry the inherited value.
    let joined = log
        .lines()
        .nth(1)
        .expect("the worktree's own join wrote a second line");
    assert!(
        joined.contains("\"type\":\"lane.declared\""),
        "the second line is not the worktree's own join:\n{log}"
    );
    assert!(
        !joined.contains("\"lane\":\"main\""),
        "the second line still signs as main:\n{log}"
    );
    assert!(
        says(joined, r#""root":"01NOTARELCOMMITAAAAAAAAAAA""#),
        "the worktree's own lane.declared does not carry the root main already declared:\n{joined}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// `t594` branch-fix-1 #6: a worktree that has not joined yet reads from
/// `ops::PENDING_VIEW`, an empty lane name, and the header used to print
/// that empty string verbatim -- `lane: ` with nothing after the colon,
/// the one byte of output that changed on a tree nobody had written to
/// from this folder yet.
#[test]
fn a_worktree_that_has_not_joined_yet_says_so_in_its_own_brief() {
    let (root, feature, home) = worktree_inside_fixture("not-joined");
    append_raw_line(
        &root,
        r#"{"seq":1,"id":"01SEEDMAINAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.declared","lane":"main","name":"main","repos":[{"path":"."}]}}"#,
    );
    seed_lanes_config(&root);

    let (before, code) = run(&feature, &home, &["brief"]);
    assert_eq!(code, 0, "{before}");
    let header = before.lines().next().unwrap_or("");
    assert!(
        says(header, "lane: not joined yet"),
        "a pending worktree's header does not say it has not joined yet:\n{header}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// Writes the `.git` file a linked worktree and a submodule both carry: a
/// file at `working_dir` naming a `gitdir` elsewhere. Without a `commondir`
/// inside that `gitdir`, this is exactly what a submodule looks like --
/// `anchor::linked_worktree`'s own criterion (`t594` task 4).
fn write_submodule_git_file(working_dir: &Path, gitdir: &Path) {
    std::fs::create_dir_all(working_dir).unwrap();
    std::fs::create_dir_all(gitdir).unwrap();
    std::fs::write(
        working_dir.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
}

/// (4): a submodule inside a linked worktree is not a worktree of its own
/// (`t594` task 4) and does not mint a lane of its own either: it stays
/// part of whatever lane already contains it, here `main`, since the
/// worktree around it never joined.
#[test]
fn a_submodule_inside_a_worktree_does_not_join_a_lane_of_its_own() {
    let (root, feature, home) = worktree_inside_fixture("submodule");
    let sub = feature.join("sub");
    write_submodule_git_file(&sub, &feature.join("sub-gitdir"));

    let (out, code) = run(&sub, &home, &["add", "From the submodule", "--why", "seed"]);
    assert_eq!(code, 0, "{out}");

    assert!(
        !sub.join(".vivac").join("lane").exists(),
        "the submodule minted a lane of its own"
    );
    assert!(
        !feature.join(".vivac").join("lane").exists(),
        "the worktree around the submodule joined a lane on the submodule's behalf"
    );
    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    assert!(
        !log.contains("\"type\":\"lane.declared\""),
        "a lane.declared reached the log for a folder that never asked to join:\n{log}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// (5), §6.9: with `lane.claimed` seeded by hand, a folder that holds the
/// tree but is not one of its lanes can still be read, and cannot write.
#[test]
fn a_folder_whose_main_was_claimed_elsewhere_can_read_but_not_write() {
    let c = Sandbox::new_seeded("claimed-main");
    c.append_raw_line(
        r#"{"seq":1,"id":"01SEEDCLAIMAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.claimed","lane":"main"}}"#,
    );

    let (brief, code) = c.run(&["brief"]);
    assert_eq!(code, 0, "{brief}");

    let (out, code) = c.run(&["push", "x", "--why", "y"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        says(
            &out,
            "This folder holds the tree but is not one of its lanes"
        ),
        "{out}"
    );
}

/// (6): a worktree `main` already declared as one of its own repositories
/// does not join a lane of its own -- it is that lane's repository at that
/// path, and nothing more (paso 1, rule 2).
#[test]
fn a_worktree_already_declared_as_a_repo_does_not_join_a_new_lane() {
    let (root, feature, home) = worktree_inside_fixture("already-declared");
    append_raw_line(
        &root,
        r#"{"seq":1,"id":"01SEEDMAINAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.declared","lane":"main","name":"main","repos":[{"path":"."},{"path":"feature"}]}}"#,
    );

    let (out, code) = run(
        &feature,
        &home,
        &["add", "Should stay main", "--why", "seed"],
    );
    assert_eq!(code, 0, "{out}");

    assert!(
        !feature.join(".vivac").join("lane").exists(),
        "an already-declared repository still minted a lane of its own"
    );
    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    assert_eq!(
        log.matches("\"type\":\"lane.declared\"").count(),
        1,
        "a second lane.declared reached the log:\n{log}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// (7): looking joins nothing. Opening a brief inside a linked worktree
/// that never wrote leaves no `.vivac/lane` and no event -- a worktree the
/// harness throws away must not leave a lane behind for having been read.
#[test]
fn looking_inside_an_unjoined_worktree_leaves_no_trace() {
    let (root, feature, home) = worktree_inside_fixture("looking");

    let (brief, code) = run(&feature, &home, &["brief"]);
    assert_eq!(code, 0, "{brief}");
    let (why, code) = run(&feature, &home, &["why", "1"]);
    assert_eq!(code, 2, "{why}");

    assert!(
        !feature.join(".vivac").join("lane").exists(),
        "looking left a lane file behind"
    );
    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap_or_default();
    assert!(
        log.is_empty(),
        "looking wrote to the tree's own log:\n{log}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------------
// `t594` fix-1, round 1: joining moved from `lock_for_write` to `emit`, so a
// command that refuses before it has anything to write never joins either.
// ---------------------------------------------------------------------------

/// Finding B: `pop` on an empty stack, `note` naming an id that does not
/// resolve, and a title the redaction guard refuses -- three ways an
/// operation can fail after `lock_for_write` ran and before it ever calls
/// `emit`. Every one of them used to leave `feature/.vivac/lane`, a
/// `lane.declared` for `main` and a config locked to "this tree holds
/// lanes" behind, on a tree nobody had run `setup` on and a command that
/// changed nothing of its own.
#[test]
fn a_refusal_from_an_unjoined_worktree_leaves_nothing_written() {
    let (root, feature, home) = worktree_inside_fixture("refusal-writes-nothing");
    let config_before = std::fs::read_to_string(root.join(".vivac").join("config")).unwrap();

    let scenarios: [(&[&str], i32); 3] = [
        (&["pop"], 2),
        (&["note", "99", "x"], 2),
        (
            &[
                "add",
                "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
                "--why",
                "seed",
            ],
            3,
        ),
    ];
    for (args, want_code) in scenarios {
        let (out, code) = run(&feature, &home, args);
        assert_eq!(code, want_code, "{args:?}: {out}");
        assert!(
            !feature.join(".vivac").join("lane").exists(),
            "{args:?} left a lane file behind"
        );
    }

    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap_or_default();
    assert!(
        log.is_empty(),
        "a refusal wrote to the tree's own log:\n{log}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".vivac").join("config")).unwrap(),
        config_before,
        "a refusal locked the config"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// Finding A: the same `push`, from two sibling worktrees, one door each.
/// Before this fix, the CLI door joined and the MCP door signed `main` and
/// put the node on the founding lane's own stack -- the exact "the log
/// says the work happened on a branch it did not happen on" the tramo
/// opened with, intact behind the door the agent actually writes through.
#[test]
fn mcp_joins_a_worktree_the_same_way_the_cli_does() {
    use std::io::{BufRead, Write};

    let root = unique("mcp-joins-root");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    std::fs::write(root.join("f.txt"), "x").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "first"]);
    let home = unique("mcp-joins-home");
    let (init_out, init_code) = run(&root, &home, &["init"]);
    assert_eq!(init_code, 0, "{init_out}");
    // `t594` branch-fix-1 #2: joining on its own requires the tree to
    // already have a lane declared somewhere, so this stands in for a
    // `setup` that ran before either worktree existed.
    append_raw_line(
        &root,
        r#"{"seq":1,"id":"01SEEDMAINAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.declared","lane":"main","name":"main","repos":[{"path":"."}]}}"#,
    );
    seed_lanes_config(&root);

    let cli_feature = root.join("cli-feature");
    let mcp_feature = root.join("mcp-feature");
    git(&root, &["worktree", "add", "cli-feature"]);
    git(&root, &["worktree", "add", "mcp-feature"]);

    let (out, code) = run(&cli_feature, &home, &["push", "CLI work", "--why", "seed"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        cli_feature.join(".vivac").join("lane").exists(),
        "the CLI door never joined"
    );

    let mut child = std::process::Command::new(BIN)
        .current_dir(&mcp_feature)
        .env("VIVAC_HOME", &home)
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    fn ask(input: &mut impl Write, output: &mut impl BufRead, line: &str) -> String {
        writeln!(input, "{line}").unwrap();
        input.flush().unwrap();
        let mut buf = String::new();
        output.read_line(&mut buf).unwrap();
        buf
    }
    ask(
        &mut input,
        &mut output,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
    );
    writeln!(
        input,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    input.flush().unwrap();
    let reply = ask(
        &mut input,
        &mut output,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"vivac_push","arguments":{"title":"MCP work","why":"seed"}}}"#,
    );
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        !reply.contains("\"isError\":true"),
        "the MCP call itself failed: {reply}"
    );

    assert!(
        mcp_feature.join(".vivac").join("lane").exists(),
        "the MCP door never joined its own worktree:\n{reply}"
    );

    let (root_stack, code) = run(&root, &home, &["stack"]);
    assert_eq!(code, 0, "{root_stack}");
    assert!(
        !root_stack.contains("MCP work"),
        "the MCP write landed on the founding lane's stack:\n{root_stack}"
    );
    assert!(!root_stack.contains("CLI work"), "{root_stack}");

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

// `web` reaches `Registry::open` through the very same path `mcp::serve`
// does (`Project::open`, given a `Whose` per root); `mcp_joins_a_worktree_
// the_same_way_the_cli_does` above is what proves that path itself joins,
// and there is no second implementation under `web` left to prove
// separately -- see `src/web/mod.rs::serve`.

/// Finding D, closed for real in fix-1 round 2: the §6.9 refusal's own
/// remedy is `vivac setup claude-code`, and running it in the very folder
/// §6.9 refuses used to refuse too, citing its own message back. `setup`
/// now mints this folder a lane of its own instead of declaring `main`
/// again -- `main` genuinely lives elsewhere, and nothing here pretends
/// otherwise -- so an ordinary write from here works afterwards, signed
/// with the lane `setup` just minted rather than `main`.
#[test]
fn setup_fixes_a_folder_whose_main_was_claimed_instead_of_refusing() {
    let c = Sandbox::new_seeded("claimed-setup-fixes-it");
    c.append_raw_line(
        r#"{"seq":1,"id":"01SEEDCLAIMAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.claimed","lane":"main"}}"#,
    );

    setup_ok(&c.0, c.global_home());
    assert!(
        c.0.join(".vivac").join("lane").exists(),
        "setup declared main again instead of minting this folder a lane"
    );
    let minted_id = lane_id_of(&c.0);
    assert_ne!(minted_id, "main");

    let (out, code) = c.run(&["push", "After setup", "--why", "seed"]);
    assert_eq!(code, 0, "{out}");
    let log = log_text(&c);
    let last = log.lines().last().expect("push wrote a line");
    assert!(
        last.contains(&format!("\"lane\":\"{minted_id}\"")),
        "the write after setup did not sign the lane setup just minted:\n{last}"
    );
}

/// Finding C: `import` requires an empty tree of *nodes* (`is_empty_tree`),
/// which a log holding only a context event -- `session.started`,
/// `lane.declared` -- already satisfies. Numbering from zero handed the
/// first imported event a `seq` another one already had.
#[test]
fn import_continues_the_logs_seq_rather_than_assuming_it_is_empty() {
    let c = Sandbox::new_seeded("import-continues-seq");
    c.append_raw_line(
        r#"{"seq":1,"id":"01SEEDSESSIONAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"session.started","source":"test"}}"#,
    );
    let tree_json = c.0.join("tree.json");
    std::fs::write(
        &tree_json,
        r#"{"nodes":{"1":{"id":1,"title":"Imported","kind":"goal","status":"active"}}}"#,
    )
    .unwrap();

    let out = c.ok(&["import", tree_json.to_str().unwrap()]);
    let _ = out;

    let log = log_text(&c);
    let seqs: Vec<u64> = log
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["seq"].as_u64().expect("every line names a seq")
        })
        .collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seqs.len(),
        "the log carries a duplicate seq: {seqs:?}"
    );
    assert_eq!(seqs, vec![1, 2], "{log}");

    let (check_out, check_code) = c.run(&["check"]);
    assert_eq!(check_code, 0, "{check_out}");
}

/// The other half of finding C: `import` used to sign every event `main`
/// no matter which lane the context actually was.
#[test]
fn import_signs_the_contexts_own_lane_not_always_main() {
    let c = Sandbox::new_seeded("import-signs-its-own-lane");
    let second = c.0.join("v2");
    std::fs::create_dir_all(&second).unwrap();
    setup_ok(&second, c.global_home());

    let tree_json = second.join("tree.json");
    std::fs::write(
        &tree_json,
        r#"{"nodes":{"1":{"id":1,"title":"Imported from v2","kind":"goal","status":"active"}}}"#,
    )
    .unwrap();

    let (out, code) = run(
        &second,
        c.global_home(),
        &["import", tree_json.to_str().unwrap()],
    );
    assert_eq!(code, 0, "{out}");

    let log = log_text(&c);
    let last = log.lines().last().expect("import wrote a line");
    assert!(
        !last.contains("\"lane\":\"main\""),
        "the imported node signed main instead of v2's own lane:\n{last}"
    );
}

// ---------------------------------------------------------------------------
// `t594` branch-fix-1, round with the final review of the whole branch.
// ---------------------------------------------------------------------------

/// Finding 2, the viga itself: a hook, not a person, writing
/// `session.started` inside a worktree over a tree nobody ran `setup`
/// on. Before this fix it joined anyway -- `.claude/settings.json` is
/// usually versioned, so a worktree the harness throws away in an hour
/// could convert a whole tree with no human asking for it. Now it does
/// not: the log stays at 0 bytes and the config comes back byte for
/// byte, the same as any other read from an unjoined worktree.
#[test]
fn a_hook_inside_a_worktree_does_not_join_a_tree_that_never_had_setup() {
    let (root, feature, home) = worktree_inside_fixture("hook-no-setup");
    let config_before = std::fs::read_to_string(root.join(".vivac").join("config")).unwrap();
    let log_before =
        std::fs::read_to_string(root.join(".vivac").join("events")).unwrap_or_default();
    assert!(
        log_before.is_empty(),
        "a freshly init'd tree already has events:\n{log_before}"
    );

    let (out, code) = run_stdin(
        &feature,
        &home,
        &["session", "start", "--hook"],
        r#"{"session_id":"s1","source":"startup"}"#,
    );
    assert_eq!(code, 0, "{out}");

    assert!(
        !feature.join(".vivac").join("lane").exists(),
        "the hook joined a tree nobody ran setup on"
    );
    // The viga does not say "writes nothing": `session.started` is
    // recorded on every open, joined or not, the same as it was before
    // this lane ever existed (`t594` task 4's own fallback through the
    // main copy). It says the tree changes by exactly what it always
    // changed by -- one `session.started`, signed `main` -- and not one
    // byte more: no lane file, no `lane.declared`, no config lock.
    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "the hook wrote something other than exactly one event:\n{log}"
    );
    let event: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(event["payload"]["type"], "session.started", "{log}");
    assert_eq!(event["lane"], "main", "{log}");
    assert_eq!(
        std::fs::read_to_string(root.join(".vivac").join("config")).unwrap(),
        config_before,
        "the hook locked the config"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// The other half of finding 2: once the tree already has a lane
/// declared somewhere -- a real `setup`, here -- the same hook joins the
/// worktree exactly as it always has.
#[test]
fn a_hook_inside_a_worktree_still_joins_once_the_tree_has_lanes() {
    let (root, feature, home) = worktree_inside_fixture("hook-with-setup");
    setup_ok(&root, &home);

    let (out, code) = run_stdin(
        &feature,
        &home,
        &["session", "start", "--hook"],
        r#"{"session_id":"s1","source":"startup"}"#,
    );
    assert_eq!(code, 0, "{out}");

    assert!(
        feature.join(".vivac").join("lane").exists(),
        "the hook stopped joining once the tree already had lanes"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// Finding 3: the `SessionStart` hook and the first `push` an MCP client
/// sends both resolve the same worktree as pending before either has
/// written, then both race for the lock. Only one lane may ever exist for
/// this folder: whichever loses the race has to adopt the winner's id
/// under the lock, not mint a second one that leaves the file naming an
/// id nobody's stack, focus or counters actually sit under.
#[test]
fn two_writers_racing_to_join_a_worktree_mint_exactly_one_lane() {
    let (root, feature, home) = worktree_inside_fixture("race-one-lane");
    append_raw_line(
        &root,
        r#"{"seq":1,"id":"01SEEDMAINAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.declared","lane":"main","name":"main","repos":[{"path":"."}]}}"#,
    );
    seed_lanes_config(&root);

    let feature_a = feature.clone();
    let home_a = home.clone();
    let a = std::thread::spawn(move || {
        run_stdin(
            &feature_a,
            &home_a,
            &["session", "start", "--hook"],
            r#"{"session_id":"a","source":"startup"}"#,
        )
    });
    let (out_b, code_b) = run(&feature, &home, &["push", "From B", "--why", "seed"]);
    let (out_a, code_a) = a.join().unwrap();
    assert_eq!(code_a, 0, "{out_a}");
    assert_eq!(code_b, 0, "{out_b}");

    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    let declared = log
        .lines()
        .filter(|l| l.contains("\"type\":\"lane.declared\"") && !l.contains("\"lane\":\"main\""))
        .count();
    assert_eq!(
        declared, 1,
        "more than one lane got declared for the same worktree:\n{log}"
    );
    let lane_id = lane_id_of(&feature);
    assert!(
        log.contains(&lane_id),
        "the file's own id never reached the log:\n{log}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// Finding 7: `name_for` ignores the folder name it is handed, so a test
/// that only calls it proves nothing about the redaction guard. This one
/// joins a worktree literally named a credential, through the real
/// auto-join path, and reads the real log: neither the name whole nor any
/// fragment of it may appear anywhere in it.
#[test]
fn a_worktree_named_a_secret_never_writes_it_to_the_log() {
    let secret = "ghp_16C7e42F292c6912E7710c838347Ae178B4a";
    let root = unique("redacted-worktree-root");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    std::fs::write(root.join("f.txt"), "x").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "first"]);
    let home = unique("redacted-worktree-home");
    let (init_out, init_code) = run(&root, &home, &["init"]);
    assert_eq!(init_code, 0, "{init_out}");
    append_raw_line(
        &root,
        r#"{"seq":1,"id":"01SEEDMAINAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"lane.declared","lane":"main","name":"main","repos":[{"path":"."}]}}"#,
    );
    seed_lanes_config(&root);

    let feature = root.join(secret);
    git(&root, &["worktree", "add", secret]);

    let (out, code) = run(&feature, &home, &["add", "Feature work", "--why", "seed"]);
    assert_eq!(code, 0, "{out}");

    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    assert!(
        !log.contains(secret),
        "the secret folder name reached the log whole:\n{log}"
    );
    for fragment in [&secret[..12], &secret[12..24], &secret[24..]] {
        assert!(
            !log.contains(fragment),
            "a fragment of the secret name reached the log ({fragment}):\n{log}"
        );
    }
    assert!(
        log.contains("\"name\":\"lane-"),
        "the redacted fallback name never reached the log:\n{log}"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}
