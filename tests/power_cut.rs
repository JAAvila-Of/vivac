//! What a power cut can leave in a tree, and what opens anyway (`d946`).
//!
//! The log is not synced on append, on purpose, so the last seconds before a
//! cut may come back torn, zeroed or unreadable. Every state below was
//! measured by injecting it into a sandbox tree (`f939`-`f942`): a reader
//! skips what it cannot read, `check` reads the log rather than trusting the
//! index, and a `config` or `lane` file that holds nothing says what it is
//! instead of a bare parser error.

mod common;
use common::Sandbox;
use std::path::{Path, PathBuf};

/// Three nodes after the goal: t2 (pushed), t3 and t4 (added under t2). The
/// line of `t3` is the one every test below damages.
fn seeded_with_nodes(name: &str) -> Sandbox {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "Alpha node", "--why", "the first reason"]);
    c.ok(&["push", "Beta node", "--why", "the second reason"]);
    c.ok(&["add", "Gamma node", "--why", "the third reason"]);
    c.ok(&["add", "Delta node", "--why", "the fourth reason"]);
    c
}

fn log_path(c: &Sandbox) -> PathBuf {
    c.0.join(".vivac").join("events")
}

fn index_path(c: &Sandbox) -> PathBuf {
    c.0.join(".vivac").join("index")
}

/// Replaces the one complete line that mentions `needle` with `bytes`,
/// keeping the newline that ends it.
fn replace_line(c: &Sandbox, needle: &str, bytes: &[u8]) {
    let raw = std::fs::read(log_path(c)).unwrap();
    let mut out = Vec::new();
    let mut hit = 0;
    for line in raw.split_inclusive(|b| *b == b'\n') {
        if String::from_utf8_lossy(line).contains(needle) {
            out.extend_from_slice(bytes);
            out.push(b'\n');
            hit += 1;
        } else {
            out.extend_from_slice(line);
        }
    }
    assert_eq!(hit, 1, "the fixture expects exactly one line with {needle}");
    std::fs::write(log_path(c), out).unwrap();
}

fn line_len(c: &Sandbox, needle: &str) -> usize {
    let raw = std::fs::read(log_path(c)).unwrap();
    raw.split_inclusive(|b| *b == b'\n')
        .find(|l| String::from_utf8_lossy(l).contains(needle))
        .unwrap()
        .len()
        - 1
}

const NOT_UTF8: &[u8] = b"\xff\xfe this line is not text \xc3";

/// Everything a person reads out of a tree that lost `t3`, and what it says.
fn assert_opens_without_the_lost_node(c: &Sandbox) {
    let (s, code) = c.run(&["brief"]);
    assert_eq!(code, 0, "{s}");

    let (s, code) = c.run(&["why", "t4"]);
    assert_eq!(code, 0, "{s}");
    assert!(s.contains("Delta node"), "{s}");

    let (s, code) = c.run(&["find", "Delta"]);
    assert_eq!(code, 0, "{s}");
    assert!(s.contains("Delta node"), "{s}");

    let (s, code) = c.run(&["find", "Gamma"]);
    assert_eq!(code, 0, "{s}");
    assert!(!s.contains("Gamma node"), "{s}");

    let (s, code) = c.run(&["tree"]);
    assert_eq!(code, 0, "{s}");
    assert!(s.contains("Alpha node") && s.contains("Beta node"), "{s}");
    assert!(s.contains("Delta node"), "{s}");

    let (s, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{s}");
    assert!(s.contains("1 unreadable line(s)"), "{s}");

    let (s, code) = c.run(&["note", "t4", "written after the damage"]);
    assert_eq!(code, 0, "{s}");
}

#[test]
fn an_unreadable_line_with_no_index_leaves_the_tree_open() {
    let c = seeded_with_nodes("power-cut-utf8");
    std::fs::remove_file(index_path(&c)).ok();
    replace_line(&c, "Gamma node", NOT_UTF8);
    assert!(!index_path(&c).exists());

    assert_opens_without_the_lost_node(&c);
}

#[test]
fn an_unreadable_line_with_an_index_built_before_it_and_then_deleted_leaves_the_tree_open() {
    let c = seeded_with_nodes("power-cut-utf8-index");
    c.ok(&["brief"]);
    assert!(index_path(&c).is_file(), "a read builds the index");
    replace_line(&c, "Gamma node", NOT_UTF8);
    std::fs::remove_file(index_path(&c)).unwrap();

    assert_opens_without_the_lost_node(&c);
}

#[test]
fn check_names_a_node_the_index_still_holds_and_the_log_no_longer_does() {
    let c = seeded_with_nodes("power-cut-check-index");
    c.ok(&["brief"]);
    assert!(index_path(&c).is_file(), "a read builds the index");
    let blank = vec![0u8; line_len(&c, "Gamma node")];
    replace_line(&c, "Gamma node", &blank);
    let index_before = std::fs::read(index_path(&c)).unwrap();

    let (s, code) = c.run(&["check"]);

    assert_eq!(code, 1, "{s}");
    assert!(s.contains("1 unreadable line(s)"), "{s}");
    assert!(
        s.contains("the index still holds t3, whose line in .vivac/events can no longer be read"),
        "{s}"
    );
    assert_eq!(
        std::fs::read(index_path(&c)).unwrap(),
        index_before,
        "check writes nothing"
    );
}

#[test]
fn check_on_a_healthy_tree_with_an_index_says_what_it_always_said() {
    let c = seeded_with_nodes("power-cut-check-healthy");
    c.ok(&["brief"]);
    assert!(index_path(&c).is_file(), "a read builds the index");

    let (s, code) = c.run(&["check"]);

    assert_eq!(code, 0, "{s}");
    assert!(s.contains("No findings. 4 nodes checked."), "{s}");
    assert!(!s.contains("the index still holds"), "{s}");
    assert!(!s.contains("STORE"), "{s}");
}

#[test]
fn check_does_not_write_the_index_when_there_is_none() {
    let c = seeded_with_nodes("power-cut-check-no-write");
    std::fs::remove_file(index_path(&c)).ok();

    c.run(&["check"]);

    assert!(
        !index_path(&c).exists(),
        "check is a read that writes nothing"
    );
}

// ---------------------------------------------------------------------------
// A `config` or `lane` file that holds nothing.
// ---------------------------------------------------------------------------

fn config_path(c: &Sandbox) -> PathBuf {
    c.0.join(".vivac").join("config")
}

/// Every name in `.vivac/`, so a test can say nothing new appeared.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn assert_config_refused(c: &Sandbox, config_bytes: &[u8], error: &str) {
    std::fs::write(config_path(c), config_bytes).unwrap();
    let log_before = std::fs::read(log_path(c)).unwrap();
    let names_before = names_in(&c.0.join(".vivac"));
    let sentence = format!(
        ".vivac/config cannot be read ({error}), so this tree cannot be opened. The log in \
         .vivac/events is untouched and nothing was written. Moving the file aside lets \
         vivac rebuild it from the log, with a new project id and closed to other \
         projects: vivac share on opens it again."
    );

    for verb in [
        vec!["brief"],
        vec!["open"],
        vec!["check"],
        vec!["why", "t2"],
        vec!["note", "t2", "a note"],
        vec!["push", "Another", "--why", "a reason"],
    ] {
        let (s, code) = c.run(&verb);
        assert_eq!(code, 5, "{verb:?}: {s}");
        assert!(s.contains(&sentence), "{verb:?}: {s}");
        assert!(s.contains("Input/output error: "), "{verb:?}: {s}");
    }

    assert_eq!(std::fs::read(log_path(c)).unwrap(), log_before);
    assert_eq!(names_in(&c.0.join(".vivac")), names_before);
    assert_eq!(std::fs::read(config_path(c)).unwrap(), config_bytes);
}

#[test]
fn an_empty_config_says_what_it_is_for_every_verb() {
    let c = seeded_with_nodes("power-cut-config-empty");
    assert_config_refused(&c, b"", "EOF while parsing a value at line 1 column 0");
}

#[test]
fn an_all_zero_config_says_what_it_is_for_every_verb() {
    let c = seeded_with_nodes("power-cut-config-nul");
    assert_config_refused(&c, &[0u8; 64], "expected value at line 1 column 1");
}

#[test]
fn a_newer_config_keeps_its_own_refusal() {
    let c = seeded_with_nodes("power-cut-config-newer");
    std::fs::write(config_path(&c), "{\"version\": 9}\n").unwrap();

    let (s, code) = c.run(&["brief"]);

    assert_eq!(code, 5, "{s}");
    assert!(
        s.contains("This tree was written by a newer vivac: its config has version 9"),
        "{s}"
    );
    assert!(!s.contains("cannot be read"), "{s}");
}

/// A tree, and a folder that joined it as a lane of its own.
fn joined(name: &str) -> (Sandbox, Sandbox) {
    let tree = seeded_with_nodes(name);
    let folder = Sandbox::new_empty_in(&format!("{name}-lane"), tree.global_home());
    folder.ok(&["init", "--yes", "--join", tree.0.to_str().unwrap()]);
    assert!(folder.0.join(".vivac").join("lane").is_file());
    (tree, folder)
}

fn lane_sentence(error: &str) -> String {
    format!(
        ".vivac/lane cannot be read ({error}), so this folder does not know which lane of \
         which tree it is. Nothing was written. Moving the file aside and running  vivac \
         init --join <folder that holds the tree> --yes  joins it again, as a new lane."
    )
}

fn assert_lane_refused_then_joined(tree: &Sandbox, folder: &Sandbox, bytes: &[u8], error: &str) {
    let lane = folder.0.join(".vivac").join("lane");
    std::fs::write(&lane, bytes).unwrap();
    let names_before = names_in(&folder.0.join(".vivac"));
    let log_before = std::fs::read(log_path(tree)).unwrap();

    for verb in [vec!["brief"], vec!["open"], vec!["why", "t2"]] {
        let (s, code) = folder.run(&verb);
        assert_eq!(code, 5, "{verb:?}: {s}");
        assert!(s.contains(&lane_sentence(error)), "{verb:?}: {s}");
        assert!(s.contains("Input/output error: "), "{verb:?}: {s}");
    }

    assert_eq!(std::fs::read(&lane).unwrap(), bytes);
    assert_eq!(names_in(&folder.0.join(".vivac")), names_before);
    assert_eq!(std::fs::read(log_path(tree)).unwrap(), log_before);

    // The way out the sentence names.
    std::fs::rename(&lane, folder.0.join(".vivac").join("lane.aside")).unwrap();
    let (s, code) = folder.run(&["init", "--join", tree.0.to_str().unwrap(), "--yes"]);
    assert_eq!(code, 0, "{s}");
    let (s, code) = folder.run(&["brief"]);
    assert_eq!(code, 0, "{s}");
}

#[test]
fn an_empty_lane_file_says_what_it_is_and_joining_again_works() {
    let (tree, folder) = joined("power-cut-lane-empty");
    assert_lane_refused_then_joined(
        &tree,
        &folder,
        b"",
        "EOF while parsing a value at line 1 column 0",
    );
}

#[test]
fn an_all_zero_lane_file_says_what_it_is_and_joining_again_works() {
    let (tree, folder) = joined("power-cut-lane-nul");
    assert_lane_refused_then_joined(
        &tree,
        &folder,
        &[0u8; 64],
        "expected value at line 1 column 1",
    );
}

#[test]
fn a_newer_lane_keeps_its_own_refusal() {
    let (_tree, folder) = joined("power-cut-lane-newer");
    let lane = folder.0.join(".vivac").join("lane");
    std::fs::write(
        &lane,
        "{\"version\": 9, \"id\": \"x\", \"project\": \"y\"}\n",
    )
    .unwrap();

    let (s, code) = folder.run(&["brief"]);

    assert_eq!(code, 5, "{s}");
    assert!(
        s.contains("This tree was written by a newer vivac: .vivac/lane has version 9"),
        "{s}"
    );
    assert!(!s.contains("cannot be read"), "{s}");
}

/// `f941`: a config that exists and cannot be read as text is not a missing
/// one. It used to be regenerated in place, with a new project id.
#[test]
fn a_config_that_is_not_utf8_fails_and_is_never_regenerated() {
    let c = seeded_with_nodes("power-cut-config-bytes");
    assert_config_refused(&c, b"\xff\xfe", "stream did not contain valid UTF-8");
}
