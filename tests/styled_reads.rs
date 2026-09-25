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
    cmd.current_dir(dir)
        .env("VIVAC_HOME", home)
        .env("TZ", "UTC")
        .args(args);
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

// Guard: `vivac mcp` and every `vivac session ... --hook` path must never
// emit an escape code, even where a harness exports `CLICOLOR_FORCE` and
// `COLUMNS` into their own environment -- an agent's channel, never a
// terminal. `style::plain_only()` is the process-wide override that makes
// this true regardless of what any call site downstream asks for.

const FORCE_ENV: [(&str, &str); 2] = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];

fn run_with_env_stdin(
    dir: &std::path::Path,
    home: &std::path::Path,
    args: &[&str],
    env: &[(&str, &str)],
    stdin: &str,
) -> (String, i32) {
    use std::io::Write;
    let mut cmd = std::process::Command::new(BIN);
    cmd.current_dir(dir)
        .env("VIVAC_HOME", home)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    match child.stdin.take().unwrap().write_all(stdin.as_bytes()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("writing the payload to the child: {e}"),
    }
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
        out.status.code().unwrap_or(-1),
    )
}

/// `session start --hook`, run twice against the same sandbox with the same
/// pinned clock -- once plain, once under `CLICOLOR_FORCE`/`COLUMNS` -- so
/// the comparison is byte for byte against this run's own plain half rather
/// than against a second sandbox, whose project name (the folder's own,
/// unique per `Sandbox`) would differ from this one's for a reason that has
/// nothing to do with styling.
#[test]
fn session_start_hook_is_never_styled_even_when_forced() {
    let c = Sandbox::new_seeded("guard-session-start");
    seed(&c);
    let now = "2026-09-24T09:00:00Z";
    let (plain, code) = run_with_env_stdin(
        &c.0,
        c.global_home(),
        &["session", "start", "--hook", "--now", now],
        &[],
        "{}",
    );
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains(ROOT_MARK), "{plain}");

    let (forced, code) = run_with_env_stdin(
        &c.0,
        c.global_home(),
        &["session", "start", "--hook", "--now", now],
        &FORCE_ENV,
        "{}",
    );
    assert_eq!(code, 0, "{forced}");
    assert!(!forced.contains('\x1b'), "{forced:?}");
    assert_eq!(forced, plain, "{forced}");
}

/// `session end --hook` prints nothing at all on the ordinary path -- an
/// empty stack is "no stop worth saving", said only outside hook mode -- so
/// both sides of this guard are the empty string, and the point is that
/// forcing colour on a hook that has nothing to say still leaves it with
/// nothing to say, rather than an escape code around an empty line.
#[test]
fn session_end_hook_is_never_styled_even_when_forced() {
    let c = Sandbox::new_seeded("guard-session-end");
    let now = "2026-09-24T09:00:00Z";
    let (plain, code) = run_with_env_stdin(
        &c.0,
        c.global_home(),
        &["session", "end", "--hook", "--now", now],
        &[],
        "{}",
    );
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");

    let (forced, code) = run_with_env_stdin(
        &c.0,
        c.global_home(),
        &["session", "end", "--hook", "--now", now],
        &FORCE_ENV,
        "{}",
    );
    assert_eq!(code, 0, "{forced}");
    assert!(!forced.contains('\x1b'), "{forced:?}");
    assert_eq!(forced, plain, "{forced}");
}

/// `session prompt --hook`, the one hook whose text carries no project name
/// or path -- `prompt_text` is a pure function of how many minutes the turn
/// held -- so two independent sandboxes, walked through the same pinned
/// timeline, are compared byte for byte against each other rather than a
/// plain and a forced run of the same one.
#[test]
fn session_prompt_hook_is_never_styled_even_when_forced() {
    fn speak(c: &Sandbox, env: &[(&str, &str)]) -> String {
        c.append_raw_line(
            r#"{"seq":100,"id":"01GUARDSESSIONSTARTAAAAAAA","ts":"2026-09-24T09:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"session.started","source":"test"}}"#,
        );
        run_with_env_stdin(
            &c.0,
            c.global_home(),
            &[
                "session",
                "prompt",
                "--hook",
                "--now",
                "2026-09-24T09:00:00Z",
            ],
            env,
            r#"{"session_id":"s1"}"#,
        );
        let (_out, code) = run_with_env_stdin(
            &c.0,
            c.global_home(),
            &["session", "end", "--hook", "--now", "2026-09-24T09:11:00Z"],
            env,
            r#"{"session_id":"s1"}"#,
        );
        assert_eq!(code, 0);
        let (speaks, code) = run_with_env_stdin(
            &c.0,
            c.global_home(),
            &[
                "session",
                "prompt",
                "--hook",
                "--now",
                "2026-09-24T09:30:00Z",
            ],
            env,
            r#"{"session_id":"s1"}"#,
        );
        assert_eq!(code, 0, "{speaks}");
        speaks
    }

    let plain = speak(&Sandbox::new_seeded("guard-prompt-plain"), &[]);
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains("nothing written to the tree"), "{plain}");

    let forced = speak(&Sandbox::new_seeded("guard-prompt-forced"), &FORCE_ENV);
    assert!(!forced.contains('\x1b'), "{forced:?}");
    assert_eq!(forced, plain, "{forced}");
}

/// An MCP stdio exchange, one line in and one line out, `vivac_brief` and
/// `vivac_why` both -- run twice against the same sandbox, once plain and
/// once forced, so the comparison stands on this run's own plain half
/// rather than a second sandbox's differently-named project.
#[test]
fn mcp_stdio_is_never_styled_even_when_forced() {
    use std::io::{BufRead, BufReader, Write};

    fn ask(c: &Sandbox, env: &[(&str, &str)]) -> String {
        let mut cmd = std::process::Command::new(BIN);
        cmd.current_dir(&c.0)
            .env("VIVAC_HOME", c.global_home())
            .arg("mcp")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut replies = String::new();
        for line in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#.to_string(),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"vivac_brief","arguments":{}}}"#.to_string(),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"vivac_why","arguments":{"id":"t2"}}}"#.to_string(),
        ] {
            writeln!(input, "{line}").unwrap();
            input.flush().unwrap();
            let mut buf = String::new();
            output.read_line(&mut buf).unwrap();
            replies.push_str(&buf);
        }
        drop(input);
        let _ = child.kill();
        let _ = child.wait();
        replies
    }

    let c = Sandbox::new_seeded("guard-mcp");
    seed(&c);
    let plain = ask(&c, &[]);
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains(ROOT_MARK), "{plain}");
    assert!(plain.contains("\\\"t2\\\""), "{plain}");

    let forced = ask(&c, &FORCE_ENV);
    assert!(!forced.contains('\x1b'), "{forced:?}");
    assert_eq!(forced, plain, "{forced}");
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

// ---------------------------------------------------------------------------
// The rest of vivac's reads. Same visual language, same three guarantees:
// styled under `CLICOLOR_FORCE`, byte for byte the same once stripped back
// out, and byte for byte plain without it -- and `--json` never carries an
// escape code either way.
// ---------------------------------------------------------------------------

#[test]
fn find_is_styled_and_wraps_with_no_line_over_the_terminal_width() {
    let c = Sandbox::new_seeded("styled-find");
    seed(&c);
    // Without `COLUMNS`, `width` answers `None` even under `CLICOLOR_FORCE`
    // (`without_columns_it_styles_but_never_wraps`), so this half stays
    // comparable to the plain run byte for byte once the codes are gone.
    let force_only = [("CLICOLOR_FORCE", "1")];
    let with_columns = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];

    let (plain, code) = c.run(&["find", "testing"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["find", "testing"], &force_only);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(forced.contains("\x1b[35m"), "g1 is a goal: {forced}");
    assert!(forced.contains("\x1b[36m"), "t2 is a task: {forced}");

    let (wrapped, code) = run_with_env(&c.0, c.global_home(), &["find", "testing"], &with_columns);
    assert_eq!(code, 0, "{wrapped}");
    assert_no_overlong_lines("find", &wrapped);

    let (json, code) = run_with_env(
        &c.0,
        c.global_home(),
        &["find", "testing", "--json"],
        &with_columns,
    );
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

#[test]
fn find_everywhere_is_styled_and_bolds_the_project_name() {
    let c = Sandbox::new_seeded("styled-find-everywhere");
    seed(&c);
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = c.run(&["find", "testing", "--everywhere"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");

    let (forced, code) = run_with_env(
        &c.0,
        c.global_home(),
        &["find", "testing", "--everywhere"],
        &env,
    );
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(forced.contains("\x1b[1m"), "no bold at all: {forced}");
}

/// `parked` — a suspended node's own title is never dimmed (every row here
/// carries the same state, so dimming all of them would say nothing), but
/// the reason it was parked for is.
#[test]
fn parked_is_styled_and_wraps_with_no_line_over_the_terminal_width() {
    let c = Sandbox::new_seeded("styled-parked");
    seed(&c);
    c.ok(&["park", "not now"]);
    let force_only = [("CLICOLOR_FORCE", "1")];
    let with_columns = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];

    let (plain, code) = c.run(&["parked"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["parked"], &force_only);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(forced.contains("\x1b[36m"), "t2 is a task: {forced}");
    assert!(forced.contains("\x1b[1m"), "no bold heading: {forced}");

    let (wrapped, code) = run_with_env(&c.0, c.global_home(), &["parked"], &with_columns);
    assert_eq!(code, 0, "{wrapped}");
    assert_no_overlong_lines("parked", &wrapped);

    let (json, code) = run_with_env(&c.0, c.global_home(), &["parked", "--json"], &with_columns);
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

#[test]
fn stack_is_styled_and_marks_the_focus() {
    let c = Sandbox::new_seeded("styled-stack");
    seed(&c);
    let force_only = [("CLICOLOR_FORCE", "1")];
    let with_columns = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];

    let (plain, code) = c.run(&["stack"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains("<- focus"), "{plain}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["stack"], &force_only);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(forced.contains("\x1b[35m"), "g1 is a goal: {forced}");
    assert!(forced.contains("\x1b[36m"), "t2 is a task: {forced}");

    let (wrapped, code) = run_with_env(&c.0, c.global_home(), &["stack"], &with_columns);
    assert_eq!(code, 0, "{wrapped}");
    assert_no_overlong_lines("stack", &wrapped);

    let (json, code) = run_with_env(&c.0, c.global_home(), &["stack", "--json"], &with_columns);
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

#[test]
fn stack_lanes_is_styled_on_the_alias_and_plain_without_force() {
    let c = Sandbox::new_seeded("styled-stack-lanes");
    seed(&c);
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = c.run(&["stack", "--lanes"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["stack", "--lanes"], &env);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");

    let (json, code) = run_with_env(&c.0, c.global_home(), &["stack", "--lanes", "--json"], &env);
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

/// A pillar and rule, seeded fresh here rather than reusing `seed`: `rules`
/// reads governance nodes, not the goal/task pair the wrap fixture is
/// built from.
fn seed_governance(c: &Sandbox) {
    c.ok(&["add", "A pillar", "--type", "pillar", "--why", "testing"]);
    c.ok(&[
        "add",
        "An armed rule",
        "--parent",
        "1",
        "--type",
        "rule",
        "--arm",
        "cargo test",
        "--arm-dir",
        ".",
        "--why",
        "testing",
    ]);
}

#[test]
fn rules_is_styled_red_on_governance_ids_and_dims_the_arms() {
    let c = Sandbox::new_seeded("styled-rules");
    seed_governance(&c);
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = c.run(&["rules"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains("armed in ./: cargo test"), "{plain}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["rules"], &env);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(
        forced.contains("\x1b[31m"),
        "a pillar and a rule are both red: {forced}"
    );
    assert!(
        forced.contains("\x1b[2m          armed in ./: cargo test\x1b[0m"),
        "the arm line is not dimmed: {forced}"
    );

    let (json, code) = run_with_env(&c.0, c.global_home(), &["rules", "--json"], &env);
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

#[test]
fn changes_is_styled_on_its_headings_and_ids() {
    let c = Sandbox::new_seeded("styled-changes");
    c.ok(&["save", "checkpoint"]);
    c.ok(&["push", "A front", "--why", "testing"]);
    c.ok(&["pop", "shipped"]);
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = c.run(&["changes", "--since", "v1"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(
        plain.contains("OPENED (1)") && plain.contains("CLOSED (1)"),
        "{plain}"
    );

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["changes", "--since", "v1"], &env);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(forced.contains("\x1b[1m"), "no bold heading: {forced}");
    // A parentless push with an empty stack is born a goal (`ops.rs`),
    // not a task.
    assert!(forced.contains("\x1b[35m"), "the node is a goal: {forced}");

    let (json, code) = run_with_env(
        &c.0,
        c.global_home(),
        &["changes", "--since", "v1", "--json"],
        &env,
    );
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

#[test]
fn vivacs_is_styled_bold_on_the_v_id_and_dim_on_the_intent() {
    let c = Sandbox::new_seeded("styled-vivacs");
    c.ok(&["save", "a stop", "--next", "keep going"]);
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = c.run(&["vivacs"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains("you were about to: keep going"), "{plain}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["vivacs"], &env);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(forced.contains("\x1b[1mv1"), "the v id is bold: {forced}");
    // The prefix and the (unwrapped, one-chunk) intent are each their own
    // dim span, back to back -- the same shape a wrapped intent's own
    // chunks would take.
    assert!(
        forced.contains("\x1b[2myou were about to: \x1b[0m\x1b[2mkeep going\x1b[0m"),
        "the intent line is not dim: {forced}"
    );

    let (json, code) = run_with_env(&c.0, c.global_home(), &["vivacs", "--json"], &env);
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

/// The focus title is the last column of the row, and on a real tree it
/// is what pushes the line past the terminal's own width. `vivacs`
/// wraps it the same way `tree`/`open`/`why` wrap a title, continuation
/// aligned under the column it started at -- and wraps the `you were
/// about to` line the same way, aligned under its own text rather than
/// under the row.
#[test]
fn vivacs_wraps_the_title_and_the_intent_with_no_line_over_the_terminal_width() {
    let c = Sandbox::new_seeded("styled-vivacs-wrap");
    seed(&c);
    c.ok(&[
        "save",
        "a stop",
        "--next",
        "extract the validator and update every one of the call sites that \
         still assume the old shape",
    ]);
    let with_columns = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];

    let (plain, code) = c.run(&["vivacs"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");

    let (wrapped, code) = run_with_env(&c.0, c.global_home(), &["vivacs"], &with_columns);
    assert_eq!(code, 0, "{wrapped}");
    assert!(wrapped.contains('\x1b'), "{wrapped:?}");
    assert_no_overlong_lines("vivacs", &wrapped);

    assert_wraps_and_aligns(&c, &["vivacs"], CHILD_MARK);
    assert_wraps_and_aligns(&c, &["vivacs"], "extract");
}

#[test]
fn triage_is_styled_bold_on_the_heading_dim_on_the_hint() {
    let c = Sandbox::new_seeded("styled-triage");
    seed(&c);
    c.ok(&["park", "not now"]);
    let force_only = [("CLICOLOR_FORCE", "1")];
    let with_columns = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];

    let (plain, code) = c.run(&["triage"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains("focus <id>  |  abandon <id>"), "{plain}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["triage"], &force_only);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(forced.contains("\x1b[1m"), "no bold heading: {forced}");
    assert!(forced.contains("\x1b[2m"), "no dim hint: {forced}");

    // The heading's own hint (`focus <id>  |  abandon <id>`) is a fixed
    // line at a fixed column, never a wrapped title, so the parked row's
    // own title is what this checks instead of the blanket width cap.
    assert_wraps_and_aligns(&c, &["triage"], CHILD_MARK);

    let (json, code) = run_with_env(&c.0, c.global_home(), &["triage", "--json"], &with_columns);
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

#[test]
fn stats_is_styled_bold_on_the_numbers() {
    let c = Sandbox::new_seeded("styled-stats");
    seed(&c);
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = c.run(&["stats"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["stats"], &env);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(forced.contains("\x1b[1m"), "no bold number: {forced}");

    let (json, code) = run_with_env(&c.0, c.global_home(), &["stats", "--json"], &env);
    assert_eq!(code, 0, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

#[test]
fn check_is_styled_green_when_clean_and_yellow_on_a_project_finding() {
    let clean = Sandbox::new_seeded("styled-check-clean");
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = clean.run(&["check"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains("No findings."), "{plain}");

    let (forced, code) = run_with_env(&clean.0, clean.global_home(), &["check"], &env);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(
        forced.contains("\x1b[32m"),
        "a clean result is not green: {forced}"
    );

    // A false close: closed, then a blocker hangs off it after the fact.
    let dirty = Sandbox::new_seeded("styled-check-dirty");
    dirty.ok(&["push", "Audit", "--why", "testing"]);
    dirty.ok(&["pop", "done"]);
    dirty.ok(&[
        "add",
        "Late finding",
        "--parent",
        "1",
        "--blocks",
        "--why",
        "testing",
    ]);
    let (dirty_plain, code) = dirty.run(&["check"]);
    assert_eq!(code, 1, "{dirty_plain}");
    assert!(!dirty_plain.contains('\x1b'), "{dirty_plain:?}");

    let (dirty_forced, code) = run_with_env(&dirty.0, dirty.global_home(), &["check"], &env);
    assert_eq!(code, 1, "{dirty_forced}");
    assert!(dirty_forced.contains('\x1b'), "{dirty_forced:?}");
    assert_eq!(strip_codes(&dirty_forced), dirty_plain, "{dirty_forced}");
    assert!(
        dirty_forced.contains("\x1b[33m"),
        "a project finding is not yellow: {dirty_forced}"
    );

    let (json, code) = run_with_env(&dirty.0, dirty.global_home(), &["check", "--json"], &env);
    assert_eq!(code, 1, "{json}");
    assert!(!json.contains('\x1b'), "{json:?}");
}

#[test]
fn check_is_styled_red_on_a_store_finding() {
    let c = Sandbox::new_seeded("styled-check-store");
    // Not valid JSON at all -- `unknown_reason_for` needs a parse to even
    // ask whether it recognises the shape, so this counts as a broken
    // line rather than a newer vivac's event.
    c.append_raw_line("this line is not json");
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = c.run(&["check"]);
    assert_eq!(code, 1, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    assert!(plain.contains("STORE ("), "{plain}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["check"], &env);
    assert_eq!(code, 1, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");
    assert!(
        forced.contains("\x1b[31m"),
        "a store finding is not red: {forced}"
    );
}

/// `brief`: styled only for `vivac brief` itself, at a terminal-equivalent
/// (`CLICOLOR_FORCE`), never for the hooks or the MCP server -- those are
/// proven plain by the guard tests above. Never wrapped by width either:
/// `to_text` keeps its own clipping, so the same sandbox run twice with
/// the same `--now` (once plain, once forced) has to come back exactly the
/// same once the codes are stripped, project name and all.
#[test]
fn brief_is_styled_on_a_terminal_and_never_wrapped() {
    let c = Sandbox::new_seeded("styled-brief");
    c.ok(&[
        "push",
        "Migrate authentication to OIDC",
        "--why",
        "the old provider is shutting down",
    ]);
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
        "push",
        "Pick a cache backend",
        "--why",
        "the token store needs one",
    ]);
    c.ok(&[
        "decide",
        "Use a distributed token store",
        "--reason",
        "a single node will not hold",
    ]);
    c.ok(&[
        "add",
        "Rescope the migration",
        "--type",
        "goal",
        "--why",
        "narrower",
    ]);
    c.ok(&["park", "5", "later, once the backend lands"]);
    c.ok(&[
        "save",
        "before touching the adapter",
        "--next",
        "extract the validator",
    ]);

    let now = "2026-09-15T10:00:00Z";
    let (plain, code) = c.run(&["brief", "--now", now]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    for section in [
        "INVARIANTS",
        "STANDING DECISIONS",
        "DO NOT TOUCH NOW",
        "LAST VIVAC",
    ] {
        assert!(plain.contains(section), "{plain}");
    }
    assert!(plain.contains("<== HERE"), "{plain}");

    let env = [("CLICOLOR_FORCE", "1"), ("COLUMNS", "60")];
    let (forced, code) = run_with_env(&c.0, c.global_home(), &["brief", "--now", now], &env);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");

    assert!(
        forced.contains("\x1b[1m INVARIANTS\x1b[0m"),
        "INVARIANTS is not bold: {forced}"
    );
    assert!(
        forced.contains("\x1b[1m STANDING DECISIONS\x1b[0m"),
        "STANDING DECISIONS is not bold: {forced}"
    );
    assert!(
        forced.contains("\x1b[1m DO NOT TOUCH NOW\x1b[0m"),
        "DO NOT TOUCH NOW is not bold: {forced}"
    );
    assert!(
        forced.contains("\x1b[1m LAST VIVAC\x1b[0m"),
        "LAST VIVAC is not bold: {forced}"
    );
    assert!(
        forced.contains("\x1b[33m<== HERE"),
        "<== HERE is not yellow: {forced}"
    );
    assert!(
        forced.contains(&"-".repeat(60)),
        "the separator is missing whole: {forced}"
    );

    // The token/depth footer line is dimmed.
    let footer = plain
        .lines()
        .find(|l| l.contains("tokens") && l.contains("depth"))
        .expect("no footer line");
    assert!(
        forced.contains(&format!("\x1b[2m{footer}\x1b[0m")),
        "the footer is not dim: {forced}"
    );

    // Session hooks and MCP never see any of this: `to_text` itself never
    // touches `style`, proven already by the guard tests above.
}

#[test]
fn brief_is_never_styled_without_a_terminal_or_force() {
    let c = Sandbox::new_seeded("styled-brief-plain");
    seed(&c);
    let out = c.ok(&["brief"]);
    assert!(!out.contains('\x1b'), "{out:?}");
}

/// `--help`: three section headings are bold on a terminal, and the plain
/// text -- the one an agent piping the command sees -- carries no escape
/// code at all.
#[test]
fn help_headings_are_bold_on_a_terminal_and_plain_otherwise() {
    let c = Sandbox::new_seeded("styled-help");
    let env = [("CLICOLOR_FORCE", "1")];

    let (plain, code) = c.run(&["--help"]);
    assert_eq!(code, 0, "{plain}");
    assert!(!plain.contains('\x1b'), "{plain:?}");

    let (forced, code) = run_with_env(&c.0, c.global_home(), &["--help"], &env);
    assert_eq!(code, 0, "{forced}");
    assert!(forced.contains('\x1b'), "{forced:?}");
    assert_eq!(strip_codes(&forced), plain, "{forced}");

    for heading in [
        "  The agent writes (the stack carries the tree on its own)",
        "  Session",
        "  Exit codes",
    ] {
        assert!(
            forced.contains(&format!("\x1b[1m{heading}\x1b[0m")),
            "{heading:?} is not bold:\n{forced}"
        );
    }
}
