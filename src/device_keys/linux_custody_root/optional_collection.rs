use super::*;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProvisionCheckpoint {
    BeforeRename,
    BeforeParentSync,
}

/// A directory under the state directory that may be absent and is published whole.
pub(crate) struct OptionalCollection {
    pub(crate) name: &'static CStr,
    pub(crate) stage_prefix: &'static str,
    #[allow(dead_code, reason = "The native probes never provision a collection.")]
    pub(crate) label: &'static str,
}

pub(crate) const ENDPOINTS: OptionalCollection = OptionalCollection {
    name: c"endpoints-v1",
    stage_prefix: ".pending-endpoints-",
    label: "endpoint",
};

pub(crate) const SESSIONS: OptionalCollection = OptionalCollection {
    name: c"sessions-v1",
    stage_prefix: ".pending-sessions-",
    label: "session",
};

#[derive(Debug)]
pub enum CollectionProvisionError {
    Unpublished(io::Error),
    PublishedDurabilityUnknown(io::Error),
}

impl CollectionProvisionError {
    /// The message for `collection`.
    #[allow(dead_code, reason = "The native probes never provision a collection.")]
    pub(crate) fn message(&self, collection: &OptionalCollection) -> String {
        self.describe(collection.label)
    }

    fn describe(&self, label: &str) -> String {
        match self {
            Self::Unpublished(error) => {
                format!("{label} directory was not published: {error}")
            }
            Self::PublishedDurabilityUnknown(error) => {
                format!("{label} directory was published but durability is unknown: {error}")
            }
        }
    }
}

impl fmt::Display for CollectionProvisionError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&self.describe("collection"))
    }
}

impl std::error::Error for CollectionProvisionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unpublished(error) | Self::PublishedDurabilityUnknown(error) => Some(error),
        }
    }
}

impl CandidateRoot {
    pub(super) fn with_optional_collection<T>(
        &self,
        collection: &OptionalCollection,
        operation: impl FnOnce(OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = with_directory(
            collection,
            &self.directories[7].descriptor,
            self.ids,
            operation,
        );
        self.validate()?;
        result
    }

    pub(super) fn optional_collection(
        &self,
        collection: &OptionalCollection,
    ) -> io::Result<Option<OwnedFd>> {
        self.validate()?;
        let result = read_optional(collection, &self.directories[7].descriptor, self.ids)?;
        self.validate()?;
        let named = read_optional(collection, &self.directories[7].descriptor, self.ids)?;
        require(
            result.as_ref().map(|directory| &directory.identity)
                == named.as_ref().map(|directory| &directory.identity),
        )?;
        Ok(result.map(|directory| directory.descriptor))
    }

    pub(super) fn provision_optional_collection(
        &self,
        collection: &OptionalCollection,
    ) -> Result<(), CollectionProvisionError> {
        if self
            .optional_collection(collection)
            .map_err(CollectionProvisionError::Unpublished)?
            .is_some()
        {
            return Err(CollectionProvisionError::Unpublished(
                io::Error::from_raw_os_error(libc::EEXIST),
            ));
        }
        publish(
            collection,
            &self.directories[7].descriptor,
            self.ids,
            || self.validate(),
            |_| Ok(()),
        )
    }
}

pub(super) fn with_directory<T>(
    collection: &OptionalCollection,
    state: &OwnedFd,
    ids: (u32, u32),
    operation: impl FnOnce(OwnedFd) -> io::Result<T>,
) -> io::Result<T> {
    let retained = read_optional(collection, state, ids)?
        .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
    let result = operation(duplicate(retained.descriptor.as_raw_fd())?);
    require(Identity::directory(retained.descriptor.as_raw_fd(), 8, ids)? == retained.identity)?;
    let named = read_optional(collection, state, ids)?.ok_or_else(invalid)?;
    require(named.identity == retained.identity)?;
    result
}

pub(super) fn read_optional(
    collection: &OptionalCollection,
    state: &OwnedFd,
    ids: (u32, u32),
) -> io::Result<Option<Directory>> {
    let descriptor = match open_at(state.as_raw_fd(), collection.name, true, true) {
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

fn stage_name(collection: &OptionalCollection) -> io::Result<CString> {
    let mut nonce = [0u8; 16];
    // SAFETY: getrandom writes only within the fixed nonce buffer.
    let count =
        unsafe { libc::getrandom(nonce.as_mut_ptr().cast(), nonce.len(), libc::GRND_NONBLOCK) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    require(count as usize == nonce.len())?;
    let mut name = String::from(collection.stage_prefix);
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

pub(super) fn publish(
    collection: &OptionalCollection,
    state: &OwnedFd,
    ids: (u32, u32),
    validate_root: impl Fn() -> io::Result<()>,
    mut checkpoint: impl FnMut(ProvisionCheckpoint) -> io::Result<()>,
) -> Result<(), CollectionProvisionError> {
    let mut published = false;
    let mut stage: Option<Directory> = None;
    let mut name = None;
    let result = (|| {
        validate_root()?;
        let parent = Identity::directory(state.as_raw_fd(), 7, ids)?;
        let component = stage_name(collection)?;
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
                collection.name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        published = true;
        let final_directory = read_optional(collection, state, ids)?.ok_or_else(invalid)?;
        require(final_directory.identity == selected.identity)?;
        validate_root()?;
        require(Identity::directory(state.as_raw_fd(), 7, ids)? == parent)?;
        checkpoint(ProvisionCheckpoint::BeforeParentSync)?;
        sync(state.as_raw_fd())?;
        validate_root()?;
        require(
            read_optional(collection, state, ids)?
                .ok_or_else(invalid)?
                .identity
                == selected.identity,
        )?;
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
            CollectionProvisionError::PublishedDurabilityUnknown(error)
        } else {
            CollectionProvisionError::Unpublished(error)
        });
    }
    Ok(())
}
