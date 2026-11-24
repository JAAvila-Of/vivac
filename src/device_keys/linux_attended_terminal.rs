use std::fs::{File, OpenOptions};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, ()>;

pub(crate) struct AttendedUnlockInput {
    secret: Zeroizing<Vec<u8>>,
}

pub(crate) struct AttendedUnlockTerminal {
    tty: File,
}

impl AttendedUnlockTerminal {
    pub(crate) fn open() -> Result<Self> {
        let terminal = Self {
            tty: OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(
                    libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW,
                )
                .open("/dev/tty")
                .map_err(|_| ())?,
        };
        terminal.validate()?;
        Ok(terminal)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        foreground(self.tty.as_raw_fd())?;
        native_line_discipline(self.tty.as_raw_fd())?;
        if attributes(self.tty.as_raw_fd())?.c_line != 0 {
            return Err(());
        }
        Ok(())
    }

    pub(crate) fn read(self) -> Result<AttendedUnlockInput> {
        let deadline = Instant::now() + Duration::from_secs(15);
        Terminal::from_retained(self)?.read_existing_unlock(deadline)
    }
}

impl AsRawFd for AttendedUnlockTerminal {
    fn as_raw_fd(&self) -> RawFd {
        self.tty.as_raw_fd()
    }
}

impl AttendedUnlockInput {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.secret
    }
}

pub(crate) const ENCODED_LENGTH: usize = 64;
const EXISTING_UNLOCK_PROMPT: &[u8] = b"Device credential unlock secret: ";
const ECHO_FLAGS: libc::tcflag_t = libc::ECHO
    | libc::ECHONL
    | libc::ECHOE
    | libc::ECHOK
    | libc::ECHOKE
    | libc::ECHOCTL
    | libc::ECHOPRT;
const SIGNALS: [libc::c_int; 7] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
];

pub(crate) fn attributes(fd: RawFd) -> Result<libc::termios> {
    let mut value = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: fd remains owned and value is writable for one complete termios.
    if unsafe { libc::tcgetattr(fd, value.as_mut_ptr()) } != 0 {
        return Err(());
    }
    // SAFETY: successful tcgetattr initialized the complete termios value.
    Ok(unsafe { value.assume_init() })
}

pub(crate) fn same_attributes(left: &libc::termios, right: &libc::termios) -> bool {
    left.c_iflag == right.c_iflag
        && left.c_oflag == right.c_oflag
        && left.c_cflag == right.c_cflag
        && left.c_lflag == right.c_lflag
        && left.c_line == right.c_line
        && left.c_cc == right.c_cc
        && left.c_ispeed == right.c_ispeed
        && left.c_ospeed == right.c_ospeed
}

fn foreground(fd: RawFd) -> Result<()> {
    // SAFETY: both functions only query process and terminal state.
    let terminal_group = unsafe { libc::tcgetpgrp(fd) };
    // SAFETY: getpgrp takes no arguments and has no failure return.
    let process_group = unsafe { libc::getpgrp() };
    if terminal_group < 0 || terminal_group != process_group {
        Err(())
    } else {
        Ok(())
    }
}

fn native_line_discipline(fd: RawFd) -> Result<()> {
    let mut discipline: libc::c_int = -1;
    // SAFETY: TIOCGETD writes one c_int to the live output pointer.
    let result = unsafe { libc::ioctl(fd, libc::TIOCGETD, &mut discipline) };
    if result == 0 && discipline == 0 {
        Ok(())
    } else {
        Err(())
    }
}

fn signal_set() -> Result<libc::sigset_t> {
    let mut value = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: sigemptyset initializes the complete signal-set object.
    if unsafe { libc::sigemptyset(value.as_mut_ptr()) } != 0 {
        return Err(());
    }
    // SAFETY: successful sigemptyset initialized this object.
    let mut value = unsafe { value.assume_init() };
    for signal in SIGNALS {
        // SAFETY: value is a live initialized signal set and signal is valid.
        if unsafe { libc::sigaddset(&mut value, signal) } != 0 {
            return Err(());
        }
    }
    Ok(value)
}

pub(crate) struct Terminal {
    tty: File,
    signal: Option<OwnedFd>,
    original: libc::termios,
    configured: libc::termios,
    original_mask: libc::sigset_t,
    cleanup_done: bool,
    cleanup_succeeded: bool,
}

impl Terminal {
    pub(crate) fn open() -> Result<Self> {
        Self::from_retained(AttendedUnlockTerminal::open()?)
    }

    pub(crate) fn from_retained(terminal: AttendedUnlockTerminal) -> Result<Self> {
        terminal.validate()?;
        let AttendedUnlockTerminal { tty } = terminal;
        let original = attributes(tty.as_raw_fd())?;
        if original.c_line != 0 {
            return Err(());
        }
        let mut configured = original;
        configured.c_lflag |= libc::ICANON | libc::ISIG;
        configured.c_lflag &= !(ECHO_FLAGS | libc::IEXTEN);
        configured.c_iflag &= !(libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::PARMRK
            | libc::IUCLC
            | libc::IXON
            | libc::IXOFF);
        configured.c_iflag |= libc::ICRNL;
        configured.c_cc[libc::VEOF] = 4;
        configured.c_cc[libc::VINTR] = 3;
        configured.c_cc[libc::VQUIT] = 28;
        configured.c_cc[libc::VSUSP] = 26;
        configured.c_cc[libc::VEOL] = 0;
        configured.c_cc[libc::VEOL2] = 0;

        let blocked = signal_set()?;
        let mut old_mask = MaybeUninit::<libc::sigset_t>::uninit();
        // Single-threaded experiment: no other thread may change these masks.
        // SAFETY: the input set is initialized and the output set is writable.
        if unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, old_mask.as_mut_ptr()) } != 0 {
            return Err(());
        }
        let mut terminal = Self {
            tty,
            signal: None,
            original,
            configured,
            // SAFETY: successful pthread_sigmask initialized the previous mask.
            original_mask: unsafe { old_mask.assume_init() },
            cleanup_done: false,
            cleanup_succeeded: false,
        };
        // SAFETY: the blocked signal set is live; -1 requests a new descriptor.
        let signal_fd =
            unsafe { libc::signalfd(-1, &blocked, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) };
        if signal_fd < 0 {
            return Err(());
        }
        // SAFETY: signalfd returned a new exclusively owned descriptor.
        terminal.signal = Some(unsafe { OwnedFd::from_raw_fd(signal_fd) });
        foreground(terminal.tty.as_raw_fd())?;
        // SAFETY: the owned tty and initialized termios live across this call.
        if unsafe {
            libc::tcsetattr(
                terminal.tty.as_raw_fd(),
                libc::TCSANOW,
                &terminal.configured,
            )
        } != 0
        {
            return Err(());
        }
        terminal.check_active()?;
        Ok(terminal)
    }

    fn read_existing_unlock(mut self, deadline: Instant) -> Result<AttendedUnlockInput> {
        let result = (|| {
            let secret = self.read_line(EXISTING_UNLOCK_PROMPT, deadline)?;
            validate_existing_unlock_encoding(&secret)?;
            Ok(secret)
        })();
        self.finish()?;
        if Instant::now() >= deadline {
            return Err(());
        }
        result.map(|secret| AttendedUnlockInput { secret })
    }

    pub(crate) fn check_active(&self) -> Result<()> {
        foreground(self.tty.as_raw_fd())?;
        native_line_discipline(self.tty.as_raw_fd())?;
        if same_attributes(&attributes(self.tty.as_raw_fd())?, &self.configured) {
            Ok(())
        } else {
            Err(())
        }
    }

    fn signal_pending(&self) -> Result<bool> {
        let signal = self.signal.as_ref().ok_or(())?;
        let mut information = MaybeUninit::<libc::signalfd_siginfo>::uninit();
        // SAFETY: signal remains owned and the buffer fits one complete record.
        let count = unsafe {
            libc::read(
                signal.as_raw_fd(),
                information.as_mut_ptr().cast(),
                std::mem::size_of::<libc::signalfd_siginfo>(),
            )
        };
        if count < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
            Ok(false)
        } else if count == std::mem::size_of::<libc::signalfd_siginfo>() as isize {
            Ok(true)
        } else {
            Err(())
        }
    }

    fn wait_for(&self, event: libc::c_short, deadline: Instant) -> Result<()> {
        let signal = self.signal.as_ref().ok_or(())?;
        loop {
            if self.signal_pending()? {
                return Err(());
            }
            let remaining = deadline.checked_duration_since(Instant::now()).ok_or(())?;
            let timeout = remaining.as_millis().max(1).min(i32::MAX as u128) as i32;
            let mut descriptors = [
                libc::pollfd {
                    fd: self.tty.as_raw_fd(),
                    events: event,
                    revents: 0,
                },
                libc::pollfd {
                    fd: signal.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: descriptors is writable for its full declared length.
            let ready = unsafe {
                libc::poll(
                    descriptors.as_mut_ptr(),
                    descriptors.len() as libc::nfds_t,
                    timeout,
                )
            };
            if ready < 0 || Instant::now() >= deadline {
                return Err(());
            }
            let failed = libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;
            if descriptors
                .iter()
                .any(|descriptor| descriptor.revents & failed != 0)
            {
                return Err(());
            }
            if descriptors[1].revents != 0 || self.signal_pending()? {
                let _ = self.signal_pending();
                return Err(());
            }
            if descriptors[0].revents & event != 0 {
                return Ok(());
            }
        }
    }

    fn prompt(&self, prompt: &[u8], deadline: Instant) -> Result<()> {
        self.check_active()?;
        // SAFETY: the owned tty is valid; this only discards pending input.
        if unsafe { libc::tcflush(self.tty.as_raw_fd(), libc::TCIFLUSH) } != 0 {
            return Err(());
        }
        let mut position = 0;
        while position < prompt.len() {
            self.wait_for(libc::POLLOUT, deadline)?;
            self.check_active()?;
            // SAFETY: the remaining prompt slice stays live for the write.
            let count = unsafe {
                libc::write(
                    self.tty.as_raw_fd(),
                    prompt[position..].as_ptr().cast(),
                    prompt.len() - position,
                )
            };
            if count > 0 {
                position += count as usize;
            } else if count < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                continue;
            } else {
                return Err(());
            }
        }
        Ok(())
    }

    pub(crate) fn read_line(&self, prompt: &[u8], deadline: Instant) -> Result<Zeroizing<Vec<u8>>> {
        self.prompt(prompt, deadline)?;
        let mut buffer = Zeroizing::new([0u8; 1026]);
        loop {
            self.wait_for(libc::POLLIN, deadline)?;
            self.check_active()?;
            // SAFETY: buffer has its complete writable capacity for this read.
            let count = unsafe {
                libc::read(
                    self.tty.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if count < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                continue;
            }
            if count <= 0 {
                return Err(());
            }
            self.check_active()?;
            if self.signal_pending()? {
                return Err(());
            }
            return validate_line(&buffer[..count as usize]);
        }
    }

    fn restore_terminal(&self) -> bool {
        // SAFETY: tty remains owned; original is the saved initialized state.
        let set =
            unsafe { libc::tcsetattr(self.tty.as_raw_fd(), libc::TCSANOW, &self.original) } == 0;
        set && attributes(self.tty.as_raw_fd())
            .map(|current| same_attributes(&current, &self.original))
            .unwrap_or(false)
    }

    pub(crate) fn finish(&mut self) -> Result<()> {
        self.restore_checked()?;
        foreground(self.tty.as_raw_fd())
    }

    fn restore_checked(&mut self) -> Result<()> {
        if self.cleanup_done {
            return if self.cleanup_succeeded {
                Ok(())
            } else {
                Err(())
            };
        }
        // All restoration attempts precede restoring the signal mask. A failed
        // or disappeared tty cannot promise echo recovery and never earns success.
        // SAFETY: the owned tty is live; failure is retained in the result.
        let mut succeeded = unsafe { libc::tcflush(self.tty.as_raw_fd(), libc::TCIFLUSH) } == 0;
        let mut terminal_restored = false;
        for _ in 0..2 {
            let restored = self.restore_terminal();
            succeeded &= restored;
            if restored {
                terminal_restored = true;
                break;
            }
        }
        // The successful input interval ends here, after terminal restoration
        // and before unblocking. Later signals do not transactionally cancel
        // key publication. Constructor failures may not yet have a signalfd.
        if self.signal.is_some() {
            succeeded &= matches!(self.signal_pending(), Ok(false));
        }
        let mut mask_restored = false;
        for _ in 0..2 {
            // SAFETY: the saved mask is initialized and no output is requested.
            let restored = unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &self.original_mask, std::ptr::null_mut())
            } == 0;
            succeeded &= restored;
            if restored {
                mask_restored = true;
                break;
            }
        }
        self.cleanup_done = true;
        self.cleanup_succeeded = succeeded && terminal_restored && mask_restored;
        if self.cleanup_succeeded {
            Ok(())
        } else {
            Err(())
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore_checked();
    }
}

fn validate_line(line: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if !(17..=513).contains(&line.len()) || line.last() != Some(&b'\n') {
        return Err(());
    }
    let password = &line[..line.len() - 1];
    if password.contains(&b'\n')
        || password.contains(&b'\r')
        || password.contains(&0)
        || std::str::from_utf8(password).is_err()
    {
        return Err(());
    }
    Ok(Zeroizing::new(password.to_vec()))
}

fn validate_existing_unlock_encoding(input: &[u8]) -> Result<()> {
    if input.len() == ENCODED_LENGTH
        && input
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        Ok(())
    } else {
        Err(())
    }
}

pub(crate) fn read_existing_unlock() -> Result<AttendedUnlockInput> {
    // This input reader shares the single-threaded and blockable-signal
    // boundary of the existing terminal experiment; it grants no key authority.
    let deadline = Instant::now() + Duration::from_secs(15);
    Terminal::open()?.read_existing_unlock(deadline)
}

#[cfg(test)]
mod tests {
    use super::{validate_existing_unlock_encoding, validate_line, AttendedUnlockTerminal};

    #[test]
    fn attended_unlock_terminal_refuses_a_nonterminal_descriptor() {
        let terminal = AttendedUnlockTerminal {
            tty: std::fs::File::options()
                .read(true)
                .write(true)
                .open("/dev/null")
                .expect("open nonterminal fixture"),
        };
        assert!(terminal.validate().is_err());
        assert!(terminal.read().is_err());
    }

    #[test]
    fn existing_unlock_encoding_accepts_canonical_values_without_normalization() {
        for input in [
            vec![b'0'; 64],
            vec![b'f'; 64],
            b"00000000000000000123456789abcdef0123456789abcdef0123456789abcdef".to_vec(),
        ] {
            assert_eq!(input.len(), 64);
            assert!(validate_existing_unlock_encoding(&input).is_ok());
        }
    }

    #[test]
    fn existing_unlock_encoding_rejects_wrong_length_and_case() {
        for input in [
            Vec::new(),
            vec![b'a'; 63],
            vec![b'a'; 65],
            vec![b'a'; 512],
            vec![b'A'; 64],
            vec![b'g'; 64],
        ] {
            assert!(validate_existing_unlock_encoding(&input).is_err());
        }
    }

    #[test]
    fn existing_unlock_encoding_rejects_control_bytes_and_non_ascii_input() {
        for byte in [0, b'\r', b'\n', b' ', b'\t', 0xff] {
            let mut input = vec![b'a'; 64];
            input[31] = byte;
            assert!(validate_existing_unlock_encoding(&input).is_err());
        }
        let mut utf8 = vec![b'a'; 62];
        utf8.extend_from_slice(&[0xc3, 0xa9]);
        assert_eq!(utf8.len(), 64);
        assert!(validate_existing_unlock_encoding(&utf8).is_err());
    }

    #[test]
    fn only_complete_bounded_utf8_lines_are_accepted() {
        for password in [vec![b'a'; 16], vec![b'b'; 512], "á".repeat(8).into_bytes()] {
            let mut line = password.clone();
            line.push(b'\n');
            let accepted = validate_line(&line).expect("public fixture should be accepted");
            assert_eq!(accepted.as_slice(), password.as_slice());
        }
        for invalid in [
            Vec::new(),
            vec![b'a'; 17],
            b"partial-eof-without-linefeed".to_vec(),
            b"short\n".to_vec(),
            [vec![b'a'; 513], vec![b'\n']].concat(),
            [vec![b'a'; 16], vec![0, b'\n']].concat(),
            [vec![b'a'; 16], vec![b'\r', b'\n']].concat(),
            [vec![b'a'; 16], vec![b'\n', b'\n']].concat(),
            [vec![0xff; 16], vec![b'\n']].concat(),
        ] {
            assert!(validate_line(&invalid).is_err());
        }
    }
}
