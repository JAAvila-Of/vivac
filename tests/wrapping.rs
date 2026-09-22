//! Two clips that already existed in this crate and stopped at their first
//! call site.
//!
//! `d330` promised a clipped ancestor collapses to one wrapped line, and
//! `brief.rs`'s own `clip` already cuts on a word boundary. `f715` found the
//! first promise broken wherever a prefix sits in front of the clipped
//! body, because the clip was sized against the bare field and the prefix
//! was glued on after. `f716` found the second rule unapplied in `snippet`,
//! which still cut by raw character offset on both edges. Both are fixed
//! the same way the rest of this crate already worked; this file is what
//! keeps either from drifting back once a third call site shows up.

mod common;
use common::Sandbox;

/// Long enough that `why`'s ancestor clip always engages, with a tag at
/// each end so a truncated line reads unmistakably: the opening tag is
/// always inside the clip, the closing one never is.
const PADDING: &str = "filler word after filler word after filler word after filler word after filler word after filler word after filler word after filler word after";

fn long(tag: &str) -> String {
    format!("{tag}-open {PADDING} {tag}-close")
}

/// One ancestor whose why, two notes and outcome all run well past the clip
/// length, one level above a plain leaf. Two notes on purpose: the dated
/// form is the one whose prefix `f715` actually broke.
fn seeded_with_long_ancestor(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "An ancestor with a long body",
        "--why",
        &long("why"),
    ]);
    c.ok(&["note", "1", &long("first-note")]);
    c.ok(&["note", "1", &long("second-note")]);
    c.ok(&["done", "1", &long("outcome")]);
    c.ok(&[
        "add",
        "A leaf under the ancestor",
        "--parent",
        "1",
        "--why",
        "the leaf itself, whole either way",
    ]);
    c
}

/// `d330`'s own promise, read back as an assertion: a clipped ancestor
/// takes exactly one line. `f715` broke it for every field printed behind
/// a prefix -- the dated note worst of all, since its prefix alone left
/// less than half the clip's own width -- by measuring the clip against
/// the bare field and only gluing the prefix on afterwards. Each field's
/// line is found by the tag `long` put at its very start, which survives
/// any clip length this test exercises, and the next line has to be the
/// next field's own tag: anything else means the one before it ran on.
#[test]
fn a_clipped_ancestor_takes_one_line() {
    let c = seeded_with_long_ancestor("one-line");
    let s = c.ok(&["why", "2"]);
    let lines: Vec<&str> = s.lines().collect();

    let why_at = lines
        .iter()
        .position(|l| l.trim_start().starts_with("why-open"))
        .unwrap_or_else(|| panic!("the ancestor's why never printed:\n{s}"));
    assert!(
        lines[why_at].trim_end().ends_with("..."),
        "the why line was not long enough to clip in the first place:\n{s}"
    );

    let first_note_at = why_at + 1;
    assert!(
        lines[first_note_at].trim_start().starts_with("! ["),
        "the why ran on past a single line, so the first note is not right \
         behind it:\n{s}"
    );
    assert!(
        lines[first_note_at].trim_end().ends_with("..."),
        "the first note line was not long enough to clip in the first place:\n{s}"
    );

    let second_note_at = first_note_at + 1;
    assert!(
        lines[second_note_at].trim_start().starts_with("! ["),
        "the first note ran on past a single line, so the second note is \
         not right behind it:\n{s}"
    );
    assert!(
        lines[second_note_at].trim_end().ends_with("..."),
        "the second note line was not long enough to clip in the first place:\n{s}"
    );

    let outcome_at = second_note_at + 1;
    assert!(
        lines[outcome_at].trim_start().starts_with("= "),
        "the second note ran on past a single line, so the outcome is not \
         right behind it:\n{s}"
    );
    assert!(
        lines[outcome_at].trim_end().ends_with("..."),
        "the outcome line was not long enough to clip in the first place:\n{s}"
    );
}

/// Thirty-odd distinct filler words, each a different letter repeated to
/// its own length, so none of them can ever appear inside another and a
/// matched fragment can only have come from the position it was placed at.
const PADDING_WORDS: &str = "bb ccc dddd eeeee ffffff ggggggg hhhhhhhh iiiiiiiii \
jjjjjjjjjj kkkkkkkkkkk lllllllllll mmmmmmmmmmmm nnnnnnnnnnnnn ooooooooooooo";

fn why_text(term: &str) -> String {
    format!("{term} {PADDING_WORDS}")
}

fn note_text(term: &str) -> String {
    let words: Vec<&str> = PADDING_WORDS.split_whitespace().collect();
    let mid = words.len() / 2;
    format!(
        "{} {term} {}",
        words[..mid].join(" "),
        words[mid..].join(" ")
    )
}

fn outcome_text(term: &str) -> String {
    format!("{PADDING_WORDS} {term}")
}

/// `clip` in `brief.rs` already cuts on a word boundary and has its own
/// test; `snippet` in `render.rs` never applied it and cut by raw character
/// offset on both edges instead. The term is placed at the start of the
/// why, the middle of the note and the end of the outcome, so all three
/// edges a window can be asked to cut -- none, one side, or both -- get
/// exercised in one search.
#[test]
fn a_snippet_never_cuts_a_word_in_half() {
    let c = Sandbox::new_seeded("word-boundary");
    let term = "zzzneedle";
    let why = why_text(term);
    let note = note_text(term);
    let outcome = outcome_text(term);
    c.ok(&[
        "push",
        "A goal with fields long enough to clip",
        "--why",
        &why,
    ]);
    c.ok(&["note", "1", &note]);
    c.ok(&["done", "1", &outcome]);

    let s = c.ok(&["find", term, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap_or_else(|e| {
        panic!("find --json is not JSON: {e}\n{s}");
    });
    let matched = v[0]["matched"]
        .as_object()
        .unwrap_or_else(|| panic!("matched is not an object:\n{s}"));

    for (field, original) in [("why", &why), ("note", &note), ("outcome", &outcome)] {
        let snippet = matched
            .get(field)
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("no {field} fragment in the hit:\n{s}"));
        let without_prefix = snippet.strip_prefix("...").unwrap_or(snippet);
        let stripped = without_prefix.strip_suffix("...").unwrap_or(without_prefix);

        let at = original.find(stripped).unwrap_or_else(|| {
            panic!(
                "the {field} snippet, with its ... trimmed, is not a \
                 substring of the original field:\nsnippet={snippet:?}\n\
                 stripped={stripped:?}\noriginal={original:?}"
            )
        });
        let before = original[..at].chars().next_back();
        assert!(
            before.is_none_or(|c| c.is_whitespace()),
            "the {field} snippet cuts into a word on the left: {snippet:?}"
        );
        let after_at = at + stripped.len();
        let after = original[after_at..].chars().next();
        assert!(
            after.is_none_or(|c| c.is_whitespace()),
            "the {field} snippet cuts into a word on the right: {snippet:?}"
        );
        assert!(
            stripped.contains(term),
            "the {field} snippet lost the term that found it: {snippet:?}"
        );
    }
}
