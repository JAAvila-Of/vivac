use super::super::linux_session::PackageLease;
use super::{
    last_error, raw_error, send_pidfd_signal, validate_descriptor, Receipt, SpawnOwner, ADMISSION,
};
use std::cell::Cell;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

struct ValidationState(Cell<bool>);

impl ValidationState {
    fn checked<T>(&self, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        if self.0.get() {
            return Err(raw_error(libc::EINVAL));
        }
        let result = operation();
        if result.is_err() {
            self.0.set(true);
        }
        result
    }
}

/// The complete retained pair of one frozen owner, without session authority.
pub struct OwnedChildRegistration<'owner> {
    owner: &'owner SpawnOwner,
    receipts: [Receipt; 2],
    originals: [RawFd; 2],
    images: [(libc::dev_t, libc::ino_t); 2],
    references: [OwnedFd; 2],
    state: ValidationState,
}

fn validate_count(count: usize) -> io::Result<()> {
    if count != 2 {
        return Err(raw_error(libc::EINVAL));
    }
    Ok(())
}

fn validate_pending(count: libc::c_int, events: libc::c_short) -> io::Result<()> {
    if count != 0 || events != 0 {
        return Err(raw_error(libc::ESRCH));
    }
    Ok(())
}

fn pending(reference: &OwnedFd) -> io::Result<()> {
    validate_descriptor(reference.as_raw_fd())?;
    let mut descriptor = libc::pollfd {
        fd: reference.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: the initialized poll descriptor stays owned during this query.
    let count = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if count < 0 {
        return Err(last_error());
    }
    validate_pending(count, descriptor.revents)?;
    validate_descriptor(reference.as_raw_fd())
}

fn validate_reference(reference: &OwnedFd) -> io::Result<()> {
    pending(reference)?;
    // The interface check uses the retained process reference, never a PID.
    send_pidfd_signal(reference, 0)?;
    pending(reference)
}

fn launch_image_identity(reference: &OwnedFd) -> io::Result<(libc::dev_t, libc::ino_t)> {
    validate_descriptor(reference.as_raw_fd())?;
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the descriptor is owned and status is writable for one stat.
    if unsafe { libc::fstat(reference.as_raw_fd(), status.as_mut_ptr()) } < 0 {
        return Err(last_error());
    }
    // SAFETY: fstat initialized status on success.
    let status = unsafe { status.assume_init() };
    if status.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(raw_error(libc::EINVAL));
    }
    Ok((status.st_dev, status.st_ino))
}

fn duplicate(reference: &OwnedFd) -> io::Result<OwnedFd> {
    validate_reference(reference)?;
    // SAFETY: fcntl duplicates the retained pidfd above stdio with CLOEXEC.
    let descriptor = unsafe { libc::fcntl(reference.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if descriptor < 0 {
        return Err(last_error());
    }
    // SAFETY: successful fcntl returned a new exclusively owned descriptor.
    let duplicate = unsafe { OwnedFd::from_raw_fd(descriptor) };
    validate_reference(reference)?;
    validate_reference(&duplicate)?;
    Ok(duplicate)
}

impl SpawnOwner {
    pub fn prepare_child_registration(&mut self) -> io::Result<OwnedChildRegistration<'_>> {
        if std::mem::replace(&mut self.registration_frozen, true) {
            return Err(raw_error(libc::EBUSY));
        }
        self.validate_registration_entries()?;
        let receipts = [self.entries[0].receipt, self.entries[1].receipt];
        let originals = [
            self.entries[0].pidfd.as_ref().unwrap().as_raw_fd(),
            self.entries[1].pidfd.as_ref().unwrap().as_raw_fd(),
        ];
        let images = [
            launch_image_identity(&self.entries[0]._resources.executable)?,
            launch_image_identity(&self.entries[1]._resources.executable)?,
        ];
        let references = [
            duplicate(self.entries[0].pidfd.as_ref().unwrap())?,
            duplicate(self.entries[1].pidfd.as_ref().unwrap())?,
        ];
        let registration = OwnedChildRegistration {
            owner: self,
            receipts,
            originals,
            images,
            references,
            state: ValidationState(Cell::new(false)),
        };
        registration.validate()?;
        Ok(registration)
    }

    fn validate_registration_context(&self) -> io::Result<()> {
        self.check_thread()?;
        if ADMISSION.uncertain() {
            return Err(raw_error(libc::EOWNERDEAD));
        }
        if self.closing || !self.registration_frozen {
            return Err(raw_error(libc::EBUSY));
        }
        Ok(())
    }

    fn validate_registration_entries(&self) -> io::Result<()> {
        self.validate_registration_context()?;
        validate_count(self.entries.len())?;
        let first = &self.entries[0];
        let second = &self.entries[1];
        if first.receipt.token == second.receipt.token || first.receipt.pid == second.receipt.pid {
            return Err(raw_error(libc::EINVAL));
        }
        for entry in &self.entries {
            if entry.exit.is_some() || entry.child.id() != entry.receipt.pid {
                return Err(raw_error(libc::ESRCH));
            }
            validate_reference(
                entry
                    .pidfd
                    .as_ref()
                    .ok_or_else(|| raw_error(libc::ENOTSUP))?,
            )?;
        }
        self.validate_registration_context()
    }
}

impl OwnedChildRegistration<'_> {
    pub(crate) fn receipts(&self) -> io::Result<[Receipt; 2]> {
        self.validate()?;
        Ok(self.receipts)
    }

    pub(crate) fn keeper_offer_references(&self, lease: &PackageLease) -> io::Result<[OwnedFd; 3]> {
        self.validate()?;
        self.owner.validate_reservation(lease)?;
        let directory = self
            .owner
            .reservation
            .as_ref()
            .ok_or_else(|| raw_error(libc::EINVAL))?
            .duplicate_locked_directory()?;
        let references = [
            directory,
            duplicate(&self.references[0])?,
            duplicate(&self.references[1])?,
        ];
        self.validate()?;
        Ok(references)
    }

    pub(crate) fn session_generation(&self, lease: &PackageLease) -> io::Result<[u8; 32]> {
        self.validate()?;
        let generation = self.owner.session_generation(lease)?;
        self.validate()?;
        Ok(generation)
    }

    pub fn validate(&self) -> io::Result<()> {
        self.state.checked(|| {
            self.owner.validate_registration_entries()?;
            for (index, entry) in self.owner.entries.iter().enumerate() {
                if entry.receipt != self.receipts[index]
                    || entry.pidfd.as_ref().map(AsRawFd::as_raw_fd) != Some(self.originals[index])
                    || launch_image_identity(&entry._resources.executable)? != self.images[index]
                {
                    return Err(raw_error(libc::EINVAL));
                }
                validate_reference(&self.references[index])?;
            }
            self.owner.validate_registration_entries()
        })
    }

    pub(crate) fn validate_executable_order(&self, expected: [&OwnedFd; 2]) -> io::Result<()> {
        self.state.checked(|| {
            self.validate()?;
            for (image, expected) in self.images.iter().zip(expected) {
                if *image != launch_image_identity(expected)? {
                    return Err(raw_error(libc::EINVAL));
                }
            }
            self.validate()
        })
    }

    /// Checks expected launch receipts and executable objects in registration order.
    pub fn validate_role_order(&self, expected: [(Receipt, &OwnedFd); 2]) -> io::Result<()> {
        self.state.checked(|| {
            let [(first, first_image), (second, second_image)] = expected;
            if self.receipts != [first, second] {
                return Err(raw_error(libc::EINVAL));
            }
            self.validate_executable_order([first_image, second_image])
        })
    }

    pub(crate) fn validate_task_restriction(&self) -> io::Result<()> {
        self.state.checked(|| {
            self.validate()?;
            if !self
                .owner
                .entries
                .iter()
                .all(|entry| entry.tasks_restricted)
            {
                return Err(raw_error(libc::EINVAL));
            }
            self.validate()
        })
    }

    pub(crate) fn with_references<T>(
        &self,
        operation: impl FnOnce(&[OwnedFd; 2]) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = operation(&self.references);
        self.validate()?;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::SpawnSpec;
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn registration_requires_exactly_two_entries() {
        for count in [0, 1, 3, 4] {
            assert!(validate_count(count).is_err());
        }
        assert!(validate_count(2).is_ok());
    }

    #[test]
    fn registration_failure_permanently_freezes_spawns_without_closing_owner() {
        let mut owner = SpawnOwner::new().unwrap();
        let next_token = owner.next_token;
        assert!(owner.prepare_child_registration().is_err());
        assert!(owner.registration_frozen);
        assert!(!owner.closing);
        assert_eq!(owner.next_token, next_token);
        assert!(owner.entries.is_empty());
        assert!(!ADMISSION.uncertain());
        let descriptor = || std::fs::File::open("/dev/null").unwrap().into();
        let spec = SpawnSpec {
            executable: descriptor(),
            arg0: OsString::from("marker"),
            arguments: Vec::new(),
            environment: Vec::new(),
            stdio: [descriptor(), descriptor(), descriptor()],
            inherited: Vec::new(),
            retained: Vec::new(),
            new_session: false,
            controlling_tty: None,
            probe: None,
        };
        assert_eq!(
            owner.spawn(spec).unwrap_err().raw_os_error(),
            Some(libc::EBUSY)
        );
        assert!(owner.prepare_child_registration().is_err());
        owner.shutdown(std::time::Duration::ZERO).unwrap();
    }

    #[test]
    fn registration_validation_failure_cannot_be_retried_after_restoration() {
        let state = ValidationState(Cell::new(false));
        assert_eq!(
            state
                .checked(|| Err::<(), _>(raw_error(libc::EPERM)))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        let called = Cell::new(false);
        assert!(state
            .checked(|| {
                called.set(true);
                Ok(())
            })
            .is_err());
        assert!(!called.get());
    }

    #[test]
    fn registration_pending_poll_rejects_every_exit_or_error_notification() {
        assert!(validate_pending(0, 0).is_ok());
        for (count, events) in [
            (1, libc::POLLIN),
            (1, libc::POLLIN | libc::POLLHUP),
            (1, libc::POLLHUP),
            (1, libc::POLLERR),
            (1, libc::POLLNVAL),
            (0, libc::POLLIN),
            (1, 0),
            (2, 0),
        ] {
            assert!(validate_pending(count, events).is_err());
        }
    }
}
