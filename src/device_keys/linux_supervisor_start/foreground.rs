use crate::attended_terminal::{attributes, same_attributes, AttendedUnlockTerminal};
use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use zeroize::Zeroizing;

/// Until `open-linux` hands off to the session unit, the foreground keeper
/// supplies the private directory that the unit's runtime directory replaces.
pub(super) fn create_runtime_parent() -> io::Result<PathBuf> {
    let mut template = b"/tmp/vivac-session-XXXXXX\0".to_vec();
    // SAFETY: mkdtemp writes within this writable NUL-terminated template.
    let created = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
    if created.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful mkdtemp left a complete string in the template.
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(unsafe {
        CStr::from_ptr(created).to_bytes()
    })))
}

impl AttendedUnlockTerminal {
    pub(crate) fn validate_foreground_hold(&self) -> io::Result<()> {
        self.validate().map_err(|_| hold_refused())?;
        let current = attributes(self.as_raw_fd()).map_err(|_| hold_refused())?;
        if current.c_lflag & libc::ICANON == 0 {
            return Err(hold_refused());
        }
        Ok(())
    }

    /// `served` receives each report the supervisor sends while the session
    /// is held; any other readiness of the supervisor channel ends the hold.
    pub(crate) fn wait_for_explicit_close(
        &self,
        supervisor: BorrowedFd<'_>,
        agent: BorrowedFd<'_>,
        mut served: impl FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        self.validate_foreground_hold()?;
        let original = attributes(self.as_raw_fd()).map_err(|_| hold_refused())?;
        let mask = super::cancellation::signal_set()?;
        // The caller blocks cancellation before starting any session threads.
        // SAFETY: the initialized mask requests a new owned signal descriptor.
        let descriptor =
            unsafe { libc::signalfd(-1, &mask, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful signalfd returned a fresh exclusively owned descriptor.
        let signal = unsafe { OwnedFd::from_raw_fd(descriptor) };
        let descriptors = [
            self.as_raw_fd(),
            supervisor.as_raw_fd(),
            agent.as_raw_fd(),
            signal.as_raw_fd(),
        ];
        poll_foreground_hold(descriptors, 0, 0, &mut served)?;
        // SAFETY: the retained tty is validated and only queued input is discarded.
        if unsafe { libc::tcflush(self.as_raw_fd(), libc::TCIFLUSH) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let prompt = b"Device custody is active. Type CLOSE to close: ";
        let mut position = 0;
        while position < prompt.len() {
            poll_foreground_hold(descriptors, libc::POLLOUT, -1, &mut served)?;
            self.validate_hold_attributes(&original)?;
            // SAFETY: the remaining prompt bytes are live for the declared length.
            let written = unsafe {
                libc::write(
                    self.as_raw_fd(),
                    prompt[position..].as_ptr().cast(),
                    prompt.len() - position,
                )
            };
            if written <= 0 {
                return Err(hold_refused());
            }
            position += written as usize;
        }
        poll_foreground_hold(descriptors, libc::POLLIN, -1, &mut served)?;
        self.validate_hold_attributes(&original)?;
        let mut input = Zeroizing::new([0u8; 7]);
        // SAFETY: input is writable for seven bytes, enough to reject an oversized line.
        let count = unsafe { libc::read(self.as_raw_fd(), input.as_mut_ptr().cast(), input.len()) };
        if count != 6 || &input[..6] != b"CLOSE\n" {
            return Err(hold_refused());
        }
        self.validate_hold_attributes(&original)?;
        // Signals remain pending so the keeper can refuse publication after cancellation.
        poll_foreground_hold(descriptors, 0, 0, &mut served)?;
        Ok(())
    }

    fn validate_hold_attributes(&self, expected: &libc::termios) -> io::Result<()> {
        self.validate_foreground_hold()?;
        let current = attributes(self.as_raw_fd()).map_err(|_| hold_refused())?;
        if !same_attributes(&current, expected) {
            return Err(hold_refused());
        }
        Ok(())
    }
}

fn hold_refused() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}

fn poll_foreground_hold(
    descriptors: [RawFd; 4],
    event: libc::c_short,
    timeout: i32,
    served: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    loop {
        let mut watched = descriptors.map(|fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
        watched[0].events = event;
        // SAFETY: watched is writable for all four initialized poll descriptors.
        let ready = unsafe { libc::poll(watched.as_mut_ptr(), watched.len() as _, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if watched[1].revents == libc::POLLIN
            && watched[2..]
                .iter()
                .all(|descriptor| descriptor.revents == 0)
            && watched[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0
        {
            served()?;
            continue;
        }
        if watched[1..]
            .iter()
            .any(|descriptor| descriptor.revents != 0)
            || watched[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
        {
            return Err(hold_refused());
        }
        if timeout == 0 || watched[0].revents & event != 0 {
            return Ok(());
        }
    }
}
