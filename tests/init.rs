//! `f566`: `init` on a tree that already exists must not touch its config.
//!
//! `Store::create` used to run unconditionally: a second `init` regenerated
//! the config with a fresh `project_id` and `actor`, and dropped `d444`'s
//! lock back to `1` on a tree that already held a pillar or a rule.

mod common;
use common::Sandbox;

const LOCK_SENTENCE: &str =
    "this tree holds pillars and rules, and this vivac is too old to read them: update vivac";

fn config_bytes(c: &Sandbox) -> Vec<u8> {
    std::fs::read(c.0.join(".vivac").join("config")).unwrap()
}

fn log_bytes(c: &Sandbox) -> Vec<u8> {
    std::fs::read(c.0.join(".vivac").join("events")).unwrap()
}

fn is_locked(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes).contains(LOCK_SENTENCE)
}

/// `f566`, reproduced exactly: init, a rule, init again. Config and log come
/// out byte for byte the same, lock included.
#[test]
fn a_second_init_leaves_a_locked_config_and_log_untouched() {
    let c = Sandbox::new_seeded("init-twice");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    let config_before = config_bytes(&c);
    let log_before = log_bytes(&c);
    assert!(
        is_locked(&config_before),
        "setup: the rule should have locked the config"
    );

    let out = c.ok(&["init"]);
    assert!(out.contains("vivac is already planted in"), "{out}");
    assert_eq!(config_before, config_bytes(&c), "the config moved");
    assert_eq!(log_before, log_bytes(&c), "the log moved");
}

/// An empty `.vivac/` holds no tree yet, so `init` plants one there rather
/// than calling it already planted.
#[test]
fn init_over_an_empty_vivac_directory_plants_a_tree() {
    let c = Sandbox::new_empty("init-empty-dir");
    std::fs::create_dir_all(c.0.join(".vivac")).unwrap();
    let out = c.ok(&["init"]);
    assert!(out.contains("vivac planted in"), "{out}");
    assert!(c.0.join(".vivac").join("config").is_file());
    assert!(c.0.join(".vivac").join("events").is_file());
}

/// `t594` §4.9: planting a tree writes `.vivac/.gitignore` alongside the
/// config and the log, so a fresh tree is never one `git add .` away from
/// being tracked.
#[test]
fn init_keeps_the_tree_out_of_version_control() {
    let c = Sandbox::new_seeded("gitignore");
    let g = std::fs::read_to_string(c.0.join(".vivac").join(".gitignore")).unwrap();
    assert_eq!(g, "*\n");
}

/// `init` opens rather than creates when there is something to open: a
/// `.vivac/` with a log and no config regenerates through `Store::open`,
/// which locks the regenerated config if the log already holds a rule.
#[test]
fn init_over_a_log_with_no_config_regenerates_it_locked() {
    let c = Sandbox::new_seeded("init-no-config");
    c.ok(&["add", "A rule", "--type", "rule", "--why", "guard"]);
    std::fs::remove_file(c.0.join(".vivac").join("config")).unwrap();

    c.ok(&["init"]);
    assert!(
        is_locked(&config_bytes(&c)),
        "a config regenerated over a governed log came back unlocked"
    );
}
