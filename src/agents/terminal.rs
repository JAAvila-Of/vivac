//! Short selections with a line-oriented fallback.

use crate::failure::Failure;
use std::io::{self, IsTerminal, Read, Write};

macro_rules! say {
    ($($arg:tt)*) => { writeln!(io::stdout(), $($arg)*)? };
}

pub(super) fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub(super) fn line(question: &str) -> Result<Option<String>, Failure> {
    write!(io::stdout(), "{} ", clean(question))?;
    io::stdout().flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        return Ok(None);
    }
    let answer = answer.trim().to_owned();
    if answer == "q" {
        Ok(None)
    } else {
        Ok(Some(answer))
    }
}

fn parse_selection(answer: &str, len: usize, defaults: &[bool]) -> Option<Vec<bool>> {
    if answer.is_empty() {
        return Some(defaults.to_vec());
    }
    if answer == "none" {
        return Some(vec![false; len]);
    }
    let mut selected = vec![false; len];
    for word in answer.split([',', ' ']).filter(|word| !word.is_empty()) {
        let index = word.parse::<usize>().ok()?.checked_sub(1)?;
        *selected.get_mut(index)? = true;
    }
    Some(selected)
}

#[derive(Debug, PartialEq)]
enum Action {
    Continue,
    Accept,
    Cancel,
}

fn key_action(key: u8, selected: &mut [bool], cursor: &mut usize) -> Action {
    match key {
        b'q' | 3 => Action::Cancel,
        b'\r' | b'\n' => Action::Accept,
        b' ' => {
            selected[*cursor] = !selected[*cursor];
            Action::Continue
        }
        b'j' => {
            *cursor = (*cursor + 1) % selected.len();
            Action::Continue
        }
        b'k' => {
            *cursor = (*cursor + selected.len() - 1) % selected.len();
            Action::Continue
        }
        _ => Action::Continue,
    }
}

pub(super) fn select(
    title: &str,
    labels: &[String],
    defaults: &[bool],
) -> Result<Option<Vec<bool>>, Failure> {
    if labels.len() != defaults.len() {
        return Err(Failure::usage("Invalid selection."));
    }
    say!("{}", clean(title));
    if labels.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let raw = io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && crate::style::enabled(crate::style::Stream::Out);
    if raw {
        if let Some(_guard) = platform::Raw::enter() {
            let mut selected = defaults.to_vec();
            let mut cursor = 0;
            say!("Space: select; arrows or j/k: move; Enter: continue; q: cancel");
            loop {
                for (index, label) in labels.iter().enumerate() {
                    say!(
                        "{} [{}] {}",
                        if cursor == index { ">" } else { " " },
                        if selected[index] { "x" } else { " " },
                        clean(label)
                    );
                }
                io::stdout().flush()?;
                let mut byte = [0];
                if io::stdin().read(&mut byte)? == 0 {
                    return Ok(None);
                }
                let key = if byte[0] == 27 {
                    let mut sequence = [0; 2];
                    if io::stdin().read_exact(&mut sequence).is_err() {
                        return Ok(None);
                    }
                    match sequence {
                        [b'[', b'B'] => b'j',
                        [b'[', b'A'] => b'k',
                        _ => 0,
                    }
                } else {
                    byte[0]
                };
                match key_action(key, &mut selected, &mut cursor) {
                    Action::Cancel => return Ok(None),
                    Action::Accept => return Ok(Some(selected)),
                    Action::Continue => {}
                }
                write!(io::stdout(), "\x1b[{}A", labels.len())?;
                for _ in labels {
                    writeln!(io::stdout(), "\x1b[2K")?;
                }
                write!(io::stdout(), "\x1b[{}A", labels.len())?;
            }
        }
    }
    for (index, label) in labels.iter().enumerate() {
        say!(
            "{}. [{}] {}",
            index + 1,
            if defaults[index] { "x" } else { " " },
            clean(label)
        );
    }
    loop {
        let Some(answer) =
            line("Numbers separated by spaces (Enter keeps selection; none clears; q cancels):")?
        else {
            return Ok(None);
        };
        if let Some(selected) = parse_selection(&answer, labels.len(), defaults) {
            return Ok(Some(selected));
        }
        say!("Choose numbers from the list.");
    }
}

#[cfg(unix)]
mod platform {
    pub(super) struct Raw(libc::termios);
    impl Raw {
        pub(super) fn enter() -> Option<Self> {
            let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
            if unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) } != 0 {
                return None;
            }
            let original = unsafe { original.assume_init() };
            let mut mode = original;
            mode.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
            mode.c_cc[libc::VMIN] = 1;
            mode.c_cc[libc::VTIME] = 0;
            if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &mode) } != 0 {
                return None;
            }
            Some(Self(original))
        }
    }
    impl Drop for Raw {
        fn drop(&mut self) {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    }
    pub(super) struct Raw {
        handle: *mut c_void,
        mode: u32,
    }
    impl Raw {
        pub(super) fn enter() -> Option<Self> {
            let handle = unsafe { GetStdHandle((-10i32) as u32) };
            let mut mode = 0;
            if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
                return None;
            }
            if unsafe { SetConsoleMode(handle, (mode & !7) | 0x200) } == 0 {
                return None;
            }
            Some(Self { handle, mode })
        }
    }
    impl Drop for Raw {
        fn drop(&mut self) {
            unsafe {
                SetConsoleMode(self.handle, self.mode);
            }
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    pub(super) struct Raw;
    impl Raw {
        pub(super) fn enter() -> Option<Self> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_controls_are_not_rendered() {
        assert_eq!(clean("agent\x1b[31m\n\u{7}"), "agent [31m  ");
    }
    #[test]
    fn line_selection_rejects_outside_choices_and_preserves_defaults() {
        assert_eq!(
            parse_selection("", 2, &[true, false]),
            Some(vec![true, false])
        );
        assert_eq!(
            parse_selection("2", 2, &[true, false]),
            Some(vec![false, true])
        );
        assert_eq!(parse_selection("0", 2, &[true, false]), None);
        assert_eq!(parse_selection("3", 2, &[true, false]), None);
    }

    #[test]
    fn keyboard_selection_and_cancellation_are_separate_from_application() {
        let mut selected = [false, false];
        let mut cursor = 0;
        assert_eq!(
            key_action(b' ', &mut selected, &mut cursor),
            Action::Continue
        );
        assert_eq!(
            key_action(b'j', &mut selected, &mut cursor),
            Action::Continue
        );
        assert_eq!(
            key_action(b' ', &mut selected, &mut cursor),
            Action::Continue
        );
        assert_eq!(selected, [true, true]);
        assert_eq!(key_action(3, &mut selected, &mut cursor), Action::Cancel);
        assert_eq!(key_action(b'q', &mut selected, &mut cursor), Action::Cancel);
        assert_eq!(
            key_action(b'\r', &mut selected, &mut cursor),
            Action::Accept
        );
    }
}
