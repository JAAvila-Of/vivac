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

/// A unique directory this test created, removed the moment this goes
/// out of scope. A panicked assertion unwinds through it exactly the
/// way it already unwinds through `tests/common::Sandbox`'s own `Drop`,
/// so a failure here leaves nothing behind in `%TEMP%` for whoever
/// runs this suite next.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        TempDir(unique(name))
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for TempDir {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
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
fn seed_repos(root: &Path, templates: &[TempDir]) {
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

/// Whether `text` names something that reads as an absolute path: a drive
/// letter on Windows (`C:\`), a UNC prefix (`\\`), or a leading `/` on
/// POSIX. Checked as both, because this suite runs on every platform of
/// the CI and not only the one that built it -- a lane's name is always a
/// folder's own name, never the path to it (`t594` §9.2.18).
fn names_an_absolute_path(text: &str) -> bool {
    text.contains(":\\")
        || text.contains("\\\\")
        || text.split_whitespace().any(|w| w.starts_with('/'))
}

/// The whole scenario, in the order `t594`'s own spec names its six parts:
/// `relocate` moving a tree into the folder that holds a product's other
/// checkouts, `setup` making a nested clone a lane through the ordinary
/// upward walk, `setup`/`--join` recognising two more clones elsewhere by
/// the repositories they share, two lanes writing at once without a
/// number repeating, `find` reaching across lanes with no `--everywhere`,
/// and -- the rest of §9.2.8, left to this tramo -- each lane's own
/// `HERE`, `OTHER LANES` naming only a lane that wrote later, and
/// `stack --lanes` naming every lane with a front of its own.
#[test]
fn the_iquorum_scenario_moves_joins_and_shares_one_tree_across_five_roots() {
    let home = TempDir::new("home");
    let templates: Vec<TempDir> = (0..4)
        .map(|i| {
            let t = TempDir::new(&format!("template-{i}"));
            make_repo(&t, &format!("repo{i}"));
            t
        })
        .collect();
    let extra_template = TempDir::new("template-extra");
    make_repo(&extra_template, "extra");

    // -----------------------------------------------------------------
    // 1: C2, outside P, already holds the tree. `relocate` moves it to
    // P with a lane name, and C2 keeps its own stack afterwards --
    // checked by reading the stack, never the lane file.
    // -----------------------------------------------------------------
    let p = TempDir::new("p");
    let c2 = TempDir::new("c2");
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
    // A `lane` file names no path -- it holds an id and a `project`,
    // the id of the tree's own first event. Existing alone proves a
    // lane of *some* tree; naming the same first event as P's own log
    // proves it is a lane of *this* one.
    let c1_lane: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(c1.join(".vivac").join("lane")).unwrap())
            .unwrap();
    let p_first_event: serde_json::Value = serde_json::from_str(
        log_lines(&p)
            .first()
            .expect("P's tree has at least one event"),
    )
    .unwrap();
    assert_eq!(
        c1_lane["project"], p_first_event["id"],
        "C1's lane names a different tree from P's own"
    );

    // -----------------------------------------------------------------
    // 3: C3, a clone outside P sharing the same four repositories, is
    // refused and joins with --join. C4 carries one repository more of
    // its own, and is refused and joins the same way.
    // -----------------------------------------------------------------
    let c3 = TempDir::new("c3");
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

    let c4 = TempDir::new("c4");
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
    // 4: C1 and C2 write at the same time, from several real processes
    // per round on each side -- every one spawned before any of them
    // is waited on -- and no number repeats. A single pair rarely
    // lands both writes inside the same critical section; four from
    // each side, spawned together, reliably queue on the write lock
    // and reliably reproduced a collision once the lock's own reread
    // was removed, which a lone pair did not.
    // -----------------------------------------------------------------
    let rounds = 3;
    let writers_per_side = 4;
    for round in 0..rounds {
        let mut children = Vec::new();
        for i in 0..writers_per_side {
            let last = round + 1 == rounds && i + 1 == writers_per_side;
            let title_a = format!("Ship the sonar dashboard, round {round} writer {i}");
            let title_b = if last {
                "Guard the sonar release notes".to_string()
            } else {
                format!("Guard the sonar release notes, round {round} writer {i}")
            };
            children.push(
                std::process::Command::new(BIN)
                    .current_dir(&c1)
                    .env("VIVAC_HOME", &home)
                    .args(["push", &title_a, "--why", "seed a"])
                    .spawn()
                    .unwrap(),
            );
            children.push(
                std::process::Command::new(BIN)
                    .current_dir(&c2)
                    .env("VIVAC_HOME", &home)
                    .args(["push", &title_b, "--why", "seed b"])
                    .spawn()
                    .unwrap(),
            );
        }
        for (i, mut child) in children.into_iter().enumerate() {
            let status = child.wait().unwrap();
            assert!(
                status.success(),
                "a concurrent push failed in round {round}, writer {i}"
            );
        }
    }

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
    let (found, found_code) = run(
        &c1,
        &home,
        &["find", "Guard the sonar release notes", "--json"],
    );
    assert_eq!(found_code, 0, "{found}");
    let hits: serde_json::Value = serde_json::from_str(&found)
        .unwrap_or_else(|e| panic!("find --json did not print an array: {e}\n{found}"));
    let hits = hits.as_array().expect("find --json prints an array");
    assert!(
        hits.iter()
            .any(|h| h["title"] == "Guard the sonar release notes"),
        "find from C1 did not see what C2 wrote:\n{found}"
    );

    // -----------------------------------------------------------------
    // 6: `t594` §9.2.8's own tail, left to this tramo -- each lane's own
    // `HERE`, `OTHER LANES` naming only a lane that wrote after this one
    // did, and `stack --lanes` naming every lane with a front of its
    // own. Sequential from here: part 4 already proved no `seq` repeats
    // under a race, and what is left needs a known order, not another
    // one. `--budget` is generous throughout, so the deep stacks part 4
    // left behind never trim the section being asserted on.
    // -----------------------------------------------------------------
    ok(
        &c1,
        &home,
        &[
            "push",
            "Confirm the concurrent writers landed",
            "--why",
            "seed",
        ],
    );
    let brief_c1 = ok(&c1, &home, &["brief", "--budget", "50000"]);
    let c1_front = brief_c1
        .lines()
        .find(|l| l.contains("Confirm the concurrent writers landed"))
        .unwrap_or_else(|| panic!("C1's own front is missing from its brief:\n{brief_c1}"));
    assert!(c1_front.contains("<== HERE"), "{brief_c1}");
    assert!(
        !brief_c1.contains("OTHER LANES"),
        "nothing wrote after C1's own last write yet:\n{brief_c1}"
    );

    let brief_c2_before = ok(&c2, &home, &["brief", "--budget", "50000"]);
    assert!(
        brief_c2_before.contains("OTHER LANES"),
        "C1 just wrote after C2's own last write did:\n{brief_c2_before}"
    );
    assert!(
        brief_c2_before.contains("Confirm the concurrent writers landed"),
        "{brief_c2_before}"
    );

    ok(
        &c2,
        &home,
        &["push", "Note the sonar lane caught up", "--why", "seed"],
    );
    let brief_c2_after = ok(&c2, &home, &["brief", "--budget", "50000"]);
    let c2_front = brief_c2_after
        .lines()
        .find(|l| l.contains("Note the sonar lane caught up"))
        .unwrap_or_else(|| panic!("C2's own front is missing from its brief:\n{brief_c2_after}"));
    assert!(c2_front.contains("<== HERE"), "{brief_c2_after}");
    assert!(
        !brief_c2_after.contains("OTHER LANES"),
        "C2 just wrote again, after every other lane:\n{brief_c2_after}"
    );

    // C3 joined in part 3 above but never wrote: an empty stack carries
    // no front to name, so it stays out of `stack --lanes` until it does.
    let before_json = ok(&c1, &home, &["stack", "--lanes", "--json"]);
    let before: serde_json::Value = serde_json::from_str(&before_json).unwrap_or_else(|e| {
        panic!("stack --lanes --json did not print an object: {e}\n{before_json}")
    });
    assert_eq!(
        before["lanes"].as_array().expect("lanes is an array").len(),
        2,
        "{before_json}"
    );

    ok(
        &c3,
        &home,
        &["push", "File the emisores follow-up", "--why", "seed"],
    );
    let brief_c3 = ok(&c3, &home, &["brief", "--budget", "50000"]);
    let c3_front = brief_c3
        .lines()
        .find(|l| l.contains("File the emisores follow-up"))
        .unwrap_or_else(|| panic!("C3's own front is missing from its brief:\n{brief_c3}"));
    assert!(c3_front.contains("<== HERE"), "{brief_c3}");

    let after_text = ok(&c1, &home, &["stack", "--lanes"]);
    let after_json = ok(&c1, &home, &["stack", "--lanes", "--json"]);
    let after: serde_json::Value = serde_json::from_str(&after_json).unwrap_or_else(|e| {
        panic!("stack --lanes --json did not print an object: {e}\n{after_json}")
    });
    let rows = after["lanes"].as_array().expect("lanes is an array");
    assert_eq!(
        rows.len(),
        3,
        "`stack --lanes` should now name every lane with a front of its own:\n{after_json}"
    );
    let titles: Vec<&str> = rows
        .iter()
        .map(|r| r["focus"]["title"].as_str().unwrap())
        .collect();
    for title in [
        "Confirm the concurrent writers landed",
        "Note the sonar lane caught up",
        "File the emisores follow-up",
    ] {
        assert!(titles.contains(&title), "{after_json}");
    }

    // §9.2.18: none of the above ever names a path, only a lane's own
    // folder name.
    for out in [
        &brief_c1,
        &brief_c2_before,
        &brief_c2_after,
        &brief_c3,
        &before_json,
        &after_text,
        &after_json,
    ] {
        assert!(
            !names_an_absolute_path(out),
            "a brief or `stack --lanes` named an absolute path instead of a lane's own folder name:\n{out}"
        );
    }

    // No cleanup here: every root above is a `TempDir`, and its own
    // `Drop` removes it whether this line is ever reached or not.
}

// ---------------------------------------------------------------------------
// Left for the next stretch of `t594`, on purpose: the HERE marker per
// lane and the OTHER LANES block in `brief`, and `stack --lanes`, are not
// part of what this task delivered. Reserved here, empty, so that work
// finds these waiting rather than inventing them again.
// ---------------------------------------------------------------------------

/// `t594`, the next stretch: `brief`'s `<== HERE` marker, shown once per
/// lane's own stack rather than only the one this process is standing in.
/// No section of `t594`'s own plan names this one on its own.
#[test]
fn brief_marks_here_on_every_lanes_own_front_not_only_this_ones() {
    let home = TempDir::new("home");
    let p = TempDir::new("p");
    std::fs::create_dir_all(&p).unwrap();
    ok(&p, &home, &["init"]);
    ok(
        &p,
        &home,
        &["push", "Track the sonar release", "--why", "seed"],
    );

    let b = TempDir::new("b");
    std::fs::create_dir_all(&b).unwrap();
    ok(
        &b,
        &home,
        &[
            "setup",
            "claude-code",
            "--join",
            p.to_str().unwrap(),
            "--lane-name",
            "sonar",
        ],
    );
    ok(
        &b,
        &home,
        &["push", "Ship the sonar dashboard", "--why", "seed"],
    );

    // P's own front carries `HERE`. B wrote after P did, so B's own
    // front shows up in OTHER LANES too -- but never marked `HERE`
    // there, since that mark belongs to a lane's own stack, not to a
    // row naming somebody else's.
    let brief_p = ok(&p, &home, &["brief"]);
    let p_own_line = brief_p
        .lines()
        .find(|l| l.contains("Track the sonar release"))
        .unwrap_or_else(|| panic!("P's own front is missing from its brief:\n{brief_p}"));
    assert!(p_own_line.contains("<== HERE"), "{brief_p}");
    if let Some(b_row) = brief_p
        .lines()
        .find(|l| l.contains("Ship the sonar dashboard"))
    {
        assert!(
            !b_row.contains("<== HERE"),
            "B's own front showed up marked HERE in P's brief:\n{brief_p}"
        );
    }

    // B's own brief marks its own front, and never carries P's at all:
    // P wrote before B ever joined, so P's row has nothing to show for
    // in OTHER LANES either.
    let brief_b = ok(&b, &home, &["brief"]);
    let b_own_line = brief_b
        .lines()
        .find(|l| l.contains("Ship the sonar dashboard"))
        .unwrap_or_else(|| panic!("B's own front is missing from its brief:\n{brief_b}"));
    assert!(b_own_line.contains("<== HERE"), "{brief_b}");
    assert!(
        !brief_b.contains("Track the sonar release"),
        "B's brief carried P's own front, and B is not standing there:\n{brief_b}"
    );
}

/// An `OTHER LANES` block in `brief`, naming what the tree's other lanes
/// have open. `t594` §5.3. `P` reads `brief` after `B` joins its tree and
/// writes -- across two folders on this machine, the same shape the rest
/// of this scenario carries, and no repository needed at all: the section
/// only ever reads a lane's own thread, never a checkout.
#[test]
fn brief_carries_an_other_lanes_block() {
    let home = TempDir::new("home");
    let p = TempDir::new("p");
    std::fs::create_dir_all(&p).unwrap();
    ok(&p, &home, &["init"]);
    ok(
        &p,
        &home,
        &["push", "Track the sonar release", "--why", "seed"],
    );

    let b = TempDir::new("b");
    std::fs::create_dir_all(&b).unwrap();
    ok(
        &b,
        &home,
        &[
            "setup",
            "claude-code",
            "--join",
            p.to_str().unwrap(),
            "--lane-name",
            "sonar",
        ],
    );
    ok(
        &b,
        &home,
        &["push", "Ship the sonar dashboard", "--why", "seed"],
    );

    let out = ok(&p, &home, &["brief"]);
    assert!(out.contains("OTHER LANES"), "{out}");
    assert!(out.contains("Ship the sonar dashboard"), "{out}");
}

/// `vivac stack --lanes`, `t594` §5.5: every lane's own stack, this
/// folder's included, rather than only the one this folder is standing
/// in. Two folders, one tree, no repository needed at all -- the same
/// shape `brief_carries_an_other_lanes_block` already carries.
#[test]
fn stack_lanes_lists_every_lanes_own_stack() {
    let home = TempDir::new("home");
    let p = TempDir::new("p");
    std::fs::create_dir_all(&p).unwrap();
    ok(&p, &home, &["init"]);
    ok(
        &p,
        &home,
        &["push", "Track the sonar release", "--why", "seed"],
    );

    let b = TempDir::new("b");
    std::fs::create_dir_all(&b).unwrap();
    ok(
        &b,
        &home,
        &[
            "setup",
            "claude-code",
            "--join",
            p.to_str().unwrap(),
            "--lane-name",
            "sonar",
        ],
    );
    ok(
        &b,
        &home,
        &["push", "Ship the sonar dashboard", "--why", "seed"],
    );

    let json = ok(&p, &home, &["stack", "--lanes", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("stack --lanes --json did not print an object: {e}\n{json}"));
    let lanes = v["lanes"].as_array().expect("lanes is an array");
    assert_eq!(lanes.len(), 2, "{json}");
    for row in lanes {
        assert_eq!(row["folder_gone"], false, "{json}");
    }
    let joined = lanes
        .iter()
        .find(|l| l["name"] == "sonar")
        .unwrap_or_else(|| panic!("the joined lane is missing:\n{json}"));
    assert_eq!(joined["focus"]["title"], "Ship the sonar dashboard");
    let main = lanes
        .iter()
        .find(|l| l["name"] == "main")
        .unwrap_or_else(|| panic!("this folder's own lane is missing:\n{json}"));
    assert_eq!(main["focus"]["title"], "Track the sonar release");

    // Text mode carries the same rows, in the exact table `t594` §5.5's
    // own spec draws: name, alias, title and the date the focus was
    // opened, each padded to the column the next one starts at.
    let text = ok(&p, &home, &["stack", "--lanes"]);
    for row in lanes {
        let expected = format!(
            "  {:<11} {:<6} {:<45} {}",
            row["name"].as_str().unwrap(),
            row["focus"]["alias"].as_str().unwrap(),
            row["focus"]["title"].as_str().unwrap(),
            row["focus"]["opened"].as_str().unwrap(),
        );
        assert!(
            text.lines().any(|l| l == expected),
            "missing row {expected:?} in:\n{text}"
        );
    }
}
