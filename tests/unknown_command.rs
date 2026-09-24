//! `d772`: a command the CLI does not dispatch is refused before anything
//! else runs -- no flag check, no positional check, no tree lookup -- with
//! a message that says so and, when there is one, a pointer at what the
//! caller probably meant.

mod common;
use common::Sandbox;

#[test]
fn a_table_word_points_at_the_real_command_it_stands_for() {
    let c = Sandbox::new_seeded("unknown-show");
    let (s, code) = c.run(&["show", "g564"]);
    assert_eq!(code, 2, "{s}");
    assert_eq!(
        s,
        "\n  \"show\" is not a vivac command.\n  \
         To read a node and why it exists:  vivac why <id>\n  \
         Every command:  vivac --help\n\n"
    );
    assert!(
        !s.contains("does not take"),
        "show must not be treated as if it existed:\n{s}"
    );
}

#[test]
fn a_bare_unknown_command_gives_the_same_three_lines_and_no_full_usage_dump() {
    let c = Sandbox::new_seeded("unknown-show-bare");
    let (s, code) = c.run(&["show"]);
    assert_eq!(code, 2, "{s}");
    assert_eq!(
        s,
        "\n  \"show\" is not a vivac command.\n  \
         To read a node and why it exists:  vivac why <id>\n  \
         Every command:  vivac --help\n\n"
    );
    assert!(
        !s.contains("Getting started"),
        "a bare unknown command must not dump the whole of USAGE:\n{s}"
    );
}

#[test]
fn an_unknown_command_close_to_a_real_one_names_it() {
    let c = Sandbox::new_seeded("unknown-brif");
    let (s, code) = c.run(&["brif"]);
    assert_eq!(code, 2, "{s}");
    assert!(s.contains("Closest:  vivac brief"), "{s}");
}

#[test]
fn an_unknown_command_with_no_close_match_has_no_hint_line() {
    let c = Sandbox::new_seeded("unknown-zzzzzz");
    let (s, code) = c.run(&["zzzzzz"]);
    assert_eq!(code, 2, "{s}");
    assert_eq!(
        s,
        "\n  \"zzzzzz\" is not a vivac command.\n  Every command:  vivac --help\n\n"
    );
}

#[test]
fn an_unknown_command_with_flags_gets_the_same_message_not_the_flags_one() {
    let c = Sandbox::new_seeded("unknown-show-json");
    let (s, code) = c.run(&["show", "--json"]);
    assert_eq!(code, 2, "{s}");
    assert!(s.contains("\"show\" is not a vivac command."), "{s}");
    assert!(!s.contains("does not take --json"), "{s}");
}

/// The tombstone from `d557` still answers first, unaffected: `hooks` is
/// deliberately not one of the commands the new check knows about, and it
/// must not fall through to "is not a vivac command" either.
#[test]
fn hooks_keeps_its_own_tombstone() {
    let c = Sandbox::new_seeded("unknown-hooks");
    let (s, code) = c.run(&["hooks"]);
    assert_ne!(code, 0, "{s}");
    assert!(
        s.contains("vivac hooks is gone: vivac setup claude-code writes the hooks itself,"),
        "{s}"
    );
    assert!(!s.contains("is not a vivac command"), "{s}");
}

/// A real command is untouched by this change: one word too many still
/// gives its old refusal, not the new one.
#[test]
fn a_real_command_with_an_extra_positional_keeps_its_old_refusal() {
    let c = Sandbox::new_seeded("unknown-real-extra");
    let (s, code) = c.run(&["tree", "1", "extra"]);
    assert_eq!(code, 2, "{s}");
    assert!(s.contains("tree does not take \"extra\""), "{s}");
    assert!(!s.contains("is not a vivac command"), "{s}");
}
