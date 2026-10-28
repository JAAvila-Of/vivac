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

fn legacy_node(num: u64, kind: serde_json::Value, parent: Option<u64>) -> serde_json::Value {
    serde_json::json!({
        "id": num, "title": "Imported", "kind": kind, "status": "active",
        "parent": parent, "opened": "2026-09-08"
    })
}

fn assert_unchanged_empty_tree(c: &Sandbox, before: &str) {
    let after = log_text(c);
    assert_eq!(after, before, "the refused import changed the event log");
    assert!(
        !after.contains("\"type\":\"node.created\""),
        "the tree must still have no nodes"
    );
}

#[test]
fn all_six_legacy_kinds_preserve_numbers_and_parent_links() {
    let c = Sandbox::new_seeded("import-spike-kinds");
    let mut nodes = serde_json::Map::new();
    for (num, kind, parent) in [
        (8, "goal", None),
        (11, "decision", Some(8)),
        (17, "finding", Some(11)),
        (23, "work", Some(8)),
        (31, "run", Some(23)),
        (42, "issue", Some(23)),
    ] {
        nodes.insert(num.to_string(), legacy_node(num, kind.into(), parent));
    }
    let tree = write_tree(&c, &serde_json::json!({ "nodes": nodes }).to_string());
    c.ok(&["import", tree.to_str().unwrap()]);
    let created: Vec<serde_json::Value> = log_text(&c)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .map(|event| event["payload"].clone())
        .filter(|event| event["type"] == "node.created")
        .collect();
    assert_eq!(created.len(), 6);
    for (index, (num, kind, parent_index)) in [
        (8, "goal", None),
        (11, "decision", Some(0)),
        (17, "finding", Some(1)),
        (23, "task", Some(0)),
        (31, "task", Some(3)),
        (42, "task", Some(3)),
    ]
    .into_iter()
    .enumerate()
    {
        let event = &created[index];
        assert_eq!(event["num"], num);
        assert_eq!(event["kind"], kind);
        let parent = parent_index
            .map(|i: usize| created[i]["node"].clone())
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(event["parent"], parent);
    }
}

#[test]
fn a_late_unsupported_kind_refuses_even_the_valid_earlier_node() {
    let c = Sandbox::new_seeded("import-late-kind");
    let before = log_text(&c);
    let tree = write_tree(
        &c,
        &serde_json::json!({"nodes": {
            "1": legacy_node(1, "goal".into(), None),
            "99": legacy_node(99, "question".into(), Some(1))
        }})
        .to_string(),
    );
    let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("#99 kind"), "{out}");
    assert_unchanged_empty_tree(&c, &before);
}

#[test]
fn unsupported_kinds_name_every_field_without_echoing_source_values() {
    let c = Sandbox::new_seeded("import-private-kinds");
    let before = log_text(&c);
    let private_kind = "S7vQ2mN9xK4pR8tW6zB3cD5fG1hJ0lU7";
    let source_title = "source-title-sentinel";
    let mut first = legacy_node(7, private_kind.into(), None);
    first["title"] = source_title.into();
    let tree = write_tree(
        &c,
        &serde_json::json!({"nodes": {
            "7": first,
            "19": legacy_node(19, "question".into(), None),
            "41": legacy_node(41, "constraint".into(), None)
        }})
        .to_string(),
    );
    let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
    assert_eq!(code, 2, "{out}");
    for num in [7, 19, 41] {
        assert!(out.contains(&format!("#{num} kind")), "{out}");
    }
    for value in [private_kind, source_title, "question", "constraint"] {
        assert!(!out.contains(value), "source value leaked: {out}");
    }
    assert_unchanged_empty_tree(&c, &before);
}

#[test]
fn current_unknown_and_malformed_kinds_refuse_without_mutation() {
    for (i, kind) in [
        Some(serde_json::json!("task")),
        Some(serde_json::json!("question")),
        Some(serde_json::json!("constraint")),
        Some(serde_json::json!("assumption")),
        Some(serde_json::json!("pillar")),
        Some(serde_json::json!("rule")),
        Some(serde_json::json!("unknown")),
        Some(serde_json::json!("")),
        Some(serde_json::json!("Goal")),
        Some(serde_json::json!("g\u{f3}al")),
        None,
        Some(serde_json::json!(null)),
        Some(serde_json::json!(42)),
        Some(serde_json::json!(true)),
        Some(serde_json::json!([])),
        Some(serde_json::json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let c = Sandbox::new_seeded(&format!("import-bad-kind-{i}"));
        let before = log_text(&c);
        let mut node = legacy_node(1, serde_json::Value::Null, None);
        let string_kind = kind.as_ref().is_some_and(serde_json::Value::is_string);
        match kind {
            Some(value) => node["kind"] = value,
            None => {
                node.as_object_mut().unwrap().remove("kind");
            }
        }
        let tree = write_tree(&c, &serde_json::json!({"nodes": {"1": node}}).to_string());
        let (out, code) = c.run(&["import", tree.to_str().unwrap()]);
        assert_eq!(code, 2, "case {i}: {out}");
        if string_kind {
            assert!(out.contains("#1 kind"), "case {i}: {out}");
        } else {
            assert!(out.contains("not a spike tree.json"), "case {i}: {out}");
        }
        assert_unchanged_empty_tree(&c, &before);
    }
}
