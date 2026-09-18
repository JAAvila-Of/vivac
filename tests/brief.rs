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

/// `t594` §5.1, the golden case: a tree nobody ran `setup` in has one lane,
/// the founding one, and it is called `main`. The header has to keep
/// printing exactly that -- byte for byte what it printed before this
/// tree learned there could be more than one lane.
#[test]
fn the_header_names_the_founding_lane_main() {
    let c = Sandbox::new_seeded("lane-header");
    let b = c.ok(&["brief", "--now", "2026-09-16T10:00:00Z"]);
    let header = b.lines().next().unwrap_or("");
    assert!(
        header.contains(" · lane: main · 2026-09-16"),
        "the founding lane's own header changed:\n{header}"
    );
}

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

/// `t594` task 4, paso 5 (§4.4): a lone repository keeps the short sha it
/// always showed, and only two or more collapse the line to a count.
#[test]
fn the_last_stop_shows_one_short_sha_and_counts_the_rest() {
    // One repository reads exactly as it did before: a tree with a single
    // repository must not notice this tranche happened.
    let one = Sandbox::new_empty("brief-repos-one");
    commit_a_repo(&one.0);
    one.ok(&["init"]);
    one.ok(&["setup", "claude-code", "--yes"]);
    // A clean working tree, so the stop's anchor is the repository's own
    // `HEAD` and nothing setup itself wrote shows up as a change since.
    git(&one.0, &["add", "-A"]);
    git(&one.0, &["commit", "-q", "-m", "setup"]);
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
    two.ok(&["init"]);
    two.ok(&["push", "Something", "--why", "seed"]);
    two.ok(&["setup", "claude-code", "--yes"]);
    two.ok(&["save", "checkpoint"]);
    let out = two.ok(&["brief"]);
    assert!(last_vivac_line(&out).contains(" · 2 repos"), "{out}");
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
    c.ok(&["init"]);
    c.ok(&["setup", "claude-code", "--yes"]);
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

/// Once the lane's own `where.changed` matches the branch again, the notice
/// is gone -- it compares against the last thing the lane wrote, not
/// against history in general.
#[test]
fn going_back_to_the_branch_makes_the_notice_disappear() {
    let c = Sandbox::new_empty("branch-moved-disappear");
    let backend = c.0.join("backend");
    commit_a_repo_on_branch(&backend, "feature/net10");
    c.ok(&["init"]);
    c.ok(&["setup", "claude-code", "--yes"]);
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
    c.ok(&["init"]);
    c.ok(&["setup", "claude-code", "--yes"]);
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
    c.ok(&["init"]);
    c.ok(&["setup", "claude-code", "--yes"]);
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
// with nothing beyond `setup --join` and `push` -- no repository needed,
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
        "setup",
        "claude-code",
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
