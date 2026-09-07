//! `vivac why <id> --project <p>` -- `d273`'s second half: a hit
//! `find --everywhere` returns is addressable, not just visible.
//!
//! `find --everywhere` already answers from the registry with no tree
//! underfoot; this is what lets an agent open one of its hits. `--project`
//! resolves the same way a hit is named -- a registry name, the bare
//! directory name `find --everywhere` prints, or a path -- and the tree
//! loads through the local index with `allow_persist: false`, the same rule
//! `find --everywhere` follows: opening another project's node must never
//! write inside that project's `.vivac/`.

mod common;
use common::Sandbox;

fn project_name(c: &Sandbox) -> String {
    c.0.file_name().unwrap().to_string_lossy().into_owned()
}

/// Pushes one node and registers the project. Registration is a side effect
/// of *using* a project (`d201`), and the push that seeds a fresh store
/// cannot trigger its own: at the moment it runs, the event it is about to
/// write is not on disk yet, so there is no first event id to key the
/// registry by. The `stack` after it is the first command that finds one.
fn seed(c: &Sandbox, title: &str, why: &str) {
    c.ok(&["push", title, "--why", why]);
    c.ok(&["stack"]);
}

#[test]
fn why_project_by_name_returns_the_other_trees_node() {
    let a = Sandbox::new_seeded("wp-name-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let b = Sandbox::new_seeded_in("wp-name-b", a.global_home());
    seed(
        &b,
        "Guard the commit messages",
        "a malformed one does not count",
    );
    let name_a = project_name(&a);

    let s = b.ok(&["why", "1", "--project", &name_a]);

    assert!(s.contains("Ship the release apparatus"), "{s}");
}

#[test]
fn why_without_project_is_unchanged() {
    let a = Sandbox::new_seeded("wp-unchanged-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );

    let s = a.ok(&["why", "1"]);

    assert!(s.contains("Ship the release apparatus"), "{s}");
}

#[test]
fn why_project_by_path_works_the_same_way() {
    let a = Sandbox::new_seeded("wp-path-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let b = Sandbox::new_seeded_in("wp-path-b", a.global_home());
    seed(
        &b,
        "Guard the commit messages",
        "a malformed one does not count",
    );

    let s = b.ok(&["why", "1", "--project", a.0.to_str().unwrap()]);

    assert!(s.contains("Ship the release apparatus"), "{s}");
}

/// A directory name nothing else in this file takes, so two projects can be
/// planted under the exact same base name -- the case `Sandbox::new_seeded`
/// cannot build, since its own uniqueness lives in the name it is given.
fn unique(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("vivac-wp-{name}-{}-{n}-{ts}", std::process::id()))
}

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

fn run(dir: &std::path::Path, home: &std::path::Path, args: &[&str]) -> (String, i32) {
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

/// `d100` already names three different decisions across three real trees
/// sharing an alias; two directories called `api` are the same shape, one
/// level up. Guessing which one a bare name means would answer a question
/// about the wrong tree and look right, so this refuses instead.
#[test]
fn an_ambiguous_name_refuses_with_exit_2_and_names_the_candidates() {
    let home = unique("ambig-home");
    let parent_x = unique("ambig-x");
    let parent_y = unique("ambig-y");
    let proj_x = parent_x.join("api");
    let proj_y = parent_y.join("api");
    std::fs::create_dir_all(&proj_x).unwrap();
    std::fs::create_dir_all(&proj_y).unwrap();

    let (out, code) = run(&proj_x, &home, &["init"]);
    assert_eq!(code, 0, "{out}");
    run(&proj_x, &home, &["push", "In x", "--why", "a"]);
    run(&proj_x, &home, &["stack"]);

    let (out, code) = run(&proj_y, &home, &["init"]);
    assert_eq!(code, 0, "{out}");
    run(&proj_y, &home, &["push", "In y", "--why", "b"]);
    run(&proj_y, &home, &["stack"]);

    let (s, code) = run(&proj_x, &home, &["why", "1", "--project", "api"]);

    assert_eq!(code, 2, "{s}");
    assert!(s.contains('2'), "the candidate count is missing:\n{s}");
    assert!(
        !s.contains(&proj_x.display().to_string()) && !s.contains(&proj_y.display().to_string()),
        "a path leaked into the refusal, which the security pillar forbids:\n{s}"
    );

    std::fs::remove_dir_all(&parent_x).ok();
    std::fs::remove_dir_all(&parent_y).ok();
    std::fs::remove_dir_all(&home).ok();
}

/// `--full` needs the raw log (`Full::from_log`), and a foreign project's
/// log is never read that way: `index::load(&store, false)` hands back a
/// folded `Tree`, not the events it was folded from. The two flags refuse
/// each other rather than `--full` silently answering half its question.
#[test]
fn project_and_full_together_refuse_rather_than_answer_half() {
    let a = Sandbox::new_seeded("wp-full-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let b = Sandbox::new_seeded_in("wp-full-b", a.global_home());
    let name_a = project_name(&a);

    let (s, code) = b.run(&["why", "1", "--project", &name_a, "--full"]);

    assert_eq!(code, 2, "--project --full should have refused:\n{s}");
    assert!(
        s.contains("--full"),
        "the refusal does not name --full:\n{s}"
    );
}

#[test]
fn an_unknown_name_refuses_rather_than_answering_empty() {
    let a = Sandbox::new_seeded("wp-unknown");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );

    let (s, code) = a.run(&["why", "1", "--project", "does-not-exist-anywhere"]);

    assert_ne!(
        code, 0,
        "an unknown project name answered as if it had succeeded:\n{s}"
    );
}

/// The constraint `d273` exists to guard: opening another project's node
/// must never write inside that project's `.vivac/`. `allow_persist: false`
/// is the mechanism; this is what would catch it slipping, the same shape
/// `the_foreign_index_is_never_written` uses in `tests/find.rs`.
#[test]
fn the_foreign_index_is_never_written() {
    let a = Sandbox::new_seeded("wp-guard-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    // Forces `a`'s derived index onto disk: a plain read persists it on the
    // first load, and `push` inside `seed` only ever loaded for a write.
    a.ok(&["stack"]);
    let index_path = a.0.join(".vivac").join("index");
    let before = std::fs::metadata(&index_path).expect("no index to guard");

    let b = Sandbox::new_seeded_in("wp-guard-b", a.global_home());
    let name_a = project_name(&a);
    let s = b.ok(&["why", "1", "--project", &name_a]);
    assert!(s.contains("Ship the release apparatus"), "{s}");

    let after = std::fs::metadata(&index_path).expect("the index disappeared");
    assert_eq!(before.len(), after.len(), "the foreign index changed size");
    assert_eq!(
        before.modified().unwrap(),
        after.modified().unwrap(),
        "the foreign index was rewritten"
    );
}

/// `why`'s own shape carries no `project` field at all, unlike a
/// `find --everywhere` hit; this is what would catch one sneaking in with a
/// path inside it, wherever it landed.
#[test]
fn why_project_output_carries_no_path_separator_in_any_project_field() {
    let a = Sandbox::new_seeded("wp-nosep-a");
    seed(
        &a,
        "Ship the release apparatus",
        "the version was a hand edit",
    );
    let b = Sandbox::new_seeded_in("wp-nosep-b", a.global_home());
    let name_a = project_name(&a);

    let s = b.ok(&["why", "1", "--project", &name_a, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("the payload is not JSON");

    assert!(
        !s.contains(&a.0.display().to_string()),
        "the foreign project's absolute path leaked into `why`'s output:\n{s}"
    );
    fn walk(x: &serde_json::Value) {
        match x {
            serde_json::Value::Object(map) => {
                if let Some(p) = map.get("project").and_then(|p| p.as_str()) {
                    assert!(
                        !p.contains('/') && !p.contains('\\'),
                        "a project field carried a path: {p}"
                    );
                }
                for v in map.values() {
                    walk(v);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(walk),
            _ => {}
        }
    }
    walk(&v);
}
