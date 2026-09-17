//! The IQuorum scenario, end to end (`t594`): one product's tree, several
//! checkouts of the very same repositories on this machine, and every
//! command this task's own stretch of `t594` built made to work across
//! all of them together.
//!
//! Four synthetic repositories, one root commit apiece, cloned into every
//! root this test touches -- the shape the registry's own root-commit
//! matching exists to recognise, and the shape a real multi-repository
//! product checked out more than once on one machine actually has on disk.
//! Five roots carry them: `p`, the folder the tree ends up living in; `c2`,
//! outside `p`, which holds the tree first and becomes a lane of `p`'s tree
//! by `relocate`; `c1`, a clone nested inside `p` itself; and `c3`/`c4`,
//! two more clones elsewhere on the machine that `setup` refuses and
//! `--join` admits.

mod common;
use std::path::{Path, PathBuf};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// A directory name nothing else in this file takes.
fn unique(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "vivac-iquorum-{name}-{}-{n}-{ts}",
        std::process::id()
    ))
}

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

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A fresh repository with one commit at `dir`: the template every clone in
/// this file traces its `.git` back to, so `git rev-list --max-parents=0
/// HEAD` -- what `repos::root_commit` actually asks -- answers with the
/// same hash no matter which clone is asked.
fn make_repo(dir: &Path, seed: &str) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    std::fs::write(dir.join("f.txt"), seed).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "first"]);
}

/// Clones `template` into `dest`, the same root commit and all.
fn clone_into(template: &Path, dest: &Path) {
    let out = std::process::Command::new("git")
        .args(["clone", "-q"])
        .arg(template)
        .arg(dest)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Clones every one of `templates` into `root`, one subfolder per
/// repository -- the shape `repos::scan` walks, two levels deep at most, so
/// a repository directly under `root` is well inside its reach.
fn seed_repos(root: &Path, templates: &[PathBuf]) {
    for (i, template) in templates.iter().enumerate() {
        clone_into(template, &root.join(format!("repo{i}")));
    }
}

fn log_lines(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(".vivac").join("events"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The whole scenario, in the order `t594`'s own spec names its five parts:
/// `relocate` moving a tree into the folder that holds a product's other
/// checkouts, `setup` making a nested clone a lane through the ordinary
/// upward walk, `setup`/`--join` recognising two more clones elsewhere by
/// the repositories they share, two lanes writing at once without a
/// number repeating, and `find` reaching across lanes with no
/// `--everywhere`.
#[test]
fn the_iquorum_scenario_moves_joins_and_shares_one_tree_across_five_roots() {
    let home = unique("home");
    let templates: Vec<PathBuf> = (0..4)
        .map(|i| {
            let t = unique(&format!("template-{i}"));
            make_repo(&t, &format!("repo{i}"));
            t
        })
        .collect();
    let extra_template = unique("template-extra");
    make_repo(&extra_template, "extra");

    // -----------------------------------------------------------------
    // 1: C2, outside P, already holds the tree. `relocate` moves it to
    // P with a lane name, and C2 keeps its own stack afterwards --
    // checked by reading the stack, never the lane file.
    // -----------------------------------------------------------------
    let p = unique("p");
    let c2 = unique("c2");
    std::fs::create_dir_all(&c2).unwrap();
    seed_repos(&c2, &templates);
    ok(&c2, &home, &["init"]);
    ok(
        &c2,
        &home,
        &["push", "Track the sonar release", "--why", "seed"],
    );
    let stack_before = ok(&c2, &home, &["stack"]);
    assert!(
        stack_before.contains("Track the sonar release"),
        "{stack_before}"
    );

    let (reloc_out, reloc_code) = run(
        &c2,
        &home,
        &["relocate", p.to_str().unwrap(), "--lane-name", "sonar"],
    );
    assert_eq!(reloc_code, 0, "{reloc_out}");

    let stack_after = ok(&c2, &home, &["stack"]);
    assert!(
        stack_after.contains("Track the sonar release"),
        "C2's own stack did not survive the move:\n{stack_after}"
    );

    // -----------------------------------------------------------------
    // 2: C1, a clone nested inside P itself, becomes a lane through the
    // ordinary upward walk.
    // -----------------------------------------------------------------
    let c1 = p.join("c1");
    std::fs::create_dir_all(&c1).unwrap();
    seed_repos(&c1, &templates);
    ok(&c1, &home, &["setup", "claude-code", "--yes"]);
    assert!(
        c1.join(".vivac").join("lane").is_file(),
        "C1 never became a lane of the tree above it"
    );

    // -----------------------------------------------------------------
    // 3: C3, a clone outside P sharing the same four repositories, is
    // refused and joins with --join. C4 carries one repository more of
    // its own, and is refused and joins the same way.
    // -----------------------------------------------------------------
    let c3 = unique("c3");
    std::fs::create_dir_all(&c3).unwrap();
    seed_repos(&c3, &templates);
    let (refusal3, code3) = run(&c3, &home, &["setup", "claude-code", "--yes"]);
    assert_eq!(code3, 1, "{refusal3}");
    assert!(refusal3.contains("--join"), "{refusal3}");
    ok(
        &c3,
        &home,
        &["setup", "claude-code", "--join", p.to_str().unwrap()],
    );
    assert!(c3.join(".vivac").join("lane").is_file());

    let c4 = unique("c4");
    std::fs::create_dir_all(&c4).unwrap();
    seed_repos(&c4, &templates);
    clone_into(&extra_template, &c4.join("repo4"));
    let (refusal4, code4) = run(&c4, &home, &["setup", "claude-code", "--yes"]);
    assert_eq!(code4, 1, "{refusal4}");
    assert!(refusal4.contains("--join"), "{refusal4}");
    ok(
        &c4,
        &home,
        &["setup", "claude-code", "--join", p.to_str().unwrap()],
    );
    assert!(c4.join(".vivac").join("lane").is_file());

    // -----------------------------------------------------------------
    // 4: C1 and C2 write at the same time, from two real processes --
    // both spawned before either is waited on -- and no number repeats.
    // -----------------------------------------------------------------
    let mut a = std::process::Command::new(BIN)
        .current_dir(&c1)
        .env("VIVAC_HOME", &home)
        .args(["push", "Ship the sonar dashboard", "--why", "seed a"])
        .spawn()
        .unwrap();
    let mut b = std::process::Command::new(BIN)
        .current_dir(&c2)
        .env("VIVAC_HOME", &home)
        .args(["push", "Guard the sonar release notes", "--why", "seed b"])
        .spawn()
        .unwrap();
    let status_a = a.wait().unwrap();
    let status_b = b.wait().unwrap();
    assert!(status_a.success(), "C1's concurrent push failed");
    assert!(status_b.success(), "C2's concurrent push failed");

    let lines = log_lines(&p);
    let seqs: Vec<u64> = lines
        .iter()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["seq"].as_u64().expect("every line names a seq")
        })
        .collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), seqs.len(), "a seq repeated: {seqs:?}");

    let (check_out, check_code) = run(&c1, &home, &["check"]);
    assert_eq!(check_code, 0, "{check_out}");

    // -----------------------------------------------------------------
    // 5: find from C1 sees what C2 just wrote, with no --everywhere.
    // -----------------------------------------------------------------
    let (found, found_code) = run(&c1, &home, &["find", "Guard the sonar release notes"]);
    assert_eq!(found_code, 0, "{found}");
    assert!(
        found.contains("Guard the sonar release notes"),
        "find from C1 did not see what C2 wrote:\n{found}"
    );

    std::fs::remove_dir_all(&home).ok();
    for t in &templates {
        std::fs::remove_dir_all(t).ok();
    }
    std::fs::remove_dir_all(&extra_template).ok();
    std::fs::remove_dir_all(&p).ok();
    std::fs::remove_dir_all(&c2).ok();
    std::fs::remove_dir_all(&c3).ok();
    std::fs::remove_dir_all(&c4).ok();
}

// ---------------------------------------------------------------------------
// Left for the next stretch of `t594`, on purpose: the HERE marker per
// lane and the OTHER LANES block in `brief`, and `stack --lanes`, are not
// part of what this task delivered. Reserved here, empty, so that work
// finds these waiting rather than inventing them again.
// ---------------------------------------------------------------------------

/// `t594`, the next stretch: `brief`'s `<== HERE` marker, shown once per
/// lane's own stack rather than only the one this process is standing in.
#[ignore = "t594, next stretch: HERE per lane in `brief`"]
#[test]
fn brief_marks_here_on_every_lanes_own_front_not_only_this_ones() {}

/// `t594`, the next stretch: an `OTHER LANES` block in `brief`, naming what
/// the tree's other lanes have open.
#[ignore = "t594, next stretch: the OTHER LANES block in `brief`"]
#[test]
fn brief_carries_an_other_lanes_block() {}

/// `t594`, the next stretch: `vivac stack --lanes`, listing every lane's
/// own stack rather than only the one this folder is.
#[ignore = "t594, next stretch: `stack --lanes`"]
#[test]
fn stack_lanes_lists_every_lanes_own_stack() {}
