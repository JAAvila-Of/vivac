use super::*;
use std::fmt;

const ENDPOINTS: &CStr = c"endpoints-v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProvisionCheckpoint {
    BeforeRename,
    BeforeParentSync,
}

#[derive(Debug)]
pub enum EndpointProvisionError {
    Unpublished(io::Error),
    PublishedDurabilityUnknown(io::Error),
}

impl fmt::Display for EndpointProvisionError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unpublished(error) => {
                write!(output, "endpoint directory was not published: {error}")
            }
            Self::PublishedDurabilityUnknown(error) => write!(
                output,
                "endpoint directory was published but durability is unknown: {error}"
            ),
        }
    }
}

impl std::error::Error for EndpointProvisionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unpublished(error) | Self::PublishedDurabilityUnknown(error) => Some(error),
        }
    }
}

impl CandidateRoot {
    pub fn with_endpoint_directory<T>(
        &self,
        operation: impl FnOnce(OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = with_directory(&self.directories[7].descriptor, self.ids, operation);
        self.validate()?;
        result
    }

    pub fn endpoint_directory(&self) -> io::Result<Option<OwnedFd>> {
        self.validate()?;
        let result = read_optional(&self.directories[7].descriptor, self.ids)?;
        self.validate()?;
        let named = read_optional(&self.directories[7].descriptor, self.ids)?;
        require(
            result.as_ref().map(|directory| &directory.identity)
                == named.as_ref().map(|directory| &directory.identity),
        )?;
        Ok(result.map(|directory| directory.descriptor))
    }

    pub fn provision_endpoint_directory(&self) -> Result<(), EndpointProvisionError> {
        if self
            .endpoint_directory()
            .map_err(EndpointProvisionError::Unpublished)?
            .is_some()
        {
            return Err(EndpointProvisionError::Unpublished(
                io::Error::from_raw_os_error(libc::EEXIST),
            ));
        }
        publish(
            &self.directories[7].descriptor,
            self.ids,
            || self.validate(),
            |_| Ok(()),
        )
    }
}

fn with_directory<T>(
    state: &OwnedFd,
    ids: (u32, u32),
    operation: impl FnOnce(OwnedFd) -> io::Result<T>,
) -> io::Result<T> {
    let retained =
        read_optional(state, ids)?.ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
    let result = operation(duplicate(retained.descriptor.as_raw_fd())?);
    require(Identity::directory(retained.descriptor.as_raw_fd(), 8, ids)? == retained.identity)?;
    let named = read_optional(state, ids)?.ok_or_else(invalid)?;
    require(named.identity == retained.identity)?;
    result
}

fn read_optional(state: &OwnedFd, ids: (u32, u32)) -> io::Result<Option<Directory>> {
    let descriptor = match open_at(state.as_raw_fd(), ENDPOINTS, true, true) {
        Ok(descriptor) => descriptor,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(None),
        Err(error) => return Err(error),
    };
    let identity = Identity::directory(descriptor.as_raw_fd(), 8, ids)?;
    let parent = Identity::directory(state.as_raw_fd(), 7, ids)?;
    require(identity.device == parent.device && identity.mount == parent.mount)?;
    Ok(Some(Directory {
        descriptor,
        identity,
    }))
}

fn sync(fd: i32) -> io::Result<()> {
    // SAFETY: fsync operates only on this retained descriptor.
    if unsafe { libc::fsync(fd) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn empty(directory: i32) -> io::Result<()> {
    let descriptor = open_at(directory, c".", true, true)?;
    use std::os::fd::IntoRawFd;
    let raw = descriptor.into_raw_fd();
    // SAFETY: fdopendir takes ownership of raw on success.
    let stream = unsafe { libc::fdopendir(raw) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: fdopendir failed and did not adopt raw.
        drop(unsafe { OwnedFd::from_raw_fd(raw) });
        return Err(error);
    }
    struct Stream(*mut libc::DIR);
    impl Drop for Stream {
        fn drop(&mut self) {
            // SAFETY: this guard exclusively owns the live directory stream.
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = Stream(stream);
    for _ in 0..3 {
        // SAFETY: this thread's errno is writable and the stream is live.
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(0) {
                Ok(())
            } else {
                Err(error)
            };
        }
        // SAFETY: readdir returned a live NUL-terminated filename.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        require(name == c"." || name == c"..")?;
    }
    Err(invalid())
}

fn stage_name() -> io::Result<CString> {
    let mut nonce = [0u8; 16];
    // SAFETY: getrandom writes only within the fixed nonce buffer.
    let count =
        unsafe { libc::getrandom(nonce.as_mut_ptr().cast(), nonce.len(), libc::GRND_NONBLOCK) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    require(count as usize == nonce.len())?;
    let mut name = String::from(".pending-endpoints-");
    for byte in nonce {
        use std::fmt::Write;
        write!(&mut name, "{byte:02x}").map_err(|_| invalid())?;
    }
    CString::new(name).map_err(|_| invalid())
}

fn validate_stage(state: i32, name: &CStr, stage: &Directory, ids: (u32, u32)) -> io::Result<()> {
    require(Identity::directory(stage.descriptor.as_raw_fd(), 8, ids)? == stage.identity)?;
    let named = open_at(state, name, true, true)?;
    require(Identity::directory(named.as_raw_fd(), 8, ids)? == stage.identity)?;
    empty(stage.descriptor.as_raw_fd())
}

fn publish(
    state: &OwnedFd,
    ids: (u32, u32),
    validate_root: impl Fn() -> io::Result<()>,
    mut checkpoint: impl FnMut(ProvisionCheckpoint) -> io::Result<()>,
) -> Result<(), EndpointProvisionError> {
    let mut published = false;
    let mut stage: Option<Directory> = None;
    let mut name = None;
    let result = (|| {
        validate_root()?;
        let parent = Identity::directory(state.as_raw_fd(), 7, ids)?;
        let component = stage_name()?;
        // SAFETY: the new component is relative to the retained state directory.
        if unsafe { libc::mkdirat(state.as_raw_fd(), component.as_ptr(), 0o700) } < 0 {
            return Err(io::Error::last_os_error());
        }
        name = Some(component);
        let descriptor = open_at(
            state.as_raw_fd(),
            name.as_deref().ok_or_else(invalid)?,
            true,
            true,
        )?;
        // SAFETY: this descriptor selects the directory just created by this attempt.
        if unsafe { libc::fchmod(descriptor.as_raw_fd(), 0o700) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let identity = Identity::directory(descriptor.as_raw_fd(), 8, ids)?;
        require(identity.device == parent.device && identity.mount == parent.mount)?;
        stage = Some(Directory {
            descriptor,
            identity,
        });
        let selected = stage.as_ref().ok_or_else(invalid)?;
        let name = name.as_deref().ok_or_else(invalid)?;
        validate_stage(state.as_raw_fd(), name, selected, ids)?;
        sync(selected.descriptor.as_raw_fd())?;
        validate_root()?;
        require(Identity::directory(state.as_raw_fd(), 7, ids)? == parent)?;
        checkpoint(ProvisionCheckpoint::BeforeRename)?;
        validate_stage(state.as_raw_fd(), name, selected, ids)?;
        // SAFETY: both terminated components are relative to this retained directory.
        if unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                state.as_raw_fd(),
                name.as_ptr(),
                state.as_raw_fd(),
                ENDPOINTS.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        published = true;
        let final_directory = read_optional(state, ids)?.ok_or_else(invalid)?;
        require(final_directory.identity == selected.identity)?;
        validate_root()?;
        require(Identity::directory(state.as_raw_fd(), 7, ids)? == parent)?;
        checkpoint(ProvisionCheckpoint::BeforeParentSync)?;
        sync(state.as_raw_fd())?;
        validate_root()?;
        require(read_optional(state, ids)?.ok_or_else(invalid)?.identity == selected.identity)?;
        Ok(())
    })();
    if let Err(error) = result {
        if !published {
            if let (Some(stage), Some(name)) = (&stage, &name) {
                if validate_stage(state.as_raw_fd(), name, stage, ids).is_ok() {
                    // SAFETY: the staging name was compared with the retained empty directory.
                    if unsafe {
                        libc::unlinkat(state.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                    } == 0
                    {
                        let _ = sync(state.as_raw_fd());
                    }
                }
            }
        }
        return Err(if published {
            EndpointProvisionError::PublishedDurabilityUnknown(error)
        } else {
            EndpointProvisionError::Unpublished(error)
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct TemporaryDirectory(std::path::PathBuf);

    impl TemporaryDirectory {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn fixture() -> (TemporaryDirectory, OwnedFd, (u32, u32)) {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "vivac-endpoint-directory-{}-{stamp}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        let directory = TemporaryDirectory(path);
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let fd = std::fs::File::open(directory.path()).unwrap().into();
        // SAFETY: both functions only query this process's credentials.
        let ids = unsafe { (libc::getuid(), libc::getgid()) };
        (directory, fd, ids)
    }

    #[test]
    fn endpoint_directory_publication_preserves_an_occupied_destination() {
        let (directory, fd, ids) = fixture();
        std::fs::create_dir(directory.path().join("endpoints-v1")).unwrap();
        std::fs::write(directory.path().join("endpoints-v1/foreign"), b"retained").unwrap();
        let error = publish(&fd, ids, || Ok(()), |_| Ok(())).unwrap_err();
        assert!(matches!(error, EndpointProvisionError::Unpublished(error)
            if error.raw_os_error() == Some(libc::EEXIST)));
        assert_eq!(
            std::fs::read(directory.path().join("endpoints-v1/foreign")).unwrap(),
            b"retained"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn endpoint_directory_failure_before_rename_removes_only_its_stage() {
        let (directory, fd, ids) = fixture();
        std::fs::write(directory.path().join("foreign"), b"retained").unwrap();
        let error = publish(
            &fd,
            ids,
            || Ok(()),
            |stage| {
                assert_eq!(stage, ProvisionCheckpoint::BeforeRename);
                Err(io::Error::from_raw_os_error(libc::ECANCELED))
            },
        )
        .unwrap_err();
        assert!(matches!(error, EndpointProvisionError::Unpublished(error)
            if error.raw_os_error() == Some(libc::ECANCELED)));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(directory.path().join("foreign")).unwrap(),
            b"retained"
        );
    }

    #[test]
    fn endpoint_directory_failure_after_rename_preserves_the_final() {
        let (directory, fd, ids) = fixture();
        let error = publish(
            &fd,
            ids,
            || Ok(()),
            |stage| {
                if stage == ProvisionCheckpoint::BeforeParentSync {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            EndpointProvisionError::PublishedDurabilityUnknown(error)
                if error.raw_os_error() == Some(libc::EIO)
        ));
        assert!(directory.path().join("endpoints-v1").is_dir());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn endpoint_location_returns_the_operation_result_with_the_retained_identity() {
        let (_directory, fd, ids) = fixture();
        publish(&fd, ids, || Ok(()), |_| Ok(())).unwrap();
        let original = read_optional(&fd, ids).unwrap().unwrap();
        let result = with_directory(&fd, ids, |duplicate| {
            assert_eq!(
                Identity::directory(duplicate.as_raw_fd(), 8, ids)?,
                original.identity
            );
            Ok(7)
        })
        .unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn endpoint_location_absence_does_not_run_the_operation_or_create_a_directory() {
        let (directory, fd, ids) = fixture();
        let error = with_directory(&fd, ids, |_| -> io::Result<()> {
            panic!("absent endpoint directory reached the operation")
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn endpoint_location_retains_the_original_and_refuses_a_named_replacement() {
        let (directory, fd, ids) = fixture();
        publish(&fd, ids, || Ok(()), |_| Ok(())).unwrap();
        let original = read_optional(&fd, ids).unwrap().unwrap();
        let path = directory.path().join("endpoints-v1");
        let previous = directory.path().join("previous");
        let error = with_directory(&fd, ids, |duplicate| {
            std::fs::rename(&path, &previous)?;
            std::fs::create_dir(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            assert_eq!(
                Identity::directory(duplicate.as_raw_fd(), 8, ids)?,
                original.identity
            );
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
        assert!(path.is_dir() && previous.is_dir());
    }

    #[test]
    fn endpoint_location_revalidates_policy_even_when_the_operation_refuses() {
        let (directory, fd, ids) = fixture();
        publish(&fd, ids, || Ok(()), |_| Ok(())).unwrap();
        let path = directory.path().join("endpoints-v1");
        let error = with_directory(&fd, ids, |_| -> io::Result<()> {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            Err(io::Error::from_raw_os_error(libc::ECANCELED))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().starts_with("directory-policy:"));
        assert!(path.is_dir());
    }
}
