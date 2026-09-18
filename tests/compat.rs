//! Reading what 0.11.3 actually wrote, not a shape built to match today's
//! own reader.
//!
//! Every other regression fixture for an older log is grown on the fly by
//! `tests/common/mod.rs` (`append_unknown_event_type` and its neighbours):
//! it proves today's code agrees with itself, not that it can still read
//! what a published release put on disk. `tests/data/events-v0.11.3.jsonl`
//! and `tests/data/config-v0.11.3.json` close that gap with a real log.
//!
//! How they were made: the `vivac` on this machine's `PATH` is the
//! published 0.11.3 binary. With `VIVAC_HOME` pointed at an empty temporary
//! directory (never the real registry) and the working directory a fresh
//! temporary folder with no git repository underneath it, this ran:
//!
//! ```text
//! vivac init
//! vivac push "Ship the pinned fixture" --why "close a real test gap" --root
//! vivac add "Write the fixture log" --why "needs real 0.11.3 bytes"
//! vivac add "Review it before committing" --why "no secrets, no paths"
//! vivac save "checkpoint" --next "close the open child"
//! vivac pop "done"
//! ```
//!
//! `.vivac/events` and `.vivac/config` were copied out byte for byte and
//! reviewed before committing: every id is an opaque ULID `vivac` minted
//! itself, there is no git anchor (the folder was never a repository, so
//! there is nothing to have one), and no path, name or email appears
//! anywhere in either file.

mod common;
use common::Sandbox;

const PINNED_LOG: &str = include_str!("data/events-v0.11.3.jsonl");
const PINNED_CONFIG: &str = include_str!("data/config-v0.11.3.json");

/// A sandbox seeded with the pinned 0.11.3 tree instead of one `init` just
/// planted, so today's code is the one reading it for the first time.
fn seeded_from_the_pinned_log(name: &str) -> Sandbox {
    let c = Sandbox::new_empty(name);
    let dir = c.0.join(".vivac");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("events"), PINNED_LOG).unwrap();
    std::fs::write(dir.join("config"), PINNED_CONFIG).unwrap();
    c
}

#[test]
fn the_pinned_log_reads_back_as_the_tree_it_was_written_from() {
    let c = seeded_from_the_pinned_log("compat-read");
    assert_eq!(
        c.log(),
        PINNED_LOG,
        "the file on disk moved before the read"
    );

    let out = c.ok(&["tree"]);
    assert!(out.contains("Ship the pinned fixture"), "{out}");
    assert!(out.contains("Write the fixture log"), "{out}");
    assert!(out.contains("Review it before committing"), "{out}");
}

#[test]
fn reading_the_pinned_log_adds_no_event_and_leaves_its_config_alone() {
    let c = seeded_from_the_pinned_log("compat-read-is-silent");

    // `stack` is a plain read, and `newer_vivac.rs` already relies on a
    // plain read being the moment a derived index gets persisted -- if
    // opening a log this old silently rewrote anything, this is where it
    // would show.
    c.ok(&["stack"]);

    assert_eq!(c.log(), PINNED_LOG, "a read appended to the log");
    let config_after = std::fs::read_to_string(c.0.join(".vivac").join("config")).unwrap();
    assert_eq!(config_after, PINNED_CONFIG, "a read rewrote the config");
}

#[test]
fn setup_over_the_pinned_log_leaves_check_clean() {
    let c = seeded_from_the_pinned_log("compat-setup-then-check");
    c.ok(&["setup", "claude-code", "--yes"]);

    let (out, code) = c.run(&["check"]);
    assert_eq!(code, 0, "{out}");
}
