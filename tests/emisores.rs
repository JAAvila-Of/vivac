//! `t594` tramo 4, task 7 (§9.2.7): the Emisores scenario end to end. A
//! root that is not a repository itself, with several repositories
//! underneath on branches of different names, exercised through the same
//! commands a person would type rather than through any one task's own
//! fixture. Nothing here is new behaviour: tasks 1 through 6 of this
//! tranche built every piece this test drives, and this is only the first
//! place they all run together.

mod common;
use common::Sandbox;
use std::path::Path;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

fn git(dir: &Path, args: &[&str]) {
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

/// A repository with one commit at `dir`, checked out onto `branch` rather
/// than whatever the local `git` calls its default.
fn commit_a_repo_on_branch(dir: &Path, branch: &str) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "first"]);
    git(dir, &["checkout", "-q", "-b", branch]);
}

/// Runs the binary in `dir` with `home` as `VIVAC_HOME` -- for the folders
/// this scenario builds beside the root a `Sandbox` holds, in particular a
/// linked worktree, which `Sandbox::run` cannot reach since it always uses
/// its own root as the working directory.
fn run(dir: &Path, home: &Path, args: &[&str]) -> (String, i32) {
    let o = std::process::Command::new(BIN)
        .current_dir(dir)
        .env("VIVAC_HOME", home)
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
        o.status.code().unwrap_or(-1),
    )
}

fn ok(dir: &Path, home: &Path, args: &[&str]) -> String {
    let (s, code) = run(dir, home, args);
    assert_eq!(
        code,
        0,
        "`vivac {}` failed with {code}:\n{s}",
        args.join(" ")
    );
    s
}

fn where_changed_count(root: &Path) -> usize {
    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap_or_default();
    log.lines()
        .filter(|l| l.contains(r#""type":"where.changed""#))
        .count()
}

/// The paths the tree's last `lane.declared` names, in the order
/// `repos::scan` writes them.
fn declared_repo_paths(root: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    let line = text
        .lines()
        .rev()
        .find(|l| l.contains(r#""type":"lane.declared""#))
        .expect("a lane.declared line exists");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    v["payload"]["repos"]
        .as_array()
        .expect("lane.declared names its repos")
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_string())
        .collect()
}

/// The BRANCH MOVED block, header through the blank line that ends it --
/// `tests/brief.rs`'s own reader (integration tests share no code across
/// files, so this is the same function, not a shared one).
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

/// The full shape §9.2.7 describes: a root with no git of its own and four
/// repositories underneath on branches of different names; `setup`
/// declaring all four; a branch that moves and a decision born on it; the
/// brief's BRANCH MOVED notice and `why`'s "not the branch you are on"
/// label; the notice going away once the lane writes on the branch it came
/// back to; a linked worktree inside the root with its own stack; and a
/// rebase in progress that does not write one event per commit.
#[test]
fn the_emisores_scenario_end_to_end() {
    let c = Sandbox::new_empty("emisores");
    let backend = c.0.join("backend");
    let web_one = c.0.join("web1");
    let web_two = c.0.join("web2");
    let web_three = c.0.join("web3");
    commit_a_repo_on_branch(&backend, "feature/net10");
    commit_a_repo_on_branch(&web_one, "feature/ng22");
    commit_a_repo_on_branch(&web_two, "feature/ng22");
    commit_a_repo_on_branch(&web_three, "feature/ng22");

    c.ok(&["init"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    assert_eq!(
        declared_repo_paths(&c.0),
        vec!["backend", "web1", "web2", "web3"],
        "setup did not declare all four repositories"
    );

    // Two objectives, on two different branches of the backend: the net10
    // migration, and a stored procedures optimisation that lives on an
    // older branch, the same shape §1 describes.
    c.ok(&[
        "push",
        "Migrate the backend to net10",
        "--why",
        "the legacy target reaches end of life",
    ]);

    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);
    c.ok(&[
        "push",
        "Speed up the billing stored procedures",
        "--why",
        "invoicing queries run too slow on the old plan",
        "--root",
    ]);
    let decide_out = c.ok(&[
        "decide",
        "Use a materialized view for invoices",
        "--parent",
        "2",
        "--reason",
        "avoids recomputing the join on every request",
    ]);
    assert!(decide_out.contains("d3"), "{decide_out}");

    // The backend goes back to the branch the lane started from, without
    // writing anything yet: the live checkout and the lane's own last
    // `where.changed` now disagree.
    git(&backend, &["checkout", "-q", "feature/net10"]);

    let moved = c.ok(&["brief"]);
    assert_eq!(
        branch_moved_block(&moved),
        vec![
            " BRANCH MOVED since this lane last wrote",
            "   backend   perf/sp -> feature/net10",
            "   last focus on feature/net10:   g1   Migrate the backend to net10",
            "   to resume:  vivac focus g1",
        ],
        "{moved}"
    );

    // `why` reads the log, not `HEAD`: the decision born on `perf/sp` does
    // not yet carry the label, because nothing has been written since it
    // moved back either.
    let why_before = c.ok(&["why", "d3", "--full"]);
    assert!(
        !why_before.contains("not the branch you are on"),
        "the label appeared before the lane wrote anything on the branch \
         it came back to:\n{why_before}"
    );

    // Writing on the branch the lane came back to is what moves the
    // lane's own last `where.changed`, which is what both the notice
    // disappearing and the label appearing key off.
    c.ok(&[
        "push",
        "Resume the net10 migration",
        "--why",
        "back on the branch this lane started from",
        "--root",
    ]);

    let back = c.ok(&["brief"]);
    assert!(
        !back.contains("BRANCH MOVED"),
        "back on the branch the lane last wrote, the notice should be gone:\n{back}"
    );

    // The decision born on `perf/sp` is not hidden, only marked (`d596`):
    // it is the same lane, on a branch it has since left behind. The where
    // it was born under is a complete photograph of all four repositories
    // (§2.4), so the line names the first three and counts the rest.
    let why_after = c.ok(&["why", "d3", "--full"]);
    let founding_lane_name = c.0.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        why_after.contains(&format!(
            "born in lane {founding_lane_name} · backend@perf/sp, web1@feature/ng22, \
             web2@feature/ng22, and 1 more (not the branch you are on)"
        )),
        "{why_after}"
    );

    // A linked worktree of the backend, inside the root itself: it joins
    // its own lane on its first write and keeps its own stack, apart from
    // `main`'s.
    let backend_worktree = c.0.join("backend-wt");
    git(
        &backend,
        &[
            "worktree",
            "add",
            backend_worktree.to_str().unwrap(),
            "--detach",
        ],
    );
    ok(
        &backend_worktree,
        c.global_home(),
        &["push", "Spike something on the worktree", "--why", "seed"],
    );
    assert!(
        backend_worktree.join(".vivac").join("lane").exists(),
        "the worktree never joined a lane of its own"
    );

    let root_stack = c.ok(&["stack"]);
    assert!(
        !root_stack.contains("Spike something on the worktree"),
        "{root_stack}"
    );
    let worktree_stack = ok(&backend_worktree, c.global_home(), &["stack"]);
    assert!(
        worktree_stack.contains("Spike something on the worktree"),
        "{worktree_stack}"
    );

    // A rebase in progress on the backend: `HEAD` detaches and moves once
    // per commit replayed, and `rebase-merge/head-name` is where the
    // branch actually being rebased lives. Two writes during the same
    // rebase must not leave two `where.changed` behind -- or even one,
    // since nothing the lane recorded about the backend has really
    // changed.
    let count_before_rebase = where_changed_count(&c.0);
    let gitdir = backend.join(".git");
    std::fs::create_dir_all(gitdir.join("rebase-merge")).unwrap();
    std::fs::write(
        gitdir.join("rebase-merge").join("head-name"),
        "refs/heads/feature/net10\n",
    )
    .unwrap();

    std::fs::write(gitdir.join("HEAD"), "1".repeat(40)).unwrap();
    c.ok(&[
        "push",
        "Replay the first commit of the rebase",
        "--why",
        "seed",
    ]);
    assert_eq!(
        where_changed_count(&c.0),
        count_before_rebase,
        "the first commit replayed during a rebase wrote a where.changed"
    );

    std::fs::write(gitdir.join("HEAD"), "2".repeat(40)).unwrap();
    c.ok(&[
        "push",
        "Replay the second commit of the rebase",
        "--why",
        "seed",
    ]);
    assert_eq!(
        where_changed_count(&c.0),
        count_before_rebase,
        "a second commit replayed during the same rebase wrote another where.changed"
    );
}

/// The commit `dir`'s `HEAD` points at, read straight from `git` rather than
/// from anything `vivac` itself wrote: what a test compares `vivacs --json`
/// against has to come from a source the code under test never touched.
fn git_head(dir: &Path) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(out.status.success(), "git rev-parse HEAD failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `f639`: `vivacs --json` used to serialize only `anchor`, so a stop
/// anchored to several declared repositories read as unanchored to a
/// script -- the same root-without-git shape [`the_emisores_scenario_end_to_end`]
/// already builds, with nothing above it changed.
#[test]
fn two_declared_repositories_each_carry_their_own_anchor_in_vivacs_json() {
    let c = Sandbox::new_empty("vivacs-anchors");
    let repo_one = c.0.join("repo-one");
    let repo_two = c.0.join("repo-two");
    commit_a_repo_on_branch(&repo_one, "feature/one");
    commit_a_repo_on_branch(&repo_two, "feature/two");

    c.ok(&["init"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    c.ok(&["save", "a checkpoint"]);

    let out = c.ok(&["vivacs", "--json"]);
    let stops: serde_json::Value = serde_json::from_str(&out).unwrap();
    let latest = &stops[0];

    assert_eq!(
        latest["anchor"]["kind"], "null",
        "the root itself holds no git, by design since f613: {latest}"
    );

    let anchors = latest["anchors"]
        .as_array()
        .expect("anchors is always an array");
    assert_eq!(
        anchors.len(),
        2,
        "one entry per declared repository: {latest}"
    );

    let mut by_path: std::collections::BTreeMap<&str, &str> = anchors
        .iter()
        .map(|a| (a["path"].as_str().unwrap(), a["sha"].as_str().unwrap()))
        .collect();
    assert_eq!(
        by_path.keys().copied().collect::<Vec<_>>(),
        vec!["repo-one", "repo-two"],
        "{latest}"
    );
    assert_eq!(by_path.remove("repo-one").unwrap(), git_head(&repo_one));
    assert_eq!(by_path.remove("repo-two").unwrap(), git_head(&repo_two));
}

/// A tree nobody ran `setup` in declares no repositories, and `f639`'s fix
/// keeps that a schema-stable empty array rather than a field that only
/// shows up once something is declared underneath.
#[test]
fn a_lane_with_no_declared_repositories_still_shows_empty_anchors_in_vivacs_json() {
    let c = Sandbox::new_seeded("no-declared-repos");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["save", "a checkpoint"]);

    let out = c.ok(&["vivacs", "--json"]);
    let stops: serde_json::Value = serde_json::from_str(&out).unwrap();
    let latest = &stops[0];

    assert_eq!(
        latest["anchors"],
        serde_json::json!([]),
        "a lane with nothing declared still gets the field: {latest}"
    );
}

/// `f639` follow-up: `reconcile --json` had the same omission `vivacs --json`
/// did, serializing only `since.anchor.short()` -- an empty string at a root
/// without git -- and dropping the per-repository list the stop it reads
/// from actually carries.
#[test]
fn reconcile_json_carries_the_anchors_of_the_stop_it_reads_from() {
    let c = Sandbox::new_empty("reconcile-anchors");
    let repo_one = c.0.join("repo-one");
    let repo_two = c.0.join("repo-two");
    commit_a_repo_on_branch(&repo_one, "feature/one");
    commit_a_repo_on_branch(&repo_two, "feature/two");

    c.ok(&["init"]);
    c.ok(&["setup", "claude-code", "--yes"]);
    c.ok(&["save", "a checkpoint"]);

    let sha_one = git_head(&repo_one);
    let sha_two = git_head(&repo_two);

    // Something for `reconcile` to actually read, after the stop it measures
    // from -- otherwise there is no history to contradict the tree with.
    std::fs::write(repo_one.join("f.txt"), "y").unwrap();
    git(&repo_one, &["add", "."]);
    git(&repo_one, &["commit", "-q", "-m", "second"]);

    let out = c.ok(&["reconcile", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();

    let anchors = v["anchors"].as_array().expect("anchors is always an array");
    let mut by_path: std::collections::BTreeMap<&str, &str> = anchors
        .iter()
        .map(|a| (a["path"].as_str().unwrap(), a["sha"].as_str().unwrap()))
        .collect();
    assert_eq!(
        by_path.keys().copied().collect::<Vec<_>>(),
        vec!["repo-one", "repo-two"],
        "{v}"
    );
    assert_eq!(by_path.remove("repo-one").unwrap(), sha_one);
    assert_eq!(by_path.remove("repo-two").unwrap(), sha_two);
}

/// The same schema-stability `f639` asked of `vivacs --json`, here for
/// `reconcile --json`: a lane with nothing declared still gets `anchors`,
/// empty rather than absent.
#[test]
fn a_lane_with_no_declared_repositories_still_shows_empty_anchors_in_reconcile_json() {
    let c = Sandbox::new_empty("no-declared-repos-reconcile");
    commit_a_repo_on_branch(&c.0, "feature/solo");

    c.ok(&["init"]);
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["save", "a checkpoint"]);

    let out = c.ok(&["reconcile", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();

    assert_eq!(
        v["anchors"],
        serde_json::json!([]),
        "a lane with nothing declared still gets the field: {v}"
    );
}
