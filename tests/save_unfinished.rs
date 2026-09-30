//! `d936`: once a stop is written, `save` looks at what the stop cannot see
//! from `HEAD` alone -- work not committed, commits not pushed, files changed
//! that no node claims -- and says so only when something is off. It never
//! refuses the stop and never changes the exit code.

mod common;
use common::Sandbox;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git is not on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with one commit at `dir`.
fn commit_a_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@example.invalid"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "start\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "init"]);
}

/// A bare repository beside `dir`, named `origin` and pushed to, so the
/// current branch has an upstream to be ahead of.
fn add_origin_and_push(dir: &Path, bare: &Path) {
    std::fs::create_dir_all(bare).unwrap();
    git(bare, &["init", "-q", "--bare"]);
    git(dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(dir, &["push", "-q", "-u", "origin", "HEAD"]);
}

fn commit_file(dir: &Path, rel: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, format!("{rel}\n")).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", &format!("add {rel}")]);
}

/// A seeded tree at the root of a repository with one commit, which is
/// clean: what `init` writes under `.vivac/` is ignored.
fn in_a_repo(name: &str) -> Sandbox {
    let c = Sandbox::new_empty(name);
    commit_a_repo(&c.0);
    c.ok(&["init", "--yes"]);
    c
}

/// A repository with an upstream and nothing left to say about it.
fn in_step_with_origin(name: &str) -> Sandbox {
    let c = in_a_repo(name);
    add_origin_and_push(&c.0, &c.global_home().join("remote.git"));
    c
}

fn lines_of(out: &str) -> Vec<&str> {
    out.lines().collect()
}

/// Nothing is off: the two lines `save` always printed, and nothing else.
#[test]
fn a_clean_repository_in_step_with_its_origin_prints_the_two_lines_it_always_did() {
    let c = in_step_with_origin("unfinished-clean");
    let out = c.ok(&["save", "a stop", "--next", "go on"]);
    let lines = lines_of(&out);
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].ends_with("a stop"), "{out}");
    assert!(lines[1].starts_with("        anchored to "), "{out}");
}

#[test]
fn one_file_not_committed_is_named() {
    let c = in_step_with_origin("unfinished-one-file");
    std::fs::write(c.0.join("loose.txt"), "x\n").unwrap();
    let out = c.ok(&["save", "a stop", "--next", "go on"]);
    assert!(
        lines_of(&out).contains(&"        1 file not committed"),
        "{out}"
    );
    assert!(!out.contains("not pushed"), "{out}");
}

#[test]
fn several_files_not_committed_are_counted_and_the_store_is_not_work() {
    let c = in_step_with_origin("unfinished-files");
    for f in ["a.txt", "b.txt", "src/c.txt"] {
        let p = c.0.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x\n").unwrap();
    }
    let out = c.ok(&["save", "a stop", "--next", "go on"]);
    assert!(
        lines_of(&out).contains(&"        3 files not committed"),
        "{out}"
    );
}

#[test]
fn two_commits_not_pushed_are_named() {
    let c = in_step_with_origin("unfinished-ahead");
    commit_file(&c.0, "one.txt");
    commit_file(&c.0, "two.txt");
    let out = c.ok(&["save", "a stop", "--next", "go on"]);
    assert!(
        lines_of(&out).contains(&"        2 commits not pushed"),
        "{out}"
    );
    assert!(!out.contains("not committed"), "{out}");
}

#[test]
fn one_commit_not_pushed_reads_in_the_singular() {
    let c = in_step_with_origin("unfinished-ahead-one");
    commit_file(&c.0, "one.txt");
    let out = c.ok(&["save", "a stop", "--next", "go on"]);
    assert!(
        lines_of(&out).contains(&"        1 commit not pushed"),
        "{out}"
    );
}

/// Both findings of one repository share a line, in the order files then
/// commits, and the lines sit between the anchor and the `--next` hint.
#[test]
fn both_findings_share_a_line_between_the_anchor_and_the_next_hint() {
    let c = in_step_with_origin("unfinished-both");
    commit_file(&c.0, "one.txt");
    std::fs::write(c.0.join("loose.txt"), "x\n").unwrap();
    let out = c.ok(&["save", "a stop"]);
    let lines = lines_of(&out);
    assert_eq!(lines.len(), 4, "{out}");
    assert!(lines[1].starts_with("        anchored to "), "{out}");
    assert_eq!(
        lines[2], "        1 file not committed, 1 commit not pushed",
        "{out}"
    );
    assert!(lines[3].starts_with("        no --next"), "{out}");
}

/// A lane that declares no repositories has no path to lead with: the one
/// folder the stop anchored is named by nothing at all.
#[test]
fn a_lane_with_no_declared_repository_prints_its_line_with_no_prefix() {
    let c = Sandbox::new_seeded("unfinished-undeclared");
    commit_a_repo(&c.0);
    std::fs::write(c.0.join("loose.txt"), "x\n").unwrap();
    let out = c.ok(&["save", "a stop", "--next", "go on"]);
    assert!(
        lines_of(&out).contains(&"        1 file not committed"),
        "{out}"
    );
}

/// A branch with no upstream has nothing to be ahead of, and nothing is said
/// about it.
#[test]
fn a_branch_with_nothing_to_push_to_says_nothing_about_pushing() {
    let c = in_a_repo("unfinished-no-upstream");
    commit_file(&c.0, "one.txt");
    let out = c.ok(&["save", "a stop", "--next", "go on"]);
    assert!(!out.contains("push"), "{out}");
    assert_eq!(lines_of(&out).len(), 2, "{out}");
}

/// The count of files no node claims equals what `reconcile --since` lists
/// under its own UNCLAIMED basket, and a file an open node governs is not
/// counted.
#[test]
fn files_changed_since_the_previous_stop_that_no_node_claims_are_counted() {
    let c = in_step_with_origin("unfinished-unclaimed");
    c.ok(&[
        "push",
        "Own the source",
        "--why",
        "it is needed",
        "--governs",
        "src/**",
    ]);
    let first = c.ok(&["save", "base", "--next", "go on"]);
    let since = first
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    commit_file(&c.0, "src/governed.rs");
    commit_file(&c.0, "docs/one.md");
    commit_file(&c.0, "docs/two.md");
    git(&c.0, &["push", "-q"]);

    let out = c.ok(&["save", "again", "--next", "go on"]);
    let expected = format!(
        "        2 files changed since {since} that no node claims:  vivac reconcile --since {since}"
    );
    assert!(lines_of(&out).contains(&expected.as_str()), "{out}");

    let reconcile = c.ok(&["reconcile", "--since", &since]);
    assert!(reconcile.contains("NOBODY CLAIMS THESE (2)"), "{reconcile}");
}

#[test]
fn a_changed_file_an_open_node_governs_is_not_reported() {
    let c = in_step_with_origin("unfinished-governed");
    c.ok(&[
        "push",
        "Own the source",
        "--why",
        "it is needed",
        "--governs",
        "src/**",
    ]);
    c.ok(&["save", "base", "--next", "go on"]);
    commit_file(&c.0, "src/governed.rs");
    git(&c.0, &["push", "-q"]);
    let out = c.ok(&["save", "again", "--next", "go on"]);
    assert!(!out.contains("no node claims"), "{out}");
    assert_eq!(lines_of(&out).len(), 2, "{out}");
}

/// With no previous stop made by hand there is nothing to measure from.
#[test]
fn with_no_previous_manual_stop_no_unclaimed_line_is_printed() {
    let c = in_step_with_origin("unfinished-first-stop");
    c.ok(&[
        "push",
        "Own the source",
        "--why",
        "it is needed",
        "--governs",
        "src/**",
    ]);
    commit_file(&c.0, "docs/one.md");
    git(&c.0, &["push", "-q"]);
    let out = c.ok(&["save", "first", "--next", "go on"]);
    assert!(!out.contains("no node claims"), "{out}");
}

/// No version control at all: nothing new is printed.
#[test]
fn with_no_git_nothing_new_is_printed() {
    let c = Sandbox::new_seeded("unfinished-no-git");
    let (out, code) = c.run(&["save", "a stop"]);
    assert_eq!(code, 0, "{out}");
    let lines = lines_of(&out);
    assert_eq!(lines.len(), 3, "{out}");
    assert!(lines[1].starts_with("        no anchor"), "{out}");
    assert!(lines[2].starts_with("        no --next"), "{out}");
}

/// The findings never change the exit code, and the stop is written anyway.
#[test]
fn the_exit_code_is_zero_with_findings_and_the_stop_is_written() {
    let c = in_step_with_origin("unfinished-exit");
    commit_file(&c.0, "one.txt");
    std::fs::write(c.0.join("loose.txt"), "x\n").unwrap();
    let (out, code) = c.run(&["save", "a stop"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("not committed"), "{out}");
    assert!(c.ok(&["vivacs"]).contains("a stop"));
}

/// A lane that declares its repositories gets one line per repository that
/// has something, led by the declared path; a clean one gets none.
#[test]
fn declared_repositories_each_get_their_own_line_with_the_path() {
    let c = Sandbox::new_empty("unfinished-declared");
    let one = c.0.join("repo-one");
    let two = c.0.join("repo-two");
    let three = c.0.join("repo-three");
    for r in [&one, &two, &three] {
        commit_a_repo(r);
    }
    add_origin_and_push(&two, &c.global_home().join("remote-two.git"));
    c.ok(&["init", "--yes"]);
    std::fs::write(one.join("loose.txt"), "x\n").unwrap();
    commit_file(&two, "ahead.txt");
    let out = c.ok(&["save", "a stop", "--next", "go on"]);
    assert!(
        lines_of(&out).contains(&"        repo-one: 1 file not committed"),
        "{out}"
    );
    assert!(
        lines_of(&out).contains(&"        repo-two: 1 commit not pushed"),
        "{out}"
    );
    assert!(!out.contains("repo-three"), "{out}");
}
