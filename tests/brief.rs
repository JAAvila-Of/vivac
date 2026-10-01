//! The test contract from `BRIEF-SPEC.md` §10, against the real binary.
//!
//! No dependencies: `CARGO_BIN_EXE_vivac` comes from cargo, and the store is a
//! temporary directory. Every test seeds its own tree, because a shared one
//! would make execution order matter.

mod common;
use common::Sandbox;

/// A tree with one of everything, so that no section comes out empty.
fn populated(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "Migrate authentication to OIDC",
        "--why",
        "the old provider is shutting down",
    ]);
    c.ok(&[
        "add",
        "No dependencies under a copyleft licence",
        "--parent",
        "1",
        "--type",
        "constraint",
        "--why",
        "company policy",
    ]);
    c.ok(&[
        "push",
        "Pick a cache backend",
        "--why",
        "the token store needs one",
        "--governs",
        "src/cache/**",
    ]);
    c.ok(&[
        "decide",
        "Use a distributed token store",
        "--reason",
        "a single node will not hold",
        "--alternative",
        "JWT with no revocation",
    ]);
    c.ok(&[
        "add",
        "Does the token volume fit in one node?",
        "--parent",
        "3",
        "--type",
        "question",
        "--blocks",
        "--why",
        "it decides the backend",
    ]);
    c.ok(&[
        "add",
        "Update the integration tests",
        "--parent",
        "3",
        "--why",
        "the backend changes what has to be stood up",
    ]);
    c.ok(&[
        "park",
        "6",
        "the backend had to be decided before touching the tests",
    ]);
    c.ok(&[
        "flag",
        "4",
        "suspect",
        "--why",
        "it assumed Redis, and there is no Redis in staging",
    ]);
    c.ok(&[
        "save",
        "before touching the adapter",
        "--next",
        "extract the validator",
    ]);
    c
}

fn section(brief: &str, title: &str) -> bool {
    brief.lines().any(|l| l.trim() == title)
}

/// §10.1 — Same log, same `--now`, two runs, same bytes.
#[test]
fn determinism() {
    let c = populated("det");
    let a = c.ok(&["brief", "--now", "2026-09-15T10:00:00Z"]);
    let b = c.ok(&["brief", "--now", "2026-09-15T10:00:00Z"]);
    assert_eq!(a, b);
    assert!(a.contains("2026-09-15"), "--now overrides the clock:\n{a}");
}

// `the_header_names_the_founding_lane_main` used to live here: `t594`
// §5.1's golden case, a tree nobody ran `setup` in printing `main` for its
// one, undeclared, founding lane. `f721` removed the state its whole
// point depended on -- `d723` folded declaring the founding lane into
// every plant, bare or not, so a tree whose founding lane is not yet
// declared cannot exist any more. `tests/lanes.rs`'s own
// `each_folders_brief_names_its_own_lane_in_the_header` still covers what
// survives: the header carries the founding lane's own name, whatever it
// is declared as.

/// §10.2 — With the budget squeezed, the spine comes out whole and says so.
///
/// It is the hardest rule in the specification: if the spine does not fit, the
/// budget is wrong, not the brief. Without it the brief does not answer
/// question 1 and has no reason to exist.
#[test]
fn the_spine_is_never_truncated() {
    let c = populated("spine");
    let spine = |b: &str| {
        assert!(
            b.contains("Migrate authentication to OIDC"),
            "the root is missing:\n{b}"
        );
        assert!(
            b.contains("Pick a cache backend"),
            "the focus is missing:\n{b}"
        );
        assert!(b.contains("<== HERE"), "the marker is missing:\n{b}");
    };

    // Tight but reachable: it fits by trimming, and it says so.
    let b = c.ok(&["brief", "--budget", "200", "--now", "2026-09-15T10:00:00Z"]);
    spine(&b);
    assert!(b.contains("trimmed"), "it trimmed without saying so:\n{b}");

    // Impossible: it does not fit even with everything truncatable gone. The
    // spine comes out anyway, and the warning says what is left over is tree,
    // not render.
    let b = c.ok(&["brief", "--budget", "40", "--now", "2026-09-15T10:00:00Z"]);
    spine(&b);
    assert!(b.contains("over budget"), "it did not warn:\n{b}");
}

/// §10.3 — As the budget drops, sections fall from the bottom up, never
/// skipping.
#[test]
fn truncation_order() {
    let c = populated("trunc");
    let whole = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(section(&whole, "LAST VIVAC"), "{whole}");
    assert!(section(&whole, "DO NOT TOUCH NOW"), "{whole}");
    assert!(section(&whole, "FLAGGED"), "{whole}");

    // The vivac is section 9 and falls before 7 and 6.
    let tight = c.ok(&["brief", "--budget", "150", "--now", "2026-09-15T10:00:00Z"]);
    assert!(
        !section(&tight, "LAST VIVAC"),
        "it should have fallen:\n{tight}"
    );

    // And the non-truncatable ones hold: invariants and blocking questions.
    assert!(section(&tight, "INVARIANTS"), "{tight}");
    assert!(section(&tight, "BLOCKS"), "{tight}");
}

/// §10.5 — A superseded decision is never rendered.
#[test]
fn superseded_is_absent() {
    let c = populated("sup");
    c.ok(&[
        "decide",
        "Use database-backed sessions",
        "--reason",
        "simpler",
        "--supersedes",
        "4",
    ]);
    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(b.contains("Use database-backed sessions"), "{b}");
    assert!(
        !b.contains("Use a distributed token store"),
        "the superseded one is still there:\n{b}"
    );
}

/// §10.7 — An empty stack produces §8, never empty output.
#[test]
fn initial_state() {
    let c = Sandbox::new_seeded("initial");
    let b = c.ok(&["brief"]);
    assert!(b.contains("No active focus"), "{b}");
    assert!(b.contains("vivac push"), "no concrete action:\n{b}");

    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["park", "unfinished"]);
    let b = c.ok(&["brief"]);
    assert!(b.contains("No active focus"), "{b}");
    assert!(b.contains("OPEN GOALS") || b.contains("focus"), "{b}");
}

/// `f911`: a freshly planted tree tells the agent it does not yet hold what
/// the project knows, and names the skill that brings it in -- in the
/// brief the agent reads before its first write, not only in the terminal
/// `setup` closed in. Once anything is in the tree the lines go: by then
/// they are noise.
#[test]
fn an_empty_tree_names_the_migrate_skill_until_something_lands() {
    let c = Sandbox::new_seeded("empty-tree-migrate");
    let lines = " Empty tree: it does not yet hold what this project already knows.\n The vivac-migrate skill brings that in, after the person says yes.\n Start with:  vivac push \"<title>\" --why \"<reason>\"";
    let b = c.ok(&["brief"]);
    assert!(b.contains(lines), "{b}");
    let (h, code) = c.run_stdin(&["session", "start", "--hook"], "{}");
    assert_eq!(code, 0, "{h}");
    assert!(h.contains(lines), "the hook brief lost them:\n{h}");

    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let b = c.ok(&["brief"]);
    assert!(!b.contains("vivac-migrate"), "{b}");

    c.ok(&["park", "unfinished"]);
    let b = c.ok(&["brief"]);
    assert!(b.contains("No active focus."), "{b}");
    assert!(!b.contains("vivac-migrate"), "{b}");
}

/// §10.8 — No empty section emits a heading.
#[test]
fn no_hollow_headings() {
    let c = Sandbox::new_seeded("hollow");
    c.ok(&["push", "Alone", "--why", "nothing hangs off it"]);
    let b = c.ok(&["brief"]);
    for t in [
        "INVARIANTS",
        "BLOCKS",
        "DO NOT TOUCH NOW",
        "FLAGGED",
        "STANDING DECISIONS",
    ] {
        assert!(!section(&b, t), "{t} came out empty:\n{b}");
    }
}

/// `f48`: `BLOCKS` orders by alias number ascending, not by the text of the
/// formatted line. Sorting the lines would put `q10` ahead of `q2`, since
/// `'1' < '2'`.
#[test]
fn blocks_are_ordered_by_alias_number_not_by_text() {
    let c = Sandbox::new_seeded("blocks-order");
    c.ok(&["push", "Root", "--why", "seed"]);
    c.ok(&[
        "add",
        "First blocker",
        "--parent",
        "1",
        "--type",
        "question",
        "--blocks",
        "--why",
        "it decides the first thing",
    ]);
    for i in 0..7 {
        c.ok(&[
            "add",
            &format!("Filler {i}"),
            "--parent",
            "1",
            "--why",
            "filler",
        ]);
    }
    c.ok(&[
        "add",
        "Second blocker",
        "--parent",
        "1",
        "--type",
        "question",
        "--blocks",
        "--why",
        "it decides the second thing",
    ]);

    let b = c.ok(&["brief"]);
    let first = b.find("First blocker").expect("q2 is missing");
    let second = b.find("Second blocker").expect("q10 is missing");
    assert!(first < second, "q10 sorted ahead of q2:\n{b}");
}

/// `f61`: a parked node's lines never get split by the truncation, and what
/// is left out is counted in nodes, not in the lines they cost. Four parked
/// nodes with an outcome each cost two lines apiece; against the six-line
/// ceiling, three come out whole -- the fourth would push the total to
/// eight -- and the notice says "1 more", never "2 more".
#[test]
fn do_not_touch_now_trims_by_whole_nodes() {
    let c = Sandbox::new_seeded("parked-trim");
    c.ok(&["push", "Root", "--why", "seed"]);
    for i in 1..=4 {
        c.ok(&[
            "add",
            &format!("Parked {i}"),
            "--parent",
            "1",
            "--why",
            "later",
        ]);
    }
    for i in 0..4 {
        let node = (2 + i).to_string();
        c.ok(&["park", &node, &format!("reason {i}")]);
    }

    let b = c.ok(&["brief"]);
    let heading_at = b.find("DO NOT TOUCH NOW").expect("the section is missing");
    let end = b[heading_at..]
        .find("\n\n")
        .map(|i| heading_at + i)
        .unwrap_or(b.len());
    let block = &b[heading_at..end];
    assert!(block.contains("Parked 1"), "{block}");
    assert!(block.contains("Parked 2"), "{block}");
    assert!(block.contains("Parked 3"), "{block}");
    assert!(
        !block.contains("Parked 4"),
        "the fourth node should have been left out whole:\n{block}"
    );
    assert!(
        !block.contains("reason 3"),
        "the fourth node's own outcome leaked without its title:\n{block}"
    );
    assert!(
        block.contains("and 1 more (vivac parked)"),
        "the count should be nodes, not the lines they cost:\n{block}"
    );
}

/// §10.9 — No flag is rendered without its reason, because it cannot be raised
/// without one.
#[test]
fn reason_is_mandatory() {
    let c = Sandbox::new_seeded("reason");
    c.ok(&["push", "Something", "--why", "it is needed"]);
    let (s, code) = c.run(&["flag", "1", "suspect"]);
    assert_eq!(code, 2, "a flag with no reason has to fail:\n{s}");
    assert!(s.contains("--why"), "{s}");
}

/// §10.6 — With no version control there are no diff lines, and it says so.
#[test]
fn degradation_without_an_anchor() {
    let c = populated("null");
    let s = c.ok(&["restore", "v1"]);
    assert!(s.contains("No anchor"), "{s}");
    assert!(!s.contains("changes since"), "it invented a diff:\n{s}");
}

/// The redaction guard holds here too: there is no back door through `decide`
/// or through `flag`.
#[test]
fn the_guard_covers_the_new_operations() {
    let c = Sandbox::new_seeded("guard");
    c.ok(&["push", "Something", "--why", "it is needed"]);
    let (_, code) = c.run(&[
        "decide",
        "Rotate",
        "--reason",
        "use ghp_16C7e42F292c6912E7710c838347Ae178B4a",
    ]);
    assert_eq!(code, 3, "decide let a credential through");
    let (_, code) = c.run(&["flag", "1", "review", "--why", "see /home/someone/.config"]);
    assert_eq!(code, 3, "flag let a personal path through");
}

/// `f30` — a standing decision is not a pending child. It shows up in its own
/// section and nowhere else: listing it twice fills the brief with things not
/// to do, which is the opposite of what it exists for.
#[test]
fn a_decision_is_not_a_front() {
    let c = populated("dec");
    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert_eq!(
        b.matches("Use a distributed token store").count(),
        1,
        "the decision shows up more than once:\n{b}"
    );

    // And that single time is under STANDING DECISIONS, not BORN FROM HERE.
    let heading_at = b
        .find("STANDING DECISIONS")
        .expect("the section is missing");
    assert!(
        b.find("Use a distributed token store").unwrap() > heading_at,
        "it shows up before its section, i.e. as a pending child:\n{b}"
    );

    let o = c.ok(&["open"]);
    assert!(
        !o.contains("Use a distributed token store"),
        "open lists it as a front:\n{o}"
    );
    assert!(
        o.contains("1 standing decision"),
        "open made it vanish without saying so:\n{o}"
    );
}

/// `q26` — closing a parent cannot make its open children invisible.
///
/// The case came out of the project's own tree: `t8` closed with `t9`, `t10`
/// and `f21` open below it, and the brief showed 3 of the 6 fronts. Listing
/// them would drag in the whole tree; counting them does not.
#[test]
fn what_is_open_under_a_closed_node_gets_counted() {
    let c = Sandbox::new_seeded("deep");
    c.ok(&["push", "The goal", "--why", "it is needed"]);
    c.ok(&["push", "A branch", "--why", "the goal needs it"]);
    c.ok(&[
        "add",
        "A finding",
        "--parent",
        "2",
        "--why",
        "spotted along the way",
    ]);
    // Closes with an open child that does not block: correct, and the case.
    c.ok(&["pop", "branch finished"]);

    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(
        b.contains("+ 1 further down"),
        "it did not warn about what was left below:\n{b}"
    );
    assert!(
        !b.contains("A finding"),
        "it listed it instead of counting it; that drags in the whole tree:\n{b}"
    );
    assert!(
        b.contains("vivac open"),
        "it counted without saying where to look:\n{b}"
    );
}

/// A decision that governs the whole product hangs off nothing, so it is on no
/// path and used to reach no brief. The invariants section had the
/// project-level clause and the decisions section did not, which was an
/// asymmetry rather than a choice -- and it hid exactly the decisions that
/// matter most, the ones that are not about one branch of the work.
#[test]
fn a_project_level_decision_reaches_the_brief() {
    let c = Sandbox::new_seeded("project-level");
    // Nothing on the stack yet, so it is born with no parent at all: it is
    // about the product and not about one branch of it.
    c.ok(&[
        "decide",
        "Keys never live in the tree",
        "--reason",
        "the tree maps where the system is weak",
    ]);
    c.ok(&[
        "push",
        "Migrate to OIDC",
        "--why",
        "the provider is shutting down",
    ]);

    let s = c.ok(&["brief"]);
    assert!(
        s.contains("STANDING DECISIONS"),
        "no section at all:
{s}"
    );
    assert!(
        s.contains("Keys never live in the tree"),
        "the decision that governs everything went missing:
{s}"
    );
}

/// `t533` piece (b) and `d536`, with two roots: the focus sits under the
/// second one, and a decision or a parked node hanging off the first still
/// reaches the brief -- project level does not mean "on this path". `BLOCKS`
/// stays by lineage: a blocking question under the first root is not this
/// branch's problem.
#[test]
fn standing_decisions_and_parked_are_project_wide_with_two_roots() {
    let c = Sandbox::new_seeded("two-roots");
    c.ok(&["push", "First goal", "--why", "the original branch"]);
    c.ok(&[
        "decide",
        "Old approach",
        "--reason",
        "settled early on",
        "--parent",
        "1",
    ]);
    c.ok(&[
        "add",
        "Parked under the first root",
        "--parent",
        "1",
        "--why",
        "stuck on something else",
    ]);
    c.ok(&["park", "3", "waiting on the other branch"]);
    c.ok(&[
        "add",
        "Blocking under the first root",
        "--parent",
        "1",
        "--type",
        "question",
        "--blocks",
        "--why",
        "it decides how the first branch continues",
    ]);
    c.ok(&["push", "Second goal", "--why", "a fresh branch", "--root"]);

    let b = c.ok(&["brief"]);
    assert!(b.contains("STANDING DECISIONS"), "{b}");
    assert!(b.contains("Old approach"), "{b}");
    assert!(b.contains("DO NOT TOUCH NOW"), "{b}");
    assert!(b.contains("Parked under the first root"), "{b}");
    assert!(
        !b.contains("Blocking under the first root"),
        "a question under the other root blocked this branch:\n{b}"
    );
}

/// `t533` piece (c): an ancestor that has become superseded carries its mark
/// on the spine, unclipped even past a long title, and a closed node that is
/// still the focus carries its own mark before `<== HERE`.
#[test]
fn the_spine_marks_closed_and_superseded_nodes() {
    let c = Sandbox::new_seeded("spine-marks");
    let long_title =
        "A decision whose title is deliberately long enough to need clipping on the spine";
    c.ok(&[
        "push",
        long_title,
        "--why",
        "the call being made",
        "--type",
        "decision",
    ]);
    c.ok(&["push", "Middle step", "--why", "work under the decision"]);
    c.ok(&["push", "Leaf step", "--why", "one more level"]);
    c.ok(&[
        "decide",
        "Replacement decision",
        "--reason",
        "the old one did not hold",
        "--supersedes",
        "1",
    ]);
    c.ok(&["done", "2", "settled early"]);
    c.ok(&["pop", "leaf done"]);

    let b = c.ok(&["brief"]);
    assert!(
        !b.contains(long_title),
        "the long title was not clipped:\n{b}"
    );
    assert!(b.contains("[superseded]"), "{b}");
    assert!(
        b.contains("[closed]   <== HERE"),
        "the closed focus lost its mark before <== HERE:\n{b}"
    );
}

/// `f49`: a blocking question born under the focus is left out of `BORN FROM
/// HERE` -- `BLOCKS` already lists it -- while a blocking task still shows
/// there, asterisk and all.
#[test]
fn a_blocking_question_under_the_focus_appears_only_in_blocks() {
    let c = Sandbox::new_seeded("blocking-question-once");
    c.ok(&["push", "Goal", "--why", "the branch to work on"]);
    c.ok(&[
        "add",
        "Blocking task",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "still has to close",
    ]);
    c.ok(&[
        "add",
        "Blocking question",
        "--parent",
        "1",
        "--type",
        "question",
        "--blocks",
        "--why",
        "it decides how this continues",
    ]);

    let b = c.ok(&["brief"]);
    assert_eq!(
        b.matches("Blocking question").count(),
        1,
        "it shows up more than once:\n{b}"
    );
    let born_at = b.find("BORN FROM HERE").expect("the section is missing");
    let blocks_at = b.find("BLOCKS").expect("the section is missing");
    assert!(
        b.find("Blocking question").unwrap() > blocks_at,
        "the question shows up before BLOCKS, i.e. in BORN FROM HERE:\n{b}"
    );
    let task_at = b.find("Blocking task").expect("the task is missing");
    assert!(
        task_at > born_at && task_at < blocks_at,
        "the blocking task should still be in BORN FROM HERE:\n{b}"
    );
    assert!(b.contains("* t2"), "the task lost its asterisk:\n{b}");
}

/// `t533` §3.6, with no focus: the project-level sections still reach the
/// brief, and `OPEN GOALS` leaves out a root decision and a root rule.
#[test]
fn no_focus_open_goals_skips_governance_kind() {
    let c = Sandbox::new_seeded("no-focus-sections");
    c.ok(&[
        "add",
        "No secrets in the tree",
        "--type",
        "constraint",
        "--why",
        "the security pillar",
    ]);
    c.ok(&["decide", "Keep it simple", "--reason", "fewer moving parts"]);
    c.ok(&["add", "A root rule", "--type", "rule", "--why", "governs"]);
    c.ok(&["add", "A goal to park", "--type", "goal", "--why", "later"]);
    c.ok(&["park", "4", "not ready yet"]);
    c.ok(&["add", "Second open goal", "--type", "goal", "--why", "next"]);
    c.ok(&["save", "checkpoint", "--next", "keep going"]);

    let b = c.ok(&["brief"]);
    assert!(b.contains("No active focus."), "{b}");
    assert!(b.contains("INVARIANTS"), "{b}");
    assert!(b.contains("No secrets in the tree"), "{b}");
    assert!(b.contains("STANDING DECISIONS"), "{b}");
    assert!(b.contains("Keep it simple"), "{b}");
    assert!(b.contains("DO NOT TOUCH NOW"), "{b}");
    assert!(b.contains("A goal to park"), "{b}");
    assert!(b.contains("LAST VIVAC"), "{b}");
    assert!(b.contains("you were about to: keep going"), "{b}");

    let open_goals_at = b.find("OPEN GOALS").expect("the section is missing");
    let block_end = b[open_goals_at..]
        .find("\n\n")
        .map(|i| open_goals_at + i)
        .unwrap_or(b.len());
    let block = &b[open_goals_at..block_end];
    assert!(block.contains("Second open goal"), "{block}");
    assert!(
        !block.contains("Keep it simple"),
        "a root decision leaked into OPEN GOALS:\n{block}"
    );
    assert!(
        !block.contains("A root rule"),
        "a root rule leaked into OPEN GOALS:\n{block}"
    );
    assert!(b.contains("Pick up with:  vivac focus g5"), "{b}");
}

/// With every goal-shaped root parked, the action names the first one parked
/// rather than saying there is nothing to pick up.
#[test]
fn no_focus_falls_back_to_a_parked_candidate() {
    let c = Sandbox::new_seeded("no-focus-parked-fallback");
    c.ok(&["push", "First goal", "--why", "it came first"]);
    c.ok(&["park", "1", "stuck for now"]);
    c.ok(&["push", "Second goal", "--why", "it came second", "--root"]);
    c.ok(&["park", "2", "stuck too"]);

    let b = c.ok(&["brief"]);
    assert!(!b.contains("OPEN GOALS"), "{b}");
    assert!(b.contains("Pick up with:  vivac focus g1"), "{b}");
    assert!(b.contains("Or open another:"), "{b}");
}

/// With nothing open and nothing parked to point at, the action opens the
/// next one instead of naming a node that does not exist.
#[test]
fn no_focus_says_open_the_next_one_when_everything_is_closed() {
    let c = Sandbox::new_seeded("no-focus-open-next");
    c.ok(&["push", "First goal", "--why", "it came first"]);
    c.ok(&["pop", "wrapped up"]);

    let b = c.ok(&["brief"]);
    assert!(!b.contains("OPEN GOALS"), "{b}");
    assert!(b.contains("Open the next one:  vivac push"), "{b}");
}

/// `t594` §4.7: everything else the brief says about lineage can already be
/// a lie the moment this folder is a copy, so the warning has to come out
/// ahead of it -- the very first line, not merely somewhere in the output.
/// Checked in both folders: the registry only ever keeps one `path`, and
/// whichever folder is not on it learns from `copies` instead, so a check
/// that only covered one side would miss the other folder's own brief
/// going without the warning.
#[test]
fn the_brief_of_a_copy_opens_with_the_warning() {
    let original = Sandbox::new_seeded("brief-copy-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = Sandbox::new_empty_in("brief-copy-copy", original.global_home());
    std::fs::create_dir_all(copy.0.join(".vivac")).unwrap();
    std::fs::copy(
        original.0.join(".vivac").join("events"),
        copy.0.join(".vivac").join("events"),
    )
    .unwrap();

    let copy_brief = copy.ok(&["brief"]);
    assert_eq!(
        copy_brief.lines().next().unwrap_or("").trim(),
        "COPY OF ANOTHER TREE",
        "the warning did not open the copy's own brief:\n{copy_brief}"
    );

    let original_brief = original.ok(&["brief"]);
    assert_eq!(
        original_brief.lines().next().unwrap_or("").trim(),
        "COPY OF ANOTHER TREE",
        "the original's own brief did not open with the warning either:\n{original_brief}"
    );
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with one commit, at `dir`.
fn commit_a_repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "first"]);
}

/// The line right after the `LAST VIVAC` heading.
fn last_vivac_line(out: &str) -> &str {
    let mut lines = out.lines();
    while let Some(l) = lines.next() {
        if l.trim() == "LAST VIVAC" {
            return lines.next().unwrap_or("");
        }
    }
    panic!("no LAST VIVAC section:\n{out}")
}

/// Every line of the `LAST VIVAC` body, heading excluded: everything from
/// right after the heading up to the blank line that either closes the
/// brief or opens the next section.
fn last_vivac_block(out: &str) -> Vec<&str> {
    let mut lines = out.lines();
    for l in &mut lines {
        if l.trim() == "LAST VIVAC" {
            break;
        }
    }
    lines.take_while(|l| !l.trim().is_empty()).collect()
}

// ---------------------------------------------------------------------------
// `LAST VIVAC`'s label and intent (`f67`, `f64`, `d652`): the label shown is
// always the one belonging to whichever stop's intent is being quoted, and a
// hook's automatic stop -- which never carries an intent on purpose -- must
// not blank out what an earlier manual one said.
// ---------------------------------------------------------------------------

/// A manual stop with both a label and an intent shows both, and the
/// intent reads "you were about to" because it is the same stop named on
/// line 1.
#[test]
fn last_vivac_shows_its_own_label_and_intent() {
    let c = Sandbox::new_seeded("vivac-label-intent");
    c.ok(&["push", "Root", "--why", "seed"]);
    c.ok(&["save", "packing up", "--next", "ship it"]);

    let b = c.ok(&["brief"]);
    assert!(last_vivac_line(&b).contains("manual"), "{b}");
    assert!(b.contains("\"packing up\""), "{b}");
    assert!(b.contains("you were about to: ship it"), "{b}");
}

/// An automatic stop right behind a manual one names itself on line 1, with
/// its own date, but the label and the intent quoted are the manual stop's
/// own -- an automatic stop never carries an intent on purpose (`f59`), and
/// blanking out what the manual one said would erase it for no reason.
#[test]
fn an_automatic_stop_does_not_blank_out_the_earlier_intent() {
    let c = Sandbox::new_seeded("vivac-auto-blank");
    c.ok(&["push", "Root", "--why", "seed"]);
    c.ok(&["save", "packing up", "--next", "ship it"]);
    c.ok(&["note", "1", "something happened"]);
    c.ok(&["session", "end", "--hook"]);

    let b = c.ok(&["brief"]);
    let line = last_vivac_line(&b);
    assert!(line.starts_with("  v3 "), "{b}");
    assert!(line.contains("auto"), "{b}");
    assert!(b.contains("\"packing up\""), "{b}");
    assert!(b.contains("v2 was about to: ship it"), "{b}");
    assert!(
        !b.contains("you were about to"),
        "the automatic stop is not the one that spoke:\n{b}"
    );
}

/// No label at all, and the line that carries it is left out entirely.
#[test]
fn last_vivac_with_no_label_omits_the_label_line() {
    let c = Sandbox::new_seeded("vivac-no-label");
    c.ok(&["push", "Root", "--why", "seed"]);

    let b = c.ok(&["brief"]);
    let block = last_vivac_block(&b);
    assert_eq!(block.len(), 2, "{block:?}");
    assert!(!block[1].contains('"'), "{block:?}");
    assert!(block[1].contains("you were about to: Root"), "{block:?}");
}

/// No stop of this lane ever carried an intent: there is no intent line,
/// and the label shown is the last stop's own.
#[test]
fn no_intent_anywhere_still_shows_the_last_stops_label() {
    let c = Sandbox::new_seeded("vivac-no-intent");
    c.ok(&["save", "wrap up"]);

    let b = c.ok(&["brief"]);
    let block = last_vivac_block(&b);
    assert_eq!(block.len(), 2, "{block:?}");
    assert!(block[1].contains("\"wrap up\""), "{block:?}");
    assert!(!b.contains("was about to"), "{b}");
}

/// The backward search never leaves the lane: a stop with an intent in
/// another lane, even a more recent one, is not the one cited.
#[test]
fn the_search_for_an_intent_does_not_cross_lanes() {
    let a = Sandbox::new_seeded("vivac-lane-a");
    a.ok(&["push", "Root A", "--why", "seed"]);
    a.ok(&["save", "label A", "--next", "handle A"]);

    let b = join_lane(&a, "vivac-lane-b", "other");
    b.ok(&["push", "Root B", "--why", "seed"]);
    b.ok(&["save", "label B", "--next", "handle B"]);

    // An automatic stop in lane A, so the search has to step back past it.
    a.ok(&["note", "1", "something happened"]);
    a.ok(&["session", "end", "--hook"]);

    let out = a.ok(&["brief"]);
    assert!(out.contains("\"label A\""), "{out}");
    assert!(out.contains("v2 was about to: handle A"), "{out}");
    assert!(!out.contains("handle B"), "{out}");
    assert!(!out.contains("Root B"), "{out}");
}

/// `f914`, `d915`: a manual stop with no `--next` has said there is nothing
/// to pick up -- `save` says so out loud -- so the brief does not reach
/// back past it for an older intent. Only an automatic stop is stepped
/// over.
#[test]
fn a_manual_stop_with_no_next_is_not_stepped_over() {
    let c = Sandbox::new_seeded("vivac-manual-no-next");
    c.ok(&["push", "Root", "--why", "seed"]);
    c.ok(&["save", "one", "--next", "write the parser"]);
    c.ok(&["save", "two"]);

    let b = c.ok(&["brief"]);
    let block = last_vivac_block(&b);
    assert!(block[0].starts_with("  v3 "), "{block:?}");
    assert!(block[1].contains("\"two\""), "{block:?}");
    assert!(!b.contains("write the parser"), "{b}");
    assert!(!b.contains("was about to"), "{b}");
}

/// `f910`, `d915`: a pop's outcome is what was finished, not what comes
/// next. Without `--next` the pop leaves no intent, and the brief quotes
/// neither the outcome nor the push that opened the node just closed.
#[test]
fn a_pop_with_no_next_leaves_nothing_to_pick_up() {
    let c = Sandbox::new_seeded("vivac-pop-no-next");
    c.ok(&["push", "Root", "--why", "seed"]);
    c.ok(&["push", "Write the parser", "--why", "needed"]);
    c.ok(&["pop", "Parser written and tested"]);

    let b = c.ok(&["brief"]);
    let block = last_vivac_block(&b);
    assert!(block[0].contains("pop"), "{block:?}");
    assert!(!b.contains("Parser written and tested"), "{b}");
    assert!(!b.contains("was about to"), "{b}");

    c.ok(&["push", "Write the lexer", "--why", "needed"]);
    c.ok(&["pop", "Lexer written", "--next", "wire both into the CLI"]);
    let b = c.ok(&["brief"]);
    assert!(
        b.contains("you were about to: wire both into the CLI"),
        "{b}"
    );
}

/// `t594` task 4, step 5 (§4.4): a lone repository keeps the short sha it
/// always showed, and only two or more collapse the line to a count.
#[test]
fn the_last_stop_shows_one_short_sha_and_counts_the_rest() {
    // One repository reads exactly as it did before: a tree with a single
    // repository must not notice this tranche happened.
    let one = Sandbox::new_empty("brief-repos-one");
    commit_a_repo(&one.0);
    // `init` writes only under `.vivac/`, which the repository ignores, so
    // the working tree stays exactly as clean as `commit_a_repo` left it:
    // the stop's anchor is the repository's own `HEAD`.
    one.ok(&["init", "--yes"]);
    one.ok(&["push", "Something", "--why", "seed"]);
    one.ok(&["save", "checkpoint"]);
    let line = last_vivac_line(&one.ok(&["brief"])).to_string();
    assert!(!line.contains("repos"), "{line}");
    let sha = line.rsplit(" · ").next().unwrap_or("");
    assert_eq!(sha.len(), 7, "{line}");
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "{line}");

    // Two or more repositories: the sha collapses to a count.
    let two = Sandbox::new_empty("brief-repos-two");
    commit_a_repo(&two.0.join("webapi"));
    commit_a_repo(&two.0.join("infra"));
    two.ok(&["init", "--yes"]);
    two.ok(&["push", "Something", "--why", "seed"]);
    two.ok(&["save", "checkpoint"]);
    let out = two.ok(&["brief"]);
    assert!(last_vivac_line(&out).contains(" · 2 repos"), "{out}");
}

/// `t594` task 1 (`d625`, closes `f623`): two git processes cost 43 ms and
/// 46 ms in a one-file repository, and the whole read ceiling for `brief` is
/// 50 ms -- git alone doubled the budget. `vivac changes` and `vivac
/// restore` answer "what changed since" on demand; `brief` never asks.
#[test]
fn the_brief_does_not_spawn_a_process() {
    // The cheapest honest test is that the line the process paid for is
    // gone. A genuine uncommitted change against the stop's own anchor is
    // what used to make the old block print it, so this fails against that
    // block still being there, not merely against the words having moved.
    let c = Sandbox::new_empty("brief-no-process");
    commit_a_repo(&c.0);
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "Something", "--why", "seed"]);
    c.ok(&["save", "checkpoint"]);
    std::fs::write(c.0.join("f.txt"), "changed after the stop").unwrap();

    let out = c.ok(&["brief"]);
    assert!(!out.contains("changes since"), "{out}");
    assert!(!out.contains("touching what it governs"), "{out}");
}

// ---------------------------------------------------------------------------
// `t594` task 6: BRANCH MOVED (§5.2), the brief's own reader of `Tree.wheres`
// and the BRANCH MOVED candidate tables the previous tasks of this tranche
// built and left unread.
// ---------------------------------------------------------------------------

/// A repository with one commit, on `branch` rather than whatever the
/// local `git` calls its default.
fn commit_a_repo_on_branch(dir: &std::path::Path, branch: &str) {
    commit_a_repo(dir);
    git(dir, &["checkout", "-q", "-b", branch]);
}

/// The BRANCH MOVED block, header through the blank line that ends it,
/// header included. Panics if the section is absent -- every test that
/// calls this expects to find it.
fn branch_moved_block(out: &str) -> Vec<&str> {
    let mut lines = out.lines();
    for l in lines.by_ref() {
        if l == " BRANCH MOVED since this lane last wrote" {
            let mut block = vec![l];
            for rest in lines.by_ref() {
                if rest.is_empty() {
                    break;
                }
                block.push(rest);
            }
            return block;
        }
    }
    panic!("no BRANCH MOVED section:\n{out}")
}

/// The literal shape of §6.11: one line for the repository that moved, its
/// own last focus on the branch it reads now, and `to resume` because that
/// candidate is the only one.
#[test]
fn the_brief_says_the_branch_moved_and_where_that_branch_last_stopped() {
    let c = Sandbox::new_empty("branch-moved-shape");
    let backend = c.0.join("backend");
    commit_a_repo_on_branch(&backend, "feature/net10");
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "First task", "--why", "seed"]);

    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);
    c.ok(&["push", "Optimize the SP", "--why", "seed"]);

    // Back on the branch the lane worked on first: the lane's own last
    // `where.changed` is still `perf/sp`, the branch the second push recorded.
    git(&backend, &["checkout", "-q", "feature/net10"]);

    let out = c.ok(&["brief"]);
    assert_eq!(
        branch_moved_block(&out),
        vec![
            " BRANCH MOVED since this lane last wrote",
            "   backend   perf/sp -> feature/net10",
            "   last focus on feature/net10:   g1   First task",
            "   to resume:  vivac focus g1",
        ],
        "{out}"
    );
}

/// `d976` (`f974`): the branch's last focus was closed after the lane left
/// it. The line still says where the lane stopped there, marked closed, and
/// `to resume` offers the nearest open node above it, which is where a pop
/// would have left the stack -- never the closed one, which `vivac focus`
/// refuses without `--reopen`.
#[test]
fn a_last_focus_closed_since_offers_the_nearest_open_node_above_it() {
    let c = Sandbox::new_empty("branch-moved-closed");
    let backend = c.0.join("backend");
    commit_a_repo_on_branch(&backend, "feature/net10");
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "First task", "--why", "seed"]);
    c.ok(&["push", "Sub task", "--why", "seed"]);

    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);
    c.ok(&["pop", "finished on the other branch"]);
    git(&backend, &["checkout", "-q", "feature/net10"]);

    let out = c.ok(&["brief"]);
    assert_eq!(
        branch_moved_block(&out),
        vec![
            " BRANCH MOVED since this lane last wrote",
            "   backend   perf/sp -> feature/net10",
            "   last focus on feature/net10:   t2   Sub task  [closed]",
            "   to resume:  vivac focus g1   First task",
        ],
        "{out}"
    );
}

/// `d976`: a parked last focus is offered no more than a closed one -- the
/// same brief lists it under DO NOT TOUCH NOW.
#[test]
fn a_last_focus_parked_since_is_not_offered_to_go_back_to() {
    let c = Sandbox::new_empty("branch-moved-parked");
    let backend = c.0.join("backend");
    commit_a_repo_on_branch(&backend, "feature/net10");
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "First task", "--why", "seed"]);
    c.ok(&["push", "Sub task", "--why", "seed"]);

    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);
    c.ok(&["park", "not now"]);
    git(&backend, &["checkout", "-q", "feature/net10"]);

    let out = c.ok(&["brief"]);
    assert_eq!(
        branch_moved_block(&out),
        vec![
            " BRANCH MOVED since this lane last wrote",
            "   backend   perf/sp -> feature/net10",
            "   last focus on feature/net10:   t2   Sub task  [parked]",
            "   to resume:  vivac focus g1   First task",
        ],
        "{out}"
    );
}

/// `d976`: with nothing open left on the way up there is nowhere to resume,
/// and `to resume` is left out, as it is when there is no candidate at all.
#[test]
fn a_last_focus_with_nothing_open_above_offers_nothing_to_go_back_to() {
    let c = Sandbox::new_empty("branch-moved-all-closed");
    let backend = c.0.join("backend");
    commit_a_repo_on_branch(&backend, "feature/net10");
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "First task", "--why", "seed"]);

    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);
    c.ok(&["pop", "finished on the other branch"]);
    git(&backend, &["checkout", "-q", "feature/net10"]);

    let out = c.ok(&["brief"]);
    assert_eq!(
        branch_moved_block(&out),
        vec![
            " BRANCH MOVED since this lane last wrote",
            "   backend   perf/sp -> feature/net10",
            "   last focus on feature/net10:   g1   First task  [achieved]",
        ],
        "{out}"
    );
    assert!(!out.contains("to resume"), "{out}");
}

/// Once the lane's own `where.changed` matches the branch again, the notice
/// is gone -- it compares against the last thing the lane wrote, not
/// against history in general.
#[test]
fn going_back_to_the_branch_makes_the_notice_disappear() {
    let c = Sandbox::new_empty("branch-moved-disappear");
    let backend = c.0.join("backend");
    commit_a_repo_on_branch(&backend, "feature/net10");
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "First task", "--why", "seed"]);

    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);
    let moved = c.ok(&["brief"]);
    assert!(moved.contains("BRANCH MOVED"), "{moved}");

    git(&backend, &["checkout", "-q", "feature/net10"]);
    let back = c.ok(&["brief"]);
    assert!(
        !back.contains("BRANCH MOVED"),
        "back on the branch the lane last wrote, the notice should be gone:\n{back}"
    );
}

/// No candidate at all: the branch reads `no earlier work on <branch>`
/// rather than inventing one, and `to resume` never appears with nothing
/// to resume to.
#[test]
fn a_branch_nobody_worked_on_says_so_instead_of_offering_a_candidate() {
    let c = Sandbox::new_empty("branch-moved-no-candidate");
    let backend = c.0.join("backend");
    commit_a_repo_on_branch(&backend, "feature/net10");
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "First task", "--why", "seed"]);

    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);
    let out = c.ok(&["brief"]);
    assert_eq!(
        branch_moved_block(&out),
        vec![
            " BRANCH MOVED since this lane last wrote",
            "   backend   feature/net10 -> perf/sp",
            "   no earlier work on perf/sp",
        ],
        "{out}"
    );
    assert!(!out.contains("to resume"), "{out}");
}

/// `Section::fixed`: the notice sits right behind the header and is bounded
/// by construction, so a budget that trims every truncable section away
/// must not touch it.
#[test]
fn the_notice_is_bounded_and_never_truncated() {
    let c = Sandbox::new_empty("branch-moved-bounded");
    let backend = c.0.join("backend");
    commit_a_repo_on_branch(&backend, "feature/net10");
    c.ok(&["init", "--yes"]);
    c.ok(&["push", "First task", "--why", "seed"]);
    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);

    let out = c.ok(&["brief", "--budget", "1"]);
    assert_eq!(
        branch_moved_block(&out),
        vec![
            " BRANCH MOVED since this lane last wrote",
            "   backend   feature/net10 -> perf/sp",
            "   no earlier work on perf/sp",
        ],
        "a fixed section must never be trimmed:\n{out}"
    );
}

/// Golden: `t594` §2.6 again, and `MODEL.md` §2 principle 5. A tree where
/// nobody ran `setup` has no repositories declared, so BRANCH MOVED has
/// nothing to compare against and must not appear -- the brief reads byte
/// for byte what it read before this section existed.
#[test]
fn a_tree_with_a_single_lane_and_no_repositories_prints_exactly_what_it_did() {
    let c = populated("branch-moved-golden");
    let a = c.ok(&["brief", "--now", "2026-09-17T10:00:00Z"]);
    assert!(
        !a.contains("BRANCH MOVED"),
        "a tree with no declared repositories must not gain this section:\n{a}"
    );
    let b = c.ok(&["brief", "--now", "2026-09-17T10:00:00Z"]);
    assert_eq!(a, b, "same log, same bytes (`MODEL.md` §2 principle 5)");
}

// ---------------------------------------------------------------------------
// OTHER LANES (`t594` §5.3): a second folder joined to the very same tree,
// with nothing beyond `init --join` and `push` -- no repository needed,
// since the section only ever reads the lane's own thread.
// ---------------------------------------------------------------------------

/// The number right before " tokens" in `emit`'s own trailer line, the
/// total the brief actually spent once every section that fit is in.
fn spent_tokens(out: &str) -> usize {
    out.lines()
        .find_map(|l| {
            let t = l.trim();
            if !t.contains("tokens") || !t.ends_with("parked") {
                return None;
            }
            t.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or_else(|| panic!("no token count line in the brief:\n{out}"))
}

/// A second folder, joined to `on`'s own tree as a lane named `name`,
/// with nothing pushed yet.
fn join_lane(on: &Sandbox, folder: &str, name: &str) -> Sandbox {
    let joined = Sandbox::new_empty_in(folder, on.global_home());
    joined.ok(&[
        "init",
        "--yes",
        "--join",
        on.0.to_str().unwrap(),
        "--lane-name",
        name,
    ]);
    joined
}

/// §5.3: only lanes that wrote *after* this one's last write. With
/// nothing new, the block does not appear at all (`d595`).
#[test]
fn a_lane_that_wrote_before_you_did_does_not_show() {
    let a = populated("other-before");
    let b = join_lane(&a, "other-before-b", "sonar");
    b.ok(&["push", "Ship the sonar dashboard", "--why", "seed"]);
    // `a` writes again after `b` did: `b`'s own last write is now behind
    // this lane's.
    a.ok(&["push", "Back on the main lane", "--why", "seed"]);

    let out = a.ok(&["brief"]);
    assert!(
        !out.contains("OTHER LANES"),
        "a lane that wrote before this one did should not show:\n{out}"
    );
    assert!(!out.contains("Ship the sonar dashboard"), "{out}");
}

#[test]
fn a_lane_with_no_stack_has_no_focus_to_show_and_stays_out() {
    let a = populated("other-no-stack");
    let _b = join_lane(&a, "other-no-stack-b", "sonar");
    // `_b` only ever declared itself through the join: nothing pushed,
    // so its stack stays empty.
    let out = a.ok(&["brief"]);
    assert!(!out.contains("OTHER LANES"), "{out}");
}

#[test]
fn a_lane_whose_folder_is_gone_stays_out_of_the_brief() {
    let a = populated("other-gone");
    let b = join_lane(&a, "other-gone-b", "sonar");
    b.ok(&["push", "Ship the sonar dashboard", "--why", "seed"]);

    // With the folder still there, the lane shows.
    let out = a.ok(&["brief"]);
    assert!(out.contains("Ship the sonar dashboard"), "{out}");

    // Gone, and it drops out -- never marked dead in the registry
    // itself, just absent from the folder `exists()` sees right now
    // (`d33`).
    std::fs::remove_dir_all(&b.0).unwrap();
    let out = a.ok(&["brief"]);
    assert!(!out.contains("OTHER LANES"), "{out}");
    assert!(!out.contains("Ship the sonar dashboard"), "{out}");
}

/// It is the last section, so it is the first to fall. Falling in
/// silence would be worse than not being there: it leaves the line that
/// says how many there were and where to read them.
#[test]
fn the_block_says_so_when_trimmed_by_the_budget() {
    let a = populated("other-budget");
    let b = join_lane(&a, "other-budget-b", "sonar");
    b.ok(&[
        "push",
        "Ship the sonar backend dashboard rewrite for the release",
        "--why",
        "seed",
    ]);

    let full = a.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(
        full.contains("Ship the sonar backend dashboard rewrite"),
        "{full}"
    );
    let spent = spent_tokens(&full);

    let tight = a.ok(&[
        "brief",
        "--budget",
        &(spent - 1).to_string(),
        "--now",
        "2026-09-15T10:00:00Z",
    ]);
    assert!(
        !tight.contains("Ship the sonar backend dashboard rewrite"),
        "the full row should have fallen:\n{tight}"
    );
    assert!(
        tight.contains("1 lane wrote here since you did (vivac stack --lanes)"),
        "no trace left behind:\n{tight}"
    );
    assert!(
        section(&tight, "LAST VIVAC"),
        "an earlier section fell first:\n{tight}"
    );
}

/// `d738`, test (b): `vivac brief` is read by a person, who already knows
/// the project's own doctrine -- telling them when to write teaches
/// nothing. The block belongs only to `session start --hook`
/// (`tests/session.rs`'s own `the_hook_brief_names_the_capture_seams`).
#[test]
fn the_capture_seams_block_is_hook_only() {
    let c = populated("capture-seams-brief");
    let out = c.ok(&["brief", "--now", "2026-09-15T10:00:00Z"]);
    assert!(
        !out.contains("WRITE AT THESE SEAMS"),
        "a person running `vivac brief` was told when to write:\n{out}"
    );
}

// `d899`: `BACK FROM PARKED` -- a park with a return date leaves DO NOT
// TOUCH NOW once local `--now` reaches it. The date has to be written
// straight into the log: `park --until` itself only ever accepts a date
// still ahead of the real clock, which a fixture pinned to 2026 cannot stay
// forever.

/// The ULID `node.created` gave `num`, read back off the log -- what a raw
/// `state.changed` line needs to name the node it acts on.
fn node_id_of(c: &Sandbox, num: u64) -> String {
    for line in c.log().lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v["payload"]["type"] == "node.created" && v["payload"]["num"] == num {
            return v["payload"]["node"].as_str().unwrap().to_string();
        }
    }
    panic!("no node.created for num {num} in:\n{}", c.log());
}

/// Parks `num` with `until`, straight in the log rather than through the
/// CLI: `park --until` only takes a date still ahead of the real clock, and
/// a fixed date in this fixture will not stay ahead of it forever.
fn park_with_until(c: &Sandbox, seq: u64, num: u64, until: &str) {
    let node = node_id_of(c, num);
    c.append_raw_line(&format!(
        r#"{{"seq":{seq},"id":"01BACKFROMPARKED{seq:09}","ts":"2026-09-10T10:00:00Z","actor":"a_test0000000","lane":"main","payload":{{"type":"state.changed","node":"{node}","state":"suspended","outcome":"waiting on day 14","forced":false,"until":"{until}"}}}}"#
    ));
}

#[test]
fn a_not_yet_due_park_shows_its_date_under_do_not_touch_now() {
    let c = Sandbox::new_seeded("back-from-parked-before");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    park_with_until(&c, 900, 1, "2026-09-14");

    let b = c.ok(&["brief", "--now", "2026-09-10T00:00:00Z"]);
    assert!(section(&b, "DO NOT TOUCH NOW"), "{b}");
    assert!(!section(&b, "BACK FROM PARKED"), "{b}");
    let heading_at = b.find("DO NOT TOUCH NOW").unwrap();
    let block = &b[heading_at..];
    assert!(block.contains("Ship the release"), "{block}");
    assert!(block.contains("until 2026-09-14"), "{block}");
}

#[test]
fn a_due_park_moves_to_back_from_parked_and_leaves_do_not_touch_now() {
    let c = Sandbox::new_seeded("back-from-parked-due");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    park_with_until(&c, 900, 1, "2026-09-14");

    let b = c.ok(&["brief", "--now", "2026-09-14T00:00:00Z"]);
    assert!(section(&b, "BACK FROM PARKED"), "{b}");
    assert!(
        !section(&b, "DO NOT TOUCH NOW"),
        "nothing else is parked, so the section should vanish whole:\n{b}"
    );
    let heading_at = b.find("BACK FROM PARKED").unwrap();
    let block = &b[heading_at..];
    assert!(block.contains("Ship the release"), "{block}");
    assert!(block.contains("vivac focus"), "no way back in:\n{block}");
    assert!(
        block.contains("--until"),
        "no way to set it aside again:\n{block}"
    );
}

#[test]
fn a_park_stays_due_well_past_its_own_date() {
    let c = Sandbox::new_seeded("back-from-parked-after");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    park_with_until(&c, 900, 1, "2026-09-14");

    let b = c.ok(&["brief", "--now", "2026-09-20T00:00:00Z"]);
    assert!(section(&b, "BACK FROM PARKED"), "{b}");
    assert!(b.contains("Ship the release"), "{b}");
}

/// A due node leaves `DO NOT TOUCH NOW`, but a park with no date, or one
/// not due yet, still shows there -- and `BACK FROM PARKED` renders above
/// it.
#[test]
fn back_from_parked_renders_above_do_not_touch_now_and_only_the_due_one_moves() {
    let c = Sandbox::new_seeded("back-from-parked-mixed");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    c.ok(&["add", "Write the notes", "--parent", "1", "--why", "seed"]);
    park_with_until(&c, 900, 1, "2026-09-14");
    c.ok(&["park", "2", "no date on this one"]);

    let b = c.ok(&["brief", "--now", "2026-09-14T00:00:00Z"]);
    let back_at = b.find("BACK FROM PARKED").expect("{b}");
    let touch_at = b.find("DO NOT TOUCH NOW").expect("{b}");
    assert!(
        back_at < touch_at,
        "BACK FROM PARKED must render first:\n{b}"
    );

    let back_block = &b[back_at..touch_at];
    assert!(back_block.contains("Ship the release"), "{back_block}");
    assert!(
        !back_block.contains("Write the notes"),
        "the still-parked node leaked into BACK FROM PARKED:\n{back_block}"
    );

    let touch_block = &b[touch_at..];
    assert!(touch_block.contains("Write the notes"), "{touch_block}");
    assert!(
        !touch_block.contains("Ship the release"),
        "the due node is still listed as untouchable:\n{touch_block}"
    );
}

/// `d899`: when the budget cannot hold both, BACK FROM PARKED wins --
/// pushed ahead of DO NOT TOUCH NOW in `to_text`, and truncation clears the
/// last truncable section first.
#[test]
fn back_from_parked_outlives_do_not_touch_now_under_a_tight_budget() {
    let c = Sandbox::new_seeded("back-from-parked-budget");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    c.ok(&["add", "Write the notes", "--parent", "1", "--why", "seed"]);
    park_with_until(&c, 900, 1, "2026-09-14");
    c.ok(&["park", "2", "no date on this one"]);

    let whole = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-14T00:00:00Z"]);
    assert!(section(&whole, "BACK FROM PARKED"), "{whole}");
    assert!(section(&whole, "DO NOT TOUCH NOW"), "{whole}");

    // Ratchet the budget down one token at a time until DO NOT TOUCH NOW
    // falls: `LAST VIVAC` sits behind it in `to_text`'s own vector order and
    // so falls first, and the exact token cost of either is not this test's
    // business -- only that BACK FROM PARKED outlives both.
    let mut budget = spent_tokens(&whole);
    let mut tight = whole;
    while section(&tight, "DO NOT TOUCH NOW") {
        budget -= 1;
        assert!(budget > 0, "ran out of budget before DO NOT TOUCH NOW fell");
        tight = c.ok(&[
            "brief",
            "--budget",
            &budget.to_string(),
            "--now",
            "2026-09-14T00:00:00Z",
        ]);
    }
    assert!(
        section(&tight, "BACK FROM PARKED"),
        "the due node should have outlived DO NOT TOUCH NOW:\n{tight}"
    );
}

// `d906`: `DUE FOR REVIEW` -- a review with a date sleeps until that day, and
// from it on the brief brings it back with the decisions made below it. As
// with `park --until`, a date in a fixture pinned to 2026 has to be written
// straight into the log: `flag --on` only takes a day still ahead of the
// real clock.

/// Raises `flag` on `num` with `on`, straight in the log rather than through
/// the CLI -- see above.
fn flag_with_on(c: &Sandbox, seq: u64, num: u64, flag: &str, on: Option<&str>) {
    let node = node_id_of(c, num);
    let on = on.map(|d| format!(r#","on":"{d}""#)).unwrap_or_default();
    c.append_raw_line(&format!(
        r#"{{"seq":{seq},"id":"01REVIEWSLEEPS{seq:012}","ts":"2026-09-10T10:00:00Z","actor":"a_test0000000","lane":"main","payload":{{"type":"flag.raised","node":"{node}","flag":"{flag}","reason":"the {flag} reason"{on}}}}}"#
    ));
}

/// The section that opens with `title`, its heading line to the line before
/// the next heading. A heading is the one thing in the brief indented by a
/// single space.
fn block_of(brief: &str, title: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for l in brief.lines() {
        if inside {
            if l.starts_with(' ') && !l.starts_with("  ") {
                break;
            }
        } else if l.trim() == title && !l.starts_with("  ") {
            inside = true;
        }
        if inside {
            out.push_str(l);
            out.push('\n');
        }
    }
    assert!(inside, "no {title} in:\n{brief}");
    out
}

/// `g1` on the stack, a constraint `c2` under it, a task `t3`, and `t4`
/// under that -- two hops from the path, off the lineage.
fn review_tree(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    c.ok(&[
        "add",
        "Never ship on a Friday",
        "--parent",
        "1",
        "--type",
        "constraint",
        "--why",
        "seed",
    ]);
    c.ok(&["add", "Write the notes", "--parent", "1", "--why", "seed"]);
    c.ok(&["add", "Proofread them", "--parent", "3", "--why", "seed"]);
    c
}

#[test]
fn a_review_asleep_is_not_a_flag_before_its_day() {
    let c = review_tree("review-sleeps");
    flag_with_on(&c, 900, 1, "review", Some("2026-09-14"));
    flag_with_on(&c, 901, 2, "review", Some("2026-09-14"));

    let b = c.ok(&["brief", "--now", "2026-09-10T00:00:00Z"]);
    assert!(!section(&b, "FLAGGED"), "{b}");
    assert!(!section(&b, "DUE FOR REVIEW"), "{b}");
    assert!(!b.contains("! review"), "the spine marks it:\n{b}");
    assert!(!b.contains("AT RISK"), "the invariant is at risk:\n{b}");
}

#[test]
fn a_review_with_no_date_is_still_a_flag() {
    let c = review_tree("review-awake");
    flag_with_on(&c, 900, 1, "review", None);
    flag_with_on(&c, 901, 2, "review", None);

    let b = c.ok(&["brief", "--now", "2026-09-10T00:00:00Z"]);
    assert!(section(&b, "FLAGGED"), "{b}");
    assert!(b.contains("! review"), "{b}");
    assert!(b.contains("AT RISK"), "{b}");
    assert!(!section(&b, "DUE FOR REVIEW"), "{b}");
}

#[test]
fn a_review_comes_due_on_its_day_and_stays_due_after() {
    let c = review_tree("review-due");
    flag_with_on(&c, 900, 3, "review", Some("2026-09-14"));

    for now in ["2026-09-14T00:00:00Z", "2026-09-20T00:00:00Z"] {
        let b = c.ok(&["brief", "--now", now]);
        assert!(section(&b, "DUE FOR REVIEW"), "{b}");
        let block = block_of(&b, "DUE FOR REVIEW");
        assert!(block.contains("t3"), "{block}");
        assert!(block.contains("Write the notes"), "{block}");
        assert!(block.contains("\"the review reason\""), "{block}");
        assert!(block.contains("since 2026-09-14\n"), "{block}");
        assert!(!block.contains("decided below"), "{block}");
    }
}

/// `t4` is neither on the path nor one hop off it: the section covers the
/// whole tree, not what the focus can reach.
#[test]
fn a_due_review_shows_even_outside_the_lineage() {
    let c = review_tree("review-due-far");
    flag_with_on(&c, 900, 4, "review", Some("2026-09-14"));

    let b = c.ok(&["brief", "--now", "2026-09-14T00:00:00Z"]);
    let block = block_of(&b, "DUE FOR REVIEW");
    assert!(block.contains("Proofread them"), "{block}");
}

#[test]
fn a_due_review_lists_the_open_decisions_made_below_it() {
    let c = review_tree("review-decided-below");
    for title in ["First call", "Second call", "Third call", "Fourth call"] {
        c.ok(&[
            "add", title, "--parent", "3", "--type", "decision", "--why", "seed",
        ]);
    }
    flag_with_on(&c, 900, 3, "review", Some("2026-09-14"));

    let b = c.ok(&["brief", "--now", "2026-09-14T00:00:00Z"]);
    let block = block_of(&b, "DUE FOR REVIEW");
    assert!(
        block.contains("since 2026-09-14 · decided below: d5 d6 d7 +1"),
        "three aliases and the rest counted:\n{block}"
    );
}

#[test]
fn a_decision_below_a_due_review_is_listed_only_while_open() {
    let c = review_tree("review-decided-open");
    for title in ["First call", "Second call"] {
        c.ok(&[
            "add", title, "--parent", "3", "--type", "decision", "--why", "seed",
        ]);
    }
    c.ok(&["abandon", "d5", "changed my mind"]);
    flag_with_on(&c, 900, 3, "review", Some("2026-09-14"));

    let b = c.ok(&["brief", "--now", "2026-09-14T00:00:00Z"]);
    let block = block_of(&b, "DUE FOR REVIEW");
    assert!(
        block.contains("since 2026-09-14 · decided below: d6\n"),
        "{block}"
    );
}

#[test]
fn a_due_review_offers_the_two_commands() {
    let c = review_tree("review-due-footer");
    flag_with_on(&c, 900, 3, "review", Some("2026-09-14"));

    let b = c.ok(&["brief", "--now", "2026-09-14T00:00:00Z"]);
    assert!(
        b.contains("  Reviewed:        vivac flag <id> review --off"),
        "{b}"
    );
    assert!(
        b.contains(
            "  Look again on:   vivac flag <id> review --why \"<what to look at>\" --on <date>"
        ),
        "{b}"
    );
    let before = c.ok(&["brief", "--now", "2026-09-10T00:00:00Z"]);
    assert!(!before.contains("Reviewed:"), "{before}");
}

/// The due review is in DUE FOR REVIEW, so its own FLAGGED row goes; any
/// other flag on the same node stays.
#[test]
fn a_due_review_is_not_repeated_in_flagged() {
    let c = review_tree("review-due-not-twice");
    flag_with_on(&c, 900, 3, "review", Some("2026-09-14"));
    flag_with_on(&c, 901, 3, "suspect", None);

    let b = c.ok(&["brief", "--now", "2026-09-14T00:00:00Z"]);
    let flagged = block_of(&b, "FLAGGED");
    assert!(flagged.contains("suspect"), "{flagged}");
    assert!(!flagged.contains("review"), "{flagged}");
    assert!(section(&b, "DUE FOR REVIEW"), "{b}");
}

#[test]
fn due_for_review_renders_after_flagged_and_before_back_from_parked() {
    let c = review_tree("review-order");
    flag_with_on(&c, 900, 3, "review", Some("2026-09-14"));
    flag_with_on(&c, 901, 1, "suspect", None);
    park_with_until(&c, 902, 4, "2026-09-14");

    let b = c.ok(&["brief", "--now", "2026-09-14T00:00:00Z"]);
    let flagged = b.find("\n FLAGGED").expect("{b}");
    let due = b.find("\n DUE FOR REVIEW").expect("{b}");
    let back = b.find("\n BACK FROM PARKED").expect("{b}");
    assert!(flagged < due && due < back, "{b}");
}

#[test]
fn flagged_points_at_the_command_that_lists_the_rest() {
    let c = Sandbox::new_seeded("review-pointer");
    c.ok(&["push", "Ship the release", "--why", "seed"]);
    for title in ["One", "Two", "Three", "Four"] {
        c.ok(&["add", title, "--parent", "1", "--why", "seed"]);
    }
    for (i, num) in [2u64, 3, 4, 5].iter().enumerate() {
        flag_with_on(&c, 900 + i as u64, *num, "suspect", None);
    }

    let b = c.ok(&["brief", "--now", "2026-09-10T00:00:00Z"]);
    assert!(b.contains("and 1 more (vivac flagged)"), "{b}");
}

/// Sets the `ts` of the `vivac.created` line for stop `num`, so a test can
/// put hours between two stops without waiting for them.
fn restamp_stop(c: &Sandbox, num: u64, ts: &str) {
    let path = c.0.join(".vivac").join("events");
    let raw = std::fs::read_to_string(&path).unwrap();
    let marker = format!("\"num\":{num},");
    let mut done = false;
    let out: Vec<String> = raw
        .lines()
        .map(|l| {
            if !done && l.contains("\"type\":\"vivac.created\"") && l.contains(&marker) {
                let at = l.find("\"ts\":\"").unwrap() + 6;
                let end = at + l[at..].find('"').unwrap();
                done = true;
                format!("{}{}{}", &l[..at], ts, &l[end..])
            } else {
                l.to_string()
            }
        })
        .collect();
    assert!(done, "no vivac.created line found for v{num}");
    std::fs::write(&path, out.join("\n") + "\n").unwrap();
}

/// `q752`, `d923`: an intent quoted from an earlier stop says how long
/// before the last stop it was spoken, and gives up that much of its own
/// width so the line grows no wider.
#[test]
fn an_intent_from_an_earlier_stop_says_how_long_before_it_was_spoken() {
    let c = Sandbox::new_seeded("vivac-stop-age");
    c.ok(&["push", "Root", "--why", "seed"]);
    c.ok(&["save", "packing up", "--next", "ship it"]);
    c.ok(&["note", "1", "something happened"]);
    c.ok(&["session", "end", "--hook"]);
    restamp_stop(&c, 2, "2026-09-28T20:00:00Z");
    restamp_stop(&c, 3, "2026-09-29T10:30:00Z");

    let b = c.ok(&["brief"]);
    assert!(b.contains("v2, 14 h earlier, was about to: ship it"), "{b}");
}

/// The age is only for a stop other than the last: when the last stop is
/// the one that spoke, there is nothing between them to measure.
#[test]
fn the_last_stop_speaking_for_itself_carries_no_age() {
    let c = Sandbox::new_seeded("vivac-stop-no-age");
    c.ok(&["push", "Root", "--why", "seed"]);
    c.ok(&["save", "packing up", "--next", "ship it"]);

    let b = c.ok(&["brief"]);
    assert!(b.contains("you were about to: ship it"), "{b}");
    assert!(!b.contains("earlier"), "{b}");
}

/// `f261`, `d929`: a correction lives in a note, since neither the title
/// nor the why ever changes, and the spine carries each node's newest note
/// under its why -- the goal's too, which shows no why at all.
#[test]
fn the_spine_carries_each_nodes_newest_note() {
    let c = Sandbox::new_seeded("spine-notes");
    c.ok(&["push", "Goal", "--why", "the branch"]);
    c.ok(&[
        "push",
        "Move the store to SQLite",
        "--why",
        "JSON cannot hold it",
    ]);
    c.ok(&["push", "Leaf", "--why", "one more level"]);
    c.ok(&["note", "1", "renamed in spirit: the goal is the store"]);
    c.ok(&["note", "2", "an older word"]);
    c.ok(&["note", "2", "SQLite was ruled out by d9"]);

    let b = c.ok(&["brief"]);
    assert!(
        b.contains(": renamed in spirit: the goal is the store"),
        "{b}"
    );
    assert!(b.contains(": SQLite was ruled out by d9"), "{b}");
    assert!(
        !b.contains("an older word"),
        "only the newest travels:\n{b}"
    );
    let leaf: Vec<&str> = b
        .lines()
        .skip_while(|l| !l.contains("Leaf"))
        .take(3)
        .collect();
    assert!(
        leaf.iter().all(|l| !l.contains("last note")),
        "a node with no notes grew a line:\n{b}"
    );
}

/// A node that closed after its last note says how it closed instead: the
/// outcome is the later word, and the note it overrides may say the
/// opposite.
#[test]
fn a_node_closed_after_its_note_shows_its_outcome() {
    let c = Sandbox::new_seeded("spine-outcome");
    c.ok(&["push", "Goal", "--why", "the branch"]);
    c.ok(&["push", "Step", "--why", "work"]);
    c.ok(&["push", "Leaf", "--why", "one more level"]);
    c.ok(&["note", "2", "stale, kept open on purpose"]);
    c.ok(&["done", "2", "shipped in 0.1 after all"]);

    let b = c.ok(&["brief"]);
    assert!(b.contains("closed "), "{b}");
    assert!(b.contains(": shipped in 0.1 after all"), "{b}");
    assert!(!b.contains("kept open on purpose"), "{b}");
}
