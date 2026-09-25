//! `d795`: `tree`, `open` and `why` colour an alias by its kind and wrap a
//! long title at the terminal's own width, on top of the same plain words
//! a pipe, a hook or `--json` still gets untouched. `tests/style.rs`
//! already proves that half of the shape for the plans `setup` prints;
//! this proves it for the three reads and for the redaction refusal.
//!
//! Every read below runs against a tree seeded with one root and one
//! child, each carrying a title built from a short marker word followed
//! by one single word eighty characters long. `style::wrap_title` keeps a
//! word that long whole rather than splitting it, so it lands on a line
//! of its own regardless of which command is asking or how much lead it
//! prints ahead of the title -- the split point never has to be guessed
//! at, only found.

#[path = "common/mod.rs"]
mod common;
use common::Sandbox;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

const ROOT_MARK: &str = "ZQXROOTMARKER";
const CHILD_MARK: &str = "ZQXCHILDMARKER";

fn root_title() -> String {
    format!("{ROOT_MARK} {}", "a".repeat(80))
}

fn child_title() -> String {
    format!("{CHILD_MARK} {}", "b".repeat(80))
}

/// Seeds `c` with one root goal (`g1`) and one child task (`t2`), each
/// carrying one of the two overlong titles above.
fn seed(c: &Sandbox) {
    c.ok(&["push", &root_title(), "--why", "testing"]);
    c.ok(&["push", &child_title(), "--why", "testing"]);
}

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
/// byte -- the same helper `tests/style.rs` already carries, kept as its
/// own copy here since neither file exports one for the other to share.
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

/// The column of the first character that is neither a space nor `tree`'s
/// own dim `|` connector -- where a continuation line's real text starts,
/// whether that line belongs to `tree` (which draws one ahead of it) or
/// to `open` and `why` (which never do).
fn text_column(line: &str) -> usize {
    line.chars()
        .position(|c| c != ' ' && c != '|')
        .unwrap_or(line.len())
}

/// The zero-based line index and column of the first line, ANSI stripped,
/// that carries `marker`. Panics with the whole output on a miss: every
/// test below controls exactly what it seeded, so a miss is a bug in the
/// test, not a real "not found".
fn marker_line(lines: &[String], marker: &str) -> (usize, usize) {
    for (i, l) in lines.iter().enumerate() {
        if let Some(col) = l.find(marker) {
            return (i, col);
        }
    }
    panic!("{marker:?} not found in:\n{}", lines.join("\n"));
}

/// Runs `args` under `CLICOLOR_FORCE=1` and `COLUMNS=60`, and asserts that
/// the line carrying `marker` wraps and that the very next line -- its
/// continuation, since nothing else ever prints between a title's own
/// wrapped lines -- starts its real text at the same column `marker`
/// itself started at.
fn assert_wraps_and_aligns(c: &Sandbox, args: &[&str], marker: &str) -> String {
    let (raw, code) = run_with_env(
        &c.0,
        c.global_home(),
        args,
        &[("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")],
    );
    assert_eq!(code, 0, "{raw}");
    assert!(raw.contains('\x1b'), "{raw:?}");
    let stripped = strip_codes(&raw);
    let lines: Vec<String> = stripped.lines().map(str::to_string).collect();
    let (idx, col) = marker_line(&lines, marker);
    let cont = lines
        .get(idx + 1)
        .unwrap_or_else(|| panic!("{marker} has no line after it:\n{stripped}"));
    assert_eq!(
        text_column(cont),
        col,
        "{marker}'s continuation did not align:\n{stripped}"
    );
    raw
}

// (a) and (b): a long title wraps in `tree`, `open` and `why` alike, and
// its continuation aligns under the column the title started at.

#[test]
fn tree_wraps_a_long_title_and_the_continuation_aligns_under_it() {
    let c = Sandbox::new_seeded("styled-tree-wrap");
    seed(&c);
    let raw = assert_wraps_and_aligns(&c, &["tree"], ROOT_MARK);
    // `tree`'s own continuation prefix is dim in the raw output, ahead of
    // any visible character -- `open` and `why` carry no such prefix, so
    // this half is checked here alone.
    let stripped = strip_codes(&raw);
    let idx = stripped
        .lines()
        .position(|l| l.contains(ROOT_MARK))
        .unwrap();
    let raw_cont = raw.lines().nth(idx + 1).unwrap();
    assert!(raw_cont.starts_with("\x1b[2m"), "{raw_cont:?}");
}

#[test]
fn open_wraps_a_long_title_and_the_continuation_aligns_under_it() {
    let c = Sandbox::new_seeded("styled-open-wrap");
    seed(&c);
    assert_wraps_and_aligns(&c, &["open"], CHILD_MARK);
}

#[test]
fn why_wraps_every_rows_title_and_each_continuation_aligns_under_it() {
    let c = Sandbox::new_seeded("styled-why-wrap");
    seed(&c);
    let (raw, code) = run_with_env(
        &c.0,
        c.global_home(),
        &["why", "t2"],
        &[("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")],
    );
    assert_eq!(code, 0, "{raw}");
    let stripped = strip_codes(&raw);
    let lines: Vec<String> = stripped.lines().map(str::to_string).collect();
    for marker in [ROOT_MARK, CHILD_MARK] {
        let (idx, col) = marker_line(&lines, marker);
        let cont = &lines[idx + 1];
        assert_eq!(
            text_column(cont),
            col,
            "{marker}'s continuation did not align:\n{stripped}"
        );
    }
}

// (c): an alias carries an escape code for its own kind's colour.

#[test]
fn aliases_carry_their_kind_colour() {
    let c = Sandbox::new_seeded("styled-kind-colour");
    seed(&c);
    let (raw, code) = run_with_env(&c.0, c.global_home(), &["tree"], &[("CLICOLOR_FORCE", "1")]);
    assert_eq!(code, 0, "{raw}");
    assert!(
        raw.contains("\x1b[35m"),
        "g1 is a goal, missing magenta:\n{raw}"
    );
    assert!(
        raw.contains("\x1b[36m"),
        "t2 is a task, missing cyan:\n{raw}"
    );
}

// (d): no line, ANSI stripped, exceeds 59 columns except a single word
// too long for the room -- the same rule `wrap_title` itself keeps.

fn assert_no_overlong_lines(label: &str, out: &str) {
    let stripped = strip_codes(out);
    for line in stripped.lines() {
        // `tree`'s own footer is fixed prose this task only dims -- `d795`
        // never asked it to wrap, and it was already past 59 columns
        // before this task touched anything.
        if line
            .trim_start()
            .starts_with("(closed nodes with no open descendants hidden")
        {
            continue;
        }
        if line.chars().count() > 59 {
            // `tree`'s own drawing characters (`|`, `` ` ``, `-`) sit ahead
            // of the real content on every line, continuation or not --
            // skip past them the same way `text_column` does before
            // asking whether what is left is a single word.
            let content: String = line.chars().skip(text_column(line)).collect();
            assert!(
                content.split_whitespace().count() <= 1,
                "{label}: a line over 59 columns is not a single word:\n{line:?}"
            );
        }
    }
}

#[test]
fn no_line_exceeds_the_terminal_width_except_a_single_overlong_word() {
    let c = Sandbox::new_seeded("styled-width-cap");
    seed(&c);
    let env = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];

    let (tree_out, code) = run_with_env(&c.0, c.global_home(), &["tree"], &env);
    assert_eq!(code, 0, "{tree_out}");
    assert_no_overlong_lines("tree", &tree_out);

    let (open_out, code) = run_with_env(&c.0, c.global_home(), &["open"], &env);
    assert_eq!(code, 0, "{open_out}");
    assert_no_overlong_lines("open", &open_out);

    let (why_t2, code) = run_with_env(&c.0, c.global_home(), &["why", "t2"], &env);
    assert_eq!(code, 0, "{why_t2}");
    assert_no_overlong_lines("why t2", &why_t2);

    // `g1`'s own "Born here and still open" list is only shown asking
    // about `g1` itself.
    let (why_g1, code) = run_with_env(&c.0, c.global_home(), &["why", "g1"], &env);
    assert_eq!(code, 0, "{why_g1}");
    assert_no_overlong_lines("why g1", &why_g1);
}

// (e): `CLICOLOR_FORCE` with no `COLUMNS` styles, but nothing wraps --
// there is no width to wrap against.

#[test]
fn without_columns_it_styles_but_never_wraps() {
    let c = Sandbox::new_seeded("styled-no-columns");
    seed(&c);
    let env = [("CLICOLOR_FORCE", "1")];

    let (tree_out, code) = run_with_env(&c.0, c.global_home(), &["tree"], &env);
    assert_eq!(code, 0, "{tree_out}");
    assert!(tree_out.contains('\x1b'), "{tree_out:?}");
    assert!(
        strip_codes(&tree_out).contains(&root_title()),
        "the title wrapped with no known width:\n{tree_out}"
    );

    let (why_out, code) = run_with_env(&c.0, c.global_home(), &["why", "t2"], &env);
    assert_eq!(code, 0, "{why_out}");
    assert!(why_out.contains('\x1b'), "{why_out:?}");
    assert!(
        strip_codes(&why_out).contains(&child_title()),
        "the title wrapped with no known width:\n{why_out}"
    );
}

// (f): without `CLICOLOR_FORCE`, and with no terminal behind the pipe
// either -- every command in this suite runs that way -- there is no
// escape byte anywhere in the output.

#[test]
fn without_clicolor_force_there_is_no_escape_byte_at_all() {
    let c = Sandbox::new_seeded("styled-no-force");
    seed(&c);
    let tree_out = c.ok(&["tree"]);
    assert!(!tree_out.contains('\x1b'), "{tree_out:?}");
    let open_out = c.ok(&["open"]);
    assert!(!open_out.contains('\x1b'), "{open_out:?}");
    let why_out = c.ok(&["why", "t2"]);
    assert!(!why_out.contains('\x1b'), "{why_out:?}");
}

// (g): `--json` never carries an escape code, even with `CLICOLOR_FORCE`.

#[test]
fn json_never_carries_an_escape_code_even_with_clicolor_force() {
    let c = Sandbox::new_seeded("styled-json-no-escape");
    seed(&c);
    let env = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];

    let (tree_out, code) = run_with_env(&c.0, c.global_home(), &["tree", "--json"], &env);
    assert_eq!(code, 0, "{tree_out}");
    assert!(!tree_out.contains('\x1b'), "{tree_out:?}");

    let (open_out, code) = run_with_env(&c.0, c.global_home(), &["open", "--json"], &env);
    assert_eq!(code, 0, "{open_out}");
    assert!(!open_out.contains('\x1b'), "{open_out:?}");

    let (why_out, code) = run_with_env(&c.0, c.global_home(), &["why", "t2", "--json"], &env);
    assert_eq!(code, 0, "{why_out}");
    assert!(!why_out.contains('\x1b'), "{why_out:?}");
}

// (h): the redaction refusal on stderr is styled under `CLICOLOR_FORCE`
// and plain without it -- the same words either way, once the codes are
// stripped back out.

#[test]
fn the_redaction_refusal_is_styled_with_clicolor_force_and_plain_without() {
    let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345";
    let c = Sandbox::new_empty("styled-refusal");

    let (plain, code) = c.run(&["init", "--yes", "--name", secret]);
    assert_eq!(code, 3, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains("Refused:"), "{plain}");

    let (styled, code) = run_with_env(
        &c.0,
        c.global_home(),
        &["init", "--yes", "--name", secret],
        &[("CLICOLOR_FORCE", "1")],
    );
    assert_eq!(code, 3, "{styled}");
    assert!(styled.contains('\x1b'), "{styled:?}");
    assert_eq!(strip_codes(&styled), plain, "{styled}");
}
