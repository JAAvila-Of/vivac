//! `d866`/`f828`: `import`'s own reading of a spike node's `opened` and
//! `closed` dates.
//!
//! `src/import.rs`'s module header names two things the port promised to
//! keep: the node number and the original date. The date half slipped --
//! anything that was not a bare `YYYY-MM-DD` silently became the moment
//! `import` happened to run, no matter how precise or well-formed the
//! spike's own stamp was. This covers what a date has to look like to be
//! kept, and that anything else refuses the whole import rather than
//! guessing.

mod common;
use common::Sandbox;

/// The raw log, as written -- same shape `tests/lanes.rs`'s own
/// `log_text` reads, kept local here rather than shared because this file
/// is small enough not to need `mod common` to grow for it.
fn log_text(c: &Sandbox) -> String {
    std::fs::read_to_string(c.0.join(".vivac").join("events")).expect("the log is there")
}

/// A one-node spike tree, `opened` and (optionally) `closed` set by hand.
fn tree_json_with(opened: &str, closed: Option<&str>) -> String {
    let closed_field = match closed {
        Some(c) => format!(",\"closed\":\"{c}\""),
        None => String::new(),
    };
    format!(
        r#"{{"nodes":{{"1":{{"id":1,"title":"Imported","kind":"goal","status":"active","opened":"{opened}"{closed_field}}}}}}}"#
    )
}

/// Two nodes, each with its own `opened`, for the two-bad-dates case.
fn tree_json_two(opened_a: &str, opened_b: &str) -> String {
    format!(
        r#"{{"nodes":{{"1":{{"id":1,"title":"First","kind":"goal","status":"active","opened":"{opened_a}"}},"2":{{"id":2,"title":"Second","kind":"goal","status":"active","opened":"{opened_b}"}}}}}}"#
    )
}

fn write_tree(c: &Sandbox, json: &str) -> std::path::PathBuf {
    let p = c.0.join("tree.json");
    std::fs::write(&p, json).unwrap();
    p
}

/// The `ts` of the first `node.created` line in the log.
fn created_ts(c: &Sandbox) -> String {
    let log = log_text(c);
    let line = log
        .lines()
        .find(|l| l.contains("\"type\":\"node.created\""))
        .expect("a node.created line");
    let at = line.find("\"ts\":\"").unwrap() + 6;
    let end = at + line[at..].find('"').unwrap();
    line[at..end].to_string()
}

#[test]
fn a_bare_date_keeps_noon_utc() {
    let c = Sandbox::new_seeded("import-bare-date");
    let tree = write_tree(&c, &tree_json_with("2026-09-08", None));
    c.ok(&["import", tree.to_str().unwrap()]);
    assert_eq!(created_ts(&c), "2026-09-08T12:00:00Z");
}

#[test]
fn a_full_stamp_with_z_is_kept_byte_for_byte() {
    let c = Sandbox::new_seeded("import-full-stamp-z");
    let tree = write_tree(&c, &tree_json_with("2026-09-08T03:04:05Z", None));
    c.ok(&["import", tree.to_str().unwrap()]);
    assert_eq!(created_ts(&c), "2026-09-08T03:04:05Z");
}

#[test]
fn an_offset_stamp_with_fractional_seconds_lands_on_the_right_utc_second_crossing_midnight() {
    let c = Sandbox::new_seeded("import-offset-fraction");
    // 00:30:00.999+02:00 on the 8th is 22:30:00 UTC on the 7th: two hours
    // west of the stamp's own offset, which is also a day earlier.
    let tree = write_tree(&c, &tree_json_with("2026-09-08T00:30:00.999+02:00", None));
    c.ok(&["import", tree.to_str().unwrap()]);
    assert_eq!(created_ts(&c), "2026-09-07T22:30:00Z");
}

#[test]
fn an_invalid_calendar_date_refuses_the_whole_import_and_writes_nothing() {
    let c = Sandbox::new_seeded("import-invalid-calendar");
    let before = log_text(&c);
    let tree = write_tree(&c, &tree_json_with("2026-02-30", None));
    let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("#1"),
        "the message should name node #1:\n{out}"
    );
    assert!(out.contains("opened"), "{out}");
    assert!(out.contains("2026-02-30"), "{out}");
    assert_eq!(
        log_text(&c),
        before,
        "the import wrote events despite refusing"
    );
}

#[test]
fn a_garbage_date_refuses_the_whole_import_and_writes_nothing() {
    let c = Sandbox::new_seeded("import-garbage-date");
    let before = log_text(&c);
    let tree = write_tree(&c, &tree_json_with("not a date", None));
    let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("opened"), "{out}");
    assert!(out.contains("not a date"), "{out}");
    assert_eq!(
        log_text(&c),
        before,
        "the import wrote events despite refusing"
    );
}

#[test]
fn an_empty_opened_refuses_the_whole_import_and_writes_nothing() {
    let c = Sandbox::new_seeded("import-empty-opened");
    let before = log_text(&c);
    // No `opened` key at all: `#[serde(default)]` leaves it `""`.
    let tree = write_tree(
        &c,
        r#"{"nodes":{"1":{"id":1,"title":"Imported","kind":"goal","status":"active"}}}"#,
    );
    let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("opened"), "{out}");
    assert_eq!(
        log_text(&c),
        before,
        "the import wrote events despite refusing"
    );
}

#[test]
fn an_invalid_closed_date_is_named_as_the_closed_field() {
    let c = Sandbox::new_seeded("import-invalid-closed");
    let before = log_text(&c);
    let json = r#"{"nodes":{"1":{"id":1,"title":"Imported","kind":"goal","status":"done","opened":"2026-09-01","closed":"2026-13-01"}}}"#;
    let tree = write_tree(&c, json);
    let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("closed"), "{out}");
    assert!(out.contains("2026-13-01"), "{out}");
    assert_eq!(
        log_text(&c),
        before,
        "the import wrote events despite refusing"
    );
}

#[test]
fn two_bad_dates_on_two_nodes_are_both_named_in_one_refusal() {
    let c = Sandbox::new_seeded("import-two-bad-dates");
    let before = log_text(&c);
    let tree = write_tree(&c, &tree_json_two("2026-02-30", "garbage"));
    let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("2026-02-30"), "{out}");
    assert!(out.contains("garbage"), "{out}");
    assert_eq!(
        log_text(&c),
        before,
        "the import wrote events despite refusing"
    );
}

/// A stamp that has the right punctuation in the right places but not
/// digits between it. Two ways that went wrong: a character wider than a
/// byte where a digit belongs put a slice boundary inside it, which
/// panicked instead of refusing, and a leading `+` is something integer
/// parsing accepts, so `+026` read as the year 26.
#[test]
fn a_date_that_is_not_all_digits_where_digits_go_refuses_rather_than_panicking() {
    for (i, date) in [
        "2026-09-0\u{e9}T12:00:00Z",
        "+026-09-27",
        "2026-09-08T+1:00:00Z",
        "2026-09-08T01:00:00+0\u{e9}:00",
    ]
    .into_iter()
    .enumerate()
    {
        let c = Sandbox::new_seeded(&format!("import-not-digits-{i}"));
        let before = log_text(&c);
        let tree = write_tree(&c, &tree_json_with(date, None));
        let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
        assert_eq!(code, 2, "{date}: {out}");
        assert!(out.contains("opened"), "{date}: {out}");
        assert_eq!(log_text(&c), before, "{date}: the import wrote events");
    }
}
