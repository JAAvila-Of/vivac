//! `d336`/`d414` — a constraint, a pillar and a rule are not open fronts.
//!
//! `d336` carried a mechanical acceptance criterion: **one test that fails
//! today for each of its two changes**, with the rest of the battery green
//! and no existing test touched. Writing it first is how that criterion gets
//! used instead of admired.
//!
//! It could only be met for one of the two, and finding out why was worth
//! more than the tests were:
//!
//! - **Change one, `is_front()`.** `a_rule_is_not_an_open_front` fails today,
//!   as intended, and it is the test that survives here.
//! - **Change two, the `constraints()` predicate.** No test could fail,
//!   because the change was unobservable: `constraints()` admits a node when
//!   it is project-wide **or** when its ancestry meets the focus path, and
//!   `ancestors()` runs all the way to the root, which sits on every focus
//!   path there is. `d414` retires that change outright rather than fixing
//!   it: gone by type is what §5 of `t411` reads for governance from now on,
//!   and the pinning test this file used to carry for the no-op --
//!   `a_task_local_constraint_reaches_every_brief` -- is retired with it.
//!
//! `d414` widens what change one excludes: `Pillar` and `Rule` join
//! `Constraint`, for the same reason `Decision` was excluded to begin with.

mod common;
use common::Sandbox;

/// A root goal with real work under it, so `open` has something to show
/// besides whatever this test is asking about.
fn seeded(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&[
        "push",
        "Migrate authentication to OIDC",
        "--why",
        "the old provider is shutting down",
    ]);
    c.ok(&[
        "add",
        "Pick a token store",
        "--parent",
        "1",
        "--why",
        "the migration needs one",
    ]);
    c
}

/// A permanent rule is neither worked on nor ever closed, so counting it
/// among the open fronts answers "what do I have open?" with governance.
/// `Decision` was excluded for this exact reason; `Constraint`, `Pillar` and
/// `Rule` qualify for it just as squarely (`d414`).
///
/// Six of them go unnoticed. Forty-seven would make half the view be
/// governance, which is `f334`.
#[test]
fn a_constraint_a_pillar_and_a_rule_are_not_open_fronts() {
    let c = seeded("front");
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
        "add",
        "Security",
        "--parent",
        "1",
        "--type",
        "pillar",
        "--why",
        "the arbiter for this tree",
    ]);
    c.ok(&[
        "add",
        "Never store a secret",
        "--parent",
        "3",
        "--type",
        "rule",
        "--why",
        "what the pillar arbitrates",
    ]);

    let out = c.ok(&["open"]);

    for title in [
        "No dependencies under a copyleft licence",
        "Security",
        "Never store a secret",
    ] {
        assert!(
            !out.contains(title),
            "a {title} is governance, not a front, and `open` still lists it:\n{out}"
        );
    }
    assert!(
        out.contains("Pick a token store"),
        "the real front disappeared, which is a different bug:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// `t594` task 6: security, the class the security pillar's ban on paths in
// the log already governs (`check.rs`'s own `gates_output_carries_no_
// absolute_path` is the precedent), stretched across the surfaces this
// task added -- a copy, a move to another folder, and a folder whose
// repositories are already tracked elsewhere.
// ---------------------------------------------------------------------------

fn git(dir: &std::path::Path, args: &[&str]) {
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

/// Removes its path when dropped, whether the test that made it passed or
/// panicked -- `relocate`'s own destination test needs one, since the
/// destination it exercises sits outside every `VIVAC_HOME` on purpose and
/// a failed assertion must not leave that tree behind on the machine that
/// ran it.
struct RemoveOnDrop(std::path::PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn run_split(c: &Sandbox, args: &[&str]) -> (String, String, i32) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

/// Every regular file under `dir`, walked recursively rather than named one
/// by one: a file a later round of this task adds gets swept the same as
/// the ones it shipped with, instead of quietly sitting outside a fixed
/// list nobody remembers to grow.
fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Whether `text` carries `root` in any of the shapes it could actually
/// take on disk: plain, with `/` standing in for `\`, escaped the way a
/// JSON file writes a backslash, and -- since Windows never tells two
/// spellings of one path apart by case -- compared without regard to case
/// either.
fn carries_absolute_path(text: &str, root: &std::path::Path) -> bool {
    let text = text.to_lowercase();
    let raw = root.to_string_lossy().to_lowercase();
    [
        raw.clone(),
        raw.replace('\\', "\\\\"),
        raw.replace('\\', "/"),
    ]
    .iter()
    .any(|form| !form.is_empty() && text.contains(form.as_str()))
}

/// A copy of `original`'s log, at a folder of its own sharing `original`'s
/// home -- a real copy of the tree, the same shape `check.rs`'s own
/// `a_copy_of` builds, not a fixture.
fn a_copy_of(original: &Sandbox, name: &str) -> Sandbox {
    let copy = Sandbox::new_empty_in(name, original.global_home());
    std::fs::create_dir_all(copy.0.join(".vivac")).unwrap();
    std::fs::copy(
        original.0.join(".vivac").join("events"),
        copy.0.join(".vivac").join("events"),
    )
    .unwrap();
    copy
}

/// 1: every `.vivac/` a `relocate`, a copy and a `--join` leave behind --
/// the origin's own renamed files, the destination's log and config, the
/// copy's log, and the joined lane's own lane file -- carries no absolute
/// path and no URL.
#[test]
fn every_vivac_directory_this_task_touches_carries_no_absolute_path_or_url() {
    let origin = Sandbox::new_seeded("sec-path-origin");
    origin.ok(&["push", "a goal", "--why", "seed"]);

    let destination = Sandbox::new_empty_in("sec-path-dest", origin.global_home());
    let (reloc_out, reloc_code) = origin.run(&[
        "relocate",
        destination.0.to_str().unwrap(),
        "--lane-name",
        "moved",
    ]);
    assert_eq!(reloc_code, 0, "{reloc_out}");

    let copy = a_copy_of(&destination, "sec-path-copy");
    let (copy_out, copy_code) = copy.run(&["check"]);
    assert_eq!(copy_code, 1, "{copy_out}");

    let joined = Sandbox::new_empty_in("sec-path-joined", origin.global_home());
    let (join_out, join_code) =
        joined.run(&["init", "--yes", "--join", destination.0.to_str().unwrap()]);
    assert_eq!(join_code, 0, "{join_out}");

    let roots = [&origin.0, &destination.0, &copy.0, &joined.0];
    for root in roots {
        for path in files_under(&root.join(".vivac")) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for candidate in roots {
                assert!(
                    !carries_absolute_path(&text, candidate),
                    "an absolute path leaked into {}:\n{text}",
                    path.display()
                );
            }
            assert!(
                !text.to_lowercase().contains("://"),
                "a url leaked into {}:\n{text}",
                path.display()
            );
        }
    }
}

/// 2: no text a `setup` refusal, a copy notice on `stderr`, or `relocate`'s
/// own output prints carries an absolute path -- every one of them names a
/// folder, never a path.
#[test]
fn no_printed_surface_this_task_added_names_an_absolute_path() {
    // A second-map refusal: two folders sharing one repository's root
    // commit. `d723` piece B: the guard is `init`'s alone now.
    let a = Sandbox::new_empty("sec-print-setup-a");
    git(&a.0, &["init", "-q"]);
    std::fs::write(a.0.join("f.txt"), "x").unwrap();
    git(&a.0, &["add", "."]);
    git(&a.0, &["commit", "-q", "-m", "first"]);
    a.ok(&["init", "--yes"]);

    let b = Sandbox::new_empty_in("sec-print-setup-b", a.global_home());
    let out = std::process::Command::new("git")
        .current_dir(&b.0)
        .args(["clone", "-q", a.0.to_str().unwrap(), "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (refusal, refusal_code) = b.run(&["init", "--yes"]);
    assert_eq!(refusal_code, 1, "{refusal}");
    assert!(refusal.contains("--join"), "{refusal}");

    // The copy notice, on stderr, from a write.
    let orig = Sandbox::new_seeded("sec-print-copy-orig");
    orig.ok(&["push", "a goal", "--why", "seed"]);
    let copy = a_copy_of(&orig, "sec-print-copy-copy");
    let (_, copy_stderr, copy_code) = run_split(&copy, &["push", "another", "--why", "seed"]);
    assert_eq!(copy_code, 0, "{copy_stderr}");
    assert!(
        copy_stderr.contains("COPY OF ANOTHER TREE"),
        "the write from a copy never warned:\n{copy_stderr}"
    );

    // `relocate`'s own output, given an absolute destination -- exercised
    // rather than evaded: the rule is not "never an absolute path" but
    // "never one this process built". Handing back exactly what the
    // caller typed is fine, and a relative destination could never tell
    // the two apart, since nothing it prints back could be absolute
    // either way.
    let reloc_origin = Sandbox::new_seeded("sec-print-reloc-origin");
    reloc_origin.ok(&["push", "a goal", "--why", "seed"]);
    let dest_name = format!(
        "sec-print-reloc-destination-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dest_abs = reloc_origin.0.parent().unwrap().join(&dest_name);
    let _cleanup = RemoveOnDrop(dest_abs.clone());
    let dest_typed = dest_abs.to_str().unwrap().to_string();
    let (reloc_out, reloc_code) = reloc_origin.run(&["relocate", &dest_typed]);
    assert_eq!(reloc_code, 0, "{reloc_out}");
    assert!(
        reloc_out.contains(&dest_typed),
        "relocate did not hand back the destination byte for byte as typed:\n{reloc_out}"
    );

    let texts = [&refusal, &copy_stderr];
    let roots = [&a.0, &b.0, &orig.0, &copy.0];
    for text in texts {
        for root in roots {
            let full = root.to_string_lossy();
            assert!(
                !text.contains(full.as_ref()),
                "an absolute path leaked:\n{text}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// `t594` tramo 4, task 7 (§9.2.18): the same class of check, stretched
// across the Emisores shape `tests/emisores.rs` exercises -- a root with no
// git of its own, several repositories below it, a branch that moves, and
// a linked worktree that joins its own lane.
// ---------------------------------------------------------------------------

/// A repository with one commit at `dir`, checked out onto `branch`.
fn commit_a_repo_on_branch(dir: &std::path::Path, branch: &str) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "first"]);
    git(dir, &["checkout", "-q", "-b", branch]);
}

/// Runs the binary in `dir` with `home` as `VIVAC_HOME`, combining stdout
/// and stderr -- for the linked worktree below, which sits outside the
/// `Sandbox` root `run_split` always answers from.
fn run_in(dir: &std::path::Path, home: &std::path::Path, args: &[&str]) -> (String, i32) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(dir)
        .env("VIVAC_HOME", home)
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
        o.status.code().unwrap_or(-1),
    )
}

fn ok_in(dir: &std::path::Path, home: &std::path::Path, args: &[&str]) -> String {
    let (s, code) = run_in(dir, home, args);
    assert_eq!(
        code,
        0,
        "`vivac {}` failed with {code}:\n{s}",
        args.join(" ")
    );
    s
}

/// A root with no git of its own, two repositories below it on different
/// branches, a branch that moves and comes back, and a linked worktree
/// that joins its own lane -- the Emisores shape, leaned down to what this
/// check needs: every surface this tranche added, at least once each.
#[test]
fn emisores_leaves_no_absolute_path_or_url_anywhere() {
    let c = Sandbox::new_empty("sec-emisores");
    let backend = c.0.join("backend");
    let web = c.0.join("web");
    commit_a_repo_on_branch(&backend, "feature/net10");
    commit_a_repo_on_branch(&web, "feature/ng22");

    // `init`'s own "vivac init, in <path>" header (`setup::init::full_plan`,
    // `f721`) is pre-existing and already accepted (`tests/init.rs`): it
    // confirms the folder the caller just ran it from, not something this
    // tranche's own surfaces leak. Left out of the scan below on purpose,
    // the same way `relocate`'s own destination is
    // (`no_printed_surface_this_task_added_names_an_absolute_path`, above).
    // §9.2.18 itself only asks this of the brief.
    c.ok(&["init", "--yes"]);
    let migrate_out = c.ok(&["push", "Migrate the backend", "--why", "seed"]);

    git(&backend, &["checkout", "-q", "-b", "perf/sp"]);
    let query_out = c.ok(&["push", "Optimize a query", "--why", "seed", "--root"]);

    git(&backend, &["checkout", "-q", "feature/net10"]);
    let brief_moved = c.ok(&["brief"]);
    assert!(
        brief_moved.contains("BRANCH MOVED"),
        "the scenario never reached the state this check means to cover:\n{brief_moved}"
    );

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
    let worktree_push_out = ok_in(
        &backend_worktree,
        c.global_home(),
        &["push", "Spike on the worktree", "--why", "seed"],
    );

    let printed = [&migrate_out, &query_out, &brief_moved, &worktree_push_out];
    let roots = [&c.0, &backend, &web, &backend_worktree];
    for text in printed {
        for root in roots {
            assert!(
                !carries_absolute_path(text, root),
                "an absolute path leaked into printed output:\n{text}"
            );
        }
        assert!(
            !text.to_lowercase().contains("://"),
            "a url leaked into printed output:\n{text}"
        );
    }

    for vivac_dir in [c.0.join(".vivac"), backend_worktree.join(".vivac")] {
        for path in files_under(&vivac_dir) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for root in roots {
                assert!(
                    !carries_absolute_path(&text, root),
                    "an absolute path leaked into {}:\n{text}",
                    path.display()
                );
            }
            assert!(
                !text.to_lowercase().contains("://"),
                "a url leaked into {}:\n{text}",
                path.display()
            );
        }
    }
}
