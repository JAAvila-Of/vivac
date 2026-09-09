//! `find` — text search over the tree.
//!
//! `PILLARS.md` budgets text search at under 100 ms, and nothing implemented
//! it: a ceiling with no floor under it, the same class of unchecked claim as
//! the test count that lied and the minimum toolchain nobody verified.
//!
//! What made it urgent is that searching is the main read of a memory, and a
//! memory you cannot search is a folder. So the search has to reach the
//! fields that carry the meaning -- `why`, the note, the outcome -- and not
//! just the titles, and it has to reach closed nodes, because what you look
//! for months later is usually finished.

mod common;
use common::Sandbox;

/// A tree with the same words spread across different fields, so a test can
/// tell which field a hit came from.
fn seeded(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "the version was a hand edit",
    ]);
    c.ok(&[
        "add",
        "Guard the commit messages",
        "--why",
        "a malformed one does not fail loudly, it silently does not count",
        "--type",
        "task",
    ]);
    c.ok(&[
        "add",
        "The sandbox named its directory from the clock",
        "--why",
        "unique by luck rather than by construction",
        "--type",
        "finding",
    ]);
    c
}

#[test]
fn a_word_in_the_title_is_found() {
    let c = seeded("title");
    let s = c.ok(&["find", "sandbox"]);
    assert!(s.contains("The sandbox named its directory"), "{s}");
}

/// The reason is where the content lives. A search that only reads titles
/// finds the label and misses the thinking.
#[test]
fn a_word_only_in_the_why_is_found() {
    let c = seeded("why");
    let s = c.ok(&["find", "malformed"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

#[test]
fn a_word_only_in_a_note_is_found() {
    let c = seeded("note");
    c.ok(&["note", "t2", "the fixtures live under tests/data"]);
    let s = c.ok(&["find", "fixtures"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

/// `f389`: a second note used to overwrite the first everywhere, taking 84
/// notes out of reach of `find` on the real tree. The old note has to stay
/// searchable once a newer one arrives (`d390`).
#[test]
fn a_word_only_in_an_old_note_is_still_found() {
    let c = seeded("old-note");
    c.ok(&["note", "t2", "the fixtures live under tests/data"]);
    c.ok(&["note", "t2", "a correction that names none of that"]);
    let s = c.ok(&["find", "fixtures"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

/// A term that lives in two different notes on the same node is still one
/// hit on one field: `find` names `note` once per hit, never once per note
/// that happened to contain it.
#[test]
fn a_term_in_two_notes_prints_the_note_field_once() {
    let c = seeded("dup-note");
    c.ok(&["note", "t2", "the fixtures live under tests/data"]);
    c.ok(&["note", "t2", "a reminder that fixtures still live there"]);
    let s = c.ok(&["find", "fixtures"]);
    assert_eq!(
        s.matches("note:").count(),
        1,
        "the note field printed once per note instead of once per hit:\n{s}"
    );
}

/// What you look for months later is usually finished. A search that stopped
/// at the open fronts would be a to-do list, not a memory.
#[test]
fn a_closed_node_is_still_found() {
    let c = seeded("closed");
    c.ok(&["done", "t2", "the guard runs on every pull request"]);
    let s = c.ok(&["find", "malformed"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

#[test]
fn the_outcome_is_searched_too() {
    let c = seeded("outcome");
    c.ok(&["done", "t2", "it runs on every pull request now"]);
    let s = c.ok(&["find", "pull"]);
    assert!(s.contains("Guard the commit messages"), "{s}");
}

/// Every term has to appear. Otherwise a second word widens the search
/// instead of narrowing it, which is the opposite of what typing more means.
#[test]
fn every_term_has_to_appear() {
    let c = seeded("terms");
    let both = c.ok(&["find", "guard messages"]);
    assert!(both.contains("Guard the commit messages"), "{both}");
    let one_missing = c.ok(&["find", "guard bicycle"]);
    assert!(
        !one_missing.contains("Guard the commit messages"),
        "a term that appears nowhere still matched:\n{one_missing}"
    );
}

#[test]
fn case_does_not_matter() {
    let c = seeded("case");
    let s = c.ok(&["find", "SANDBOX"]);
    assert!(s.contains("The sandbox named its directory"), "{s}");
}

/// A hit with no lineage is a line of text. The whole product is the edge.
#[test]
fn the_lineage_travels_with_the_hit() {
    let c = seeded("lineage");
    let s = c.ok(&["find", "malformed"]);
    assert!(s.contains("via"), "no lineage:\n{s}");
    assert!(s.contains("g1"), "the lineage does not name the goal:\n{s}");
}

/// A result you cannot judge is noise: the hit says where it matched.
#[test]
fn it_says_where_the_hit_came_from() {
    let c = seeded("field");
    let s = c.ok(&["find", "malformed"]);
    assert!(s.contains("why:"), "it did not name the field:\n{s}");
    assert!(s.contains("malformed"), "it did not show the text:\n{s}");
}

#[test]
fn nothing_matching_says_so_and_is_not_an_error() {
    let c = seeded("empty");
    let (s, code) = c.run(&["find", "bicycle"]);
    assert_eq!(code, 0, "{s}");
    assert!(s.to_lowercase().contains("nothing"), "{s}");
}

#[test]
fn find_without_a_query_is_a_usage_error() {
    let c = seeded("usage");
    let (s, code) = c.run(&["find"]);
    assert_eq!(code, 2, "{s}");
    assert!(s.contains("usage"), "{s}");
}

#[test]
fn the_json_twin_carries_the_hits_and_says_where_they_matched() {
    let c = seeded("json");
    let s = c.ok(&["find", "malformed", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("the payload is not JSON");
    let hit = &v[0];
    assert!(hit["lineage"].is_array(), "{s}");
    let matched = hit["matched"]
        .as_object()
        .unwrap_or_else(|| panic!("matched is not an object, it is a list of field names: {s}"));
    let why = matched["why"]
        .as_str()
        .unwrap_or_else(|| panic!("matched has no why fragment: {s}"));
    assert!(why.contains("malformed"), "{s}");
}

/// A hit carries only the six fields a handle needs: `alias`, `kind`,
/// `state`, `title`, `lineage`, `matched`. Everything else -- `why`, `note`,
/// `outcome`, the twelve bookkeeping fields `json_node` also carries -- comes
/// from `why` on the alias, not from the hit itself.
#[test]
fn a_hit_has_exactly_the_six_handle_fields() {
    let c = seeded("keys");
    let s = c.ok(&["find", "malformed", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("the payload is not JSON");
    let hit = v[0].as_object().expect("a hit is not an object");
    let mut keys: Vec<&str> = hit.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["alias", "kind", "lineage", "matched", "state", "title"],
        "{s}"
    );
}

/// The JSON carries the fragment `snippet` produces, not the whole field. A
/// `why` far wider than `snippet`'s window, with a rare word at each end,
/// proves it: the word at the front is what the query hit, and the word at
/// the far end should never reach the payload.
#[test]
fn the_json_snippet_does_not_carry_the_whole_field() {
    let c = seeded("snippet");
    let padding = "filler word ".repeat(12);
    let long_why = format!("zzzfrontword {padding}zzzendword");
    c.ok(&[
        "add",
        "Something with a long reason",
        "--why",
        &long_why,
        "--type",
        "task",
    ]);
    let s = c.ok(&["find", "zzzfrontword", "--json"]);
    assert!(s.contains("zzzfrontword"), "{s}");
    assert!(!s.contains("zzzendword"), "{s}");
}

/// The title is not repeated as a reason line in the prose, but the JSON is
/// data, not rendering: a title hit has to name `title` inside `matched` or
/// an agent cannot tell a title hit from a `why` hit.
#[test]
fn a_title_hit_names_the_title_in_matched() {
    let c = seeded("title-match");
    let s = c.ok(&["find", "sandbox", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("the payload is not JSON");
    let hit = &v[0];
    assert_eq!(
        hit["title"],
        "The sandbox named its directory from the clock"
    );
    assert!(hit["matched"]["title"].is_string(), "{s}");
}

/// `find --everywhere` — the registry-wide fan-out (`d273`). `--everywhere`
/// reads the registry instead of the tree underfoot, so proving it needs
/// two or more projects sharing one `VIVAC_HOME`, which is what
/// `Sandbox::new_seeded_in` is for.
fn project_name(c: &Sandbox) -> String {
    c.0.file_name().unwrap().to_string_lossy().into_owned()
}

/// Pushes one node and registers the project.
///
/// Registration is a side effect of *using* a project (`t265`), and the
/// push that seeds a fresh store cannot trigger its own: at the moment it
/// runs, the event it is about to write is not on disk yet, so there is no
/// first event id to key the registry by. The `stack` after it is the
/// first command that finds one.
fn seed(c: &Sandbox, title: &str, why: &str) {
    c.ok(&["push", title, "--why", why]);
    c.ok(&["stack"]);
}

#[test]
fn a_query_matching_in_two_projects_shows_both_with_their_own_names() {
    let a = Sandbox::new_seeded("ew-both-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let b = Sandbox::new_seeded_in("ew-both-b", a.global_home());
    seed(&b, "Guard the release notes", "the version was a hand edit");
    let (name_a, name_b) = (project_name(&a), project_name(&b));

    let s = b.ok(&["find", "hand edit", "--everywhere"]);

    assert!(s.contains(&name_a), "{s}");
    assert!(s.contains(&name_b), "{s}");
    assert!(s.contains("Ship the release apparatus"), "{s}");
    assert!(s.contains("Guard the release notes"), "{s}");
}

#[test]
fn a_query_matching_in_one_project_only_prints_that_project() {
    let a = Sandbox::new_seeded("ew-one-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let b = Sandbox::new_seeded_in("ew-one-b", a.global_home());
    seed(
        &b,
        "Guard the commit messages",
        "a malformed one does not fail loudly",
    );
    let (name_a, name_b) = (project_name(&a), project_name(&b));

    let s = b.ok(&["find", "apparatus", "--everywhere"]);

    assert!(s.contains(&name_a), "{s}");
    assert!(
        !s.contains(&name_b),
        "the project with no hit was printed anyway:\n{s}"
    );
}

#[test]
fn everywhere_hits_carry_the_six_keys_plus_project() {
    let a = Sandbox::new_seeded("ew-keys-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );

    let s = a.ok(&["find", "apparatus", "--everywhere", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("the payload is not JSON");
    let hit = v[0].as_object().expect("a hit is not an object");
    let mut keys: Vec<&str> = hit.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["alias", "kind", "lineage", "matched", "project", "state", "title"],
        "{s}"
    );
}

#[test]
fn everywhere_project_is_the_bare_directory_name_with_no_separator() {
    let a = Sandbox::new_seeded("ew-name-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let name_a = project_name(&a);

    let s = a.ok(&["find", "apparatus", "--everywhere", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("the payload is not JSON");
    let project = v[0]["project"].as_str().expect("project is not a string");

    assert_eq!(project, name_a, "{s}");
    assert!(!project.contains('/'), "{s}");
    assert!(!project.contains('\\'), "{s}");
}

#[test]
fn a_vanished_root_is_reported_and_the_others_still_answer() {
    let a = Sandbox::new_seeded("ew-gone-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let b = Sandbox::new_seeded_in("ew-gone-b", a.global_home());
    seed(
        &b,
        "Guard the commit messages",
        "a malformed one does not fail loudly",
    );
    let name_a = project_name(&a);
    std::fs::remove_dir_all(&a.0).unwrap();

    let (s, code) = b.run(&["find", "commit", "--everywhere"]);

    assert_eq!(code, 0, "{s}");
    assert!(s.contains("Guard the commit messages"), "{s}");
    assert!(s.contains(&name_a), "the vanished root was not named:\n{s}");

    // `a`'s directory is already gone; `Sandbox::drop` swallows a second
    // attempt at removing it.
}

#[test]
fn an_empty_registry_says_nothing_matched() {
    let c = Sandbox::new_empty("ew-empty");

    let (s, code) = c.run(&["find", "anything", "--everywhere"]);

    assert_eq!(code, 0, "{s}");
    assert!(s.to_lowercase().contains("nothing"), "{s}");
}

#[test]
fn everywhere_works_from_a_directory_with_no_project_above_it() {
    let a = Sandbox::new_seeded("ew-nowhere-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let outside = Sandbox::new_empty_in("ew-nowhere-outside", a.global_home());
    assert!(!outside.0.join(".vivac").exists());

    let (s, code) = outside.run(&["find", "apparatus", "--everywhere"]);

    assert_eq!(code, 0, "{s}");
    assert!(s.contains("Ship the release apparatus"), "{s}");
}

/// The constraint `d273` exists to guard: searching from one project must
/// never write inside another project's `.vivac/`. `allow_persist: false`
/// is the mechanism; this is what would catch it slipping.
#[test]
fn the_foreign_index_is_never_written() {
    let a = Sandbox::new_seeded("ew-guard-a");
    a.ok(&[
        "push",
        "Ship the release apparatus",
        "--why",
        "the version was a hand edit",
    ]);
    // Forces `a`'s derived index to exist: a plain read persists it on the
    // first load, and `push` above only ever loaded for a write.
    a.ok(&["stack"]);
    let index_path = a.0.join(".vivac").join("index");
    let before = std::fs::metadata(&index_path).expect("no index to guard");

    let outside = Sandbox::new_empty_in("ew-guard-outside", a.global_home());
    let s = outside.ok(&["find", "apparatus", "--everywhere"]);
    assert!(s.contains("Ship the release apparatus"), "{s}");

    let after = std::fs::metadata(&index_path).expect("the index disappeared");
    assert_eq!(before.len(), after.len(), "the foreign index changed size");
    assert_eq!(
        before.modified().unwrap(),
        after.modified().unwrap(),
        "the foreign index was rewritten"
    );
}

// `d362` — hits sort by field first, subtree size second, and recency only
// as the last tiebreak. The five tests below defend the three positions of
// that tuple and the two places the order has to hold: the JSON payload and
// the `--everywhere` fan-out.

/// The field a hit lands in outranks *both* how much tree it holds up and
/// how recent it is -- not recency alone. The why-hit node is the one with
/// two children hung off it, so it also has the larger subtree, and it is
/// the older of the two; the title-hit node is a newer, childless sibling.
/// If the field did not dominate, the second key -- subtree size -- would
/// put the why-hit node first on its own, and this would not catch it.
#[test]
fn a_title_hit_wins_over_a_why_hit() {
    let c = Sandbox::new_seeded("rank-field");
    c.ok(&[
        "push",
        "Foundational topic",
        "--why",
        "the widget carries the whole argument here",
    ]);
    c.ok(&[
        "add",
        "Support child one",
        "--parent",
        "1",
        "--why",
        "just support",
    ]);
    c.ok(&[
        "add",
        "Support child two",
        "--parent",
        "1",
        "--why",
        "just support",
    ]);
    c.ok(&[
        "add",
        "Widget in the headline",
        "--parent",
        "1",
        "--why",
        "no relation at all",
        "--type",
        "task",
    ]);
    let s = c.ok(&["find", "widget"]);
    let title_hit = s
        .find("Widget in the headline")
        .expect("the title hit is missing");
    let why_hit = s
        .find("Foundational topic")
        .expect("the why hit is missing");
    assert!(
        title_hit < why_hit,
        "the title hit did not lead despite holding up less subtree and being newer:\n{s}"
    );
}

/// Within the same field, the node that holds up more tree wins over the
/// one that is merely newer. Two children are hung off the older node so
/// its subtree total is the larger of the two, and the newer, childless
/// node still has to come second.
#[test]
fn the_older_node_wins_with_more_subtree() {
    let c = Sandbox::new_seeded("rank-subtree");
    c.ok(&[
        "push",
        "Foundational item",
        "--why",
        "the gadget carries real weight",
    ]);
    c.ok(&[
        "add",
        "Support child one",
        "--parent",
        "1",
        "--why",
        "supporting detail",
    ]);
    c.ok(&[
        "add",
        "Support child two",
        "--parent",
        "1",
        "--why",
        "supporting detail",
    ]);
    c.ok(&[
        "add",
        "Fresh item",
        "--parent",
        "1",
        "--why",
        "a gadget appears here too",
        "--type",
        "task",
    ]);
    let s = c.ok(&["find", "gadget"]);
    let more_subtree = s
        .find("Foundational item")
        .expect("the more-subtree hit is missing");
    let leaf_hit = s.find("Fresh item").expect("the leaf hit is missing");
    assert!(
        more_subtree < leaf_hit,
        "the node with more subtree did not lead:\n{s}"
    );
}

/// With the field and the subtree total tied, the later node wins: today's
/// behaviour, degraded to the last tiebreak. The two matching nodes are
/// siblings, both leaves, so a subtree total of zero ties them and only
/// recency can tell them apart -- a parent and its own child never tie,
/// since a parent's subtree always holds more than the child's.
#[test]
fn the_later_node_wins_when_field_and_subtree_are_equal() {
    let c = Sandbox::new_seeded("rank-recency");
    c.ok(&["push", "Root node", "--why", "an unrelated reason"]);
    c.ok(&[
        "add",
        "Alpha node",
        "--parent",
        "1",
        "--why",
        "the gizmo is mentioned here",
        "--type",
        "task",
    ]);
    c.ok(&[
        "add",
        "Beta node",
        "--parent",
        "1",
        "--why",
        "the gizmo is mentioned again",
        "--type",
        "task",
    ]);
    let s = c.ok(&["find", "gizmo"]);
    let later = s.find("Beta node").expect("the later hit is missing");
    let older = s.find("Alpha node").expect("the older hit is missing");
    assert!(later < older, "the later node did not lead:\n{s}");
}

/// `find --json` is a second reader of the same order `hits_for` builds, not
/// a second implementation of it. A search that spreads across two fields
/// and two subtree sizes proves the two never drift apart.
#[test]
fn json_hits_are_in_the_same_order_as_the_text_hits() {
    let c = Sandbox::new_seeded("rank-json-order");
    c.ok(&["push", "Alpha owl mention", "--why", "an unrelated reason"]);
    c.ok(&[
        "add",
        "Beta node",
        "--why",
        "an owl shows up only here",
        "--type",
        "task",
    ]);
    c.ok(&[
        "add",
        "Owl in the title here",
        "--why",
        "another unrelated reason",
        "--type",
        "task",
    ]);
    let text = c.ok(&["find", "owl"]);
    let json_text = c.ok(&["find", "owl", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json_text).expect("the payload is not JSON");
    // Compared by title, not by alias: a hit's alias also shows up inside
    // the "via <alias>" lineage line of any other hit under the same
    // parent, so searching the text for a bare alias can land on someone
    // else's line. A title never does.
    let titles: Vec<String> = v
        .as_array()
        .expect("the payload is not a list")
        .iter()
        .map(|h| h["title"].as_str().expect("a hit has no title").to_string())
        .collect();
    assert!(
        titles.len() >= 3,
        "need at least three hits to prove an order:\n{text}"
    );
    let mut by_position: Vec<(usize, &String)> = titles
        .iter()
        .map(|title| {
            (
                text.find(title.as_str())
                    .unwrap_or_else(|| panic!("{title} is missing from the text output:\n{text}")),
                title,
            )
        })
        .collect();
    by_position.sort_by_key(|(pos, _)| *pos);
    let text_order: Vec<&String> = by_position.into_iter().map(|(_, title)| title).collect();
    let json_order: Vec<&String> = titles.iter().collect();
    assert_eq!(
        json_order, text_order,
        "json and text disagree on order:\ntext:\n{text}\njson:\n{json_text}"
    );
}

/// `find --everywhere` (`d273`) fans the same search out over the registry;
/// the ordering rule has to travel with it inside each project's own
/// section, not just on the single-project path. Same inversion as
/// [`a_title_hit_wins_over_a_why_hit`]: the why-hit node carries the larger
/// subtree and is the older of the two, and the title-hit node still has to
/// lead.
#[test]
fn find_everywhere_has_the_same_order_inside_each_project() {
    let a = Sandbox::new_seeded("rank-everywhere");
    a.ok(&[
        "push",
        "Foundational topic",
        "--why",
        "the crocodile carries the whole argument here",
    ]);
    a.ok(&[
        "add",
        "Support child one",
        "--parent",
        "1",
        "--why",
        "just support",
    ]);
    a.ok(&[
        "add",
        "Support child two",
        "--parent",
        "1",
        "--why",
        "just support",
    ]);
    a.ok(&[
        "add",
        "Crocodile in the headline",
        "--parent",
        "1",
        "--why",
        "no relation at all",
        "--type",
        "task",
    ]);
    a.ok(&["stack"]);
    let s = a.ok(&["find", "crocodile", "--everywhere"]);
    let title_hit = s
        .find("Crocodile in the headline")
        .expect("the title hit is missing");
    let why_hit = s
        .find("Foundational topic")
        .expect("the why hit is missing");
    assert!(
        title_hit < why_hit,
        "the title hit did not lead inside the project despite holding up less subtree and being newer:\n{s}"
    );
}
