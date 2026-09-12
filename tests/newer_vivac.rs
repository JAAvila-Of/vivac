//! `t411` §13 (`d423`, `f419`): the reader refuses a well-formed line it
//! does not recognise instead of skipping it in silence.
//!
//! `Store::read_all` already skipped and counted any line that failed to
//! deserialise, and that stays true for a genuinely broken one -- bad JSON,
//! a truncated tail, garbage dropped in by something else entirely. What
//! changes is the one shape that is not an accident: a line that *is* valid
//! JSON, carries a numeric `seq` and a `payload.type`, and names an event
//! type or a node kind this version has never heard of. That can only mean
//! a newer vivac wrote it, and reading past it would mean acting on a tree
//! this version cannot actually see all of.

mod common;
use common::Sandbox;

/// How many lines `events` already holds, so a test can say which line its
/// own appended one will be without hard-coding how many events one `push`
/// happens to write.
fn line_count(c: &Sandbox) -> usize {
    c.log().lines().filter(|l| !l.trim().is_empty()).count()
}

fn unknown_event_message(line_no: usize) -> String {
    format!(
        "This tree was written by a newer vivac: line {line_no} of .vivac/events is an \
         event this version does not know (node.evolved). Update vivac to read it. \
         Nothing was written."
    )
}

fn unknown_kind_message(line_no: usize) -> String {
    format!(
        "This tree was written by a newer vivac: line {line_no} of .vivac/events creates \
         a node of a type this version does not know (epic). Update vivac to read it. \
         Nothing was written."
    )
}

#[test]
fn an_unknown_event_type_refuses_tree_before_writing_and_leaves_the_log_untouched() {
    let c = Sandbox::new_seeded("nv-unknown-event-tree");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let line_no = line_count(&c) + 1;
    c.append_unknown_event_type();
    let before = c.log();

    let (out, code) = c.run(&["tree"]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains(&unknown_event_message(line_no)), "{out}");
    assert_eq!(before, c.log(), "a refused read still wrote:\n{out}");
}

#[test]
fn an_unknown_event_type_refuses_brief_before_writing_and_leaves_the_log_untouched() {
    let c = Sandbox::new_seeded("nv-unknown-event-brief");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let line_no = line_count(&c) + 1;
    c.append_unknown_event_type();
    let before = c.log();

    let (out, code) = c.run(&["brief"]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains(&unknown_event_message(line_no)), "{out}");
    assert_eq!(before, c.log(), "a refused read still wrote:\n{out}");
}

#[test]
fn an_unknown_event_type_refuses_add_before_writing_and_leaves_the_log_untouched() {
    let c = Sandbox::new_seeded("nv-unknown-event-add");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let line_no = line_count(&c) + 1;
    c.append_unknown_event_type();
    let before = c.log();

    let (out, code) = c.run(&["add", "Another node", "--why", "reason"]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains(&unknown_event_message(line_no)), "{out}");
    assert_eq!(before, c.log(), "a refused write still wrote:\n{out}");
}

#[test]
fn an_unknown_node_kind_refuses_before_writing_and_leaves_the_log_untouched() {
    let c = Sandbox::new_seeded("nv-unknown-kind");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let line_no = line_count(&c) + 1;
    c.append_unknown_node_kind();
    let before = c.log();

    let (tree_out, tree_code) = c.run(&["tree"]);
    assert_eq!(tree_code, 5, "{tree_out}");
    assert!(
        tree_out.contains(&unknown_kind_message(line_no)),
        "{tree_out}"
    );

    let (brief_out, brief_code) = c.run(&["brief"]);
    assert_eq!(brief_code, 5, "{brief_out}");
    assert!(
        brief_out.contains(&unknown_kind_message(line_no)),
        "{brief_out}"
    );

    let (add_out, add_code) = c.run(&["add", "Another node", "--why", "reason"]);
    assert_eq!(add_code, 5, "{add_out}");
    assert!(
        add_out.contains(&unknown_kind_message(line_no)),
        "{add_out}"
    );

    assert_eq!(before, c.log(), "a refused read or write still wrote");
}

/// `t411` §13, test 3: the same refusal once a derived index already
/// covered the line before it. The index falls back to folding the log
/// fresh rather than trusting a tail it cannot read past.
#[test]
fn an_unknown_event_after_the_index_was_already_built_still_refuses() {
    let c = Sandbox::new_seeded("nv-unknown-after-index");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    c.ok(&["stack"]); // a plain read persists the derived index over what exists so far
    let index_path = c.0.join(".vivac").join("index");
    assert!(index_path.exists(), "no index to grow from");

    let line_no = line_count(&c) + 1;
    c.append_unknown_event_type();
    let before = c.log();

    let (out, code) = c.run(&["tree"]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains(&unknown_event_message(line_no)), "{out}");
    assert_eq!(before, c.log(), "a refused read still wrote:\n{out}");
}

/// Fixes what already worked: a truncated last line -- no closing brace, no
/// final newline, exactly what a crash mid-write leaves behind -- is still
/// counted and skipped, not refused.
#[test]
fn a_truncated_last_line_still_reads_as_today_and_check_counts_it() {
    let c = Sandbox::new_seeded("nv-truncated");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(c.0.join(".vivac").join("events"))
            .unwrap();
        // No trailing newline, and the object itself is cut off mid-field.
        write!(f, "{{\"seq\":2,\"id\":\"chopped").unwrap();
    }

    let out = c.ok(&["tree"]);
    assert!(out.contains("Ship it"), "{out}");

    let (check_out, check_code) = c.run(&["check"]);
    assert_eq!(check_code, 1, "{check_out}");
    assert!(
        check_out.contains("1 unreadable line(s) in .vivac/events (skipped while reading)"),
        "{check_out}"
    );
}

/// Fixes what already worked: garbage sitting between two good lines is
/// still counted and skipped, and writes past it still land.
#[test]
fn garbage_in_the_middle_of_the_log_still_reads_as_today_and_check_counts_it() {
    let c = Sandbox::new_seeded("nv-garbage-middle");
    c.ok(&["push", "First", "--why", "reason"]);
    c.append_raw_line("this is not even json");
    c.ok(&["pop"]);

    let out = c.ok(&["tree"]);
    assert!(out.contains("First"), "{out}");

    let (check_out, check_code) = c.run(&["check"]);
    assert_eq!(check_code, 1, "{check_out}");
    assert!(
        check_out.contains("1 unreadable line(s) in .vivac/events (skipped while reading)"),
        "{check_out}"
    );
}

fn unreadable_known_event_message(line_no: usize) -> String {
    format!(
        "This tree was written by a newer vivac: line {line_no} of .vivac/events is a \
         flag.raised event whose fields this version cannot read. Update vivac to read \
         it. Nothing was written."
    )
}

/// A type this version knows, with a value it does not: a flag that is not
/// `suspect`, `review` or `stale`. The day a newer vivac adds a value to a
/// field that already exists, this is what an older one meets, and skipping
/// it would be `f419` again one release later.
#[test]
fn a_known_event_with_a_value_this_version_does_not_know_refuses_tree() {
    let c = Sandbox::new_seeded("nv-unreadable-known-tree");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let line_no = line_count(&c) + 1;
    c.append_unreadable_known_event();
    let before = c.log();

    let (out, code) = c.run(&["tree"]);
    assert_eq!(code, 5, "{out}");
    assert!(
        out.contains(&unreadable_known_event_message(line_no)),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused read still wrote:\n{out}");
}

#[test]
fn a_known_event_with_a_value_this_version_does_not_know_refuses_add() {
    let c = Sandbox::new_seeded("nv-unreadable-known-add");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let line_no = line_count(&c) + 1;
    c.append_unreadable_known_event();
    let before = c.log();

    let (out, code) = c.run(&["add", "Another node", "--why", "reason"]);
    assert_eq!(code, 5, "{out}");
    assert!(
        out.contains(&unreadable_known_event_message(line_no)),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused write still wrote:\n{out}");
}

// ---------------------------------------------------------------------------
// `t411` §22.12, `d441`: the two shapes an arm took before the folder was
// part of it are lines with the shape of an event this version cannot read,
// not lines with a value it merely does not recognise -- `arm.added` has no
// `dir` at all, and an old `node.created` carries `arms` as bare strings.
// ---------------------------------------------------------------------------

fn unreadable_arm_added_message(line_no: usize) -> String {
    format!(
        "This tree was written by a newer vivac: line {line_no} of .vivac/events is a \
         arm.added event whose fields this version cannot read. Update vivac to read \
         it. Nothing was written."
    )
}

fn unreadable_old_arm_shape_message(line_no: usize) -> String {
    format!(
        "This tree was written by a newer vivac: line {line_no} of .vivac/events is a \
         node.created event whose fields this version cannot read. Update vivac to \
         read it. Nothing was written."
    )
}

#[test]
fn an_arm_added_with_no_folder_refuses_tree_and_leaves_the_log_untouched() {
    let c = Sandbox::new_seeded("nv-arm-no-dir");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let line_no = line_count(&c) + 1;
    c.append_arm_added_without_dir();
    let before = c.log();

    let (out, code) = c.run(&["tree"]);
    assert_eq!(code, 5, "{out}");
    assert!(
        out.contains(&unreadable_arm_added_message(line_no)),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused read still wrote:\n{out}");
}

#[test]
fn a_node_created_with_string_arms_refuses_tree_and_leaves_the_log_untouched() {
    let c = Sandbox::new_seeded("nv-old-arm-shape");
    c.ok(&["push", "Ship it", "--why", "reason"]);
    let line_no = line_count(&c) + 1;
    c.append_node_created_with_string_arms();
    let before = c.log();

    let (out, code) = c.run(&["tree"]);
    assert_eq!(code, 5, "{out}");
    assert!(
        out.contains(&unreadable_old_arm_shape_message(line_no)),
        "{out}"
    );
    assert_eq!(before, c.log(), "a refused read still wrote:\n{out}");
}
