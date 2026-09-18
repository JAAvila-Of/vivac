//! `reconcile` — the diff between the tree and the anchor's history.
//!
//! `ROADMAP.md` §7: the principal risk is that the graph goes stale and starts
//! to lie. These tests are about the four answers the command can give -- no
//! stop, no anchor, no `governs`, and the real diff -- because three of the
//! four are the ones a user actually meets first, and a command that answers
//! them badly gets run once.

mod common;
use common::Sandbox;
use std::process::Command;

/// A sandbox that is also a git repository with one commit, which is what it
/// takes for `Anchor` to be `Git` and not `Null`.
fn with_git(name: &str) -> Sandbox {
    let c = Sandbox::new_empty(name);
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .current_dir(&c.0)
            .args(args)
            .output()
            .expect("git is not on PATH");
        assert!(ok.status.success(), "git {args:?}: {ok:?}");
    };
    git(&["init", "-q", "."]);
    git(&["config", "user.email", "t@example.invalid"]);
    git(&["config", "user.name", "t"]);
    write(&c, "README.md", "start\n");
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    c.ok(&["init"]);
    c
}

fn write(c: &Sandbox, path: &str, body: &str) {
    let p = c.0.join(path);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

fn git_at(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git is not on PATH");
    assert!(ok.status.success(), "git {args:?}: {ok:?}");
}

/// A repository with one commit, at `dir`.
fn commit_a_repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    git_at(dir, &["init", "-q"]);
    git_at(dir, &["config", "user.email", "t@example.invalid"]);
    git_at(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "start\n").unwrap();
    git_at(dir, &["add", "-A"]);
    git_at(dir, &["commit", "-qm", "init"]);
}

/// Nothing to measure from. It is the first thing a fresh tree hits, and the
/// answer has to carry the command that fixes it.
#[test]
fn a_tree_with_no_stop_says_which_command_makes_one() {
    let c = with_git("recon-nostop");
    let s = c.ok(&["reconcile"]);
    assert!(s.contains("no vivacs yet"), "{s}");
    assert!(s.contains("vivac save"), "no way out of it:\n{s}");
}

/// No version control. `MODEL.md` §8 calls `Null` the floor of the product and
/// not a placeholder, so this cannot read as breakage.
#[test]
fn with_no_version_control_it_says_so_without_calling_it_a_failure() {
    let c = Sandbox::new_seeded("recon-null");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["save", "a stop"]);
    let s = c.ok(&["reconcile"]);
    assert!(s.contains("no anchor"), "{s}");
    assert!(s.contains("not a failure"), "it reads like breakage:\n{s}");
}

/// The tree that has never declared a `governs` is most trees on the first
/// run. Listing every changed file as unclaimed would be technically true and
/// useless: the finding is that nothing can be claimed at all.
#[test]
fn with_no_governs_it_says_the_real_problem_once() {
    let c = with_git("recon-nogoverns");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["save", "a stop"]);
    write(&c, "src/one.rs", "a\n");
    write(&c, "src/two.rs", "b\n");
    let s = c.ok(&["reconcile"]);
    assert!(s.contains("No node declares what it governs"), "{s}");
    assert!(s.contains("--governs"), "no way out of it:\n{s}");
    assert!(
        !s.contains("NOBODY CLAIMS"),
        "it listed the symptom per file:\n{s}"
    );
}

/// The case `INTEGRATION.md` §9 is about: files moved and no node says they
/// are its. The command names them and hands over the command, and it does
/// **not** decide which thread they belong to -- that judgement is not the
/// tool's.
#[test]
fn an_unclaimed_file_comes_out_with_the_command_that_claims_it() {
    let c = with_git("recon-unclaimed");
    c.ok(&[
        "push",
        "Fix the auth adapter",
        "--why",
        "sessions expire early",
        "--governs",
        "src/auth/**",
    ]);
    c.ok(&["save", "a stop"]);
    write(&c, "src/auth/store.rs", "claimed\n");
    write(&c, "src/util/retry.rs", "nobody asked for this\n");

    let s = c.ok(&["reconcile"]);
    assert!(s.contains("NOBODY CLAIMS THESE (1)"), "{s}");
    assert!(s.contains("src/util/retry.rs"), "{s}");
    assert!(s.contains("--governs <path>"), "no concrete action:\n{s}");
    // The claimed one is not a finding, and does not clutter the view.
    assert!(
        !s.contains("NOBODY CLAIMS THESE (2)"),
        "it claimed nothing:\n{s}"
    );
    assert!(
        s.contains("under work that is open"),
        "it hid the healthy half entirely:\n{s}"
    );
    let all = c.ok(&["reconcile", "--all"]);
    assert!(all.contains("src/auth/store.rs"), "--all hid it:\n{all}");
}

/// The signal that matters most: the file kept moving after the only node that
/// claims it closed. Either the work came back or the `governs` is too wide,
/// and both are worth a look.
#[test]
fn a_file_whose_only_claim_is_closed_is_the_finding() {
    let c = with_git("recon-stale");
    c.ok(&[
        "push",
        "Auth adapter",
        "--why",
        "it is due",
        "--governs",
        "src/auth/**",
    ]);
    c.ok(&["pop", "adapter shipped"]);
    c.ok(&["save", "after closing it"]);
    write(&c, "src/auth/store.rs", "moved anyway\n");

    let s = c.ok(&["reconcile"]);
    assert!(s.contains("CLAIMED ONLY BY CLOSED WORK (1)"), "{s}");
    assert!(s.contains("src/auth/store.rs"), "{s}");
    assert!(s.contains("--reopen"), "no concrete action:\n{s}");
}

/// The tool's own log is not the project's work. Without this the command
/// reports the store it just wrote to, every single time.
#[test]
fn the_store_is_not_work() {
    let c = with_git("recon-store");
    c.ok(&[
        "push",
        "A goal",
        "--why",
        "it is needed",
        "--governs",
        "src/**",
    ]);
    c.ok(&["save", "a stop"]);
    write(&c, "src/one.rs", "a\n");
    let s = c.ok(&["reconcile"]);
    assert!(!s.contains(".vivac"), "it reported its own store:\n{s}");
}

/// `--since` measures from another stop, and one that does not exist is a
/// usage error rather than a silent fall back to the last.
#[test]
fn since_picks_another_stop_and_refuses_one_that_is_not_there() {
    let c = with_git("recon-since");
    c.ok(&[
        "push",
        "A goal",
        "--why",
        "it is needed",
        "--governs",
        "src/**",
    ]);
    c.ok(&["save", "first"]);
    write(&c, "src/one.rs", "a\n");
    c.ok(&["save", "second"]);

    let s = c.ok(&["reconcile", "--since", "v1"]);
    assert!(s.contains("since v1"), "{s}");
    let (s, code) = c.run(&["reconcile", "--since", "v99"]);
    assert_eq!(code, 2, "an absent stop passed as usable:\n{s}");
}

/// The agent's half of the audience. `d1`: everything a maintainer can read,
/// an agent can parse.
#[test]
fn the_json_carries_the_three_baskets() {
    let c = with_git("recon-json");
    c.ok(&[
        "push",
        "Auth adapter",
        "--why",
        "it is due",
        "--governs",
        "src/auth/**",
    ]);
    c.ok(&["save", "a stop"]);
    write(&c, "src/auth/store.rs", "a\n");
    write(&c, "src/util/retry.rs", "b\n");

    let s = c.ok(&["reconcile", "--json"]);
    for k in [
        "\"unclaimed\"",
        "\"claimed_by_closed_work\"",
        "\"claimed_and_open\"",
        "\"since\"",
        "\"anchor\"",
        "\"governing_nodes\"",
    ] {
        assert!(s.contains(k), "{k} missing:\n{s}");
    }
    assert!(s.contains("src/util/retry.rs"), "{s}");
    // The whole file list, never the truncated view.
    assert!(!s.contains("more   --json"), "the json got trimmed:\n{s}");
}

/// `t594` task 4, paso 6 (§4.4): once a lane has more than one declared
/// repository, every changed file is prefixed by the one it belongs to.
#[test]
fn reconcile_prefixes_each_change_with_its_repository() {
    let c = Sandbox::new_empty("recon-prefix");
    commit_a_repo(&c.0.join("webapi"));
    commit_a_repo(&c.0.join("infra"));
    c.ok(&["init"]);
    c.ok(&[
        "push",
        "A goal",
        "--why",
        "it is needed",
        "--governs",
        "unrelated/**",
    ]);
    c.ok(&["setup", "claude-code", "--yes"]);
    c.ok(&["save", "a stop"]);
    write(&c, "webapi/src/one.rs", "a\n");
    write(&c, "infra/main.tf", "b\n");

    let s = c.ok(&["reconcile"]);
    assert!(s.contains("webapi/src/one.rs"), "{s}");
    assert!(s.contains("infra/main.tf"), "{s}");
}

/// `t594` task 4, paso 6 (§4.4): a repository whose branch moved since the
/// stop is named and not diffed -- comparing across branches would be
/// inferring whether something merged, which `d596` puts out of scope.
#[test]
fn reconcile_says_a_repository_is_on_another_branch_instead_of_diffing_it() {
    let c = Sandbox::new_empty("recon-branch-moved");
    commit_a_repo(&c.0.join("webapi"));
    git_at(&c.0.join("webapi"), &["branch", "-m", "feature/net10"]);
    c.ok(&["init"]);
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    c.ok(&["save", "a stop"]);

    git_at(&c.0.join("webapi"), &["checkout", "-qb", "perf/sp"]);
    write(&c, "webapi/src/one.rs", "changed\n");

    let s = c.ok(&["reconcile"]);
    assert!(s.contains("webapi   feature/net10 -> perf/sp"), "{s}");
    assert!(
        s.contains("The stop anchored feature/net10, so what changed here belongs to"),
        "{s}"
    );
    assert!(
        s.contains("another branch and not to this stop. Nothing compared."),
        "{s}"
    );
    assert!(
        !s.contains("webapi/src/one.rs"),
        "the moved repository was diffed anyway:\n{s}"
    );
}
