//! The sole owner of standard output for line-by-line rendering.
//!
//! `println!` writes through `Stdout`'s `LineWriter`, which flushes -- one
//! syscall -- on every newline. `tree` at 10 000 nodes composes 6845 lines in
//! under 12 ms and then spent roughly twice that just handing them to the
//! terminal one at a time, which is most of why it broke its own 50 ms
//! budget.
//!
//! `outln!` is the replacement. Every line goes into a `BufWriter` around the
//! stream instead of straight to the OS, and nothing reaches the terminal
//! until [`flush`] runs. `main` calls it on every path out -- success or
//! failure -- because `std::process::exit` skips `Drop`, and an unflushed
//! buffer would vanish with it rather than reach the reader.
//!
//! The buffer wraps `Stdout`, not a `StdoutLock`: a lock cannot live in a
//! `static` -- it is neither `Send` nor `Sync`, tied to the thread that took
//! it -- and holding one for the run's whole length would leave nothing free
//! to lock for the one-shot writes below the moment this buffer's first line
//! landed. `Stdout` itself locks internally, once per flush rather than once
//! per line, which is the same saving without that risk.
//!
//! A small number of one-shot writes (`USAGE`, an `Outcome`, the brief) stay
//! on plain `print!`: they hand the whole text over in a single call already,
//! so `LineWriter` never flushes mid-render for them, and they are gone
//! before this module's buffer is ever touched in the same run.

use std::io::{self, BufWriter, Write};
use std::sync::{Mutex, OnceLock};

fn handle() -> &'static Mutex<BufWriter<io::Stdout>> {
    static HANDLE: OnceLock<Mutex<BufWriter<io::Stdout>>> = OnceLock::new();
    HANDLE.get_or_init(|| Mutex::new(BufWriter::new(io::stdout())))
}

/// Writes one line into the buffer. Nothing reaches the terminal until
/// [`flush`] runs.
///
/// A broken pipe (`vivac tree | head`) is swallowed here rather than left to
/// panic the way `println!` does: the read end is gone, there is nobody left
/// to tell, and the right thing is to finish quietly instead of tearing down
/// with a panic message on a pipe that already closed.
pub fn write_line(args: std::fmt::Arguments) {
    let mut w = handle().lock().unwrap_or_else(|e| e.into_inner());
    let _ = w.write_fmt(args);
    let _ = w.write_all(b"\n");
}

/// Empties the buffer onto the real stream.
///
/// Called once by `main` on every path out, success or failure, and once
/// more before a failure writes to stderr, so a refusal never overtakes the
/// output that came before it.
pub fn flush() {
    let mut w = handle().lock().unwrap_or_else(|e| e.into_inner());
    let _ = w.flush();
}

/// Writes one line through the sole owner of standard output, in place of
/// `println!` -- see the module doc for why.
macro_rules! outln {
    () => {
        $crate::output::write_line(format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::output::write_line(format_args!($($arg)*))
    };
}
pub(crate) use outln;
