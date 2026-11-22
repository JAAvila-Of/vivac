use crate::linux_ipc::require_procfs;
use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SOCKET_NAME: &CStr = c"endpoint.sock";

fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn before(deadline: Instant) -> io::Result<()> {
    if Instant::now() < deadline {
        Ok(())
    } else {
        Err(io::ErrorKind::TimedOut.into())
    }
}

fn stable_ids() -> io::Result<(u32, u32)> {
    // SAFETY: these calls query only the current process credentials.
    let (uid, gid, euid, egid) = unsafe {
        (
            libc::getuid(),
            libc::getgid(),
            libc::geteuid(),
            libc::getegid(),
        )
    };
    require(uid == euid && gid == egid)?;
    Ok((uid, gid))
}

fn cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: F_GETFD only queries the retained descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    require(fd >= 3 && flags & libc::FD_CLOEXEC != 0)
}

fn status(fd: RawFd) -> io::Result<libc::stat> {
    let mut value = MaybeUninit::uninit();
    // SAFETY: fstat initializes one complete stat object on success.
    if unsafe { libc::fstat(fd, value.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstat initialized value.
    Ok(unsafe { value.assume_init() })
}

fn directory_identity(directory: &OwnedFd) -> io::Result<(u64, u64)> {
    cloexec(directory.as_raw_fd())?;
    let metadata = status(directory.as_raw_fd())?;
    let (uid, gid) = stable_ids()?;
    require(
        metadata.st_mode & libc::S_IFMT == libc::S_IFDIR
            && metadata.st_mode & 0o7777 == 0o700
            && metadata.st_uid == uid
            && metadata.st_gid == gid,
    )?;
    Ok((metadata.st_dev, metadata.st_ino))
}

pub(super) struct RuntimeRoot {
    path: PathBuf,
    directory: OwnedFd,
    identity: (u64, u64),
}

impl RuntimeRoot {
    pub(super) fn create() -> io::Result<Self> {
        let mut template = b"/tmp/vivac-endpoint-XXXXXX\0".to_vec();
        // SAFETY: mkdtemp writes within this writable NUL-terminated template.
        let created = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
        if created.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful mkdtemp left a complete string in the template.
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(unsafe {
            CStr::from_ptr(created).to_bytes()
        }));
        let directory: OwnedFd = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?
            .into();
        let identity = directory_identity(&directory)?;
        let root = Self {
            path,
            directory,
            identity,
        };
        root.validate()?;
        Ok(root)
    }

    fn validate(&self) -> io::Result<()> {
        require(directory_identity(&self.directory)? == self.identity)?;
        let named = std::fs::symlink_metadata(&self.path)?;
        let (uid, gid) = stable_ids()?;
        require(
            named.is_dir()
                && named.mode() & 0o7777 == 0o700
                && named.uid() == uid
                && named.gid() == gid
                && (named.dev(), named.ino()) == self.identity,
        )
    }

    pub(super) fn directory(&self) -> &OwnedFd {
        &self.directory
    }

    pub(super) fn remove_empty(self) -> io::Result<()> {
        self.validate()?;
        std::fs::remove_dir(&self.path)
    }
}

pub(super) struct EndpointRuntime {
    directory: OwnedFd,
    identity: (u64, u64),
    name: CString,
    path: PathBuf,
}

pub(super) struct EndpointSocket {
    directory_identity: (u64, u64),
    identity: (u64, u64),
    descriptor: OwnedFd,
}

pub(super) struct EndpointListener {
    socket: OwnedFd,
    leaf: EndpointSocket,
}

impl AsRawFd for EndpointListener {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

impl EndpointRuntime {
    pub(super) fn from_directory(directory: OwnedFd) -> io::Result<Self> {
        Self::from_named_directory(directory, SOCKET_NAME)
    }

    pub(super) fn from_named_directory(directory: OwnedFd, name: &CStr) -> io::Result<Self> {
        require(
            !name.to_bytes().is_empty()
                && name != c"."
                && name != c".."
                && !name.to_bytes().contains(&b'/'),
        )?;
        let identity = directory_identity(&directory)?;
        let mut path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        path.push(std::ffi::OsStr::from_bytes(name.to_bytes()));
        let runtime = Self {
            directory,
            identity,
            name: name.to_owned(),
            path,
        };
        runtime.validate()?;
        Ok(runtime)
    }

    fn validate(&self) -> io::Result<()> {
        require(directory_identity(&self.directory)? == self.identity)?;
        let proc = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/proc/self/fd")?;
        require_procfs(&proc)?;
        // The procfs magic link is deliberately followed to the retained directory.
        let resolved = File::open(self.path.parent().ok_or_else(invalid)?)?;
        let metadata = resolved.metadata()?;
        require(metadata.is_dir() && (metadata.dev(), metadata.ino()) == self.identity)
    }

    fn address(&self) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
        self.validate()?;
        // SAFETY: zero initializes the complete sockaddr and its final NUL.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        let bytes = self.path.as_os_str().as_bytes();
        require(bytes.len() < address.sun_path.len() && !bytes.contains(&0))?;
        address.sun_family = libc::AF_UNIX as _;
        for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as _;
        }
        let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as _;
        Ok((address, length))
    }

    fn named_socket(&self) -> io::Result<EndpointSocket> {
        self.validate()?;
        #[repr(C)]
        struct OpenHow {
            flags: u64,
            mode: u64,
            resolve: u64,
        }
        let how = OpenHow {
            flags: (libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC) as u64,
            mode: 0,
            resolve: libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_XDEV,
        };
        // SAFETY: name is terminated and how has the kernel's fixed open_how layout.
        let raw = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                &how,
                size_of::<OpenHow>(),
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat2 returned a fresh exclusively owned descriptor.
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        cloexec(descriptor.as_raw_fd())?;
        let identity = self.socket_identity(&descriptor)?;
        Ok(EndpointSocket {
            directory_identity: self.identity,
            identity,
            descriptor,
        })
    }

    fn socket_identity(&self, descriptor: &OwnedFd) -> io::Result<(u64, u64)> {
        let metadata = status(descriptor.as_raw_fd())?;
        let (uid, gid) = stable_ids()?;
        require(
            metadata.st_mode & libc::S_IFMT == libc::S_IFSOCK
                && metadata.st_dev == self.identity.0
                && metadata.st_uid == uid
                && metadata.st_gid == gid
                && metadata.st_nlink == 1,
        )?;
        Ok((metadata.st_dev, metadata.st_ino))
    }

    fn validate_socket(&self, expected: &EndpointSocket) -> io::Result<()> {
        self.validate()?;
        require(expected.directory_identity == self.identity)?;
        require(self.socket_identity(&expected.descriptor)? == expected.identity)?;
        require(self.named_socket()?.identity == expected.identity)
    }

    pub(super) fn bind_listener(&self, deadline: Instant) -> io::Result<EndpointListener> {
        before(deadline)?;
        let listener = socket()?;
        let (address, length) = self.address()?;
        // SAFETY: the initialized address remains live for its exact length.
        if unsafe {
            libc::bind(
                listener.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                length,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let identity = self.named_socket()?;
        // SAFETY: listen acts only on this owned nonblocking Unix socket.
        if unsafe { libc::listen(listener.as_raw_fd(), 1) } < 0 {
            return Err(io::Error::last_os_error());
        }
        before(deadline)?;
        self.validate_socket(&identity)?;
        Ok(EndpointListener {
            socket: listener,
            leaf: identity,
        })
    }

    pub(super) fn accept(
        &self,
        listener: &EndpointListener,
        deadline: Instant,
    ) -> io::Result<Option<OwnedFd>> {
        before(deadline)?;
        self.validate_socket(&listener.leaf)?;
        // SAFETY: null output pointers omit the peer address; returned fd is owned.
        let raw = unsafe {
            libc::accept4(
                listener.socket.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            )
        };
        if raw < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(error)
            };
        }
        // SAFETY: successful accept4 returned a fresh exclusively owned socket.
        let connected = unsafe { OwnedFd::from_raw_fd(raw) };
        passcred(&connected)?;
        before(deadline)?;
        self.validate_socket(&listener.leaf)?;
        Ok(Some(connected))
    }

    pub(super) fn listen_and_accept(
        &self,
        deadline: Instant,
    ) -> io::Result<(OwnedFd, EndpointSocket)> {
        let listener = self.bind_listener(deadline)?;
        loop {
            before(deadline)?;
            self.validate()?;
            wait_readable(&listener.socket, deadline)?;
            match self.accept(&listener, deadline) {
                Ok(Some(connected)) => return Ok((connected, listener.leaf)),
                Ok(None) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    pub(super) fn connect_once(&self, deadline: Instant) -> io::Result<OwnedFd> {
        self.connect_once_with(deadline, || Ok(()))
    }

    pub(super) fn with_connection<T>(
        &self,
        deadline: Instant,
        operation: impl FnOnce(OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.connect_with(deadline, || Ok(()), operation)
    }

    fn connect_once_with(
        &self,
        deadline: Instant,
        after_pin: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<OwnedFd> {
        self.connect_with(deadline, after_pin, Ok)
    }

    fn connect_with<T>(
        &self,
        deadline: Instant,
        after_pin: impl FnOnce() -> io::Result<()>,
        operation: impl FnOnce(OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        before(deadline)?;
        let leaf = self.named_socket()?;
        after_pin()?;
        self.validate_socket(&leaf)?;
        let connected = socket()?;
        let (address, length) = self.address()?;
        // SAFETY: connect reads the initialized sockaddr for its exact length.
        if unsafe {
            libc::connect(
                connected.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                length,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        before(deadline)?;
        self.validate_socket(&leaf)?;
        let result = operation(connected);
        before(deadline)?;
        self.validate_socket(&leaf)?;
        result
    }

    pub(super) fn connect(&self, deadline: Instant) -> io::Result<OwnedFd> {
        loop {
            before(deadline)?;
            let error = match self.connect_once(deadline) {
                Ok(connected) => return Ok(connected),
                Err(error) => error,
            };
            if !matches!(
                error.raw_os_error(),
                Some(libc::ENOENT | libc::ECONNREFUSED | libc::EAGAIN)
            ) {
                return Err(error);
            }
            std::thread::sleep(
                Duration::from_millis(2).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    pub(super) fn remove_socket(&self, expected: EndpointSocket) -> io::Result<()> {
        self.validate_socket(&expected)?;
        // SAFETY: unlinkat targets only the fixed name in the retained directory.
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        self.validate()
    }

    pub(super) fn check_listener(
        &self,
        listener: &EndpointListener,
        current_directory: &OwnedFd,
    ) -> io::Result<()> {
        require(directory_identity(current_directory)? == self.identity)?;
        self.validate_socket(&listener.leaf)
    }

    pub(super) fn close_listener(&self, listener: EndpointListener) -> io::Result<()> {
        let EndpointListener { socket, leaf } = listener;
        drop(socket);
        self.remove_socket(leaf)
    }
}

fn socket() -> io::Result<OwnedFd> {
    // SAFETY: socket returns a new descriptor and takes no memory arguments.
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful socket returned an exclusively owned descriptor.
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };
    cloexec(socket.as_raw_fd())?;
    passcred(&socket)?;
    Ok(socket)
}

fn passcred(socket: &OwnedFd) -> io::Result<()> {
    let enabled: libc::c_int = 1;
    // SAFETY: setsockopt reads one initialized integer value.
    if unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            (&enabled as *const libc::c_int).cast(),
            size_of::<libc::c_int>() as _,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn wait_readable(socket: &OwnedFd, deadline: Instant) -> io::Result<()> {
    loop {
        before(deadline)?;
        let mut item = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        // SAFETY: poll writes only one complete initialized pollfd.
        let count = unsafe { libc::poll(&mut item, 1, timeout) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        before(deadline)?;
        if count == 1 {
            require(item.revents == libc::POLLIN)?;
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_runtime_rejects_an_ordinary_file() {
        let file = File::open("/dev/null").unwrap();
        assert!(EndpointRuntime::from_directory(file.into()).is_err());
    }

    #[test]
    fn removing_runtime_preserves_unknown_children() {
        let root = RuntimeRoot::create().unwrap();
        let path = root.path.clone();
        std::fs::write(path.join("unknown"), b"fixture").unwrap();
        assert!(root.remove_empty().is_err());
        assert_eq!(std::fs::read(path.join("unknown")).unwrap(), b"fixture");
        std::fs::remove_file(path.join("unknown")).unwrap();
        std::fs::remove_dir(path).unwrap();
    }

    #[test]
    fn removing_endpoint_preserves_a_replacement_socket() {
        let root = RuntimeRoot::create().unwrap();
        let runtime =
            EndpointRuntime::from_directory(root.directory().try_clone().unwrap()).unwrap();
        let (address, length) = runtime.address().unwrap();
        let bind = |socket: &OwnedFd| {
            // SAFETY: this test owns the socket and initialized fixed address.
            assert_eq!(
                unsafe {
                    libc::bind(
                        socket.as_raw_fd(),
                        (&address as *const libc::sockaddr_un).cast(),
                        length,
                    )
                },
                0
            );
        };
        let original = socket().unwrap();
        bind(&original);
        let selected = runtime.named_socket().unwrap();
        std::fs::remove_file(root.path.join("endpoint.sock")).unwrap();
        let replacement = socket().unwrap();
        bind(&replacement);
        assert!(runtime.remove_socket(selected).is_err());
        let retained = runtime.named_socket().unwrap();
        runtime.remove_socket(retained).unwrap();
        drop((original, replacement, runtime));
        root.remove_empty().unwrap();
    }

    #[test]
    fn retained_listener_accepts_separate_connections_without_rebinding() {
        let root = RuntimeRoot::create().unwrap();
        let runtime =
            EndpointRuntime::from_directory(root.directory().try_clone().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let listener = runtime.bind_listener(deadline).unwrap();
        assert!(runtime.accept(&listener, deadline).unwrap().is_none());
        for _ in 0..2 {
            let client = runtime.connect_once(deadline).unwrap();
            let connected = runtime.accept(&listener, deadline).unwrap().unwrap();
            drop((client, connected));
        }
        runtime.remove_socket(listener.leaf).unwrap();
        drop((listener.socket, runtime));
        root.remove_empty().unwrap();
    }

    #[test]
    fn connecting_endpoint_refuses_a_symlink_before_contacting_its_target() {
        let root = RuntimeRoot::create().unwrap();
        let runtime =
            EndpointRuntime::from_directory(root.directory().try_clone().unwrap()).unwrap();
        let decoy = EndpointRuntime::from_named_directory(
            root.directory().try_clone().unwrap(),
            c"decoy.sock",
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let listener = decoy.bind_listener(deadline).unwrap();
        std::os::unix::fs::symlink("decoy.sock", root.path.join("endpoint.sock")).unwrap();
        assert_eq!(
            runtime.connect_once(deadline).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(decoy.accept(&listener, deadline).unwrap().is_none());
        std::fs::remove_file(root.path.join("endpoint.sock")).unwrap();
        decoy.remove_socket(listener.leaf).unwrap();
        drop((listener.socket, decoy, runtime));
        root.remove_empty().unwrap();
    }

    #[test]
    fn connecting_endpoint_refuses_a_leaf_replaced_after_it_was_pinned() {
        let root = RuntimeRoot::create().unwrap();
        let runtime =
            EndpointRuntime::from_directory(root.directory().try_clone().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let original = runtime.bind_listener(deadline).unwrap();
        let mut replacement = None;
        let result = runtime.connect_once_with(deadline, || {
            std::fs::remove_file(root.path.join("endpoint.sock"))?;
            replacement = Some(runtime.bind_listener(deadline)?);
            Ok(())
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        let replacement = replacement.unwrap();
        assert!(runtime.accept(&replacement, deadline).unwrap().is_none());
        assert!(runtime.remove_socket(original.leaf).is_err());
        runtime.remove_socket(replacement.leaf).unwrap();
        drop((original.socket, replacement.socket, runtime));
        root.remove_empty().unwrap();
    }

    #[test]
    fn endpoint_operation_returns_its_result_after_validating_the_retained_leaf() {
        let root = RuntimeRoot::create().unwrap();
        let runtime =
            EndpointRuntime::from_directory(root.directory().try_clone().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let listener = runtime.bind_listener(deadline).unwrap();
        let result = runtime.with_connection(deadline, |client| {
            let connected = runtime.accept(&listener, deadline)?.ok_or_else(invalid)?;
            drop((client, connected));
            Ok(7)
        });
        assert_eq!(result.unwrap(), 7);
        runtime.remove_socket(listener.leaf).unwrap();
        drop((listener.socket, runtime));
        root.remove_empty().unwrap();
    }

    #[test]
    fn endpoint_operation_refuses_success_after_its_retained_leaf_is_replaced() {
        let root = RuntimeRoot::create().unwrap();
        let runtime =
            EndpointRuntime::from_directory(root.directory().try_clone().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let original = runtime.bind_listener(deadline).unwrap();
        let mut replacement = None;
        let result = runtime.with_connection(deadline, |client| {
            let connected = runtime.accept(&original, deadline)?.ok_or_else(invalid)?;
            std::fs::remove_file(root.path.join("endpoint.sock"))?;
            replacement = Some(runtime.bind_listener(deadline)?);
            drop((client, connected));
            Ok(7)
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        let replacement = replacement.unwrap();
        assert!(runtime.remove_socket(original.leaf).is_err());
        runtime.remove_socket(replacement.leaf).unwrap();
        drop((original.socket, replacement.socket, runtime));
        root.remove_empty().unwrap();
    }
}
