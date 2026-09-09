//! `open` -- the fronts view, ordered by what the pillar of UX asks it to
//! answer: what is waiting for you right now, and what has been open so
//! long you are not actually doing it any more (`d383`).

mod common;
use common::Sandbox;

/// Howard Hinnant's algorithm, the same one `src/clock.rs` implements. There
/// is no lib target this suite can call into, so a fixed date has to be
/// built by hand to backdate a node's `opened` day; duplicating the eight
/// lines that do that costs less than a date crate, same reasoning as
/// `clock.rs`'s own module header.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// An RFC 3339 stamp `days` whole days before `ts`. `open` takes no `--now`
/// override the way `brief` does, so the only way to make a node look old in
/// a test is to backdate its `node.created` line directly.
fn days_before(ts: &str, days: i64) -> String {
    let y: i64 = ts[0..4].parse().unwrap();
    let m: i64 = ts[5..7].parse().unwrap();
    let d: i64 = ts[8..10].parse().unwrap();
    let (y, m, d) = civil_from_days(days_from_civil(y, m, d) - days);
    format!("{y:04}-{m:02}-{d:02}T00:00:00Z")
}

/// Rewrites the `ts` of the `node.created` line for `num`, so its `opened`
/// day moves without waiting real days for it. The log is one JSON object
/// per line, so a substring replace on that one line is enough.
fn backdate(c: &Sandbox, num: u64, days: i64) {
    let path = c.0.join(".vivac").join("events");
    let raw = std::fs::read_to_string(&path).unwrap();
    let marker = format!("\"num\":{num},");
    let mut done = false;
    let out: Vec<String> = raw
        .lines()
        .map(|l| {
            if !done && l.contains("\"type\":\"node.created\"") && l.contains(&marker) {
                let ts_at = l.find("\"ts\":\"").unwrap() + 6;
                let ts_end = ts_at + l[ts_at..].find('"').unwrap();
                let target = days_before(&l[ts_at..ts_end], days);
                done = true;
                format!("{}{}{}", &l[..ts_at], target, &l[ts_end..])
            } else {
                l.to_string()
            }
        })
        .collect();
    assert!(done, "no node.created line found for num {num}");
    std::fs::write(&path, out.join("\n") + "\n").unwrap();
}

fn aliases(s: &str) -> Vec<&str> {
    s.lines()
        .filter(|l| l.trim_start().starts_with(['t', 'g', 'f']))
        .filter_map(|l| l.split_whitespace().next())
        .collect()
}

/// A blocker outranks tree size: the first key of the tuple beats the
/// second one, whatever the second one says. Born after the bigger one, and
/// with none of its own tree, so neither `num` ascending nor subtree size
/// would put it first by accident -- only the blocker key does.
#[test]
fn a_blocker_comes_before_a_bigger_subtree() {
    let c = Sandbox::new_seeded("blocks-first");
    c.ok(&["push", "Root", "--why", "it is needed"]);
    c.ok(&[
        "add",
        "Bigger child",
        "--parent",
        "1",
        "--why",
        "holds more tree",
    ]);
    c.ok(&[
        "decide",
        "A call under the bigger one",
        "--parent",
        "2",
        "--reason",
        "gives it a subtree",
    ]);
    c.ok(&[
        "add",
        "Blocking child",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "must close first",
    ]);
    let s = c.ok(&["open"]);
    let order = aliases(&s);
    let bigger = order.iter().position(|a| *a == "t2").expect("t2 listed");
    let blocker = order.iter().position(|a| *a == "t4").expect("t4 listed");
    assert!(blocker < bigger, "the blocker did not sort first:\n{s}");
}

/// Between two fronts that neither blocks, the one holding up more tree
/// comes first.
#[test]
fn among_non_blockers_the_bigger_subtree_comes_first() {
    let c = Sandbox::new_seeded("bigger-first");
    c.ok(&["push", "Root", "--why", "it is needed"]);
    c.ok(&["add", "Small", "--parent", "1", "--why", "nothing under it"]);
    c.ok(&["add", "Bigger", "--parent", "1", "--why", "holds more tree"]);
    c.ok(&[
        "decide",
        "A call under the bigger one",
        "--parent",
        "3",
        "--reason",
        "gives it a subtree",
    ]);
    let s = c.ok(&["open"]);
    let order = aliases(&s);
    let small = order.iter().position(|a| *a == "t2").expect("t2 listed");
    let bigger = order.iter().position(|a| *a == "t3").expect("t3 listed");
    assert!(
        bigger < small,
        "the bigger subtree did not sort first:\n{s}"
    );
}

/// At a tie on the first two keys, the newer node -- the higher `num` --
/// comes first.
#[test]
fn a_tie_breaks_toward_the_newer_node() {
    let c = Sandbox::new_seeded("tie-newest");
    c.ok(&["push", "Root", "--why", "it is needed"]);
    c.ok(&["add", "Older", "--parent", "1", "--why", "opened first"]);
    c.ok(&["add", "Newer", "--parent", "1", "--why", "opened after"]);
    let s = c.ok(&["open"]);
    let order = aliases(&s);
    let older = order.iter().position(|a| *a == "t2").expect("t2 listed");
    let newer = order.iter().position(|a| *a == "t3").expect("t3 listed");
    assert!(newer < older, "the newer node did not sort first:\n{s}");
}

/// More than ten fronts: only ten print, and the tail line says how many did
/// not.
#[test]
fn more_than_ten_fronts_are_capped_with_a_tail_line() {
    let c = Sandbox::new_seeded("capped");
    c.ok(&["push", "Root", "--why", "it is needed"]);
    for i in 1..=12 {
        c.ok(&[
            "add",
            &format!("Front {i}"),
            "--parent",
            "1",
            "--why",
            "one of many",
        ]);
    }
    let s = c.ok(&["open"]);
    let order = aliases(&s);
    assert_eq!(order.len(), 10, "did not cap at ten:\n{s}");
    assert!(
        s.contains("2 more"),
        "no tail line with the right count:\n{s}"
    );
    assert!(s.contains("vivac open --all"), "no way out offered:\n{s}");
}

/// `--all` lifts the cap and drops the tail line: quitting access to the
/// full list was never on the table.
#[test]
fn all_lifts_the_cap_and_drops_the_tail() {
    let c = Sandbox::new_seeded("all-flag");
    c.ok(&["push", "Root", "--why", "it is needed"]);
    for i in 1..=12 {
        c.ok(&[
            "add",
            &format!("Front {i}"),
            "--parent",
            "1",
            "--why",
            "one of many",
        ]);
    }
    let s = c.ok(&["open", "--all"]);
    let order = aliases(&s);
    assert_eq!(order.len(), 12, "--all did not print every front:\n{s}");
    assert!(!s.contains("more,"), "the tail line survived --all:\n{s}");
}

/// `--json` carries every front, in the same order the text does. Born in
/// ascending-`num` order the old sort would have kept, so a twin left on the
/// old key would disagree with the text side, which already moved.
#[test]
fn json_carries_all_fronts_in_the_new_order() {
    let c = Sandbox::new_seeded("json-order");
    c.ok(&["push", "Root", "--why", "it is needed"]);
    c.ok(&[
        "add",
        "Bigger child",
        "--parent",
        "1",
        "--why",
        "holds more tree",
    ]);
    c.ok(&[
        "decide",
        "A call under the bigger one",
        "--parent",
        "2",
        "--reason",
        "gives it a subtree",
    ]);
    c.ok(&[
        "add",
        "Blocking child",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "must close first",
    ]);
    let text = c.ok(&["open"]);
    let json = c.ok(&["open", "--json"]);
    let first_text = aliases(&text)[0];
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let first_json = v[0]["alias"].as_str().unwrap();
    assert_eq!(
        first_text, first_json,
        "text and json disagree on first:\n{text}\n{json}"
    );
}

/// The tail names the age of the oldest node **among the hidden ones**, not
/// the oldest of the whole set: the overall oldest front here is shown,
/// because it blocks, and its age must not leak into the tail line.
#[test]
fn the_tail_ages_the_oldest_hidden_front_not_the_oldest_overall() {
    let c = Sandbox::new_seeded("tail-age");
    c.ok(&["push", "Root", "--why", "it is needed"]);
    c.ok(&[
        "add",
        "Old blocker",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "shown regardless of age",
    ]);
    backdate(&c, 2, 100);
    for i in 1..=10 {
        c.ok(&[
            "add",
            &format!("Front {i}"),
            "--parent",
            "1",
            "--why",
            "one of many",
        ]);
    }
    // num 3, the lowest-numbered plain front, is the one the tie-break
    // pushes to the bottom -- the only front left out.
    backdate(&c, 3, 5);
    let s = c.ok(&["open"]);
    assert!(
        s.contains("the oldest open for 5 days"),
        "did not age the hidden front:\n{s}"
    );
    assert!(
        !s.contains("100"),
        "leaked the age of a shown front into the tail:\n{s}"
    );
}
