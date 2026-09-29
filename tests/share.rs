//! `vivac share` (`d916`): a project decides whether the other projects on
//! this machine can find what it knows. The mark lives in the tree's own
//! config, and it only ever binds a read that comes from another project:
//! from inside, everything reads as it always did.

mod common;
use common::Sandbox;

const WORD: &str = "quokka";

/// What only that node's title says: no refusal and no count line has it.
const TITLE: &str = "habitat";

fn project_name(c: &Sandbox) -> String {
    c.0.file_name().unwrap().to_string_lossy().into_owned()
}

fn config(c: &Sandbox) -> String {
    std::fs::read_to_string(c.0.join(".vivac").join("config")).unwrap()
}

/// Two trees in one `VIVAC_HOME`, both registered, with `WORD` only in B.
/// Registration is a side effect of using a project, and the `stack` after
/// the push is the first command that finds a first event to key it by.
fn pair(name: &str) -> (Sandbox, Sandbox) {
    let a = Sandbox::new_seeded(&format!("{name}-a"));
    a.ok(&["push", "Ship the release", "--why", "the tag is cut"]);
    a.ok(&["stack"]);
    let b = Sandbox::new_seeded_in(&format!("{name}-b"), a.global_home());
    b.ok(&[
        "push",
        &format!("Guard the {WORD} habitat"),
        "--why",
        "nobody else looks after it",
    ]);
    b.ok(&["stack"]);
    (a, b)
}

#[test]
fn by_default_another_project_finds_what_a_tree_knows() {
    let (a, b) = pair("share-default");

    let s = a.ok(&["find", WORD, "--everywhere"]);
    assert!(s.contains(WORD), "{s}");
    assert!(s.contains(&project_name(&b)), "{s}");
    assert!(!s.contains("to itself"), "{s}");
    let j = a.ok(&["find", WORD, "--everywhere", "--json"]);
    assert!(j.contains(WORD), "{j}");
    let w = a.ok(&["why", "1", "--project", &project_name(&b)]);
    assert!(w.contains(WORD), "{w}");
}

#[test]
fn a_closed_project_is_skipped_by_every_read_from_another_one() {
    let (a, b) = pair("share-closed");
    b.ok(&["share", "off"]);

    let s = a.ok(&["find", WORD, "--everywhere"]);
    assert!(!s.contains(TITLE), "{s}");
    assert!(
        s.contains("1 project keeps what it knows to itself."),
        "{s}"
    );

    let j = a.ok(&["find", WORD, "--everywhere", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&j).unwrap();
    assert_eq!(v, serde_json::json!([]), "{j}");

    let (w, code) = a.run(&["why", "1", "--project", &project_name(&b)]);
    assert_eq!(code, 1, "{w}");
    assert_eq!(
        w.trim(),
        format!(
            "{} keeps what it knows to itself: other projects cannot read it.",
            project_name(&b)
        ),
        "{w}"
    );
    assert!(!w.contains(TITLE), "{w}");
}

#[test]
fn a_closed_project_still_reads_as_itself() {
    let (_a, b) = pair("share-inside");
    b.ok(&["share", "off"]);

    let s = b.ok(&["find", WORD, "--everywhere"]);
    assert!(s.contains(WORD), "{s}");
    assert!(!s.contains("to itself"), "{s}");
    let w = b.ok(&["why", "1", "--project", &project_name(&b)]);
    assert!(w.contains(WORD), "{w}");
}

#[test]
fn a_directory_with_no_tree_is_another_project_to_every_tree() {
    let (a, b) = pair("share-outside");
    b.ok(&["share", "off"]);
    let outside = Sandbox::new_empty_in("share-outside-none", a.global_home());

    let s = outside.ok(&["find", WORD, "--everywhere"]);
    assert!(!s.contains(TITLE), "{s}");
    assert!(
        s.contains("1 project keeps what it knows to itself."),
        "{s}"
    );
}

#[test]
fn the_count_is_plural_when_several_projects_keep_to_themselves() {
    let (a, b) = pair("share-plural");
    let c = Sandbox::new_seeded_in("share-plural-c", a.global_home());
    c.ok(&["push", "Feed the quokka", "--why", "it is hungry"]);
    c.ok(&["stack"]);
    b.ok(&["share", "off"]);
    c.ok(&["share", "off"]);

    let s = a.ok(&["find", "nothing-matches-this", "--everywhere"]);
    assert!(
        s.contains("2 projects keep what they know to themselves."),
        "{s}"
    );
    assert!(s.contains("Nothing matches"), "{s}");
}

#[test]
fn the_mark_is_written_to_the_config_of_the_closed_tree_only() {
    let (a, b) = pair("share-config");
    assert!(!config(&a).contains("share"), "{}", config(&a));
    assert!(!config(&b).contains("share"), "{}", config(&b));
    let before = config(&b);

    b.ok(&["share", "off"]);
    assert!(config(&b).contains("\"share\": false"), "{}", config(&b));
    assert!(!config(&a).contains("share"), "{}", config(&a));
    assert!(!b.log().contains("\"share\""), "the mark went into the log");
    assert_ne!(before, config(&b));
}

#[test]
fn closing_twice_says_already_and_leaves_the_file_alone() {
    let (_a, b) = pair("share-twice");
    let name = project_name(&b);
    let first = b.ok(&["share", "off"]);
    assert_eq!(
        first,
        format!(
            "  {name} now keeps what it knows to itself: other projects on this machine no longer find it.\n  To share it again, from a terminal:  vivac share on\n"
        )
    );
    let closed = config(&b);

    let second = b.ok(&["share", "off"]);
    assert_eq!(
        second,
        format!("  {name} already keeps what it knows to itself.\n")
    );
    assert_eq!(closed, config(&b));
}

#[test]
fn reopening_needs_a_terminal_and_writes_nothing_without_one() {
    let (_a, b) = pair("share-on-none");
    b.ok(&["share", "off"]);
    let closed = config(&b);

    let (s, code) = b.run(&["share", "on"]);
    assert_eq!(code, 1, "{s}");
    assert_eq!(
        s.trim(),
        "Sharing again needs a person at a terminal, and there is none here.\n  An agent cannot reopen a project that was closed: type  vivac share on  in a terminal."
    );
    assert_eq!(closed, config(&b));
}

#[test]
fn share_on_takes_no_yes_flag() {
    let (_a, b) = pair("share-on-yes");
    b.ok(&["share", "off"]);
    let closed = config(&b);

    let (s, code) = b.run(&["share", "on", "--yes"]);
    assert_ne!(code, 0, "{s}");
    assert!(s.contains("share does not take --yes."), "{s}");
    assert_eq!(closed, config(&b));
}

#[test]
fn share_on_over_an_open_tree_says_so_and_writes_nothing() {
    let (_a, b) = pair("share-on-open");
    let before = config(&b);

    let s = b.ok(&["share", "on"]);
    assert_eq!(
        s,
        format!("  {} already shares what it knows.\n", project_name(&b))
    );
    assert_eq!(before, config(&b));
}

#[test]
fn share_with_no_word_says_the_state_either_way() {
    let (_a, b) = pair("share-state");
    let name = project_name(&b);

    let open = b.ok(&["share"]);
    assert_eq!(
        open,
        format!(
            "  {name} shares what it knows with the other projects on this machine.\n  To keep it to itself:  vivac share off\n"
        )
    );

    b.ok(&["share", "off"]);
    let closed = b.ok(&["share"]);
    assert_eq!(
        closed,
        format!(
            "  {name} keeps what it knows to itself.\n  To share it again, from a terminal:  vivac share on\n"
        )
    );
}

#[test]
fn share_with_any_other_word_is_a_usage_error() {
    let (_a, b) = pair("share-usage");
    let (s, code) = b.run(&["share", "maybe"]);
    assert_eq!(code, 2, "{s}");
    assert!(s.contains("usage: vivac share [on|off]"), "{s}");
}

#[test]
fn share_with_no_tree_fails_like_every_other_command() {
    let c = Sandbox::new_empty("share-no-tree");
    let (s, code) = c.run(&["share"]);
    let (brief, brief_code) = c.run(&["stack"]);
    assert_eq!(code, brief_code, "{s}");
    assert_eq!(s, brief);
}

#[test]
fn share_is_announced_in_the_help() {
    let c = Sandbox::new_empty("share-help");
    let help = c.ok(&["--help"]);
    assert!(help.contains("vivac share [on|off]"), "{help}");
    assert!(help.contains("whether other projects can find\n"), "{help}");
    assert!(help.contains("what this one knows\n"), "{help}");
}
