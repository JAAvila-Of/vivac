//! `f556`: a flag that never takes a value must never eat the word after it.
//!
//! `Args::parse` used to grab the next bare word for any flag at all, switch
//! or not. `push --blocks "title"` took the title as `--blocks`'s own value
//! and left the command with none -- found while drafting the command line
//! for `setup claude-code`, where `--yes` did the same thing to the harness
//! name.

mod common;
use common::Sandbox;

/// `push --blocks "the title" --why w` has to leave the title as the title,
/// not feed it to `--blocks`.
#[test]
fn a_switch_before_the_title_does_not_swallow_it() {
    let c = Sandbox::new_seeded("switch-blocks");
    let out = c.ok(&["push", "--blocks", "the title", "--why", "w"]);
    assert!(
        out.contains("  the title"),
        "the title never made it through:\n{out}"
    );
    assert!(
        out.contains("blocks its parent from closing"),
        "--blocks was not read as set:\n{out}"
    );
}

/// `--blocks=x` has nothing to store a value in, so it is a usage error
/// rather than a value kept in silence.
#[test]
fn equals_on_a_switch_is_a_usage_error() {
    let c = Sandbox::new_empty("switch-equals");
    let (out, code) = c.run(&["push", "--blocks=x"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--blocks"), "{out}");
    assert!(out.contains("does not take a value"), "{out}");
}
