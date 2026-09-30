//! `f730`, `d935`: why no stop appeared, without reading the code.
//!
//! The close hook prints nothing (`f708`), so an empty stack, nothing new
//! since the last stop and a hook that never ran used to look the same from
//! outside. Each run of the hook now leaves its verdict beside the turn
//! clock, and `session end --dry-run` says what closing would do now and
//! what the hook decided the last time, writing nothing.

mod common;
use common::Sandbox;

const AT_NOON: &str = "2026-09-30T12:00:00Z";
const THREE_MIN_LATER: &str = "2026-09-30T12:03:00Z";

fn hook(c: &Sandbox, now: &str) {
    let (out, code) = c.run_stdin(
        &["session", "end", "--hook", "--now", now],
        r#"{"session_id":"s1"}"#,
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, "", "the close hook spoke:\n{out}");
}

fn dry_run(c: &Sandbox, now: &str) -> String {
    c.ok(&["session", "end", "--dry-run", "--now", now])
}

/// The `N` of the first `v<N>` after `after` in `out`.
fn stop_after(out: &str, after: &str) -> String {
    let rest = &out[out.find(after).expect(out) + after.len()..];
    rest.trim_start_matches('v')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

/// Before the hook ever ran, there is no record of it, and the rehearsal
/// says what closing would do now.
#[test]
fn with_no_run_it_says_there_is_no_record() {
    let c = Sandbox::new_seeded("dry-run-no-record");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let out = dry_run(&c, AT_NOON);
    assert!(out.contains("Now: closing would write v"), "{out}");
    assert!(
        out.contains("No record of the close hook running in this lane"),
        "{out}"
    );
}

/// The rehearsal writes nothing: no stop, no event.
#[test]
fn the_dry_run_writes_nothing() {
    let c = Sandbox::new_seeded("dry-run-writes-nothing");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let before = c.log();
    dry_run(&c, AT_NOON);
    assert_eq!(c.log(), before, "the dry run wrote to the log");
}

/// The case `f730` met on a real tree: the stack was empty.
#[test]
fn an_empty_stack_reads_as_such_now_and_in_the_last_run() {
    let c = Sandbox::new_seeded("dry-run-empty");
    hook(&c, AT_NOON);
    let out = dry_run(&c, THREE_MIN_LATER);
    assert!(
        out.contains("Now: the stack is empty, so closing writes no stop."),
        "{out}"
    );
    assert!(
        out.contains("The close hook last ran here 3 min ago: the stack was empty"),
        "{out}"
    );
}

/// A run that wrote a stop names it, and the next run with nothing new names
/// the same stop as the one nothing changed since.
#[test]
fn a_stop_written_and_then_nothing_new() {
    let c = Sandbox::new_seeded("dry-run-stopped");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    hook(&c, AT_NOON);
    let out = dry_run(&c, AT_NOON);
    assert!(
        out.contains("The close hook last ran here less than a minute ago: it wrote v"),
        "{out}"
    );
    let wrote = stop_after(&out, "it wrote ");
    assert!(
        out.contains(&format!("Now: nothing changed since v{wrote}")),
        "{out}"
    );

    hook(&c, THREE_MIN_LATER);
    let out = dry_run(&c, THREE_MIN_LATER);
    assert!(
        out.contains(&format!(
            "nothing had changed since v{wrote}, so it wrote no stop"
        )),
        "{out}"
    );
}

/// What is kept is what the hook decided: a person closing by hand is told
/// on the spot and leaves no verdict behind.
#[test]
fn a_person_closing_by_hand_leaves_no_verdict() {
    let c = Sandbox::new_seeded("dry-run-by-hand");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let out = c.ok(&["session", "end"]);
    assert!(out.contains("automatic stop at session close"), "{out}");
    let out = dry_run(&c, AT_NOON);
    assert!(out.contains("No record of the close hook"), "{out}");
}

/// A rehearsal goes with `session end` alone, and never with `--hook`.
#[test]
fn the_dry_run_is_refused_anywhere_else() {
    let c = Sandbox::new_seeded("dry-run-refused");
    for args in [
        &["session", "end", "--dry-run", "--hook"][..],
        &["session", "start", "--dry-run"][..],
        &["session", "prompt", "--dry-run"][..],
    ] {
        let (out, code) = c.run(args);
        assert_ne!(code, 0, "{args:?} was accepted:\n{out}");
        assert!(
            out.contains("--dry-run goes with session end"),
            "{args:?}:\n{out}"
        );
    }
}
