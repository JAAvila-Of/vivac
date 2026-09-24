//! The two session hooks, against the binary.
//!
//! `f568`: Claude Code does have a `SessionEnd` event, but the automatic stop
//! hangs off `Stop` instead. `Stop` runs **on every turn**, so the automatic
//! stop has to know when there is nothing to stop for.

mod common;
use common::Sandbox;

fn how_many(vivacs: &str, kind: &str) -> usize {
    vivacs.lines().filter(|l| l.contains(kind)).count()
}

/// `Sandbox::run_stdin`, with `stdout` and `stderr` kept apart: proving the
/// copy warning does not repeat what the brief already showed needs the
/// two streams told apart, the same reason `tests/registry.rs`'s own
/// `run_split` exists.
fn run_stdin_split(c: &Sandbox, args: &[&str], stdin: &str) -> (String, String, i32) {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let o = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

/// `t594`: `session start --hook` printed the brief --
/// which already opens with the copy block (`t594` §4.7) -- and then, once
/// its own write ran, the generic dispatch preamble said the very same
/// thing again on `stderr`. The two streams are checked apart, not
/// together: the review's own reproduction counted both at once, and that
/// is exactly what let the leak into `stderr` hide behind a passing count.
#[test]
fn session_start_hook_from_a_copy_shows_the_notice_once() {
    let original = Sandbox::new_seeded("session-copy-orig");
    original.ok(&["push", "a goal", "--why", "so the log has a first event"]);
    let copy = Sandbox::new_empty_in("session-copy-copy", original.global_home());
    std::fs::create_dir_all(copy.0.join(".vivac")).unwrap();
    std::fs::copy(
        original.0.join(".vivac").join("events"),
        copy.0.join(".vivac").join("events"),
    )
    .unwrap();

    let (stdout, stderr, code) = run_stdin_split(&copy, &["session", "start", "--hook"], "{}");
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        stdout.matches("COPY OF ANOTHER TREE").count(),
        1,
        "the brief itself must still carry it once:\n{stdout}"
    );
    assert_eq!(
        stderr.matches("COPY OF ANOTHER TREE").count(),
        0,
        "the stderr echo repeated what the brief already showed:\n{stderr}"
    );
}

/// Forty turns are not forty stops. A stop that repeats identically is not a
/// stop: it is a log.
#[test]
fn one_stop_per_turn_does_not_leave_one_stop_per_turn() {
    let c = Sandbox::new_seeded("turns");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    for _ in 0..5 {
        c.ok(&["session", "end", "--hook"]);
    }
    let v = c.ok(&["vivacs"]);
    assert_eq!(how_many(&v, "auto"), 1, "one stop per turn:\n{v}");
}

/// But as soon as the tree changes, the next stop does count.
#[test]
fn a_new_stop_once_something_changed() {
    let c = Sandbox::new_seeded("change");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["note", "1", "something happened"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert_eq!(how_many(&v, "auto"), 2, "it swallowed the good stop:\n{v}");
}

/// With no stack there is no pitch to close.
#[test]
fn no_stack_no_stop() {
    let c = Sandbox::new_seeded("nostack");
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert_eq!(how_many(&v, "auto"), 0, "it invented an empty stop:\n{v}");
}

/// `f403`, `f404`: the brief goes straight to stdout in plain text, the shape
/// Claude Code's own hook reference says becomes context on `SessionStart`.
/// No JSON envelope, and the opening still lands in the log.
#[test]
fn the_start_hook_prints_the_brief_as_plain_text() {
    let c = Sandbox::new_seeded("plaintext");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let (s, code) = c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"startup"}"#,
    );
    assert_eq!(code, 0, "{s}");
    assert!(!s.contains("hookSpecificOutput"), "{s}");
    assert!(s.starts_with("vivac · project:"), "{s}");
    assert!(s.contains("A goal"), "the brief went out empty:\n{s}");
    let log = c.log();
    assert!(log.contains("session.started"), "no opening:\n{log}");
    assert!(
        log.contains(r#""source":"startup""#),
        "the source did not survive:\n{log}"
    );
}

/// **A hook that fails in every directory without a tree gets switched off
/// within two days.** Both stay quiet and exit 0 where there is no `.vivac/`,
/// which is what makes it safe to leave them in the global configuration.
#[test]
fn they_stay_quiet_where_there_is_no_tree() {
    let c = Sandbox::new_empty("notree");
    for args in [["session", "start", "--hook"], ["session", "end", "--hook"]] {
        let (s, code) = c.run(&args);
        assert_eq!(code, 0, "{args:?} failed outside a tree:\n{s}");
        assert_eq!(s.trim(), "", "{args:?} said too much:\n{s}");
    }
}

/// `f708`: Codex's own hook manual is taxative about `Stop` -- plain text on
/// its stdout is **invalid**, and exiting 0 with nothing printed is what
/// counts as success there. The end hook already behaves this way, in every
/// shape a turn can leave it: no tree at all, a tree with nothing new to
/// close, and a tree with a change that does leave an automatic stop behind.
/// Nothing here made that true on purpose -- it only holds because nothing
/// in `session::end`'s hook path ever calls `outln!` -- so this is the test
/// that would have caught the day a courtesy line got added to it, the
/// mirror of [`the_start_hook_prints_the_brief_as_plain_text`] for the
/// opposite promise: not what the hook prints, but that it prints nothing
/// at all.
#[test]
fn the_end_hook_prints_nothing_to_stdout() {
    let no_tree = Sandbox::new_empty("end-hook-silent-no-tree");
    let (stdout, _stderr, code) = run_stdin_split(&no_tree, &["session", "end", "--hook"], "");
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        stdout, "",
        "the end hook printed something with no tree:\n{stdout}"
    );

    let unchanged = Sandbox::new_seeded("end-hook-silent-unchanged");
    let (stdout, _stderr, code) = run_stdin_split(&unchanged, &["session", "end", "--hook"], "");
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        stdout, "",
        "the end hook printed something with nothing changed:\n{stdout}"
    );

    let changed = Sandbox::new_seeded("end-hook-silent-changed");
    changed.ok(&["push", "A goal", "--why", "it is needed"]);
    let (stdout, _stderr, code) = run_stdin_split(&changed, &["session", "end", "--hook"], "");
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        stdout, "",
        "the end hook printed something even though it left a stop:\n{stdout}"
    );
}

/// An automatic stop nobody declared still has to say something. Two autos in
/// a row that read identically do not segment a session, they log it (`f59`).
/// The label is derived from the seams --what the segment contained-- and never
/// from a judgement of relevance, which `DX` already measured at zero uses.
#[test]
fn an_automatic_stop_says_what_its_segment_contained() {
    let c = Sandbox::new_seeded("segment");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["add", "First finding", "--why", "it turned up"]);
    c.ok(&["add", "Second finding", "--why", "it turned up too"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("2 new"),
        "the automatic stop did not say what it closed:\n{v}"
    );
}

/// Closing is as much of a seam as opening. A segment that only settled things
/// would otherwise read as if nothing had happened in it.
#[test]
fn an_automatic_stop_counts_what_its_segment_closed() {
    let c = Sandbox::new_seeded("closed");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["add", "First finding", "--why", "it turned up"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["done", "2", "it was settled"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("1 closed"),
        "the automatic stop counted no closes:\n{v}"
    );
}

/// A segment made only of notes is still a segment. The real tree has turns
/// that wrote nothing but notes, and they have to be tellable apart.
#[test]
fn an_automatic_stop_counts_the_notes_of_its_segment() {
    let c = Sandbox::new_seeded("notes");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["note", "1", "something turned up"]);
    c.ok(&["note", "1", "and something else"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("2 notes"),
        "the automatic stop counted no notes:\n{v}"
    );
}

/// One note is a note, not notes. The label is prose the maintainer reads in
/// `vivac vivacs`, and prose that counts wrong reads like a machine talking.
#[test]
fn a_single_note_is_not_pluralised() {
    let c = Sandbox::new_seeded("plural");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["note", "1", "something turned up"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(v.contains("1 note"), "it counted no notes:\n{v}");
    assert!(!v.contains("1 notes"), "it said `1 notes`:\n{v}");
}

/// Not every seam is a birth, a close or a note. A segment that only raised a
/// flag still moved the tree, and if the label came out empty the stop would be
/// back to being the blank line `f59` was about.
#[test]
fn a_segment_of_none_of_the_three_still_says_something() {
    let c = Sandbox::new_seeded("other");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["flag", "1", "review", "--why", "it needs a second look"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert!(
        v.contains("1 change"),
        "the stop came out blank after a flag:\n{v}"
    );
}

/// `d45` retired the Spanish layer, and retiring it means the old spelling is
/// an unknown subcommand rather than a silent synonym. A word that is accepted
/// instead of rejected teaches the caller a spelling that does not exist, and
/// `session` was the last place in the product still doing it (`f57`).
#[test]
fn the_spanish_spellings_of_the_session_hooks_are_rejected() {
    let c = Sandbox::new_seeded("spanish");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    for args in [["session", "inicio"], ["session", "fin"]] {
        let (s, code) = c.run(&args);
        assert_ne!(code, 0, "`vivac {}` was accepted:\n{s}", args.join(" "));
        assert!(
            s.contains("usage"),
            "`vivac {}` failed without saying how:\n{s}",
            args.join(" ")
        );
    }
}

/// `SESSION-EVENT.md` §0: opening a session left no trace, so question 1 of
/// the falsification criterion --was the brief read?-- could not be answered
/// from the log at all. Only the gap between two writes, which also happens
/// when somebody goes to lunch.
#[test]
fn opening_a_session_leaves_a_trace() {
    let c = Sandbox::new_seeded("opening");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let (out, code) = c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"startup"}"#,
    );
    assert_eq!(code, 0, "the start hook failed:\n{out}");
    assert!(
        c.log().contains("session.started"),
        "the opening left no trace:\n{}",
        c.log()
    );
}

/// The four openings are not the same experiment --the brief competes with
/// nothing on a cold start and with a whole restored transcript on a resume--
/// so which one it was has to survive into the log.
#[test]
fn the_source_of_the_opening_travels_into_the_log() {
    let c = Sandbox::new_seeded("source");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"compact"}"#,
    );
    assert!(
        c.log().contains(r#""source":"compact""#),
        "the source did not survive:\n{}",
        c.log()
    );
}

/// The hook writes; the command a person runs does not. `vivac session start`
/// typed by hand stays a pure read, which is what confines the write to the
/// seam of the machine.
#[test]
fn the_command_a_person_runs_writes_nothing() {
    let c = Sandbox::new_seeded("pureread");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let before = c.log();
    c.ok(&["session", "start"]);
    assert_eq!(before, c.log(), "the read path wrote to the log");
}

/// A hook that fails in every directory gets switched off within two days,
/// and the measurement goes with it. Garbage on stdin is not a reason to fail.
#[test]
fn a_broken_payload_still_opens_the_session() {
    let c = Sandbox::new_seeded("garbage");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    let (out, code) = c.run_stdin(&["session", "start", "--hook"], "not json at all");
    assert_eq!(code, 0, "garbage on stdin took the hook down:\n{out}");
    assert!(
        c.log().contains(r#""source":"unknown""#),
        "an unsaid source has to read as `unknown`, not as empty:\n{}",
        c.log()
    );
}

/// An opening is a fact about the session, not about the tree. If it counted
/// as a change, the next `Stop` would leave an automatic stop for a session
/// that did nothing -- which is the repeated stop the guard exists to avoid.
#[test]
fn an_opening_does_not_arm_the_automatic_stop() {
    let c = Sandbox::new_seeded("noarm");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.ok(&["session", "end", "--hook"]);
    let before = how_many(&c.ok(&["vivacs"]), "auto");
    c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"startup"}"#,
    );
    c.ok(&["session", "end", "--hook"]);
    assert_eq!(
        how_many(&c.ok(&["vivacs"]), "auto"),
        before,
        "the opening armed a stop for a session that did nothing"
    );
}

/// What the brief claimed, so that "was it followed?" stops being a judgement
/// and becomes a query. The identifiers are the ones the rest of the log
/// already uses, not the alias, which is recomputed on every fold.
///
/// These are inputs, never a verdict: what counts as *following* the brief
/// lives in whoever reads.
#[test]
fn the_opening_records_what_the_brief_claimed() {
    let c = Sandbox::new_seeded("claimed");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"startup"}"#,
    );
    let log = c.log();
    let opening = log
        .lines()
        .find(|l| l.contains("session.started"))
        .unwrap_or_else(|| panic!("no opening in the log:\n{log}"));
    assert!(
        opening.contains(r#""session":"abc-123""#),
        "the session identifier did not survive:\n{opening}"
    );
    assert!(
        !opening.contains(r#""focus":null"#),
        "the opening recorded no focus, and there was one:\n{opening}"
    );
    // The push left a stop behind, so there is a last one to point at.
    assert!(
        !opening.contains(r#""vivac":null"#),
        "the opening recorded no last stop, and there was one:\n{opening}"
    );
}

/// The line inside the payload: an opaque identifier yes, a filesystem path
/// no. The transcript path arrives beside the session id and carries the
/// user's home directory, which the security pillar vetoes.
#[test]
fn the_transcript_path_never_reaches_the_log() {
    let c = Sandbox::new_seeded("noleak");
    c.ok(&["push", "A goal", "--why", "it is needed"]);
    c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"startup","transcript_path":"/home/someone/.claude/projects/x/y.jsonl","cwd":"/home/someone/work"}"#,
    );
    let log = c.log();
    assert!(
        !log.contains("someone"),
        "a path out of the payload reached the log:\n{log}"
    );
    assert!(
        !log.contains("transcript"),
        "the transcript path reached the log:\n{log}"
    );
}

/// With nothing on the stack the brief names no focus, and the opening has to
/// say so rather than invent one.
#[test]
fn an_opening_with_no_focus_records_none() {
    let c = Sandbox::new_seeded("nofocus");
    c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"startup"}"#,
    );
    let log = c.log();
    assert!(log.contains("session.started"), "no opening:\n{log}");
    assert!(
        log.contains(r#""focus":null"#),
        "it invented a focus out of an empty stack:\n{log}"
    );
}

/// A payload the guard refuses still opens the session: refusing the write
/// outright would drop the seam in silence, and a hook has nobody standing
/// by to reword the sentence that tripped it.
#[test]
fn a_refused_payload_still_opens_the_session() {
    let c = Sandbox::new_seeded("refused-open");
    let (out, code) = c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"/home/someone/.claude/projects/x"}"#,
    );
    assert_eq!(code, 0, "a refused field took the hook down:\n{out}");
    let log = c.log();
    assert!(log.contains("session.started"), "no opening:\n{log}");
    assert!(
        !log.contains("someone"),
        "the refused text reached the log:\n{log}"
    );
    assert!(
        log.contains(r#""source":"refused: "#),
        "the refused field was not replaced:\n{log}"
    );
}

/// What replaces a refused field names the rule and nothing past it, so the
/// log shows a refusal happened without repeating what caused it.
#[test]
fn the_refusal_names_the_rule_not_the_text() {
    let c = Sandbox::new_seeded("refused-rule");
    c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"abc-123","source":"/home/someone/.claude/projects/x"}"#,
    );
    let log = c.log();
    assert!(
        log.contains(r#""source":"refused: path to a user home directory (personal data)""#),
        "the rule did not land in the field as written:\n{log}"
    );
}

/// A refused session identifier is no different from a refused source: it
/// does not take the rest of the opening, or its sibling field, down with it.
#[test]
fn a_refused_session_identifier_does_not_take_the_opening_down() {
    let c = Sandbox::new_seeded("refused-session");
    c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"someone@example.com","source":"startup"}"#,
    );
    let log = c.log();
    assert!(log.contains("session.started"), "no opening:\n{log}");
    assert!(
        log.contains(r#""source":"startup""#),
        "the clean field was touched by its refused sibling:\n{log}"
    );
    assert!(
        !log.contains("someone@example.com"),
        "the refused session identifier reached the log:\n{log}"
    );
    assert!(
        log.contains(r#""session":"refused: email address (personal data)""#),
        "the session field was not replaced:\n{log}"
    );
}

/// The regression that matters most: a real, UUID-shaped session identifier
/// from Claude Code passes through untouched. This rests on `known_shape`
/// exempting UUIDs from the entropy check.
#[test]
fn a_real_session_identifier_passes_through_untouched() {
    let c = Sandbox::new_seeded("real-session");
    c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"session_id":"019316f8-4c2a-7b31-9f0e-8d1a2b3c4d5e","source":"startup"}"#,
    );
    let log = c.log();
    assert!(
        log.contains("019316f8-4c2a-7b31-9f0e-8d1a2b3c4d5e"),
        "a real session identifier did not survive:\n{log}"
    );
    assert!(
        !log.contains("refused:"),
        "the guard fired on a real payload:\n{log}"
    );
}

// ---------------------------------------------------------------------------
// `d738`: the hook brief names the capture seams. `f737` measured why -- an
// agent with no project doctrine of its own only writes to the tree when a
// person asks, because nothing it receives unasked says when to.
// ---------------------------------------------------------------------------

/// The approved text, byte for byte (`d757`). Widths measured by machine,
/// every line included, none over the 76-column ceiling that
/// `capture_seams_lines_never_widen_past_76_columns`, below, checks against
/// the real rendering rather than against this constant.
const CAPTURE_SEAMS_BLOCK: &str = "\n WRITE AT THESE SEAMS\n  Look first: vivac find \"<words>\". Work the tree already holds goes under\n  its node, never into a second one. The focus above is where work was\n  left, maybe not by you: hang new work from what it continues.\n  Write before you answer: what you tell the person goes in the tree first.\n  new line of work     vivac push \"<title>\" --why \"<why>\" --parent <id>\n                       or --root, when it continues nothing in the tree\n  a choice is settled  vivac decide \"<t>\" --reason \"<r>\" --alternative \"<x>\"\n  you report findings  vivac add \"<t>\" --type finding --why \"<where>\"\n                       as you tell the person, one for each thing found\n                       asks nothing? close it: vivac done <id> \"Record: ...\"\n  told \"not now\"       vivac park <id> \"<their words>\"\n                       nothing to park yet? vivac add it, then park it\n  changed outside git  vivac note <id> \"<what changed, where>\"\n                       CI, a tracker, the cloud: the tree is its only record\n  the work is done     vivac pop \"<outcome>\"\n                       and again if that settles the node it returns to\n  Or the same moves through the vivac_* tools.\n";

/// Test (a): the hook's own brief carries the block, exactly.
#[test]
fn the_hook_brief_names_the_capture_seams() {
    let c = Sandbox::new_seeded("capture-seams-hook");
    let (s, code) = c.run_stdin(&["session", "start", "--hook"], "{}");
    assert_eq!(code, 0, "{s}");
    assert!(
        s.contains(CAPTURE_SEAMS_BLOCK),
        "the hook brief did not carry the capture-seams block byte for byte:\n{s}"
    );
    // It sits right ahead of the closing rule and the tokens/depth footer,
    // not buried earlier in the body.
    let block_at = s.find(CAPTURE_SEAMS_BLOCK).unwrap();
    let footer_at = s.find("tokens · depth").unwrap();
    assert!(
        block_at < footer_at,
        "the block did not land ahead of the footer:\n{s}"
    );
}

/// Test (b)'s other half lives in `tests/brief.rs`, as
/// `the_capture_seams_block_is_hook_only`; this is the same claim read off
/// the person-facing render this file already exercises: a plain `vivac
/// session start` (no `--hook`) is the CLI path a person runs, and it stays
/// out.
#[test]
fn a_person_running_session_start_without_hook_gets_no_capture_seams() {
    let c = Sandbox::new_seeded("capture-seams-no-hook");
    let out = c.ok(&["session", "start"]);
    assert!(
        !out.contains("WRITE AT THESE SEAMS"),
        "a person reading `vivac session start` was told when to write:\n{out}"
    );
}

/// Test (f): no line of the block, as actually rendered, is wider than 76
/// columns. Read off `s` itself rather than off [`CAPTURE_SEAMS_BLOCK`], so
/// a row that grows without anyone updating that constant still gets
/// caught here.
#[test]
fn capture_seams_lines_never_widen_past_76_columns() {
    let c = Sandbox::new_seeded("capture-seams-width");
    let (s, code) = c.run_stdin(&["session", "start", "--hook"], "{}");
    assert_eq!(code, 0, "{s}");
    let start = s
        .find(" WRITE AT THESE SEAMS")
        .expect("no capture-seams heading in the hook brief");
    let block = &s[start..];
    let end = block.find("\n\n").unwrap_or(block.len());
    for line in block[..end].lines() {
        assert!(
            line.chars().count() <= 76,
            "a capture-seams line is wider than 76 columns ({} chars): {line:?}",
            line.chars().count()
        );
    }
}

/// A command line, split the way a shell would: whitespace-separated,
/// except inside a pair of double quotes. Enough to run the exact commands
/// the capture-seams block shows, placeholders swapped for real values.
fn shell_split(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Test (d): every command the block shows is a real one -- its verb
/// dispatches, and every flag on its row is a flag that verb accepts.
/// Checked two ways per row: a bogus flag on the verb comes back with
/// "does not take" (the verb was recognised) rather than "unknown command"
/// (it was not), and the row itself, with its `<placeholders>` swapped for
/// real values, exits 0.
///
/// `d757` adds the look-first line's own `vivac find` and `push`'s new
/// `--parent <id>`: `push` needs a real, open node to point `--parent` at,
/// the same as `park` and `pop` already need one to act on, so it joins
/// them below.
#[test]
fn every_capture_seam_command_dispatches_and_takes_its_flags() {
    let rows = [
        "vivac find \"<words>\"",
        "vivac push \"<title>\" --why \"<why>\" --parent <id>",
        "vivac decide \"<t>\" --reason \"<r>\" --alternative \"<x>\"",
        "vivac add \"<t>\" --type finding --why \"<where>\"",
        "vivac done <id> \"Record: ...\"",
        "vivac park <id> \"<their words>\"",
        "vivac note <id> \"<what changed, where>\"",
        "vivac pop \"<outcome>\"",
    ];
    for row in rows {
        let mut argv = shell_split(row);
        assert_eq!(argv.remove(0), "vivac");
        let verb = argv[0].clone();

        // The verb dispatches: a bogus flag is refused by name, not by
        // "unknown command" -- proof the command itself was recognised.
        let (out, code) = Sandbox::new_seeded(&format!("capture-seams-bogus-{verb}"))
            .run(&[verb.as_str(), "--bogus-flag-for-this-test"]);
        assert_ne!(code, 0, "a bogus flag was silently accepted:\n{out}");
        assert!(
            out.contains("does not take"),
            "{verb} did not dispatch as a known command:\n{out}"
        );
        assert!(
            !out.contains(&format!("unknown command: {verb}")),
            "{verb} is not a real command:\n{out}"
        );

        // Every flag on the row is accepted: the row itself, run for real
        // with placeholders swapped for plain values, exits 0.
        let c = Sandbox::new_seeded(&format!("capture-seams-real-{verb}"));
        if verb == "park" || verb == "pop" || verb == "push" || verb == "done" || verb == "note" {
            c.ok(&["push", "a title to act on", "--why", "seed"]);
        }
        let real_args: Vec<String> = argv
            .into_iter()
            .map(|a| {
                if a == "<id>" {
                    "1".to_string()
                } else if a.starts_with('<') && a.ends_with('>') {
                    "a value".to_string()
                } else {
                    a
                }
            })
            .collect();
        let arg_refs: Vec<&str> = real_args.iter().map(String::as_str).collect();
        c.ok(&arg_refs);
    }

    // `--root` is `push`'s own other way of saying where a node is born
    // (`d757`'s hint line names it): on an empty stack it needs nothing to
    // point at and still exits 0.
    let c = Sandbox::new_seeded("capture-seams-push-root");
    c.ok(&["push", "A goal", "--why", "a value", "--root"]);
}

// ---------------------------------------------------------------------------
// `d779`: `session prompt --hook`. Read-only, and always exits 0: the seams
// tell an agent when to write, and this is the one that fires when nothing
// did, on every message rather than at the two boundaries of a session.
// ---------------------------------------------------------------------------

/// The exact two-line text, `{n}` filled in with the whole minutes elapsed
/// since the last thing worth calling a reference point -- the session's own
/// opening, or a capture since, whichever is more recent.
fn nudge_text(n: i64) -> String {
    format!(
        "vivac: nothing written to the tree in the last {n} min of this session. If a seam\npassed since (a new line of work, a choice, a finding you told, a \"not now\", work done, a change outside the repo), write it now, before you answer.\n"
    )
}

/// A raw `session.started` line, appended straight to `events` so its `ts`
/// can be set by hand: the CLI itself only ever stamps the real clock, and
/// this test needs a session that has been open for an exact, arbitrary
/// stretch.
fn raw_session_started(seq: u64, ts: &str, lane: &str) -> String {
    format!(
        r#"{{"seq":{seq},"id":"01PROMPTSTARTEDAAAAAAAAA{seq:02}","ts":"{ts}","actor":"a_test0000000","lane":"{lane}","payload":{{"type":"session.started","source":"test"}}}}"#
    )
}

/// A raw capture event -- `node.noted`, chosen because it is on the list and
/// needs no real node to name -- at a `ts` this test controls.
fn raw_capture(seq: u64, ts: &str, lane: &str) -> String {
    format!(
        r#"{{"seq":{seq},"id":"01PROMPTCAPTUREAAAAAAAA{seq:02}","ts":"{ts}","actor":"a_test0000000","lane":"{lane}","payload":{{"type":"node.noted","node":"ghost","note":"synthetic capture"}}}}"#
    )
}

/// A raw `vivac.created` of kind `auto` -- the automatic stop the `Stop`
/// hook writes on an ordinary turn, never a capture in its own right.
fn raw_auto_vivac(seq: u64, ts: &str, lane: &str) -> String {
    format!(
        r#"{{"seq":{seq},"id":"01PROMPTAUTOAAAAAAAAAAAA{seq:02}","ts":"{ts}","actor":"a_test0000000","lane":"{lane}","payload":{{"type":"vivac.created","vivac":"01PROMPTAUTOVIVACAAAAAAA{seq:02}","num":{seq},"kind":"auto","stack":[],"working_set":[],"next_intent":""}}}}"#
    )
}

/// `session prompt --hook`, with a minimal stdin payload and an explicit
/// `--now`, the way `brief`'s own tests pin the clock.
fn run_prompt(c: &Sandbox, now: &str) -> (String, i32) {
    c.run_stdin(
        &["session", "prompt", "--hook", "--now", now],
        r#"{"session_id":"s1"}"#,
    )
}

#[test]
fn with_no_session_started_it_says_nothing() {
    let c = Sandbox::new_seeded("prompt-no-session-start");
    let (out, code) = run_prompt(&c, "2026-09-24T09:20:00Z");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, "");
}

#[test]
fn a_three_minute_old_session_says_nothing() {
    let c = Sandbox::new_seeded("prompt-three-minutes");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));
    let (out, code) = run_prompt(&c, "2026-09-24T09:03:00Z");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, "");
}

#[test]
fn a_capture_four_minutes_ago_in_a_twenty_minute_session_says_nothing() {
    let c = Sandbox::new_seeded("prompt-quiet-not-yet");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));
    c.append_raw_line(&raw_capture(101, "2026-09-24T09:16:00Z", "main"));
    let (out, code) = run_prompt(&c, "2026-09-24T09:20:00Z");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, "");
}

#[test]
fn a_twenty_minute_session_with_no_capture_says_twenty() {
    let c = Sandbox::new_seeded("prompt-twenty-no-capture");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));
    let (out, code) = run_prompt(&c, "2026-09-24T09:20:00Z");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, nudge_text(20));
}

#[test]
fn a_capture_twelve_minutes_ago_in_a_thirty_minute_session_says_twelve() {
    let c = Sandbox::new_seeded("prompt-twelve");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));
    c.append_raw_line(&raw_capture(101, "2026-09-24T09:18:00Z", "main"));
    let (out, code) = run_prompt(&c, "2026-09-24T09:30:00Z");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, nudge_text(12));
}

/// An automatic stop from the `Stop` hook is not a capture: reading it as
/// one would let a turn that only ever closed itself with `auto` silence
/// the nudge forever.
#[test]
fn an_automatic_stop_does_not_count_as_a_capture() {
    let c = Sandbox::new_seeded("prompt-auto-not-capture");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));
    c.append_raw_line(&raw_auto_vivac(101, "2026-09-24T09:15:00Z", "main"));
    let (out, code) = run_prompt(&c, "2026-09-24T09:20:00Z");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, nudge_text(20));
}

/// Having spoken once, the same session stays quiet for ten minutes; past
/// that, the next call speaks again.
#[test]
fn it_cools_down_for_ten_minutes_then_speaks_again() {
    let c = Sandbox::new_seeded("prompt-cooldown");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));

    let (first, code) = run_prompt(&c, "2026-09-24T09:20:00Z");
    assert_eq!(code, 0, "{first}");
    assert_eq!(first, nudge_text(20));

    let (second, code) = run_prompt(&c, "2026-09-24T09:22:00Z");
    assert_eq!(code, 0, "{second}");
    assert_eq!(second, "", "spoke again inside the cooldown");

    let (third, code) = run_prompt(&c, "2026-09-24T09:31:00Z");
    assert_eq!(code, 0, "{third}");
    assert_eq!(third, nudge_text(31), "stayed quiet past the cooldown");
}

#[test]
fn broken_stdin_still_exits_zero_and_says_nothing() {
    let c = Sandbox::new_seeded("prompt-broken-stdin");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));
    let (out, code) = c.run_stdin(
        &[
            "session",
            "prompt",
            "--hook",
            "--now",
            "2026-09-24T09:20:00Z",
        ],
        "not json at all",
    );
    assert_eq!(code, 0, "{out}");
    // Garbage on stdin only costs the cooldown key its session id, and the
    // rest of the computation still runs off the log: this stays a real
    // nudge, not an outright failure, which is exactly the point.
    assert_eq!(out, nudge_text(20));
}

#[test]
fn empty_stdin_still_exits_zero() {
    let c = Sandbox::new_seeded("prompt-empty-stdin");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));
    let (out, code) = c.run_stdin(
        &[
            "session",
            "prompt",
            "--hook",
            "--now",
            "2026-09-24T09:20:00Z",
        ],
        "",
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, nudge_text(20));
}

#[test]
fn outside_a_tree_it_exits_zero_and_says_nothing() {
    let c = Sandbox::new_empty("prompt-no-tree");
    let (out, code) = run_prompt(&c, "2026-09-24T09:20:00Z");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, "");
}

/// Every hook reads the whole payload the harness writes, even where there
/// is no tree and nothing to do. A payload larger than a pipe's buffer
/// makes this deterministic: a hook that exits without reading leaves the
/// write blocked until it dies and then failing with a broken pipe, which
/// is what a harness would get.
#[test]
fn every_hook_drains_its_input_even_outside_a_tree() {
    use std::io::Write;
    let c = Sandbox::new_empty("hooks-drain-stdin");
    let payload = format!(
        "{{\"session_id\":\"s1\",\"source\":\"startup\",\"pad\":\"{}\"}}",
        "x".repeat(256 * 1024)
    );
    for sub in ["start", "prompt", "end"] {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
            .current_dir(&c.0)
            .args(["session", sub, "--hook"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let wrote = child.stdin.take().unwrap().write_all(payload.as_bytes());
        let o = child.wait_with_output().unwrap();
        assert!(
            wrote.is_ok(),
            "session {sub} --hook left its input unread: {wrote:?}"
        );
        assert_eq!(o.status.code(), Some(0), "session {sub} --hook");
    }
}

/// The one promise that matters most: whatever it decides to say, the hook
/// never writes a byte to the log itself.
#[test]
fn the_log_never_grows_from_calling_it() {
    let c = Sandbox::new_seeded("prompt-log-unchanged");
    c.append_raw_line(&raw_session_started(100, "2026-09-24T09:00:00Z", "main"));
    let before = c.log();
    let (out, code) = run_prompt(&c, "2026-09-24T09:20:00Z");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, nudge_text(20), "the fixture stopped nudging");
    assert_eq!(before, c.log(), "session prompt wrote to the log");
}
