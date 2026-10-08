//! Short selections with a line-oriented fallback.

use crate::failure::Failure;
use crate::style::{self, Stream::Out};
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

fn parse_single(answer: &str, labels: &[String], default: usize) -> Option<usize> {
    if answer.is_empty() {
        return (default < labels.len()).then_some(default);
    }
    labels.iter().position(|label| label == answer).or_else(|| {
        answer
            .parse::<usize>()
            .ok()?
            .checked_sub(1)
            .filter(|index| *index < labels.len())
    })
}

fn single_key_action(key: u8, len: usize, cursor: &mut usize) -> Action {
    match key {
        b'q' | 3 => Action::Cancel,
        b'\r' | b'\n' => Action::Accept,
        b'j' => {
            *cursor = (*cursor + 1) % len;
            Action::Continue
        }
        b'k' => {
            *cursor = (*cursor + len - 1) % len;
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn read_key() -> Result<Option<u8>, Failure> {
    let mut byte = [0];
    if io::stdin().read(&mut byte)? == 0 {
        return Ok(None);
    }
    if byte[0] != 27 {
        return Ok(Some(byte[0]));
    }
    let mut sequence = [0; 2];
    if io::stdin().read_exact(&mut sequence).is_err() {
        return Ok(None);
    }
    Ok(Some(match sequence {
        [b'[', b'B'] => b'j',
        [b'[', b'A'] => b'k',
        _ => 0,
    }))
}

fn clear_choices(len: usize) -> Result<(), Failure> {
    write!(io::stdout(), "\x1b[{len}A")?;
    for _ in 0..len {
        writeln!(io::stdout(), "\x1b[2K")?;
    }
    write!(io::stdout(), "\x1b[{len}A")?;
    Ok(())
}

fn choice_row(prefix: &str, mark: &str, label: &str, focused: bool, selected: bool) -> String {
    let prefix = clean(prefix);
    let mark = clean(mark);
    let label = clean(label);
    let prefix = if focused {
        style::bold(Out, &style::path(Out, &prefix))
    } else {
        prefix
    };
    let mark = if selected {
        style::good(Out, &mark)
    } else {
        style::dim(Out, &mark)
    };
    let label = if label.contains("configuration differs") {
        style::gone(Out, &label)
    } else if label.contains("not managed") {
        style::warn(Out, &label)
    } else if focused {
        style::bold(Out, &style::path(Out, &label))
    } else if selected {
        style::good(Out, &label)
    } else {
        style::dim(Out, &label)
    };
    format!("{prefix} {mark} {label}")
}

pub(super) fn single(
    title: &str,
    labels: &[String],
    default: usize,
    allow_custom: bool,
) -> Result<Option<String>, Failure> {
    if labels.is_empty() {
        return line(title);
    }
    if default >= labels.len() {
        return Err(Failure::usage("Invalid selection."));
    }
    say!(
        "{}",
        crate::style::bold(crate::style::Stream::Out, &clean(title))
    );
    if io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && crate::style::enabled(crate::style::Stream::Out)
    {
        if let Some(_guard) = platform::Raw::enter() {
            let mut choices = labels.to_vec();
            if allow_custom {
                choices.push("Enter a model identifier".into());
            }
            let mut cursor = default;
            say!("Arrows or j/k: move; Enter: confirm; q: cancel");
            loop {
                for (index, label) in choices.iter().enumerate() {
                    say!(
                        "{}",
                        choice_row(
                            if cursor == index { ">" } else { " " },
                            if cursor == index { "(*)" } else { "( )" },
                            label,
                            cursor == index,
                            cursor == index
                        )
                    );
                }
                io::stdout().flush()?;
                let Some(key) = read_key()? else {
                    return Ok(None);
                };
                match single_key_action(key, choices.len(), &mut cursor) {
                    Action::Cancel => return Ok(None),
                    Action::Accept => {
                        drop(_guard);
                        if cursor == labels.len() {
                            loop {
                                let Some(answer) = line("Model identifier (q cancels):")? else {
                                    return Ok(None);
                                };
                                if !answer.is_empty() && !answer.chars().any(char::is_control) {
                                    return Ok(Some(answer));
                                }
                                say!("Enter a non-empty model identifier.");
                            }
                        }
                        return Ok(Some(labels[cursor].clone()));
                    }
                    Action::Continue => clear_choices(choices.len())?,
                }
            }
        }
    }
    for (index, label) in labels.iter().enumerate() {
        say!(
            "{}",
            choice_row(
                &format!("  {}.", index + 1),
                if index == default { "(*)" } else { "( )" },
                label,
                false,
                index == default
            )
        );
    }
    loop {
        let Some(answer) = line(&format!(
            "{} [{}] (value or number; q cancels):",
            clean(title),
            clean(&labels[default])
        ))?
        else {
            return Ok(None);
        };
        if let Some(index) = parse_single(&answer, labels, default) {
            return Ok(Some(labels[index].clone()));
        }
        if allow_custom && !answer.is_empty() && !answer.chars().any(char::is_control) {
            return Ok(Some(answer));
        }
        say!("Choose one value or number from the list.");
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
    say!("{}", style::bold(Out, &clean(title)));
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
                        "{}",
                        choice_row(
                            if cursor == index { ">" } else { " " },
                            if selected[index] { "[x]" } else { "[ ]" },
                            label,
                            cursor == index,
                            selected[index]
                        )
                    );
                }
                io::stdout().flush()?;
                let Some(key) = read_key()? else {
                    return Ok(None);
                };
                match key_action(key, &mut selected, &mut cursor) {
                    Action::Cancel => return Ok(None),
                    Action::Accept => return Ok(Some(selected)),
                    Action::Continue => {}
                }
                clear_choices(labels.len())?;
            }
        }
    }
    for (index, label) in labels.iter().enumerate() {
        say!(
            "{}",
            choice_row(
                &format!("{}.", index + 1),
                if defaults[index] { "[x]" } else { "[ ]" },
                label,
                false,
                defaults[index]
            )
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
    fn selector_styles_preserve_plain_text_and_honor_terminal_opt_outs() {
        const MARKER: &str = "VIVAC_TEST_SELECTOR_STYLES";
        if std::env::var_os(MARKER).is_some() {
            for (focused, selected, label) in [
                (true, true, "model"),
                (false, true, "reviewer: configuration differs"),
                (false, false, "reviewer: not managed"),
            ] {
                crate::output::outln!(
                    "ROW {}",
                    choice_row(
                        if focused { ">" } else { " " },
                        if selected { "[x]" } else { "[ ]" },
                        label,
                        focused,
                        selected
                    )
                );
            }
            crate::output::flush();
            return;
        }
        let render = |no_color: bool, dumb: bool| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "agents::terminal::tests::selector_styles_preserve_plain_text_and_honor_terminal_opt_outs", "--nocapture"])
                .env(MARKER, "1").env("CLICOLOR_FORCE", "1").env_remove("NO_COLOR").env_remove("TERM");
            if no_color {
                command.env("NO_COLOR", "1");
            }
            if dumb {
                command.env("TERM", "dumb");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter(|line| line.starts_with("ROW "))
                .map(str::to_owned)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let forced = render(false, false);
        let plain = render(true, false);
        assert!(forced.contains('\x1b'));
        assert!(!plain.contains('\x1b'));
        let mut stripped = String::new();
        let mut sequence = false;
        for character in forced.chars() {
            if character == '\x1b' {
                sequence = true;
            } else if sequence {
                if character == 'm' {
                    sequence = false;
                }
            } else {
                stripped.push(character);
            }
        }
        assert_eq!(stripped, plain);
        assert_eq!(render(false, true), plain);
    }
    #[test]
    fn choice_rows_keep_focus_selection_and_status_as_text() {
        let row = choice_row(">", "[x]", "reviewer: configuration differs\n", true, true);
        assert!(row.contains(">"));
        assert!(row.contains("[x]"));
        assert!(row.contains("configuration differs"));
        assert!(!row.contains('\n'));
        assert!(!row.contains("\x1b[2J"));
    }
    #[test]
    fn single_choice_accepts_one_value_and_never_a_checkbox_set() {
        let labels = vec!["inherit".into(), "high".into()];
        assert_eq!(parse_single("", &labels, 0), Some(0));
        assert_eq!(parse_single("2", &labels, 0), Some(1));
        assert_eq!(parse_single("high", &labels, 0), Some(1));
        assert_eq!(parse_single("1 2", &labels, 0), None);
        assert_eq!(parse_single("none", &labels, 0), None);
        let mut cursor = 0;
        assert_eq!(single_key_action(b'j', 2, &mut cursor), Action::Continue);
        assert_eq!(cursor, 1);
        assert_eq!(single_key_action(b' ', 2, &mut cursor), Action::Continue);
        assert_eq!(cursor, 1);
        assert_eq!(single_key_action(b'\r', 2, &mut cursor), Action::Accept);
        assert_eq!(single_key_action(b'q', 2, &mut cursor), Action::Cancel);
    }
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
