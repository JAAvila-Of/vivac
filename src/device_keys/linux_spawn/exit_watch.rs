use super::{last_error, raw_error, validate_descriptor, SpawnOwner, ADMISSION};
use std::cell::Cell;
use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::rc::Rc;

#[derive(Debug, Eq, PartialEq)]
enum PollState {
    Pending,
    Exited,
}

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn classify_poll(count: libc::c_int, events: libc::c_short) -> io::Result<PollState> {
    match (count, events) {
        (0, 0) => Ok(PollState::Pending),
        (1, events)
            if events & libc::POLLIN != 0 && events & !(libc::POLLIN | libc::POLLHUP) == 0 =>
        {
            Ok(PollState::Exited)
        }
        _ => Err(invalid()),
    }
}

fn poll_once(pidfd: &OwnedFd) -> io::Result<PollState> {
    validate_descriptor(pidfd.as_raw_fd())?;
    let mut descriptor = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: the single initialized poll descriptor remains owned throughout.
    let count = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if count < 0 {
        return Err(last_error());
    }
    let state = classify_poll(count, descriptor.revents)?;
    validate_descriptor(pidfd.as_raw_fd())?;
    Ok(state)
}

// The caller supplies a process pidfd whose identity was validated while live.
pub(crate) fn retained_process_exited(pidfd: &OwnedFd) -> io::Result<bool> {
    Ok(poll_once(pidfd)? == PollState::Exited)
}

struct Origin {
    process: libc::pid_t,
    thread: libc::pid_t,
}

impl Origin {
    fn validate(&self) -> io::Result<()> {
        // SAFETY: both calls only query this process and thread identity.
        if unsafe { libc::getpid() } != self.process
            || unsafe { libc::syscall(libc::SYS_gettid) } != libc::c_long::from(self.thread)
        {
            return Err(raw_error(libc::EPERM));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Active,
    Failed,
    Consumed,
}

struct State(Cell<Phase>);

impl State {
    fn new() -> Self {
        Self(Cell::new(Phase::Active))
    }

    fn checked<T>(&self, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        if self.0.get() != Phase::Active {
            return Err(invalid());
        }
        match operation() {
            Ok(value) => Ok(value),
            Err(error) => {
                self.0.set(Phase::Failed);
                Err(error)
            }
        }
    }

    fn consume(&self) {
        self.0.set(Phase::Consumed);
    }
}

/// A retained child-process reference, not session or store authority.
pub struct ChildExitWatch {
    pidfd: Option<OwnedFd>,
    origin: Origin,
    state: State,
    _thread_affine: PhantomData<Rc<()>>,
}

/// Observed termination of one retained child, without a session-close claim.
pub struct ObservedChildExit {
    pidfd: OwnedFd,
    origin: Origin,
    state: State,
    _thread_affine: PhantomData<Rc<()>>,
}

impl SpawnOwner {
    pub fn watch_child_exit(&self, token: u64) -> io::Result<ChildExitWatch> {
        self.check_thread()?;
        if ADMISSION.uncertain() {
            return Err(raw_error(libc::EOWNERDEAD));
        }
        if self.closing {
            return Err(raw_error(libc::EBUSY));
        }
        if let Some(reservation) = &self.reservation {
            reservation.validate_owned()?;
        }
        let entry = self.entry(token)?;
        if entry.exit.is_some() {
            return Err(raw_error(libc::ESRCH));
        }
        let retained = entry
            .pidfd
            .as_ref()
            .ok_or_else(|| raw_error(libc::ENOTSUP))?;
        if poll_once(retained)? != PollState::Pending {
            return Err(raw_error(libc::ESRCH));
        }
        // SAFETY: fcntl duplicates this retained pidfd above stdio with CLOEXEC.
        let descriptor = unsafe { libc::fcntl(retained.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if descriptor < 0 {
            return Err(last_error());
        }
        // SAFETY: successful fcntl returned a fresh owned descriptor.
        let pidfd = unsafe { OwnedFd::from_raw_fd(descriptor) };
        if poll_once(&pidfd)? != PollState::Pending {
            return Err(raw_error(libc::ESRCH));
        }
        self.check_thread()?;
        if ADMISSION.uncertain() {
            return Err(raw_error(libc::EOWNERDEAD));
        }
        if let Some(reservation) = &self.reservation {
            reservation.validate_owned()?;
        }
        Ok(ChildExitWatch {
            pidfd: Some(pidfd),
            origin: Origin {
                process: self.parent,
                thread: self.creator_tid,
            },
            state: State::new(),
            _thread_affine: PhantomData,
        })
    }
}

impl ChildExitWatch {
    pub fn try_observe(&mut self) -> io::Result<Option<ObservedChildExit>> {
        let outcome = self.state.checked(|| {
            self.origin.validate()?;
            let pidfd = self.pidfd.as_ref().ok_or_else(invalid)?;
            let outcome = poll_once(pidfd)?;
            self.origin.validate()?;
            Ok(outcome)
        })?;
        if outcome == PollState::Pending {
            return Ok(None);
        }
        let pidfd = self.pidfd.take().ok_or_else(invalid)?;
        self.state.consume();
        Ok(Some(ObservedChildExit {
            pidfd,
            origin: Origin {
                process: self.origin.process,
                thread: self.origin.thread,
            },
            state: State::new(),
            _thread_affine: PhantomData,
        }))
    }
}

impl ObservedChildExit {
    pub fn validate(&self) -> io::Result<()> {
        self.state.checked(|| {
            self.origin.validate()?;
            if poll_once(&self.pidfd)? != PollState::Exited {
                return Err(invalid());
            }
            self.origin.validate()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_watch_pending_requires_no_readiness() {
        assert_eq!(classify_poll(0, 0).unwrap(), PollState::Pending);
    }

    #[test]
    fn exit_watch_accepts_readable_and_reaped_processes() {
        for events in [libc::POLLIN, libc::POLLIN | libc::POLLHUP] {
            assert_eq!(classify_poll(1, events).unwrap(), PollState::Exited);
        }
    }

    #[test]
    fn exit_watch_rejects_errors_and_hangup_without_readability() {
        for events in [
            libc::POLLHUP,
            libc::POLLERR,
            libc::POLLNVAL,
            libc::POLLIN | libc::POLLERR,
            libc::POLLIN | libc::POLLNVAL,
            libc::POLLOUT,
            libc::POLLIN | libc::POLLOUT,
        ] {
            assert!(classify_poll(1, events).is_err());
        }
    }

    #[test]
    fn exit_watch_rejects_inconsistent_poll_counts_and_flags() {
        for (count, events) in [(0, libc::POLLIN), (1, 0), (2, libc::POLLIN), (-1, 0)] {
            assert!(classify_poll(count, events).is_err());
        }
    }

    #[test]
    fn exit_watch_first_error_is_preserved_without_retrying_the_operation() {
        let state = State::new();
        let error = state
            .checked(|| Err::<(), _>(raw_error(libc::EINTR)))
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EINTR));
        let called = Cell::new(false);
        let error = state
            .checked(|| {
                called.set(true);
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!called.get());
    }

    #[test]
    fn exit_watch_consumption_prevents_another_observation() {
        let state = State::new();
        assert_eq!(state.checked(|| Ok(17)).unwrap(), 17);
        state.consume();
        let called = Cell::new(false);
        let error = state
            .checked(|| {
                called.set(true);
                Ok(23)
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!called.get());
    }
}
