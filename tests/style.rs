//! `d792`: styles are added on top of the same plain text, never in place
//! of it, and only when the stream actually asked for them.
//!
//! The suite itself runs with no terminal behind its pipes, so every other
//! integration test already proves the plain half of this by construction
//! -- none of them has ever had to strip an escape code out of an
//! assertion. What is left to prove here is the other half: that
//! `CLICOLOR_FORCE` turns styling on without touching a single word, and
//! that `NO_COLOR` still wins once both are set.

#[path = "common/mod.rs"]
mod common;
use common::Sandbox;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

fn run_with_env(
    dir: &std::path::Path,
    home: &std::path::Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> (String, i32) {
    let mut cmd = std::process::Command::new(BIN);
    cmd.current_dir(dir).env("VIVAC_HOME", home).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
        out.status.code().unwrap_or(-1),
    )
}

/// Strips every `\x1b[...m` SGR escape this crate ever emits, byte for
/// byte: the inverse of what `style::span` adds.
fn strip_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&d) = chars.peek() {
                chars.next();
                if d == 'm' {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// (a): the suite's own child processes run with no terminal on either
/// end of the pipe, so a plain run never carries an escape code at all --
/// no `CLICOLOR_FORCE` has to say so for it.
#[test]
fn plain_output_carries_no_escape_code() {
    let c = Sandbox::new_empty("style-plain");
    let out = c.ok(&["init", "--yes"]);
    assert!(!out.contains('\x1b'), "{out:?}");
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(!out.contains('\x1b'), "{out:?}");
}

/// (b): `CLICOLOR_FORCE` styles the very same words a plain run prints,
/// never different ones -- stripping its codes back out has to land on
/// the plain run byte for byte.
#[test]
fn clicolor_force_styles_the_same_words_the_plain_run_has() {
    let c = Sandbox::new_empty("style-clicolor-force");
    let (plain, plain_code) = run_with_env(&c.0, c.global_home(), &["init", "--dry-run"], &[]);
    assert_eq!(plain_code, 0, "{plain}");

    let (forced, forced_code) = run_with_env(
        &c.0,
        c.global_home(),
        &["init", "--dry-run"],
        &[("CLICOLOR_FORCE", "1")],
    );
    assert_eq!(forced_code, 0, "{forced}");
    assert!(
        forced.contains('\x1b'),
        "CLICOLOR_FORCE=1 added no codes:\n{forced:?}"
    );
    assert_eq!(
        strip_codes(&forced),
        plain,
        "styled and plain disagree once the codes are gone"
    );
}

/// (c): `NO_COLOR` wins over `CLICOLOR_FORCE` -- the one env var that can
/// never be overridden into styling, because it is the one a person sets
/// to say their terminal cannot render it at all.
#[test]
fn no_color_wins_over_clicolor_force() {
    let c = Sandbox::new_empty("style-no-color-wins");
    let (out, code) = run_with_env(
        &c.0,
        c.global_home(),
        &["init", "--dry-run"],
        &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")],
    );
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains('\x1b'), "{out:?}");
}

/// (d): every successful `init` ends with a `Next:` block naming both
/// harnesses `setup` knows, whether it planted or joined.
#[test]
fn init_ends_with_the_next_block_naming_both_harnesses() {
    let c = Sandbox::new_empty("style-init-next-block");
    let out = c.ok(&["init", "--yes"]);
    assert!(out.contains("Next:"), "{out}");
    assert!(out.contains("vivac setup claude-code"), "{out}");
    assert!(out.contains("vivac setup codex"), "{out}");
}
