//! When a project enters the registry, and when it must not.
//!
//! `d199` says a project enters by being used, so nothing here calls a command
//! whose job is to register: the registration is a side effect of ordinary work
//! and these tests only ever do ordinary work.

mod common;
use common::Sandbox;

/// The keys in `<VIVAC_HOME>/projects`, or nothing at all if the file is not
/// there yet -- which is itself an answer, and the one a fresh home gives.
fn keys(c: &Sandbox) -> Vec<String> {
    let path = c.global_home().join("projects");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let v: serde_json::Value = serde_json::from_str(&text).expect("the registry parses");
    v["projects"]
        .as_object()
        .expect("projects is an object")
        .keys()
        .cloned()
        .collect()
}

/// The `id` of line 1 of the log, which is what `d201` keys the registry by.
fn first_event_id(c: &Sandbox) -> String {
    let log = c.0.join(".vivac").join("events");
    let text = std::fs::read_to_string(&log).expect("the log is there");
    let line = text.lines().next().expect("the log has a first line");
    let v: serde_json::Value = serde_json::from_str(line).expect("the first line parses");
    v["id"].as_str().expect("an event has an id").to_string()
}

/// `f277`. Registration hangs off finding the root, which happens before the
/// command runs, and the key is the id of the project's first event. A tree
/// planted a moment ago has no such event, so the first `push` -- the command
/// that writes it -- could not be the command that registered it, and the
/// project stayed out of the registry until whatever came next. Out of
/// `find --everywhere` too, and quietly, which is the part that matters.
#[test]
fn the_first_write_registers_the_project() {
    let c = Sandbox::new_seeded("reg-first");
    assert!(
        keys(&c).is_empty(),
        "an empty tree has no identity under d201 and must not be given one"
    );
    c.ok(&["push", "a goal", "--why", "it is the first thing"]);
    assert_eq!(
        keys(&c),
        vec![first_event_id(&c)],
        "the command that wrote the first event has to be the one that registers it"
    );
}

/// Before the upward search learned to skip it, a directory under the home and
/// outside any project resolved to the home itself, so the home went into the
/// registry as a project and stayed there. The walk no longer does that, but a
/// registry written while it did is still on disk and still names such a root.
/// Reading the list has to skip it, or the fan-out opens the global store as
/// though it were somebody's work -- which, once that store also holds the tree
/// for what has no project, it would then search.
#[test]
fn the_global_store_never_comes_back_as_a_project() {
    let home = std::env::temp_dir().join(format!(
        "vivac-reg-global-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let plain = Sandbox::new_seeded_in("reg-global-plain", &home);
    let store = Sandbox::new_seeded_in("reg-global-store", &home);
    plain.ok(&["push", "shared word", "--why", "so both trees match"]);
    store.ok(&["push", "shared word", "--why", "so both trees match"]);
    let asker = Sandbox::new_empty_in("reg-global-asker", &home);
    // Both are ordinary projects, and both answer.
    let before = asker.ok(&["find", "shared", "--everywhere"]);
    assert!(
        before.contains("2 projects"),
        "both to begin with: {before}"
    );
    // Now one of them is the global store: it holds the registry, which is the
    // only thing that ever marks one, and nothing else writes that file.
    std::fs::write(store.0.join(".vivac").join("projects"), "{}").unwrap();
    let after = asker.ok(&["find", "shared", "--everywhere"]);
    assert!(
        after.contains("1 project") && !after.contains("2 projects"),
        "the global store must not answer as a project: {after}"
    );
}

/// The other half of the same rule: planting a tree is not using it, and a
/// directory with `init` run in it and nothing else is not a project anybody
/// has worked in. It stays out until it has something to say.
#[test]
fn init_alone_registers_nothing() {
    let c = Sandbox::new_seeded("reg-init");
    c.ok(&["brief"]);
    c.ok(&["stack"]);
    assert!(
        keys(&c).is_empty(),
        "reads on an empty tree must not conjure an identity for it"
    );
}

/// Registering twice must not enter the same project twice. The key is stable
/// across the life of the log, so the second command finds its own entry and
/// leaves it alone.
#[test]
fn working_on_it_again_does_not_enter_it_twice() {
    let c = Sandbox::new_seeded("reg-twice");
    c.ok(&["push", "a goal", "--why", "it is the first thing"]);
    let after_first = keys(&c);
    c.ok(&["add", "a finding", "--type", "finding", "--why", "measured"]);
    c.ok(&["brief"]);
    assert_eq!(
        after_first,
        keys(&c),
        "one project, one entry, however often"
    );
    assert_eq!(keys(&c).len(), 1);
}

// ---------------------------------------------------------------------------
// `t594` §4.7: the stderr warning every write makes when this folder turns
// out to be a copy. `tests/check.rs` already covers `check`'s own block and
// the registry's `path`/`copies` bookkeeping; these are only the two
// surfaces that file does not reach.
// ---------------------------------------------------------------------------

/// A second folder, sharing `original`'s home, whose log starts with the
/// very same first event: not a fixture, an actual copy of the tree.
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

/// Runs the binary with `stdout` and `stderr` kept apart. `Sandbox::run`
/// merges the two, which is right for most tests here but wrong for the
/// two below: they prove the warning landed on the stream the agent's own
/// parsing does not touch, and merging the streams first would hide
/// exactly the mistake they exist to catch.
fn run_split(c: &Sandbox, args: &[&str]) -> (String, String, i32) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

/// The write is never refused for being a copy (`d201`, §4.7): the registry
/// is a side effect of using a project, not a gate on whether the project
/// can be used. The warning goes to `stderr` and never `stdout`, because
/// the agent's own output on `stdout` gets parsed and this can never land
/// inside it.
#[test]
fn writing_from_a_copy_warns_on_stderr_and_still_writes() {
    let original = Sandbox::new_seeded("reg-write-warn-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = a_copy_of(&original, "reg-write-warn-copy");

    let (stdout, stderr, code) = run_split(
        &copy,
        &["push", "a second thing", "--why", "checking the streams"],
    );
    assert_eq!(
        code, 0,
        "a copy must still be free to write:\n{stdout}{stderr}"
    );
    assert!(
        stderr.contains("COPY OF ANOTHER TREE"),
        "the warning never reached stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("COPY OF ANOTHER TREE"),
        "the warning leaked into stdout, where the agent parses:\n{stdout}"
    );
    assert!(
        copy.log().contains("a second thing"),
        "the event this command was asked to write never reached the log"
    );
}

/// A parent with two open children, both falling in one `abandon --cascade`:
/// one command, several events appended in a single write. The warning is
/// about the process, not the event, so it still comes out exactly once.
fn branch_with_two_children(c: &Sandbox) {
    c.ok(&["push", "a parent", "--why", "root of what falls together"]);
    c.ok(&[
        "add",
        "a child",
        "--parent",
        "1",
        "--why",
        "one of several that fall",
    ]);
    c.ok(&[
        "add",
        "another child",
        "--parent",
        "1",
        "--why",
        "a second one that falls",
    ]);
}

#[test]
fn the_warning_is_printed_once_per_process() {
    let original = Sandbox::new_seeded("reg-once-orig");
    branch_with_two_children(&original);
    let copy = a_copy_of(&original, "reg-once-copy");

    let (_, stderr, code) = run_split(&copy, &["abandon", "1", "--cascade", "letting it go"]);
    assert_eq!(code, 0, "{stderr}");
    let count = stderr.matches("COPY OF ANOTHER TREE").count();
    assert_eq!(
        count, 1,
        "a command that wrote several events warned {count} times:\n{stderr}"
    );
}
