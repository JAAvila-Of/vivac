//! Terminal styles for a message a person reads (`d792`).
//!
//! `t789`: a plan that hard-wraps a status into a fixed column reads as a
//! table with its cells shuffled, not as prose. Colour and bold are the
//! fix for the *rhythm* of a plan -- which word is the verb, which is the
//! path -- and they are only ever added on top of the same plain words
//! [`enabled`] would otherwise print, never in place of them: a stream
//! that cannot render them (a pipe, `cmd.exe`, `--json`, a test) gets
//! exactly the same text, with no escape code in it at all.

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
    }

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

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
}
