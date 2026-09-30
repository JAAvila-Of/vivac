//! `d937`: closing something says how long it has been since anyone said what
//! comes next. `pop` and `done` add one short block when the lane has made
//! 25 changes or more since its last stop that someone gave an intent to, and
//! only at the first close after each multiple of 25.

mod common;
use common::Sandbox;

const BLOCK_START: &str = "changes since";
const NO_STOP: &str = "no stop has said what comes next";

/// A node to hang notes on. One change.
fn holder(c: &Sandbox) {
    c.ok(&["add", "Keeper", "--why", "it holds the notes"]);
}

/// A node to close later. One change.
fn open(c: &Sandbox, title: &str) {
    c.ok(&["add", title, "--why", "it is needed"]);
}

/// `n` notes on node 1, one change each. None of them may say anything
/// about the next step: only a close does.
fn notes(c: &Sandbox, n: usize) {
    for i in 0..n {
        let out = c.ok(&["note", "1", &format!("note {i}")]);
        assert!(
            !out.contains(BLOCK_START) && !out.contains(NO_STOP),
            "{out}"
        );
    }
}

fn has_block(out: &str) -> bool {
    out.contains(BLOCK_START) || out.contains(NO_STOP)
}

/// The two lines of the block, exactly.
fn block(count: u64, stop: &str) -> String {
    format!(
        "  {count} changes since {stop}, the last stop that said what came next.\n  \
         To leave the next step:  vivac save --next \"<what comes next>\""
    )
}

/// Below the threshold a close says nothing more than it always did.
#[test]
fn a_close_below_25_changes_shows_no_block() {
    let c = Sandbox::new_seeded("ask-below");
    c.ok(&["save", "--next", "carry on"]);
    holder(&c);
    open(&c, "Short job");
    notes(&c, 5);
    let out = c.ok(&["done", "2", "finished"]);
    assert!(!has_block(&out), "{out}");
}

/// The close that takes the count across 25 shows it, exactly, naming the
/// spoken stop. The close counts itself: 1 keeper, 1 node, 21 notes, 1 node
/// and the close make 25.
#[test]
fn the_close_that_crosses_25_shows_the_block() {
    let c = Sandbox::new_seeded("ask-cross");
    c.ok(&["save", "--next", "carry on"]);
    holder(&c);
    open(&c, "Job");
    notes(&c, 21);
    open(&c, "Other job");
    let out = c.ok(&["done", "2", "finished"]);
    assert!(out.contains(&block(25, "v1")), "{out}");
    assert!(out.trim_end().ends_with("<what comes next>\""), "{out}");
}

/// Once per 25 changes: the next close before 50 is silent, the one that
/// crosses 50 shows the block again, with the new count.
#[test]
fn the_block_shows_once_per_25_changes() {
    let c = Sandbox::new_seeded("ask-once");
    c.ok(&["save", "--next", "carry on"]);
    holder(&c);
    open(&c, "Job");
    notes(&c, 21);
    open(&c, "Other job");
    let first = c.ok(&["done", "2", "finished"]);
    assert!(first.contains(&block(25, "v1")), "{first}");

    // 25 so far: two more nodes and two closes make 29, still below 50.
    open(&c, "Third");
    open(&c, "Fourth");
    let quiet = c.ok(&["done", "3", "finished"]);
    assert!(!has_block(&quiet), "{quiet}");
    let also_quiet = c.ok(&["done", "4", "finished"]);
    assert!(!has_block(&also_quiet), "{also_quiet}");

    // 29 now. 19 notes make 48, one node 49, its close 50.
    notes(&c, 19);
    open(&c, "Fifth");
    let again = c.ok(&["done", "5", "finished"]);
    assert!(again.contains(&block(50, "v1")), "{again}");
}

/// Crossing 25 through notes alone says nothing then; the next close does.
#[test]
fn a_crossing_made_by_notes_shows_at_the_next_close() {
    let c = Sandbox::new_seeded("ask-notes");
    c.ok(&["save", "--next", "carry on"]);
    holder(&c);
    open(&c, "Job");
    notes(&c, 25);
    let out = c.ok(&["done", "2", "finished"]);
    assert!(out.contains(&block(28, "v1")), "{out}");
}

/// A `save --next` is a spoken stop: 20 changes later a close is silent even
/// though the lane is far past 25 in all.
#[test]
fn a_save_with_next_starts_the_count_over() {
    let c = Sandbox::new_seeded("ask-save-next");
    c.ok(&["save", "--next", "carry on"]);
    holder(&c);
    notes(&c, 23);
    c.ok(&["save", "--next", "and then this"]);
    open(&c, "Job");
    notes(&c, 18);
    let out = c.ok(&["done", "2", "finished"]);
    assert!(!has_block(&out), "{out}");
}

/// A `pop --next` is a spoken stop itself: its own answer never carries the
/// block, even though without `--next` it would have crossed.
#[test]
fn a_pop_with_next_starts_over_and_shows_no_block() {
    let with = Sandbox::new_seeded("ask-pop-next");
    let without = Sandbox::new_seeded("ask-pop-plain");
    for c in [&with, &without] {
        c.ok(&["save", "--next", "carry on"]);
        holder(c);
        notes(c, 22);
        c.ok(&["push", "Detour", "--why", "it is needed"]);
    }
    // 1 + 22 + push (2) = 25; the close is the 26th and the stack stepping
    // back the 27th, which is the count the answer reads.
    let plain = without.ok(&["pop", "finished"]);
    assert!(plain.contains(&block(27, "v1")), "{plain}");

    let spoken = with.ok(&["pop", "finished", "--next", "review it"]);
    assert!(!has_block(&spoken), "{spoken}");
    // And it started over: two changes and a close are far from 25.
    open(&with, "Job");
    open(&with, "Another");
    let after = with.ok(&["done", "4", "finished"]);
    assert!(!has_block(&after), "{after}");
}

/// A `save` with no `--next`, a `push` and a `park` are not spoken stops:
/// the count keeps running and the block still names the one that was.
#[test]
fn stops_with_no_intent_do_not_reset() {
    let c = Sandbox::new_seeded("ask-unspoken");
    c.ok(&["save", "--next", "carry on"]);
    holder(&c);
    notes(&c, 6);
    c.ok(&["save", "a label and no next step"]);
    c.ok(&["push", "Detour", "--why", "it is needed"]);
    notes(&c, 6);
    c.ok(&["park", "Not now"]);
    open(&c, "Job");
    notes(&c, 12);
    // 1 + 6 + push (2) + 6 + park (2) + 1 + 12 = 30, and the close is 31.
    let out = c.ok(&["done", "3", "finished"]);
    assert!(out.contains("31 changes since v1,"), "{out}");
    assert!(
        !out.contains("since v2") && !out.contains("since v3"),
        "{out}"
    );
}

/// A lane that has never had a spoken stop says so in the first line.
#[test]
fn a_lane_with_no_spoken_stop_says_so() {
    let c = Sandbox::new_seeded("ask-no-stop");
    holder(&c);
    open(&c, "Job");
    notes(&c, 24);
    open(&c, "Other job");
    let out = c.ok(&["done", "2", "finished"]);
    assert!(
        out.contains(
            "  28 changes and no stop has said what comes next.\n  \
             To leave the next step:  vivac save --next \"<what comes next>\""
        ),
        "{out}"
    );
}

/// A close that closed nothing -- the node was already closed -- does not
/// show the block a second time.
#[test]
fn a_close_that_changed_nothing_does_not_repeat_the_block() {
    let c = Sandbox::new_seeded("ask-repeat");
    holder(&c);
    open(&c, "Job");
    notes(&c, 25);
    let first = c.ok(&["done", "2", "finished"]);
    assert!(first.contains(NO_STOP), "{first}");
    let again = c.ok(&["done", "2", "finished again"]);
    assert!(!has_block(&again), "{again}");
}
