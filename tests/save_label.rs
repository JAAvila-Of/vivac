//! `d936`: `save` with no label composes one from what the lane did since its
//! own last stop made by hand, so the stop says what it held without the
//! agent having to write it.

mod common;
use common::Sandbox;

fn add(c: &Sandbox, title: &str, kind: &str) {
    c.ok(&["add", title, "--why", "it is needed", "--type", kind]);
}

/// The label line `save` printed, without the `v<N>` column.
fn label_of(save_output: &str) -> String {
    let first = save_output.lines().next().expect(save_output);
    let rest = first.trim_start().trim_start_matches('v');
    rest.trim_start_matches(|digit: char| digit.is_ascii_digit())
        .trim()
        .to_string()
}

/// A push, an add, a done and a note: the exact label, in `save`'s answer
/// and again in `vivacs`.
#[test]
fn a_bare_save_names_what_was_opened_closed_and_noted() {
    let c = Sandbox::new_seeded("label-composed");
    c.ok(&["push", "Ship it", "--why", "it is needed"]);
    add(&c, "Found a thing", "finding");
    c.ok(&["note", "1", "a note"]);
    c.ok(&["done", "1"]);

    let out = c.ok(&["save"]);
    let label = "2 new (g1, f2), 1 closed (g1), 1 note";
    assert_eq!(label_of(&out), label, "{out}");

    let vivacs = c.ok(&["vivacs"]);
    assert!(
        vivacs.lines().any(|l| l.trim() == label),
        "the label is not in vivacs:\n{vivacs}"
    );
}

/// Several notes read as a count, in the plural.
#[test]
fn several_notes_read_as_a_plural_count() {
    let c = Sandbox::new_seeded("label-notes");
    c.ok(&["push", "Ship it", "--why", "it is needed"]);
    for n in ["one", "two", "three"] {
        c.ok(&["note", "1", n]);
    }
    let out = c.ok(&["save"]);
    assert_eq!(label_of(&out), "1 new (g1), 3 notes", "{out}");
}

/// A label the caller gives is kept word for word.
#[test]
fn a_given_label_is_kept_verbatim() {
    let c = Sandbox::new_seeded("label-given");
    c.ok(&["push", "Ship it", "--why", "it is needed"]);
    let out = c.ok(&["save", "before the migration"]);
    assert_eq!(label_of(&out), "before the migration", "{out}");
    let vivacs = c.ok(&["vivacs"]);
    assert!(vivacs.contains("before the migration"), "{vivacs}");
    assert!(!vivacs.contains("1 new"), "{vivacs}");
}

/// Nothing since the last stop made by hand: there is nothing to say, and
/// the answer says so the way it always has.
#[test]
fn nothing_since_the_last_manual_stop_leaves_no_label() {
    let c = Sandbox::new_seeded("label-nothing");
    let first = c.ok(&["save"]);
    assert_eq!(label_of(&first), "no label", "{first}");
    c.ok(&["push", "Ship it", "--why", "it is needed"]);
    c.ok(&["save", "a stop"]);
    let second = c.ok(&["save"]);
    assert_eq!(label_of(&second), "no label", "{second}");
    assert!(c.ok(&["vivacs"]).contains("v4"), "the stop was written");
}

/// More than three of a kind: the full count, and the newest three named
/// under `latest`.
#[test]
fn more_than_three_closed_names_the_latest_three_with_the_full_count() {
    let c = Sandbox::new_seeded("label-latest");
    for i in 1..=5 {
        add(&c, &format!("task {i}"), "task");
    }
    for i in 1..=5 {
        c.ok(&["done", &i.to_string()]);
    }
    let out = c.ok(&["save"]);
    assert_eq!(
        label_of(&out),
        "5 new (latest t3, t4, t5), 5 closed (latest t3, t4, t5)",
        "{out}"
    );
}

/// A node closed twice appears once, at its newest position.
#[test]
fn a_node_closed_twice_is_named_once_at_its_newest_position() {
    let c = Sandbox::new_seeded("label-twice");
    add(&c, "first", "task");
    add(&c, "second", "task");
    c.ok(&["done", "1"]);
    c.ok(&["done", "2"]);
    c.ok(&["focus", "1", "--reopen"]);
    c.ok(&["done", "1"]);
    let out = c.ok(&["save"]);
    assert_eq!(
        label_of(&out),
        "2 new (t1, t2), 3 closed (latest t2, t1)",
        "{out}"
    );
}

/// The stretch runs from the last stop made by hand, not from the last stop
/// of any kind: the hook's automatic stop in between does not cut it.
#[test]
fn an_automatic_stop_in_between_does_not_cut_the_stretch() {
    let c = Sandbox::new_seeded("label-auto");
    c.ok(&["save", "start"]);
    c.ok(&["push", "Ship it", "--why", "it is needed"]);
    add(&c, "first", "task");
    let (out, code) = c.run_stdin(&["session", "end", "--hook"], r#"{"session_id":"s1"}"#);
    assert_eq!(code, 0, "{out}");
    assert!(
        c.ok(&["vivacs"]).contains("auto"),
        "the hook wrote no stop, so this proves nothing:\n{}",
        c.ok(&["vivacs"])
    );
    add(&c, "second", "task");
    let out = c.ok(&["save"]);
    assert_eq!(label_of(&out), "3 new (g1, t2, t3)", "{out}");
}

/// The brief's last stop shows the composed label.
#[test]
fn the_brief_shows_the_stretch_label_on_the_last_stop() {
    let c = Sandbox::new_seeded("label-brief");
    c.ok(&["push", "Ship it", "--why", "it is needed"]);
    add(&c, "a finding", "finding");
    c.ok(&["save"]);
    let brief = c.ok(&["brief"]);
    let last = brief
        .split("LAST VIVAC")
        .nth(1)
        .unwrap_or_else(|| panic!("{brief}"));
    assert!(last.contains("\"2 new (g1, f2)\""), "{brief}");
}
