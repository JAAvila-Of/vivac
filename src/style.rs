//! Terminal styles for a message a person reads (`d792`).
//!
//! `t789`: a plan that hard-wraps a status into a fixed column reads as a
//! table with its cells shuffled, not as prose. Colour and bold are the
//! fix for the *rhythm* of a plan -- which word is the verb, which is the
//! path -- and they are only ever added on top of the same plain words
//! [`enabled`] would otherwise print, never in place of them: a stream
//! that cannot render them (a pipe, `cmd.exe`, `--json`, a test) gets
//! exactly the same text, with no escape code in it at all.
//!
//! `d795`: `tree`, `open` and `why` add a second thing on top of the same
//! plain words -- wrapping a title at the terminal's own width instead of
//! at a fixed column, so a long one still reads as a tree and not as a
//! table with its own cells split at the wrong place. [`width`] answers
//! that question the same way [`enabled`] answers whether colour renders:
//! a real terminal, or `CLICOLOR_FORCE` standing in for one in a test.

use crate::event::{Kind, State};
use std::io::IsTerminal;
use std::sync::OnceLock;

/// Which stream a message is about to go out on. `Out` is every plan,
/// prompt and closing line `setup` and `init` print; `Err` is a failure's
/// own `print_to_stderr`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    Out,
    Err,
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const MAGENTA: &str = "\x1b[35m";
const BLUE: &str = "\x1b[34m";

/// The whole decision, minus the two reads ([`std::env::var`] and
/// `is_terminal`) that would make it impossible to test without racing
/// every other test over the same process environment. `enabled` is the
/// only caller that ever gathers real inputs for this; every test below
/// drives it with its own.
///
/// Order matters: `NO_COLOR` and `TERM=dumb` win over everything else,
/// `CLICOLOR_FORCE` wins over the terminal check (that is the whole point
/// of it existing -- a test can ask for styled output without a real
/// terminal to back it), and a plain terminal is the fallback once none of
/// the three env vars said anything.
fn decide(no_color: Option<&str>, term: Option<&str>, force: Option<&str>, is_tty: bool) -> bool {
    if no_color.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if term == Some("dumb") {
        return false;
    }
    if force.is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    is_tty
}

fn is_terminal(stream: Stream) -> bool {
    match stream {
        Stream::Out => std::io::stdout().is_terminal(),
        Stream::Err => std::io::stderr().is_terminal(),
    }
}

// ---------------------------------------------------------------------------
// Windows: a terminal that answers `is_terminal()` still needs to be asked,
// separately, whether it renders ANSI escapes at all -- `cmd.exe` and an
// old console host do not, until told to. The crate's only `unsafe` (`d792`).
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows_vt {
    use super::Stream;
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n: u32) -> *mut c_void;
        fn GetConsoleMode(h: *mut c_void, m: *mut u32) -> i32;
        fn SetConsoleMode(h: *mut c_void, m: u32) -> i32;
        fn GetConsoleScreenBufferInfo(h: *mut c_void, info: *mut ScreenBufferInfo) -> i32;
    }

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    /// `COORD`: two `SHORT`s, an x and a y.
    #[repr(C)]
    struct Coord {
        x: i16,
        y: i16,
    }

    /// `SMALL_RECT`: four `SHORT`s, in this order.
    #[repr(C)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }

    /// `CONSOLE_SCREEN_BUFFER_INFO`, field for field and in the same
    /// order, so `GetConsoleScreenBufferInfo` fills it exactly as it would
    /// its own struct. Only `window` is ever read; the rest exist because
    /// the kernel writes the whole thing regardless of which part is
    /// wanted.
    #[repr(C)]
    #[allow(dead_code)]
    struct ScreenBufferInfo {
        size: Coord,
        cursor: Coord,
        attributes: u16,
        window: SmallRect,
        maximum_window: Coord,
    }

    /// Whether `stream`'s console already renders ANSI escapes, turning it
    /// on first if it does not. Any failure along the way -- no handle, no
    /// mode to read, no mode `SetConsoleMode` would accept -- answers
    /// `false`: the side to be wrong on is the plain one.
    pub(super) fn enabled(stream: Stream) -> bool {
        let which = match stream {
            Stream::Out => STD_OUTPUT_HANDLE,
            Stream::Err => STD_ERROR_HANDLE,
        };
        // SAFETY: `which` is one of the two handle constants `GetStdHandle`
        // itself defines; the call cannot block and only ever returns a
        // handle or a null/invalid sentinel.
        let handle = unsafe { GetStdHandle(which) };
        if handle.is_null() {
            return false;
        }
        let mut mode: u32 = 0;
        // SAFETY: `handle` came back from `GetStdHandle` above, and `mode`
        // is a live, aligned `u32` the API writes one value into.
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            return false;
        }
        if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 {
            return true;
        }
        // SAFETY: the same handle, and the only value ever written is
        // `mode` with one known flag added, never anything read from
        // outside this function.
        unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0 }
    }

    /// The console's own column count: the window's right edge minus its
    /// left edge, plus one, the same arithmetic `GetConsoleScreenBufferInfo`'s
    /// own callers always do -- `dwSize` is the scrollback buffer, not the
    /// visible width. Any failure along the way answers `None`, the same
    /// side `enabled` answers wrong on above.
    pub(super) fn width(stream: Stream) -> Option<usize> {
        let which = match stream {
            Stream::Out => STD_OUTPUT_HANDLE,
            Stream::Err => STD_ERROR_HANDLE,
        };
        // SAFETY: `which` is one of the two handle constants `GetStdHandle`
        // itself defines; the call cannot block and only ever returns a
        // handle or a null/invalid sentinel.
        let handle = unsafe { GetStdHandle(which) };
        if handle.is_null() {
            return None;
        }
        let mut info = ScreenBufferInfo {
            size: Coord { x: 0, y: 0 },
            cursor: Coord { x: 0, y: 0 },
            attributes: 0,
            window: SmallRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            maximum_window: Coord { x: 0, y: 0 },
        };
        // SAFETY: `handle` came from `GetStdHandle` above, and `info` is a
        // live, aligned struct laid out field for field like
        // `CONSOLE_SCREEN_BUFFER_INFO`, which the API fills in fully on
        // success.
        if unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } == 0 {
            return None;
        }
        let columns = i32::from(info.window.right) - i32::from(info.window.left) + 1;
        if columns > 0 {
            Some(columns as usize)
        } else {
            None
        }
    }
}

/// Whether `stream` accepts styled text right now, computed once per
/// stream and cached: two `OnceLock`s rather than one, since `Out` and
/// `Err` can answer differently (output piped, errors still to a terminal,
/// or the reverse).
pub fn enabled(stream: Stream) -> bool {
    static OUT: OnceLock<bool> = OnceLock::new();
    static ERR: OnceLock<bool> = OnceLock::new();
    let cell = match stream {
        Stream::Out => &OUT,
        Stream::Err => &ERR,
    };
    *cell.get_or_init(|| compute(stream))
}

fn compute(stream: Stream) -> bool {
    let no_color = std::env::var("NO_COLOR").ok();
    let term = std::env::var("TERM").ok();
    let force = std::env::var("CLICOLOR_FORCE").ok();
    let is_tty = is_terminal(stream);
    if !decide(
        no_color.as_deref(),
        term.as_deref(),
        force.as_deref(),
        is_tty,
    ) {
        return false;
    }
    // `decide` said yes either because `CLICOLOR_FORCE` asked for it
    // outright, which is the one door that never needs a real console
    // behind it, or because the stream is an ordinary terminal, which on
    // Windows still has to be asked whether it renders ANSI at all.
    if force.as_deref().is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    #[cfg(windows)]
    {
        windows_vt::enabled(stream)
    }
    #[cfg(not(windows))]
    {
        true
    }
}

fn span(stream: Stream, code: &str, text: &str) -> String {
    if enabled(stream) {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

/// Two codes on the one piece of text -- bold and a colour together,
/// never two separate spans a reader would have to notice both landed on
/// the same word. Written as two escapes back to back rather than one
/// merged `\x1b[1;NNm`: the same visible result, and it keeps every code
/// this module already has exactly as it was.
fn dual_span(stream: Stream, a: &str, b: &str, text: &str) -> String {
    if enabled(stream) {
        format!("{a}{b}{text}{RESET}")
    } else {
        text.to_string()
    }
}

pub fn bold(stream: Stream, text: &str) -> String {
    span(stream, BOLD, text)
}

pub fn dim(stream: Stream, text: &str) -> String {
    span(stream, DIM, text)
}

/// A path or a file name: cyan, the one colour this module gives to
/// anything other than a verb.
pub fn path(stream: Stream, text: &str) -> String {
    span(stream, CYAN, text)
}

pub fn good(stream: Stream, text: &str) -> String {
    span(stream, GREEN, text)
}

pub fn change(stream: Stream, text: &str) -> String {
    span(stream, YELLOW, text)
}

pub fn gone(stream: Stream, text: &str) -> String {
    span(stream, RED, text)
}

/// A lead phrase ahead of a warning: yellow, the same colour `change` uses,
/// because the two are never on screen at once and sharing it costs
/// nothing.
pub fn warn(stream: Stream, text: &str) -> String {
    span(stream, YELLOW, text)
}

/// `word`'s own colour, decided from the word itself rather than passed
/// in: `d792` -- colour never carries meaning alone, so every one of these
/// stays a plain word with or without a terminal behind it, and this only
/// ever adds emphasis on top of it.
pub fn verb(stream: Stream, word: &str) -> String {
    match word {
        "create" | "add" | "plant" | "write" => good(stream, word),
        "replace" | "update" | "lock" => change(stream, word),
        "remove" => gone(stream, word),
        "keep" | "kept" | "already there" | "left as it is" | "unchanged" => dim(stream, word),
        _ => word.to_string(),
    }
}

/// An alias, bold and coloured by the node's own `Kind` (`d795`): the same
/// nine colours `tree`, `open` and `why` all read an alias by, so a reader
/// learns to recognise `t` as a task and `d` as a decision from the colour
/// alone, on top of the prefix letter that already says so in black and
/// white.
pub fn kind_id(stream: Stream, kind: Kind, alias: &str) -> String {
    let colour = match kind {
        Kind::Goal | Kind::Assumption => MAGENTA,
        Kind::Task => CYAN,
        Kind::Decision => GREEN,
        Kind::Finding => YELLOW,
        Kind::Question => BLUE,
        Kind::Pillar | Kind::Rule | Kind::Constraint => RED,
    };
    dual_span(stream, BOLD, colour, alias)
}

/// The bracketed one-letter mark `tree` and `why` print ahead of a title,
/// coloured by state -- the brackets are part of the styled span, not
/// added around it. `State::Active`'s mark is a bare space and stays
/// plain: there is nothing there for a colour to reinforce.
pub fn mark(stream: Stream, state: State) -> String {
    let text = format!("[{}]", state.mark());
    match state {
        State::Active => text,
        State::Done => good(stream, &text),
        State::Suspended => change(stream, &text),
        State::Abandoned => gone(stream, &text),
        State::Superseded => dim(stream, &text),
    }
}

/// Word-wraps `text` to fit the room left once `lead` columns are already
/// spoken for on the first line -- and, since every continuation line in
/// this crate is a hanging indent back to that same column, on every line
/// after it too. Returns the plain chunks, one per output line, with no
/// indentation added: the caller places each one and styles it only after
/// wrapping decided where the breaks fall, so an escape code this adds
/// next never counts toward `width`.
///
/// A single word wider than the room available for it is kept whole
/// rather than split, the same rule [`crate::render::wrap`] already uses.
pub fn wrap_title(lead: usize, text: &str, width: usize) -> Vec<String> {
    let room = width.saturating_sub(lead).max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let extra = usize::from(!cur.is_empty());
        if !cur.is_empty() && cur.chars().count() + extra + word.chars().count() > room {
            lines.push(std::mem::take(&mut cur));
        } else if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

// ---------------------------------------------------------------------------
// Terminal width. `d795`: a title `tree`, `open` or `why` prints has to
// wrap at the width the terminal actually has, not at a fixed column --
// wrapping at the wrong place reads as a table with its cells shuffled,
// which is the same failure `d792` already fixed for colour. `width`
// answers with the same two doors `enabled` uses: a real terminal, or
// `CLICOLOR_FORCE` standing in for one so a test can drive it without one.
// ---------------------------------------------------------------------------

/// The pure decision, in `decide`'s own shape: every real input the one
/// caller that gathers any (`compute_width`) has, so a test drives it with
/// plain values instead of a process environment and a real console. The
/// gate is the same as `enabled`'s -- a terminal, or `CLICOLOR_FORCE` --
/// and `NO_COLOR` never reaches this far: wrapping is layout, not colour,
/// and a person who cannot render colour can still have a terminal with a
/// real width. `COLUMNS`, when it parses to a positive integer, wins over
/// whatever the system answers; either way, a width under 40 columns comes
/// back as no answer at all, to keep a degenerate terminal from wrapping
/// every title down to one word a line.
fn decide_width(
    is_tty: bool,
    force: Option<&str>,
    columns: Option<&str>,
    system: Option<usize>,
) -> Option<usize> {
    let forced = force.is_some_and(|v| !v.is_empty() && v != "0");
    if !is_tty && !forced {
        return None;
    }
    let from_columns = columns
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0);
    from_columns.or(system).filter(|n| *n >= 40)
}

#[cfg(windows)]
fn system_width(stream: Stream) -> Option<usize> {
    windows_vt::width(stream)
}

#[cfg(unix)]
fn system_width(stream: Stream) -> Option<usize> {
    unix_tty::width(stream)
}

#[cfg(not(any(windows, unix)))]
fn system_width(_stream: Stream) -> Option<usize> {
    None
}

fn compute_width(stream: Stream) -> Option<usize> {
    let force = std::env::var("CLICOLOR_FORCE").ok();
    let columns = std::env::var("COLUMNS").ok();
    decide_width(
        is_terminal(stream),
        force.as_deref(),
        columns.as_deref(),
        system_width(stream),
    )
}

/// The terminal's own column count behind `stream`, cached once per stream
/// like [`enabled`] and for the same reason: two tests must never race
/// each other over the same process environment.
pub fn width(stream: Stream) -> Option<usize> {
    static OUT: OnceLock<Option<usize>> = OnceLock::new();
    static ERR: OnceLock<Option<usize>> = OnceLock::new();
    let cell = match stream {
        Stream::Out => &OUT,
        Stream::Err => &ERR,
    };
    *cell.get_or_init(|| compute_width(stream))
}

#[cfg(unix)]
mod unix_tty {
    use super::Stream;

    /// `struct winsize` from `<sys/ioctl.h>`: four `unsigned short`s, row
    /// and column first, then the pixel size nothing here reads. Declared
    /// with the same field order and width so the kernel fills it exactly
    /// as it would its own struct -- the names are this module's, not the
    /// kernel's, since only the layout has to match.
    #[repr(C)]
    #[allow(dead_code)]
    struct WinSize {
        row: u16,
        col: u16,
        x_pixel: u16,
        y_pixel: u16,
    }

    /// The type of `ioctl`'s request argument, which is not the same in
    /// every C library: glibc and the BSDs declare `unsigned long`, musl --
    /// what the Linux release binaries link -- declares `int`.
    #[cfg(all(target_os = "linux", target_env = "musl"))]
    type Request = std::ffi::c_int;
    #[cfg(not(all(target_os = "linux", target_env = "musl")))]
    type Request = std::ffi::c_ulong;

    #[cfg(target_os = "linux")]
    const TIOCGWINSZ: Request = 0x5413;
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    const TIOCGWINSZ: Request = 0x40087468;

    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    extern "C" {
        fn ioctl(fd: std::ffi::c_int, request: Request, ...) -> std::ffi::c_int;
    }

    /// The terminal's column count straight from the kernel -- the same
    /// call `stty size` makes. `fd` is `1` for [`Stream::Out`] and `2` for
    /// [`Stream::Err`], the only two descriptors this crate ever asks
    /// about; there is no path here for a caller to hand in an arbitrary
    /// one.
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    pub(super) fn width(stream: Stream) -> Option<usize> {
        let fd = match stream {
            Stream::Out => 1,
            Stream::Err => 2,
        };
        let mut ws = WinSize {
            row: 0,
            col: 0,
            x_pixel: 0,
            y_pixel: 0,
        };
        // SAFETY: `fd` is one of the two standard descriptors above,
        // `TIOCGWINSZ` is the kernel's read-only "report the window size"
        // request, and `ws` is a live, aligned buffer exactly the size the
        // kernel expects to fill.
        let rc = unsafe { ioctl(fd, TIOCGWINSZ, std::ptr::addr_of_mut!(ws)) };
        if rc == 0 && ws.col > 0 {
            Some(ws.col as usize)
        } else {
            None
        }
    }

    /// Every other Unix: no known `TIOCGWINSZ` value for it, so no call is
    /// made rather than guessing one.
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    pub(super) fn width(_stream: Stream) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_wins_over_everything_else() {
        assert!(!decide(Some("1"), None, Some("1"), true));
        assert!(!decide(Some("1"), None, Some("1"), false));
    }

    #[test]
    fn an_empty_no_color_does_not_count() {
        // `NO_COLOR`'s own spec: only "present and non-empty" turns it on.
        assert!(decide(Some(""), None, Some("1"), false));
    }

    #[test]
    fn term_dumb_disables_regardless_of_force() {
        assert!(!decide(None, Some("dumb"), Some("1"), true));
    }

    #[test]
    fn clicolor_force_enables_with_no_terminal_at_all() {
        assert!(decide(None, None, Some("1"), false));
        assert!(decide(None, None, Some("yes"), false));
    }

    #[test]
    fn clicolor_force_zero_does_not_count() {
        assert!(!decide(None, None, Some("0"), false));
    }

    #[test]
    fn with_no_env_the_terminal_alone_decides() {
        assert!(decide(None, None, None, true));
        assert!(!decide(None, None, None, false));
    }

    #[test]
    fn verb_colours_come_from_the_word_not_a_flag() {
        assert_eq!(verb(Stream::Out, "create"), good(Stream::Out, "create"));
        assert_eq!(verb(Stream::Out, "replace"), change(Stream::Out, "replace"));
        assert_eq!(verb(Stream::Out, "remove"), gone(Stream::Out, "remove"));
        assert_eq!(verb(Stream::Out, "keep"), dim(Stream::Out, "keep"));
        assert_eq!(verb(Stream::Out, "in"), "in");
    }

    #[test]
    fn spans_carry_the_word_untouched_either_way() {
        for f in [bold, dim, path, good, change, gone, warn] as [fn(Stream, &str) -> String; 7] {
            assert!(f(Stream::Out, "hook").contains("hook"));
        }
    }

    #[test]
    fn kind_id_carries_the_alias_untouched() {
        for k in [
            Kind::Goal,
            Kind::Task,
            Kind::Decision,
            Kind::Question,
            Kind::Constraint,
            Kind::Finding,
            Kind::Assumption,
            Kind::Pillar,
            Kind::Rule,
        ] {
            assert!(kind_id(Stream::Out, k, "x1").contains("x1"));
        }
    }

    // Compared against `dual_span` under the same call's own environment
    // rather than against a literal escape code, the same way
    // `verb_colours_come_from_the_word_not_a_flag` above compares `verb`
    // against `good`/`dim`/`gone`: whether either side actually carries a
    // colour here depends on the process this test happens to run in, and
    // `tests/styled_reads.rs` is where that is pinned down with
    // `CLICOLOR_FORCE`.
    #[test]
    fn kind_id_is_bold_and_coloured_in_the_one_span() {
        assert_eq!(
            kind_id(Stream::Out, Kind::Task, "t1"),
            dual_span(Stream::Out, BOLD, CYAN, "t1")
        );
        assert_eq!(
            kind_id(Stream::Out, Kind::Decision, "d1"),
            dual_span(Stream::Out, BOLD, GREEN, "d1")
        );
    }

    #[test]
    fn mark_colours_the_brackets_by_state() {
        assert_eq!(mark(Stream::Out, State::Active), "[ ]");
        assert_eq!(mark(Stream::Out, State::Done), good(Stream::Out, "[x]"));
        assert_eq!(
            mark(Stream::Out, State::Suspended),
            change(Stream::Out, "[~]")
        );
        assert_eq!(
            mark(Stream::Out, State::Abandoned),
            gone(Stream::Out, "[!]")
        );
        assert_eq!(
            mark(Stream::Out, State::Superseded),
            dim(Stream::Out, "[-]")
        );
    }

    #[test]
    fn wrap_title_keeps_short_text_on_one_line() {
        assert_eq!(wrap_title(8, "short title", 60), vec!["short title"]);
    }

    #[test]
    fn wrap_title_breaks_on_word_boundaries_within_the_room_lead_leaves() {
        let lines = wrap_title(8, "one two three four five six seven eight nine ten", 20);
        assert!(lines.iter().all(|l| l.chars().count() <= 12), "{lines:?}");
        assert_eq!(
            lines.join(" "),
            "one two three four five six seven eight nine ten"
        );
    }

    #[test]
    fn a_single_word_longer_than_the_room_stays_whole() {
        let lines = wrap_title(8, "supercalifragilisticexpialidocious", 20);
        assert_eq!(lines, vec!["supercalifragilisticexpialidocious"]);
    }

    #[test]
    fn wrap_title_returns_plain_chunks_with_no_leading_space() {
        let lines = wrap_title(8, "alpha beta gamma delta epsilon zeta", 20);
        assert!(lines.iter().all(|l| !l.starts_with(' ')), "{lines:?}");
    }

    #[test]
    fn width_answers_nothing_without_a_terminal_or_force() {
        assert_eq!(decide_width(false, None, Some("80"), Some(80)), None);
    }

    #[test]
    fn width_prefers_columns_over_the_system_call() {
        assert_eq!(decide_width(true, None, Some("100"), Some(80)), Some(100));
    }

    #[test]
    fn width_falls_back_to_the_system_call_without_columns() {
        assert_eq!(decide_width(true, None, None, Some(80)), Some(80));
    }

    #[test]
    fn force_alone_answers_with_columns_and_no_terminal_at_all() {
        assert_eq!(decide_width(false, Some("1"), Some("72"), None), Some(72));
    }

    #[test]
    fn an_unusable_columns_value_falls_back_to_the_system_call() {
        assert_eq!(decide_width(true, None, Some("0"), Some(80)), Some(80));
        assert_eq!(decide_width(true, None, Some("nope"), Some(80)), Some(80));
    }

    #[test]
    fn a_width_under_forty_columns_is_treated_as_no_answer() {
        assert_eq!(decide_width(true, None, Some("39"), None), None);
        assert_eq!(decide_width(true, None, None, Some(10)), None);
    }

    #[test]
    fn no_system_answer_and_no_columns_is_no_answer() {
        assert_eq!(decide_width(true, None, None, None), None);
    }
}
