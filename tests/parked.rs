//! `vivac parked` — the return date `--until` gives a park, and whether it
//! is due. `d899`.
//!
//! Due-ness compares against the real local date: `parked` takes no
//! `--now` (`brief` does, and that is where determinism against a fixed
//! date is tested). A not-due fixture uses a date far enough in the future
//! that `park --until` accepts it through the CLI; a due one is written
//! straight into the log, since `park --until` never accepts a date that
//! has already passed.

mod common;
use common::Sandbox;
use serde_json::Value;

/// The ULID `node.created` gave `num`, read back off the log.
fn node_id_of(c: &Sandbox, num: u64) -> String {
    for line in c.log().lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        if v["payload"]["type"] == "node.created" && v["payload"]["num"] == num {
            return v["payload"]["node"].as_str().unwrap().to_string();
        }
    }
    panic!("no node.created for num {num} in:\n{}", c.log());
}

/// Parks `num` with `until` already in the past, straight in the log.
fn park_already_due(c: &Sandbox, seq: u64, num: u64, until: &str) {
    let node = node_id_of(c, num);
    c.append_raw_line(&format!(
        r#"{{"seq":{seq},"id":"01PARKEDDUE{seq:014}","ts":"2020-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{{"type":"state.changed","node":"{node}","state":"suspended","outcome":"long overdue","forced":false,"until":"{until}"}}}}"#
    ));
}

#[test]
fn a_not_due_park_shows_its_own_date_in_text_and_json() {
    let c = Sandbox::new_seeded("parked-until-not-due");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    c.ok(&["park", "waiting on the release", "--until", "9999-12-31"]);

    let text = c.ok(&["parked"]);
    assert!(text.contains("until 9999-12-31"), "{text}");
    assert!(
        !text.contains("back since") && !text.contains("due"),
        "not due yet:\n{text}"
    );

    let json = c.ok(&["parked", "--json"]);
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v[0]["until"], "9999-12-31");
    assert_eq!(v[0]["due"], false);
}

#[test]
fn a_due_park_is_marked_in_text_and_json() {
    let c = Sandbox::new_seeded("parked-until-due");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    park_already_due(&c, 900, 1, "2020-01-02");

    let text = c.ok(&["parked"]);
    assert!(text.contains("2020-01-02"), "{text}");
    assert!(
        text.contains("back since") || text.contains("due"),
        "no text marker for a due park:\n{text}"
    );

    let json = c.ok(&["parked", "--json"]);
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v[0]["until"], "2020-01-02");
    assert_eq!(v[0]["due"], true);
}

#[test]
fn a_park_with_no_until_carries_none_and_is_not_due() {
    let c = Sandbox::new_seeded("parked-until-absent");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    c.ok(&["park", "waiting"]);

    let text = c.ok(&["parked"]);
    assert!(
        !text.contains("until") && !text.contains("due"),
        "a park with no date should say nothing about one:\n{text}"
    );

    let json = c.ok(&["parked", "--json"]);
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v[0]["due"], false);
    assert!(v[0]["until"].is_null(), "{json}");
}
