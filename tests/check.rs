//! `check` — the `MODEL.md` §9 invariants, plus `--gates` (`d350`/`d351`).
//!
//! `STORE` and `PROJECT` are about one tree. `GATES` is about every tree
//! this machine's registry knows: the store is fine and nothing is
//! delivering it, measured off each project's own log. A project reports
//! here when every node it holds was written before the first session was
//! ever opened -- "zero openings, ever" is not the criterion: a real tree
//! had exactly one opening, arriving after all of its work, and that late
//! opening was enough to clear a "never opened" filter while the hook still
//! was not wired up for the months of work that came before it.

mod common;
use common::Sandbox;

fn project_name(c: &Sandbox) -> String {
    c.0.file_name().unwrap().to_string_lossy().into_owned()
}

/// Writes a node and registers the project, without ever opening a session.
fn seed_no_session(c: &Sandbox, title: &str, why: &str) {
    c.ok(&["push", title, "--why", why]);
    c.ok(&["stack"]);
}

/// Writes a node, then opens a session the way the hook does: the opening
/// arrives after the only node this project holds.
fn seed_then_open_session(c: &Sandbox, title: &str, why: &str) {
    c.ok(&["push", title, "--why", why]);
    c.ok(&["session", "start", "--hook"]);
}

/// Opens a session first, then writes a node: at least one node came after
/// an opening, which is the one shape `--gates` must not report.
fn open_session_then_seed(c: &Sandbox, title: &str, why: &str) {
    c.ok(&["session", "start", "--hook"]);
    c.ok(&["push", title, "--why", why]);
}

/// Registers the project with no node ever written: the only event a store
/// can hold with nothing on the stack is `session.started` itself.
fn note_project_with_no_nodes(c: &Sandbox) {
    c.ok(&["session", "start", "--hook"]);
}

#[test]
fn a_project_with_nodes_and_no_session_appears_in_gates() {
    let c = Sandbox::new_seeded("gates-reports");
    seed_no_session(&c, "Ship the release", "the tag is cut");
    let name = project_name(&c);

    let (s, code) = c.run(&["check", "--gates"]);

    assert_eq!(code, 1, "{s}");
    assert!(
        s.contains("GATES (1)  <- the store is fine; nothing delivers it"),
        "{s}"
    );
    assert!(
        s.contains(&format!(
            "{name}: 1 nodes written, and not one after a session ever opened"
        )),
        "{s}"
    );
}

/// The case that motivated the correction: a session opened, but only after
/// every node the project holds. All of its work is still before the first
/// opening, so it has to appear -- "zero openings, ever" would have missed
/// exactly this tree.
#[test]
fn a_project_with_a_session_opened_only_after_all_its_nodes_appears() {
    let c = Sandbox::new_seeded("gates-late-open");
    seed_then_open_session(&c, "Ship the release", "the tag is cut");
    let name = project_name(&c);

    let (s, code) = c.run(&["check", "--gates"]);

    assert_eq!(code, 1, "{s}");
    assert!(
        s.contains(&format!(
            "{name}: 1 nodes written, and not one after a session ever opened"
        )),
        "{s}"
    );
}

/// The other side of the same line: a node written **after** a session was
/// already open does not appear, because that node was delivered.
#[test]
fn a_project_with_a_node_written_after_a_session_does_not_appear() {
    let c = Sandbox::new_seeded("gates-early-open");
    open_session_then_seed(&c, "Ship the release", "the tag is cut");
    let name = project_name(&c);

    let s = c.ok(&["check", "--gates"]);

    assert!(
        !s.contains("GATES ("),
        "a project with work after its opening was reported:\n{s}"
    );
    assert!(!s.contains(&name), "{s}");
}

#[test]
fn a_fresh_project_with_no_nodes_does_not_appear() {
    let c = Sandbox::new_seeded("gates-empty");
    note_project_with_no_nodes(&c);
    let name = project_name(&c);

    let s = c.ok(&["check", "--gates"]);

    assert!(
        !s.contains("GATES ("),
        "an empty project was reported:\n{s}"
    );
    assert!(!s.contains(&name), "{s}");
}

/// The line format and the JSON array, against a two-project registry so the
/// count and the closing advice are both exercised.
#[test]
fn gates_json_carries_the_same_lines_as_a_key() {
    let a = Sandbox::new_seeded("gates-json-a");
    seed_no_session(&a, "Ship the release", "the tag is cut");
    let name_a = project_name(&a);
    let b = Sandbox::new_seeded_in("gates-json-b", a.global_home());
    open_session_then_seed(&b, "Guard the release notes", "the version was a hand edit");

    let (s, code) = a.run(&["check", "--gates", "--json"]);
    assert_eq!(code, 1, "{s}");
    let v: serde_json::Value = serde_json::from_str(&s).expect("check --json is not JSON");

    let gates = v["gates"].as_array().expect("gates is not an array");
    assert_eq!(gates.len(), 1, "{s}");
    assert_eq!(
        gates[0].as_str().unwrap(),
        format!("{name_a}: 1 nodes written, and not one after a session ever opened")
    );
    assert_eq!(v["ok"], serde_json::json!(false), "{s}");
}

/// The advice line closes the section, the same way `PROJECT` closes its own.
#[test]
fn gates_prints_the_advice_that_closes_the_section() {
    let c = Sandbox::new_seeded("gates-advice");
    seed_no_session(&c, "Ship the release", "the tag is cut");

    let (s, _) = c.run(&["check", "--gates"]);

    assert!(
        s.contains("A tree nobody opens is a tree nobody reads. Run  vivac hooks  inside"),
        "{s}"
    );
    assert!(s.contains("that project and paste what it prints."), "{s}");
}

/// The regression: without `--gates`, `check` never looks at the registry,
/// never mentions `GATES`, and its `ok`/exit code are exactly what they were
/// before this flag existed -- even with another project sitting in the same
/// registry that `--gates` would report.
#[test]
fn without_the_flag_check_is_unchanged_even_with_a_project_gates_flagged() {
    let a = Sandbox::new_seeded("gates-off-a");
    seed_no_session(&a, "Ship the release", "the tag is cut");
    let b = Sandbox::new_seeded_in("gates-off-b", a.global_home());

    let (text, text_code) = b.run(&["check"]);
    assert_eq!(text_code, 0, "{text}");
    assert!(text.contains("No findings. 0 nodes checked."), "{text}");
    assert!(!text.contains("GATES"), "{text}");

    let (json, json_code) = b.run(&["check", "--json"]);
    assert_eq!(json_code, 0, "{json}");
    let v: serde_json::Value = serde_json::from_str(&json).expect("check --json is not JSON");
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["ok", "project", "store"], "{json}");
}

/// `--gates` names projects by the registry's bare name, never by path: an
/// absolute path names the account and the machine, and the security pillar
/// allows neither into a result.
#[test]
fn gates_output_carries_no_absolute_path() {
    let a = Sandbox::new_seeded("gates-path-a");
    seed_no_session(&a, "Ship the release", "the tag is cut");
    let b = Sandbox::new_seeded_in("gates-path-b", a.global_home());
    seed_no_session(&b, "Guard the release notes", "the version was a hand edit");

    let (s, code) = a.run(&["check", "--gates"]);
    assert_eq!(code, 1, "{s}");

    for root in [&a.0, &b.0] {
        let full = root.to_string_lossy();
        assert!(!s.contains(full.as_ref()), "an absolute path leaked:\n{s}");
    }
}

/// A flag the parser does not know is refused, not ignored, and `--gates` is
/// the one flag `check` gained.
#[test]
fn gates_is_a_known_flag() {
    let c = Sandbox::new_seeded("gates-known");
    let (s, code) = c.run(&["check", "--gates"]);
    assert_eq!(code, 0, "--gates was refused as unknown:\n{s}");
}

// ---------------------------------------------------------------------------
// `t594` §4.9: git findings, on top of the store corruption above.
// ---------------------------------------------------------------------------

fn git(dir: &std::path::Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

#[test]
fn check_says_when_the_log_is_tracked_by_git() {
    let c = Sandbox::new_seeded("tracked");
    c.ok(&["push", "Something", "--why", "so the log is not empty"]);
    git(&c.0, &["init", "-q"]);
    git(&c.0, &["add", "-f", ".vivac/events"]);
    git(&c.0, &["commit", "-q", "-m", "track the log by mistake"]);
    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(".vivac/events is tracked by git here"),
        "{out}"
    );
}

/// `f619`: `check` and `setup` used to warn about a tracked log in two
/// different sentences, and `check`'s did not even name a worktree. The
/// two are compared against each other, not against a literal either
/// one could still drift toward alone.
///
/// `d723` piece B: showing this warning moved from `setup` to `init`, so
/// `init`'s own dry-run is the other side of the comparison now.
#[test]
fn check_and_init_warn_about_vivac_in_git_the_same_way() {
    let c = Sandbox::new_seeded("tracked-check-side");
    c.ok(&["push", "Something", "--why", "so the log is not empty"]);
    git(&c.0, &["init", "-q"]);
    git(&c.0, &["add", "-f", ".vivac/events"]);
    git(&c.0, &["commit", "-q", "-m", "track the log by mistake"]);
    let (check_out, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{check_out}");

    let s = Sandbox::new_seeded("tracked-init-side");
    git(&s.0, &["init", "-q"]);
    git(&s.0, &["add", "-f", ".vivac/events"]);
    git(&s.0, &["commit", "-q", "-m", "track the log by mistake"]);
    let (init_out, init_code) = s.run(&["init", "--dry-run"]);
    assert_eq!(init_code, 0, "{init_out}");

    assert_eq!(
        tracked_warning_words(&check_out),
        tracked_warning_words(&init_out),
        "check said:\n{check_out}\n\ninit said:\n{init_out}"
    );
}

/// The tracked-by-git warning, reduced to its words: whitespace collapsed
/// so a hand-wrapped paragraph and a single unwrapped line compare equal
/// when the wording is the same, which is all this test cares about --
/// each surface still lays the words out to its own shape.
fn tracked_warning_words(out: &str) -> String {
    let start = out
        .find(".vivac/events is tracked by git here")
        .unwrap_or_else(|| panic!("no tracked-by-git warning in:\n{out}"));
    let rest = &out[start..];
    let end = rest
        .find("keep it out.")
        .unwrap_or_else(|| panic!("warning does not end where expected:\n{out}"))
        + "keep it out.".len();
    rest[..end].split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn check_says_when_the_gitignore_is_missing_inside_a_repo() {
    let c = Sandbox::new_seeded("no-gitignore");
    git(&c.0, &["init", "-q"]);
    std::fs::remove_file(c.0.join(".vivac").join(".gitignore")).unwrap();
    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(".vivac/.gitignore is missing"), "{out}");
}

#[test]
fn outside_a_repo_check_says_nothing_about_git() {
    let c = Sandbox::new_seeded("no-repo");
    std::fs::remove_file(c.0.join(".vivac").join(".gitignore")).unwrap();
    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 0, "{out}");
}

/// `anchor::tracks` cannot tell "git says no" apart from "git could not be
/// asked" unless the caller lets it fail loudly: a `PATH` with no `git` on
/// it is the easiest way to force that failure without touching the real
/// one.
#[test]
fn check_says_when_it_could_not_ask_git() {
    let c = Sandbox::new_seeded("no-git-on-path");
    c.ok(&["push", "Something", "--why", "so the log is not empty"]);
    git(&c.0, &["init", "-q"]);

    let o = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .env("PATH", c.0.join("no-such-dir"))
        .args(["check"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("git could not tell whether .vivac/events is tracked here"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// `t594` §4.7: another folder on this machine holding the same first event.
// ---------------------------------------------------------------------------

/// A second folder, sharing `original`'s home, whose log starts with the
/// very same first event: not a fixture, an actual copy of the tree, which
/// is what turns into `d201`'s "copy" the moment both reach the registry.
fn a_copy_of(original: &Sandbox, name: &str) -> Sandbox {
    let copy = Sandbox::new_empty_in(name, original.global_home());
    std::fs::create_dir_all(copy.0.join(".vivac")).unwrap();
    std::fs::copy(
        original.0.join(".vivac").join("events"),
        copy.0.join(".vivac").join("events"),
    )
    .unwrap();
    copy
}

/// A directory name nothing else can take, for the one test below that
/// needs a folder with an exact name `Sandbox` cannot mint on its own.
fn temp_dir(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "vivac-check-{name}-{}-{n}-{ts}",
        std::process::id()
    ))
}

/// Runs the binary in a folder that is not a `Sandbox`, for the one test
/// that needs a folder name `Sandbox` cannot produce.
fn run_bin(dir: &std::path::Path, home: &std::path::Path, args: &[&str]) -> (String, i32) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
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

/// The registry only ever holds one `path` per project, so the folder that
/// is not on it -- the one that reached the registry second, not
/// necessarily the copy in reality -- is what learns first, the moment it
/// is sighted.
#[test]
fn check_exits_1_while_a_copy_exists() {
    let original = Sandbox::new_seeded("copy-exists-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = a_copy_of(&original, "copy-exists-copy");

    let (copy_out, copy_code) = copy.run(&["check"]);
    assert_eq!(copy_code, 1, "{copy_out}");
    assert!(copy_out.contains("COPY OF ANOTHER TREE"), "{copy_out}");
}

/// Whitespace normalized to single spaces, newlines included: the copy
/// block's prose is wrapped by width now, not by hand, so a test that
/// cares about the words has to stop caring which line they landed on.
fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// No printed line runs past `NOTICE_WIDTH` (76) plus `check`'s own
/// 6-space indent, except the one line that carries the `--join` command,
/// which is never wrapped no matter how long it gets.
fn assert_no_wrapped_line_too_long(out: &str) {
    for line in out.lines() {
        if line.trim_start().starts_with("vivac init --join") {
            continue;
        }
        assert!(
            line.chars().count() <= 82,
            "a wrapped line ran past 76 columns: {line:?}\nfull output:\n{out}"
        );
    }
}

#[test]
fn check_names_the_other_folder() {
    let original = Sandbox::new_seeded("copy-name-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let name = project_name(&original);
    let copy = a_copy_of(&original, "copy-name-copy");

    let (out, code) = copy.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    let flat = words(&out);
    assert!(
        flat.contains(&format!("the one in folder \"{name}\"")),
        "{out}"
    );
    assert!(flat.contains(&format!("vivac init --join {name}")), "{out}");
    assert_no_wrapped_line_too_long(&out);
}

#[test]
fn check_withholds_a_name_the_guard_rejects() {
    let rejected_name = "someone@example.com";
    // The guard's classification does not depend on which field it is told
    // the text came from -- only the message does -- so a push whose own
    // title is this string goes through the very same check `folder_name`
    // runs on it. An integration test has no direct call into
    // `redact::check_field`, so this is the closest thing to affirming it
    // first: proven here, not assumed.
    let c = Sandbox::new_seeded("copy-redacted-push");
    let (push_out, push_code) = c.run(&[
        "push",
        rejected_name,
        "--why",
        "proving the guard actually rejects this name before trusting the rest",
    ]);
    assert_eq!(
        push_code, 3,
        "the guard must actually reject this name, or the test proves nothing: {push_out}"
    );

    let home = temp_dir("copy-redacted-home");
    let parent = temp_dir("copy-redacted-parent");
    std::fs::create_dir_all(&parent).unwrap();
    let original_dir = parent.join(rejected_name);
    std::fs::create_dir_all(&original_dir).unwrap();
    run_bin(&original_dir, &home, &["init", "--yes"]);
    run_bin(
        &original_dir,
        &home,
        &["push", "a goal", "--why", "so the log has a first event"],
    );

    let copy_dir = temp_dir("copy-redacted-copy");
    std::fs::create_dir_all(copy_dir.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        copy_dir.join(".vivac").join("events"),
    )
    .unwrap();

    let (out, code) = run_bin(&copy_dir, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("COPY OF ANOTHER TREE"), "{out}");
    assert!(
        !out.contains(rejected_name),
        "the rejected name leaked into check's output:\n{out}"
    );
    assert!(out.contains("as one in another folder on this"), "{out}");
    assert!(
        out.contains("vivac init --join <path to that folder>"),
        "{out}"
    );
    assert_no_wrapped_line_too_long(&out);

    std::fs::remove_dir_all(&parent).ok();
    std::fs::remove_dir_all(&copy_dir).ok();
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn deleting_one_copy_clears_the_warning() {
    let original = Sandbox::new_seeded("copy-clears-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = a_copy_of(&original, "copy-clears-copy");

    let (out, code) = copy.run(&["check"]);
    assert_eq!(code, 1, "{out}");

    std::fs::remove_dir_all(&original.0).unwrap();

    let (out2, code2) = copy.run(&["check"]);
    assert_eq!(code2, 0, "{out2}");
    assert!(!out2.contains("COPY OF ANOTHER TREE"), "{out2}");
}

/// The folder not on `path` learns the moment it is sighted; the folder
/// on `path` learns from its own `copies`, which that sighting is what
/// wrote. Neither is favored once both have been used.
#[test]
fn both_folders_warn_once_the_copy_has_been_used() {
    let original = Sandbox::new_seeded("copy-both-warn-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = a_copy_of(&original, "copy-both-warn-copy");

    let (copy_out, copy_code) = copy.run(&["check"]);
    assert_eq!(copy_code, 1, "{copy_out}");
    assert!(copy_out.contains("COPY OF ANOTHER TREE"), "{copy_out}");

    let (original_out, original_code) = original.run(&["check"]);
    assert_eq!(original_code, 1, "{original_out}");
    assert!(
        original_out.contains("COPY OF ANOTHER TREE"),
        "{original_out}"
    );
}

/// The original's own warning comes from its `copies`, re-verified on
/// every read rather than trusted once and kept forever: once the copy is
/// gone, so is the warning, with nobody having to edit the registry.
#[test]
fn deleting_the_copy_clears_the_warning_in_the_original() {
    let original = Sandbox::new_seeded("copy-clears-in-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = a_copy_of(&original, "copy-clears-in-orig-copy");

    // A filesystem copy on its own teaches the registry nothing: the copy
    // has to run a command at least once for `original`'s own `copies` to
    // learn about it.
    let (copy_out, copy_code) = copy.run(&["check"]);
    assert_eq!(copy_code, 1, "{copy_out}");

    let (out, code) = original.run(&["check"]);
    assert_eq!(code, 1, "{out}");

    std::fs::remove_dir_all(&copy.0).unwrap();

    let (out2, code2) = original.run(&["check"]);
    assert_eq!(code2, 0, "{out2}");
    assert!(!out2.contains("COPY OF ANOTHER TREE"), "{out2}");
}

/// A second, independent spelling of `p`'s own folder name -- every ASCII
/// letter's case swapped -- answered rather than assumed, so a caller
/// with nothing to swap skips its own test with a reason instead of
/// silently comparing a path against itself. Windows only: case is what
/// `f612` was actually caught by.
#[cfg(windows)]
fn second_spelling(p: &std::path::Path) -> Option<std::path::PathBuf> {
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
fn second_spelling(_p: &std::path::Path) -> Option<std::path::PathBuf> {
    None
}

/// `f612`, the second time: the folder the registry already knows, visited
/// under a different spelling of its own name, is not a copy of itself.
#[test]
fn the_same_folder_by_two_spellings_is_not_a_copy() {
    let c = Sandbox::new_seeded("Spelling-Folder");
    c.ok(&["push", "a goal", "--why", "so the log has a first event"]);

    let Some(second) = second_spelling(&c.0) else {
        eprintln!(
            "skipped: this platform offers no second spelling of the same folder to check with"
        );
        return;
    };

    let (out, code) = run_bin(&second, c.global_home(), &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("COPY OF ANOTHER TREE"), "{out}");
}

/// A folder name with a space breaks the line that copies and pastes it
/// unless it is quoted -- the one line `t594` task 4 promises a test that
/// runs literally.
#[test]
fn the_join_command_quotes_a_name_with_a_space() {
    let home = temp_dir("copy-space-home");
    let parent = temp_dir("copy-space-parent");
    std::fs::create_dir_all(&parent).unwrap();
    let original_dir = parent.join("My Project");
    std::fs::create_dir_all(&original_dir).unwrap();
    run_bin(&original_dir, &home, &["init", "--yes"]);
    run_bin(
        &original_dir,
        &home,
        &["push", "a goal", "--why", "so the log has a first event"],
    );

    let copy_dir = temp_dir("copy-space-copy");
    std::fs::create_dir_all(copy_dir.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        copy_dir.join(".vivac").join("events"),
    )
    .unwrap();

    let (out, code) = run_bin(&copy_dir, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("vivac init --join \"My Project\""), "{out}");

    std::fs::remove_dir_all(&parent).ok();
    std::fs::remove_dir_all(&copy_dir).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// The surface the agent actually reads: `--json` needs its own key
/// checked, the same way `gates_json_carries_the_same_lines_as_a_key`
/// already does for its own section.
#[test]
fn copy_json_carries_the_other_folder_and_flips_ok() {
    let original = Sandbox::new_seeded("copy-json-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let name = project_name(&original);
    let copy = a_copy_of(&original, "copy-json-copy");

    let (s, code) = copy.run(&["check", "--json"]);
    assert_eq!(code, 1, "{s}");
    let v: serde_json::Value = serde_json::from_str(&s).expect("check --json is not JSON");

    assert_eq!(v["ok"], serde_json::json!(false), "{s}");
    // The key first, its value after: a mutation that renamed or dropped
    // either `copy` or its own `others` must not read the same as one
    // that just left `others` empty or null.
    assert!(
        v.get("copy").is_some(),
        "check --json dropped the copy key: {s}"
    );
    assert!(
        v["copy"].get("others").is_some(),
        "check --json dropped copy's own others key: {s}"
    );
    assert_eq!(v["copy"]["others"], serde_json::json!([name]), "{s}");
}

#[test]
fn copy_json_carries_a_null_other_when_the_guard_withholds_the_name() {
    let rejected_name = "someone@example.com";
    let home = temp_dir("copy-json-redacted-home");
    let parent = temp_dir("copy-json-redacted-parent");
    std::fs::create_dir_all(&parent).unwrap();
    let original_dir = parent.join(rejected_name);
    std::fs::create_dir_all(&original_dir).unwrap();
    run_bin(&original_dir, &home, &["init", "--yes"]);
    run_bin(
        &original_dir,
        &home,
        &["push", "a goal", "--why", "so the log has a first event"],
    );

    let copy_dir = temp_dir("copy-json-redacted-copy");
    std::fs::create_dir_all(copy_dir.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        copy_dir.join(".vivac").join("events"),
    )
    .unwrap();

    let (s, code) = run_bin(&copy_dir, &home, &["check", "--json"]);
    assert_eq!(code, 1, "{s}");
    let v: serde_json::Value = serde_json::from_str(&s).expect("check --json is not JSON");
    assert!(
        v.get("copy").is_some(),
        "check --json dropped the copy key: {s}"
    );
    assert!(
        v["copy"].get("others").is_some(),
        "check --json dropped copy's own others key: {s}"
    );
    assert_eq!(v["copy"]["others"], serde_json::json!([null]), "{s}");

    std::fs::remove_dir_all(&parent).ok();
    std::fs::remove_dir_all(&copy_dir).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// `gates_output_carries_no_absolute_path` is the precedent: the same rule
/// applies to this block.
#[test]
fn copy_output_carries_no_absolute_path() {
    let original = Sandbox::new_seeded("copy-path-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = a_copy_of(&original, "copy-path-copy");

    let (out, code) = copy.run(&["check"]);
    assert_eq!(code, 1, "{out}");

    for root in [&original.0, &copy.0] {
        let full = root.to_string_lossy();
        assert!(
            !out.contains(full.as_ref()),
            "an absolute path leaked:\n{out}"
        );
    }
}

#[test]
fn copy_output_carries_no_absolute_path_when_the_guard_withholds_the_name() {
    let rejected_name = "someone@example.com";
    let home = temp_dir("copy-path-redacted-home");
    let parent = temp_dir("copy-path-redacted-parent");
    std::fs::create_dir_all(&parent).unwrap();
    let original_dir = parent.join(rejected_name);
    std::fs::create_dir_all(&original_dir).unwrap();
    run_bin(&original_dir, &home, &["init", "--yes"]);
    run_bin(
        &original_dir,
        &home,
        &["push", "a goal", "--why", "so the log has a first event"],
    );

    let copy_dir = temp_dir("copy-path-redacted-copy");
    std::fs::create_dir_all(copy_dir.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        copy_dir.join(".vivac").join("events"),
    )
    .unwrap();

    let (out, code) = run_bin(&copy_dir, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    for root in [&original_dir, &copy_dir] {
        let full = root.to_string_lossy();
        assert!(
            !out.contains(full.as_ref()),
            "an absolute path leaked:\n{out}"
        );
    }
    assert_no_wrapped_line_too_long(&out);

    std::fs::remove_dir_all(&parent).ok();
    std::fs::remove_dir_all(&copy_dir).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// The registry's own `projects` object, read directly: some assertions
/// need to see `copies` itself, which no `check` output shows in words.
fn registry_projects(home: &std::path::Path) -> serde_json::Value {
    let text = std::fs::read_to_string(home.join("projects")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["projects"].clone()
}

/// `f612` a third time, and this one is in the registry's own bookkeeping,
/// not only in what `check` prints: entering the same folder under two
/// spellings must not read as two folders sharing one tree, or `copies`
/// grows one entry per spelling anybody ever used, and every command run
/// from the new spelling writes the registry again for nothing.
#[test]
fn two_spellings_leave_one_entry_in_the_registry() {
    let original = Sandbox::new_seeded("Two-Spelling-Orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = a_copy_of(&original, "Two-Spelling-Copy");

    let (first_out, first_code) = copy.run(&["check"]);
    assert_eq!(first_code, 1, "{first_out}");

    let Some(second) = second_spelling(&copy.0) else {
        eprintln!(
            "skipped: this platform offers no second spelling of the same folder to check with"
        );
        return;
    };
    let (out2, code2) = run_bin(&second, copy.global_home(), &["check"]);
    assert_eq!(code2, 1, "{out2}");

    let projects = registry_projects(copy.global_home());
    let entries = projects.as_object().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "two spellings of the same folder left more than one project entry: {projects}"
    );
    let project = entries.values().next().unwrap();
    let copies = project["copies"].as_array().unwrap();
    assert_eq!(
        copies.len(),
        1,
        "two spellings of the same folder left more than one copy entry: {project}"
    );
}

/// `note` prunes `copies` the next time it has anything to write anyway,
/// never on a read: `copy_of` must not move the registry out from under
/// whoever else might be reading it at the same time.
#[test]
fn a_deleted_copy_is_pruned_from_copies_on_the_next_write() {
    let original = Sandbox::new_seeded("copy-prune-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy_a = a_copy_of(&original, "copy-prune-a");

    let (out_a, code_a) = copy_a.run(&["check"]);
    assert_eq!(code_a, 1, "{out_a}");

    std::fs::remove_dir_all(&copy_a.0).unwrap();

    // A brand-new copy, sighted for the first time: this is what actually
    // writes to `original`'s own entry, and pruning rides along on that
    // write. Reading `original` or the now-deleted `copy_a` over and over
    // would never touch the file at all.
    let copy_b = a_copy_of(&original, "copy-prune-b");
    let (out_b, code_b) = copy_b.run(&["check"]);
    assert_eq!(code_b, 1, "{out_b}");

    let projects = registry_projects(original.global_home());
    let project = projects.as_object().unwrap().values().next().unwrap();
    let copies = project["copies"].as_array().unwrap();
    assert_eq!(
        copies.len(),
        1,
        "the deleted copy survived a write that had every reason to prune it: {project}"
    );
}

/// Two live copies: `dup` and `third` each warn from their own, ordinary
/// one-other view, and `original` -- the only folder whose own `copies`
/// holds more than one entry -- warns with both of them named.
#[test]
fn two_live_copies_all_three_folders_warn_and_are_both_named() {
    let original = Sandbox::new_seeded("copy-multi-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy_a = a_copy_of(&original, "copy-multi-a");
    let copy_b = a_copy_of(&original, "copy-multi-b");

    let (a_out, a_code) = copy_a.run(&["check"]);
    assert_eq!(a_code, 1, "{a_out}");
    let (b_out, b_code) = copy_b.run(&["check"]);
    assert_eq!(b_code, 1, "{b_out}");

    let (out, code) = original.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("COPIES OF THIS TREE"), "{out}");
    assert!(out.contains(&project_name(&copy_a)), "{out}");
    assert!(out.contains(&project_name(&copy_b)), "{out}");
    assert_no_wrapped_line_too_long(&out);
}

/// The other half of N4: the original sees both copies, but each copy
/// used to see only the original, so following "delete the other" from
/// either one would leave the third folder standing. Now every one of
/// the three prints the same membership, minus itself.
#[test]
fn two_live_copies_each_folder_sees_the_other_two() {
    let original = Sandbox::new_seeded("copy-see-all-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy_a = a_copy_of(&original, "copy-see-all-a");
    let copy_b = a_copy_of(&original, "copy-see-all-b");

    let (a_out, a_code) = copy_a.run(&["check"]);
    assert_eq!(a_code, 1, "{a_out}");
    let (b_out, b_code) = copy_b.run(&["check"]);
    assert_eq!(b_code, 1, "{b_out}");

    let original_name = project_name(&original);
    let a_name = project_name(&copy_a);
    let b_name = project_name(&copy_b);

    let (original_out, original_code) = original.run(&["check"]);
    assert_eq!(original_code, 1, "{original_out}");
    assert!(original_out.contains(&a_name), "{original_out}");
    assert!(original_out.contains(&b_name), "{original_out}");
    assert!(
        !original_out.contains(&format!("\"{original_name}\"")),
        "{original_out}"
    );

    let (a_out2, a_code2) = copy_a.run(&["check"]);
    assert_eq!(a_code2, 1, "{a_out2}");
    assert!(a_out2.contains(&original_name), "{a_out2}");
    assert!(a_out2.contains(&b_name), "{a_out2}");
    assert!(!a_out2.contains(&format!("\"{a_name}\"")), "{a_out2}");

    let (b_out2, b_code2) = copy_b.run(&["check"]);
    assert_eq!(b_code2, 1, "{b_out2}");
    assert!(b_out2.contains(&original_name), "{b_out2}");
    assert!(b_out2.contains(&a_name), "{b_out2}");
    assert!(!b_out2.contains(&format!("\"{b_name}\"")), "{b_out2}");

    for out in [&original_out, &a_out2, &b_out2] {
        assert_no_wrapped_line_too_long(out);
    }
}

/// One of two copies has a name the guard withholds: the block still
/// names the one it can, and says in words that at least one more holds
/// the same tree under a name it will not write down.
#[test]
fn two_copies_one_name_withheld_says_more_hold_it_too() {
    let home = temp_dir("copy-multi-mixed-home");
    let parent = temp_dir("copy-multi-mixed-parent");
    std::fs::create_dir_all(&parent).unwrap();
    let original_dir = parent.join("Orig");
    std::fs::create_dir_all(&original_dir).unwrap();
    run_bin(&original_dir, &home, &["init", "--yes"]);
    run_bin(
        &original_dir,
        &home,
        &["push", "a goal", "--why", "so the log has a first event"],
    );

    let named_dir = parent.join("Named");
    std::fs::create_dir_all(named_dir.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        named_dir.join(".vivac").join("events"),
    )
    .unwrap();
    let (named_out, named_code) = run_bin(&named_dir, &home, &["check"]);
    assert_eq!(named_code, 1, "{named_out}");

    let rejected_dir = parent.join("someone@example.com");
    std::fs::create_dir_all(rejected_dir.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        rejected_dir.join(".vivac").join("events"),
    )
    .unwrap();
    let (rejected_out, rejected_code) = run_bin(&rejected_dir, &home, &["check"]);
    assert_eq!(rejected_code, 1, "{rejected_out}");

    let (out, code) = run_bin(&original_dir, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("COPIES OF THIS TREE"), "{out}");
    assert!(out.contains("\"Named\""), "{out}");
    assert!(
        words(&out).contains(&words("More hold it too, under names this tool will not")),
        "{out}"
    );
    assert!(!out.contains("someone@example.com"), "{out}");
    assert_no_wrapped_line_too_long(&out);

    std::fs::remove_dir_all(&parent).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// The fifth form: two or more copies, none of them nameable. Distinct
/// from the fourth (which lists whatever names it has before saying more
/// exist) in that this one lists no name at all -- checked here word for
/// word, since a name list that came out empty in this branch would still
/// compile and would still look like output.
#[test]
fn two_copies_both_names_withheld_says_other_folders_with_no_list() {
    let home = temp_dir("copy-multi-all-withheld-home");
    let parent = temp_dir("copy-multi-all-withheld-parent");
    std::fs::create_dir_all(&parent).unwrap();
    let original_dir = parent.join("Orig");
    std::fs::create_dir_all(&original_dir).unwrap();
    run_bin(&original_dir, &home, &["init", "--yes"]);
    run_bin(
        &original_dir,
        &home,
        &["push", "a goal", "--why", "so the log has a first event"],
    );

    let rejected_a = parent.join("someone@example.com");
    std::fs::create_dir_all(rejected_a.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        rejected_a.join(".vivac").join("events"),
    )
    .unwrap();
    let (out_a, code_a) = run_bin(&rejected_a, &home, &["check"]);
    assert_eq!(code_a, 1, "{out_a}");

    let rejected_b = parent.join("another@example.com");
    std::fs::create_dir_all(rejected_b.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        rejected_b.join(".vivac").join("events"),
    )
    .unwrap();
    let (out_b, code_b) = run_bin(&rejected_b, &home, &["check"]);
    assert_eq!(code_b, 1, "{out_b}");

    let (out, code) = run_bin(&original_dir, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    let expected_prose = words(
        "Other folders on this machine hold a tree that starts with the same \
         event as this one, under names this tool will not write down. They \
         are copies of each other, and copies diverge in silence. Keep one, \
         delete the rest, and join the folders you still work in to the one \
         you kept:",
    );
    assert!(out.contains("COPIES OF THIS TREE"), "{out}");
    assert!(words(&out).contains(&expected_prose), "{out}");
    assert!(
        out.contains("vivac init --join <the folder you kept>"),
        "{out}"
    );
    assert!(!out.contains("someone@example.com"), "{out}");
    assert!(!out.contains("another@example.com"), "{out}");
    assert_no_wrapped_line_too_long(&out);

    std::fs::remove_dir_all(&parent).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// `A&B` carries no space, so the old rule -- quote only when there is
/// one -- would have let it straight through: unquoted, `cmd.exe` runs
/// `--join A` and then tries to run `B` as a command of its own.
#[test]
fn the_join_command_quotes_a_name_that_is_not_just_safe_characters() {
    let home = temp_dir("copy-symbol-home");
    let parent = temp_dir("copy-symbol-parent");
    std::fs::create_dir_all(&parent).unwrap();
    let original_dir = parent.join("A&B");
    std::fs::create_dir_all(&original_dir).unwrap();
    run_bin(&original_dir, &home, &["init", "--yes"]);
    run_bin(
        &original_dir,
        &home,
        &["push", "a goal", "--why", "so the log has a first event"],
    );

    let copy_dir = temp_dir("copy-symbol-copy");
    std::fs::create_dir_all(copy_dir.join(".vivac")).unwrap();
    std::fs::copy(
        original_dir.join(".vivac").join("events"),
        copy_dir.join(".vivac").join("events"),
    )
    .unwrap();

    let (out, code) = run_bin(&copy_dir, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("vivac init --join \"A&B\""), "{out}");

    std::fs::remove_dir_all(&parent).ok();
    std::fs::remove_dir_all(&copy_dir).ok();
    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------------
// `f610`/`f604`: the two log corruptions `check` did not name before now.
// ---------------------------------------------------------------------------

/// A hand-merged log, the shape two writers who both thought they were the
/// only one leave behind: two lines, each claiming `seq` 1. `repeated_nums`
/// already names the same shape for `num`; nothing named it for `seq`.
#[test]
fn check_names_a_repeated_seq_and_says_where() {
    let c = Sandbox::new_seeded("check-repeated-seq");
    c.append_raw_line(
        r#"{"seq":1,"id":"01REPEATEDSEQAAAAAAAAAAAAA","ts":"2026-09-18T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"node.created","node":"01REPEATEDSEQBBBBBBBBBBBBB","num":1,"kind":"goal","title":"The first writer's claim on seq 1"}}"#,
    );
    c.append_raw_line(
        r#"{"seq":1,"id":"01REPEATEDSEQCCCCCCCCCCCCC","ts":"2026-09-18T00:00:01Z","actor":"a_test0000000","lane":"main","payload":{"type":"node.created","node":"01REPEATEDSEQDDDDDDDDDDDDD","num":2,"kind":"goal","title":"The second writer's claim on seq 1"}}"#,
    );

    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("seq 1 appears twice, at line 1 and line 2"),
        "{out}"
    );
}

/// A log whose last line has no newline eats whatever is appended
/// behind it. `check` has to name it: it is not recoverable, so the
/// least it can do is stop it being silent.
#[test]
fn check_names_the_event_a_torn_tail_swallowed() {
    let c = Sandbox::new_seeded("check-torn-tail");
    // `f721`: `new_seeded` now plants with a founding lane already on line
    // 1, so the line this test hand-crafts is `seq` 2, not 1, and the torn
    // line behind it is line 3.
    c.append_raw_line(
        r#"{"seq":2,"id":"01TORNTAILAAAAAAAAAAAAAAAA","ts":"2026-09-18T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"node.created","node":"01TORNTAILBBBBBBBBBBBBBBBB","num":1,"kind":"goal","title":"The line that survived"}}"#,
    );
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(c.0.join(".vivac").join("events"))
            .unwrap();
        // No trailing newline, and the object itself is cut off mid-field --
        // exactly what a crash mid-write leaves behind.
        write!(f, "{{\"seq\":3,\"id\":\"chopped").unwrap();
    }

    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(
            "line 3 does not end with a newline: whatever was appended after it was \
             swallowed and cannot be recovered from this log"
        ),
        "{out}"
    );
}

/// The other half of `f604`: a torn tail cannot be repaired, but the next
/// write must not compound it. Before the fix, `append` opens in `append`
/// mode and writes straight behind the torn bytes, gluing its own first
/// line onto them; the merged line fails to parse and that write is gone
/// too, the same way the one behind it already was.
#[test]
fn appending_behind_a_torn_tail_does_not_swallow_the_new_event() {
    let c = Sandbox::new_seeded("check-torn-tail-append");
    c.ok(&["push", "Before the tear", "--why", "seed a node to note"]);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(c.0.join(".vivac").join("events"))
            .unwrap();
        write!(f, "{{\"seq\":99,\"id\":\"chopped").unwrap();
    }

    c.ok(&["note", "g1", "This note must survive the tear behind it"]);

    let raw = c.log();
    let last_line = raw
        .lines()
        .last()
        .expect("the log must hold at least one line");
    let v: serde_json::Value = serde_json::from_str(last_line).expect(
        "the event written behind the torn tail must be its own readable line, not glued onto it",
    );
    assert_eq!(
        v["payload"]["note"], "This note must survive the tear behind it",
        "{raw}"
    );

    let out = c.ok(&["why", "g1"]);
    assert!(
        out.contains("This note must survive the tear behind it"),
        "{out}"
    );
}
