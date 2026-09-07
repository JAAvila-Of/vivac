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
