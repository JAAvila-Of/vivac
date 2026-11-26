use std::cell::Cell;
use std::io;
use std::mem::zeroed;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};

static SSH_HANGUP: AtomicBool = AtomicBool::new(false);
static CLAIMED: AtomicBool = AtomicBool::new(false);

extern "C" fn ssh_hangup(_: libc::c_int) {
    SSH_HANGUP.store(true, Ordering::Relaxed);
}

fn refused() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectCause {
    Eof,
    Eio,
    Sighup,
}
impl DisconnectCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eof => "eof",
            Self::Eio => "eio",
            Self::Sighup => "sighup",
        }
    }
}

fn stdin_result(size: isize, errno: Option<i32>) -> io::Result<DisconnectCause> {
    match (size, errno) {
        (0, _) => Ok(DisconnectCause::Eof),
        (-1, Some(libc::EIO)) => Ok(DisconnectCause::Eio),
        _ => Err(refused()),
    }
}

/// One process-wide SSH lifetime, installed before starting session threads.
/// Dropping restores the prior handler but never permits another instance.
pub struct SshLifetime {
    descriptor: OwnedFd,
    disconnect: Cell<Option<DisconnectCause>>,
    previous: Option<libc::sigaction>,
}

impl SshLifetime {
    pub fn stdin() -> io::Result<Self> {
        // SAFETY: F_DUPFD_CLOEXEC duplicates the existing standard input atomically.
        let descriptor = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful fcntl returned a fresh owned descriptor.
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| refused())?;
        // SAFETY: zero initializes sigaction; its mask is initialized before installation.
        let mut action: libc::sigaction = unsafe { zeroed() };
        // SAFETY: sigaction fills this initialized output with the original action.
        let mut previous: libc::sigaction = unsafe { zeroed() };
        action.sa_sigaction = ssh_hangup as *const () as usize;
        // SAFETY: the handler only stores an atomic flag and both action pointers are valid.
        unsafe {
            if libc::sigemptyset(&mut action.sa_mask) != 0
                || libc::sigaction(libc::SIGHUP, &action, &mut previous) != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self {
            descriptor,
            disconnect: Cell::new(None),
            previous: Some(previous),
        })
    }

    pub fn disconnect_cause(&self) -> Option<DisconnectCause> {
        self.disconnect.get()
    }

    pub fn poll(&self) -> io::Result<Option<DisconnectCause>> {
        if let Some(cause) = self.disconnect.get() {
            return Ok(Some(cause));
        }
        loop {
            if SSH_HANGUP.load(Ordering::Relaxed) {
                self.disconnect.set(Some(DisconnectCause::Sighup));
                return Ok(self.disconnect.get());
            }
            let mut input = libc::pollfd {
                fd: self.descriptor.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll receives one initialized writable pollfd with a zero timeout.
            let result = unsafe { libc::poll(&mut input, 1, 0) };
            if result < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io::Error::last_os_error());
            }
            if input.revents & libc::POLLNVAL != 0 {
                return Err(refused());
            }
            if input.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
                return Ok(None);
            }
            let mut byte = 0u8;
            // SAFETY: readiness was observed and read writes at most this one byte.
            let size = unsafe {
                libc::read(
                    self.descriptor.as_raw_fd(),
                    (&mut byte as *mut u8).cast(),
                    1,
                )
            };
            let error = if size < 0 {
                io::Error::last_os_error().raw_os_error()
            } else {
                None
            };
            if size < 0 && error == Some(libc::EINTR) {
                continue;
            }
            let cause = if SSH_HANGUP.load(Ordering::Relaxed) {
                DisconnectCause::Sighup
            } else {
                stdin_result(size, error)?
            };
            self.disconnect.set(Some(cause));
            return Ok(Some(cause));
        }
    }
}

impl AsRawFd for SshLifetime {
    fn as_raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }
}

impl Drop for SshLifetime {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            // SAFETY: restore the exact action captured by this unique process-wide instance.
            let _ = unsafe { libc::sigaction(libc::SIGHUP, &previous, std::ptr::null_mut()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn stdin_disconnect_requires_eof_or_terminal_eio() {
        assert_eq!(stdin_result(0, None).expect("EOF"), DisconnectCause::Eof);
        assert_eq!(
            stdin_result(-1, Some(libc::EIO)).expect("EIO"),
            DisconnectCause::Eio
        );
        for (size, error) in [
            (1, None),
            (-1, Some(libc::EBADF)),
            (-1, Some(libc::EAGAIN)),
            (-1, None),
        ] {
            assert!(stdin_result(size, error).is_err());
        }
        assert_eq!(DisconnectCause::Sighup.as_str(), "sighup");
    }

    fn pipe() -> (SshLifetime, std::fs::File) {
        let mut descriptors = [-1; 2];
        // SAFETY: pipe2 writes two fresh descriptors into the initialized array.
        assert_eq!(
            unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        // SAFETY: successful pipe2 transferred the two distinct descriptors.
        let input = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        // SAFETY: the second fresh descriptor belongs to the returned writer.
        let output = unsafe { std::fs::File::from_raw_fd(descriptors[1]) };
        (
            SshLifetime {
                descriptor: input,
                disconnect: Cell::new(None),
                previous: None,
            },
            output,
        )
    }

    #[test]
    fn own_pipe_reports_no_data_then_sticky_eof_without_waiting() {
        let (lifetime, output) = pipe();
        assert_eq!(lifetime.poll().unwrap(), None);
        drop(output);
        assert_eq!(lifetime.poll().unwrap(), Some(DisconnectCause::Eof));
        assert_eq!(lifetime.poll().unwrap(), Some(DisconnectCause::Eof));
    }

    #[test]
    fn bytes_on_the_lifetime_pipe_are_not_observer_commands() {
        let (lifetime, mut output) = pipe();
        output.write_all(b"C").unwrap();
        assert!(lifetime.poll().is_err());
    }

    #[test]
    fn singleton_owns_a_cloexec_duplicate_and_restores_the_previous_handler() {
        // SAFETY: sigaction fills this initialized output without changing the action.
        let mut original: libc::sigaction = unsafe { zeroed() };
        // SAFETY: querying the current process handler does not deliver a signal.
        assert_eq!(
            unsafe { libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut original) },
            0
        );
        let lifetime = SshLifetime::stdin().unwrap();
        assert!(lifetime.as_raw_fd() >= 3);
        // SAFETY: F_GETFD queries the owned descriptor without changing it.
        assert_ne!(
            unsafe { libc::fcntl(lifetime.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert!(SshLifetime::stdin().is_err());
        drop(lifetime);
        // SAFETY: query the restored process handler without delivering a signal.
        let mut restored: libc::sigaction = unsafe { zeroed() };
        // SAFETY: the output pointer is valid for one sigaction.
        assert_eq!(
            unsafe { libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut restored) },
            0
        );
        assert_eq!(restored.sa_sigaction, original.sa_sigaction);
        assert!(SshLifetime::stdin().is_err());
    }
}
