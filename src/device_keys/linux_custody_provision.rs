use super::linux_custody_anchor::{namespace_root, Binding, MAX_BINDING};
use std::ffi::{CStr, CString};
use std::fmt;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvisionStage {
    BeforeScaffoldSync(u8),
    ScaffoldSynced,
    StageCreated,
    BeforeOwnershipHandoff,
    OwnershipHandedOff,
    BeforeBindingSync,
    BeforePackagesSync,
    BeforeAdmissionsSync,
    BeforeOpeningsSync,
    BeforeEndpointsSync,
    BeforeSessionsSync,
    BeforeStateSync,
    BeforeStageSync,
    BeforeRename,
    Renamed,
    BeforeParentSync,
    ParentSynced,
}

#[derive(Debug)]
pub enum ProvisionError {
    Unpublished(io::Error),
    PublishedDurabilityUnknown(io::Error),
}

impl fmt::Display for ProvisionError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unpublished(error) => write!(output, "installation was not published: {error}"),
            Self::PublishedDurabilityUnknown(error) => write!(
                output,
                "installation was published but durability is unknown: {error}"
            ),
        }
    }
}

impl std::error::Error for ProvisionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unpublished(error) | Self::PublishedDurabilityUnknown(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub struct Publication {
    pub binding: Binding,
}

/// Administrative candidate restricted to a supplied synthetic filesystem.
pub struct Provisioner {
    directories: Vec<Directory>,
}

fn descriptor(fd: i32) -> io::Result<()> {
    require(fd >= 3)?;
    // SAFETY: fcntl only queries this live descriptor.
    let (flags, status) = unsafe {
        (
            libc::fcntl(fd, libc::F_GETFD),
            libc::fcntl(fd, libc::F_GETFL),
        )
    };
    if flags < 0 || status < 0 {
        return Err(io::Error::last_os_error());
    }
    require(flags & libc::FD_CLOEXEC != 0 && status & libc::O_PATH == 0)
}

fn duplicate(fd: i32) -> io::Result<OwnedFd> {
    // SAFETY: this atomically creates a fresh CLOEXEC descriptor at least 3.
    let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fcntl returned one fresh owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn filesystem(fd: i32) -> io::Result<()> {
    let mut value = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs initializes value on success.
    if unsafe { libc::fstatfs(fd, value.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatfs succeeded.
    require(unsafe { value.assume_init() }.f_type == 0xEF53)
}

fn acl(fd: i32, directory: bool) -> io::Result<()> {
    let names: &[&CStr] = if directory {
        &[c"system.posix_acl_access", c"system.posix_acl_default"]
    } else {
        &[c"system.posix_acl_access"]
    };
    for name in names {
        // SAFETY: the terminated fixed name and empty buffer query attribute size.
        if unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0) } >= 0 {
            return Err(invalid());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENODATA) {
            return Err(error);
        }
    }
    Ok(())
}

fn stat(fd: i32) -> io::Result<libc::statx> {
    at("descriptor", descriptor(fd))?;
    let mut value = std::mem::MaybeUninit::<libc::statx>::uninit();
    let mask = libc::STATX_BASIC_STATS | libc::STATX_MNT_ID;
    // SAFETY: AT_EMPTY_PATH selects this live descriptor; statx initializes value.
    if unsafe {
        libc::syscall(
            libc::SYS_statx,
            fd,
            c"".as_ptr(),
            libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
            mask,
            value.as_mut_ptr(),
        )
    } < 0
    {
        return at("statx-syscall", Err(io::Error::last_os_error()));
    }
    // SAFETY: statx succeeded.
    let value = unsafe { value.assume_init() };
    at("statx-mask", require(value.stx_mask & mask == mask))?;
    Ok(value)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Identity {
    device: (u32, u32),
    inode: u64,
    mount: u64,
    uid: u32,
    gid: u32,
    mode: u16,
}

impl Identity {
    fn from_stat(value: &libc::statx) -> Self {
        Self {
            device: (value.stx_dev_major, value.stx_dev_minor),
            inode: value.stx_ino,
            mount: value.stx_mnt_id,
            uid: value.stx_uid,
            gid: value.stx_gid,
            mode: value.stx_mode,
        }
    }

    fn directory(fd: i32, index: usize, ids: (u32, u32)) -> io::Result<Self> {
        let value = stat(fd)?;
        at("directory-acl", acl(fd, true))?;
        if index >= 4 {
            at("filesystem", filesystem(fd))?;
        }
        let (owner, mode) = if index < 4 {
            ((0, 0), None)
        } else if index < 7 {
            ((0, 0), Some(0o755))
        } else {
            (ids, Some(0o700))
        };
        let permissions = u32::from(value.stx_mode) & 0o7777;
        at(
            "directory-policy",
            require(
                u32::from(value.stx_mode) & libc::S_IFMT == libc::S_IFDIR
                    && (value.stx_uid, value.stx_gid) == owner
                    && mode.map_or(permissions & 0o022 == 0, |expected| permissions == expected),
            ),
        )?;
        Ok(Self::from_stat(&value))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BindingIdentity {
    object: Identity,
    size: u64,
    links: u32,
    changed: (i64, u32),
    modified: (i64, u32),
}

impl BindingIdentity {
    fn capture(fd: i32) -> io::Result<Self> {
        let value = stat(fd)?;
        at("filesystem", filesystem(fd))?;
        at("binding-acl", acl(fd, false))?;
        at(
            "binding-policy",
            require(
                u32::from(value.stx_mode) & libc::S_IFMT == libc::S_IFREG
                    && u32::from(value.stx_mode) & 0o7777 == 0o644
                    && value.stx_uid == 0
                    && value.stx_gid == 0
                    && value.stx_nlink == 1
                    && (1..=MAX_BINDING as u64).contains(&value.stx_size),
            ),
        )?;
        Ok(Self {
            object: Identity::from_stat(&value),
            size: value.stx_size,
            links: value.stx_nlink,
            changed: (value.stx_ctime.tv_sec, value.stx_ctime.tv_nsec),
            modified: (value.stx_mtime.tv_sec, value.stx_mtime.tv_nsec),
        })
    }
}

fn open_at(parent: i32, name: &CStr, directory: bool, no_mounts: bool) -> io::Result<OwnedFd> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: (libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | libc::O_NOFOLLOW
            | if directory { libc::O_DIRECTORY } else { 0 }) as u64,
        mode: 0,
        resolve: libc::RESOLVE_BENEATH
            | libc::RESOLVE_NO_SYMLINKS
            | if no_mounts { libc::RESOLVE_NO_XDEV } else { 0 },
    };
    // SAFETY: name is terminated and how has the kernel's fixed open_how layout.
    let raw = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            parent,
            name.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat2 returned a fresh owned descriptor.
    let opened = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    descriptor(opened.as_raw_fd())?;
    Ok(opened)
}

fn read_bytes(fd: i32, expected: u64) -> io::Result<Vec<u8>> {
    require((1..=MAX_BINDING as u64).contains(&expected))?;
    let mut buffer = [0; MAX_BINDING + 1];
    let mut used = 0;
    while used < buffer.len() {
        // SAFETY: the remaining buffer is writable; pread preserves shared offsets.
        let count = unsafe {
            libc::pread(
                fd,
                buffer[used..].as_mut_ptr().cast(),
                buffer.len() - used,
                used as libc::off_t,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            break;
        }
        used += count as usize;
    }
    require(used as u64 == expected)?;
    Ok(buffer[..used].to_vec())
}

struct Directory {
    descriptor: OwnedFd,
    identity: Identity,
}

const COMPONENTS: [&CStr; 5] = [c"var", c"lib", c"vivac", c"custody-v1", c"users"];

fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(libc::EINVAL))
    }
}

fn invalid() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

fn at<T>(stage: &'static str, result: io::Result<T>) -> io::Result<T> {
    result.map_err(|error| {
        let errno = error
            .raw_os_error()
            .or_else(|| {
                error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<ContextError>())
                    .map(|source| source.errno)
            })
            .unwrap_or(libc::EIO);
        io::Error::new(error.kind(), ContextError { stage, errno })
    })
}

#[derive(Debug)]
struct ContextError {
    stage: &'static str,
    errno: i32,
}

impl fmt::Display for ContextError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(output, "{}: errno {}", self.stage, self.errno)
    }
}

impl std::error::Error for ContextError {}

fn admin() -> io::Result<()> {
    // SAFETY: the identity queries take no arguments and do not mutate state.
    require(unsafe {
        libc::getuid() == 0 && libc::geteuid() == 0 && libc::getgid() == 0 && libc::getegid() == 0
    })?;
    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }
    #[derive(Clone, Copy, Default)]
    #[repr(C)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut header = Header {
        version: 0x20080522,
        pid: 0,
    };
    let mut data = [Data::default(); 2];
    // SAFETY: capget v3 writes exactly two initialized data structures.
    if unsafe { libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    require(data[0].effective & 5 == 5 && data[0].permitted & 5 == 5)
}

fn chmod(fd: i32, mode: u32) -> io::Result<()> {
    // SAFETY: fchmod acts only on this live retained descriptor.
    if unsafe { libc::fchmod(fd, mode) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn sync(fd: i32) -> io::Result<()> {
    // SAFETY: fsync acts only on this live retained descriptor.
    if unsafe { libc::fsync(fd) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn mkdir(parent: i32, name: &CStr, mode: u32) -> io::Result<()> {
    // SAFETY: parent is retained and name is a terminated single component.
    if unsafe { libc::mkdirat(parent, name.as_ptr(), mode) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn random() -> io::Result<[u8; 16]> {
    let mut bytes = [0; 16];
    // SAFETY: the fixed-size buffer is writable for the requested byte count.
    let count =
        unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), libc::GRND_NONBLOCK) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    require(count == 16)?;
    Ok(bytes)
}

fn installation_id(bytes: [u8; 16]) -> String {
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for byte in encoded.iter_mut().rev() {
        *byte = b"0123456789abcdefghjkmnpqrstvwxyz"[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec()).expect("fixed identifier alphabet")
}

fn selectors(uid: u32, gid: u32) -> io::Result<()> {
    require(uid != 0 && gid != 0 && uid != u32::MAX && gid != u32::MAX)
}

fn classify(published: bool, error: io::Error) -> ProvisionError {
    if published {
        ProvisionError::PublishedDurabilityUnknown(error)
    } else {
        ProvisionError::Unpublished(error)
    }
}

fn capture(fd: OwnedFd, index: usize, ids: (u32, u32)) -> io::Result<Directory> {
    let identity = Identity::directory(fd.as_raw_fd(), index, ids)?;
    Ok(Directory {
        descriptor: fd,
        identity,
    })
}

fn directory_fd(directory: &Directory) -> i32 {
    directory.descriptor.as_raw_fd()
}

fn validate_directory(directory: &Directory, index: usize, ids: (u32, u32)) -> io::Result<()> {
    require(Identity::directory(directory_fd(directory), index, ids)? == directory.identity)
}

fn same_filesystem(left: &Identity, right: &Identity) -> io::Result<()> {
    require(left.device == right.device && left.mount == right.mount)
}

fn scaffold_validate(directories: &[Directory]) -> io::Result<()> {
    admin()?;
    for (index, directory) in directories.iter().enumerate() {
        validate_directory(directory, index, (0, 0))?;
        if index > 0 {
            let named = open_at(
                directory_fd(&directories[index - 1]),
                COMPONENTS[index - 1],
                true,
                index > 4,
            )?;
            require(Identity::directory(named.as_raw_fd(), index, (0, 0))? == directory.identity)?;
        }
        if index > 4 {
            same_filesystem(&directories[4].identity, &directory.identity)?;
        }
    }
    Ok(())
}

fn shape(fd: i32, expected: &[&[u8]]) -> io::Result<()> {
    let scan = open_at(fd, c".", true, true)?;
    let mut buffer = [0u8; 4096];
    // SAFETY: getdents64 writes at most the supplied fixed buffer length.
    let count = unsafe {
        libc::syscall(
            libc::SYS_getdents64,
            scan.as_raw_fd(),
            buffer.as_mut_ptr(),
            buffer.len(),
        )
    };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    let count = count as usize;
    require(count <= buffer.len())?;
    let mut offset = 0;
    let mut names = Vec::new();
    while offset < count {
        require(count - offset >= 20)?;
        let length = u16::from_ne_bytes([buffer[offset + 16], buffer[offset + 17]]) as usize;
        require(length >= 20 && length <= count - offset)?;
        let bytes = &buffer[offset + 19..offset + length];
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(invalid)?;
        let name = &bytes[..end];
        if name != b"." && name != b".." {
            require(
                names.len() < expected.len() && expected.contains(&name) && !names.contains(&name),
            )?;
            names.push(name);
        }
        offset += length;
    }
    require(names.len() == expected.len())?;
    let mut tail = [0u8; 4096];
    // SAFETY: this second bounded read must establish end-of-directory.
    let count = unsafe {
        libc::syscall(
            libc::SYS_getdents64,
            scan.as_raw_fd(),
            tail.as_mut_ptr(),
            tail.len(),
        )
    };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    require(count == 0)
}

struct Slot {
    directories: Vec<Directory>,
    binding_file: OwnedFd,
    binding_identity: BindingIdentity,
    bytes: Vec<u8>,
    ids: (u32, u32),
}

impl Slot {
    fn validate(&self, users: i32, name: &CStr) -> io::Result<()> {
        let stage = directory_fd(&self.directories[0]);
        for (index, directory) in self.directories.iter().enumerate() {
            validate_directory(directory, index + 6, self.ids)?;
            same_filesystem(&self.directories[0].identity, &directory.identity)?;
            let (parent, component) = match index {
                0 => (users, name),
                1 => (stage, c"state"),
                2 => (directory_fd(&self.directories[1]), c"packages"),
                3 => (directory_fd(&self.directories[1]), c"admissions"),
                4 => (directory_fd(&self.directories[1]), c"openings-v1"),
                5 => (directory_fd(&self.directories[1]), c"endpoints-v1"),
                _ => (directory_fd(&self.directories[1]), c"sessions-v1"),
            };
            let named = open_at(parent, component, true, true)?;
            require(
                Identity::directory(named.as_raw_fd(), index + 6, self.ids)? == directory.identity,
            )?;
        }
        shape(stage, &[b"binding.json", b"state"])?;
        shape(
            directory_fd(&self.directories[1]),
            &[
                b"packages",
                b"admissions",
                b"openings-v1",
                b"endpoints-v1",
                b"sessions-v1",
            ],
        )?;
        for directory in &self.directories[2..] {
            shape(directory_fd(directory), &[])?;
        }
        let named = open_at(stage, c"binding.json", false, true)?;
        for fd in [self.binding_file.as_raw_fd(), named.as_raw_fd()] {
            require(BindingIdentity::capture(fd)? == self.binding_identity)?;
            require(read_bytes(fd, self.bytes.len() as u64)? == self.bytes)?;
            require(BindingIdentity::capture(fd)? == self.binding_identity)?;
        }
        Ok(())
    }
}

impl Provisioner {
    pub fn open_fixed_namespace() -> io::Result<Self> {
        Self::from_fixture_filesystem(namespace_root()?)
    }

    pub fn from_fixture_filesystem(root: OwnedFd) -> io::Result<Self> {
        at("admin", admin())?;
        let mut directories = vec![at("root", capture(root, 0, (0, 0)))?];
        for index in 1..=2 {
            let fd = at(
                "ancestor-open",
                open_at(
                    directory_fd(&directories[index - 1]),
                    COMPONENTS[index - 1],
                    true,
                    false,
                ),
            )?;
            directories.push(at("ancestor-policy", capture(fd, index, (0, 0)))?);
        }
        at("ancestor-validation", scaffold_validate(&directories))?;
        Ok(Self { directories })
    }

    pub fn provision(&self, uid: u32, gid: u32) -> Result<Publication, ProvisionError> {
        self.provision_with(uid, gid, &mut |_| Ok(()))
    }

    pub fn provision_with(
        &self,
        uid: u32,
        gid: u32,
        checkpoint: &mut impl FnMut(ProvisionStage) -> io::Result<()>,
    ) -> Result<Publication, ProvisionError> {
        let mut published = false;
        let result = self.prepare(uid, gid, checkpoint, &mut published);
        result.map_err(|error| classify(published, error))
    }

    fn prepare(
        &self,
        uid: u32,
        gid: u32,
        checkpoint: &mut impl FnMut(ProvisionStage) -> io::Result<()>,
        published: &mut bool,
    ) -> io::Result<Publication> {
        at("admin", admin())?;
        at("selectors", selectors(uid, gid))?;
        at("initial-anchors", scaffold_validate(&self.directories))?;
        let mut scaffold = Vec::new();
        for (index, directory) in self.directories.iter().enumerate() {
            scaffold.push(at(
                "duplicate-anchor",
                capture(duplicate(directory_fd(directory))?, index, (0, 0)),
            )?);
        }
        for index in 3..=5 {
            at("scaffold-anchors", scaffold_validate(&scaffold))?;
            let parent = directory_fd(&scaffold[index - 1]);
            let name = COMPONENTS[index - 1];
            let fd = match open_at(parent, name, true, index > 4) {
                Ok(fd) => fd,
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                    at("scaffold-filesystem", filesystem(parent))?;
                    match mkdir(parent, name, 0o755) {
                        Ok(()) => {
                            let fd =
                                at("new-scaffold-open", open_at(parent, name, true, index > 4))?;
                            at("new-scaffold-mode", chmod(fd.as_raw_fd(), 0o755))?;
                            fd
                        }
                        Err(error) if error.raw_os_error() == Some(libc::EEXIST) => at(
                            "existing-scaffold-open",
                            open_at(parent, name, true, index > 4),
                        )?,
                        Err(error) => return at("scaffold-create", Err(error)),
                    }
                }
                Err(error) => return at("scaffold-open", Err(error)),
            };
            scaffold.push(at("scaffold-policy", capture(fd, index, (0, 0)))?);
        }
        at("scaffold-validation", scaffold_validate(&scaffold))?;
        for index in (2..=5).rev() {
            at(
                "checkpoint",
                checkpoint(ProvisionStage::BeforeScaffoldSync(index as u8)),
            )?;
            at("scaffold-sync", sync(directory_fd(&scaffold[index])))?;
        }
        at("checkpoint", checkpoint(ProvisionStage::ScaffoldSynced))?;
        at("scaffold-validation", scaffold_validate(&scaffold))?;
        let id = at("installation-random", random())?;
        let suffix = at("stage-random", random())?;
        let name = CString::new(format!(".provision-{:032x}", u128::from_be_bytes(suffix)))
            .expect("fixed stage alphabet");
        let users = directory_fd(&scaffold[5]);
        at("stage-create", mkdir(users, &name, 0o700))?;
        let stage = at("stage-open", open_at(users, &name, true, true))?;
        at("stage-mode", chmod(stage.as_raw_fd(), 0o700))?;
        let stage_identity = at(
            "stage-policy",
            Identity::directory(stage.as_raw_fd(), 7, (0, 0)),
        )?;
        at(
            "stage-filesystem",
            same_filesystem(&scaffold[5].identity, &stage_identity),
        )?;
        at("checkpoint", checkpoint(ProvisionStage::StageCreated))?;
        let binding = Binding {
            version: 1,
            installation_id: installation_id(id),
            owner_uid: uid,
            owner_gid: gid,
            profile_revision: 1,
        };
        let bytes = at("binding-codec", binding.canonical_bytes())?;
        #[repr(C)]
        struct CreateHow {
            flags: u64,
            mode: u64,
            resolve: u64,
        }
        let how = CreateHow {
            flags: (libc::O_WRONLY
                | libc::O_CREAT
                | libc::O_EXCL
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK) as u64,
            mode: 0o600,
            resolve: libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_XDEV,
        };
        // SAFETY: the fixed terminated name and kernel-layout how select only a new exclusive file.
        let raw = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                stage.as_raw_fd(),
                c"binding.json".as_ptr(),
                &how,
                std::mem::size_of::<CreateHow>(),
            )
        };
        if raw < 0 {
            return at("binding-create", Err(io::Error::last_os_error()));
        }
        // SAFETY: successful openat2 returned a fresh owned descriptor.
        let writable = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        let raw = writable.as_raw_fd();
        at("binding-descriptor", descriptor(raw))?;
        at("binding-private-mode", chmod(raw, 0o600))?;
        // SAFETY: the public canonical binding buffer is readable for its exact length.
        let written = unsafe { libc::write(raw, bytes.as_ptr().cast(), bytes.len()) };
        if written < 0 {
            return at("binding-write", Err(io::Error::last_os_error()));
        }
        at(
            "binding-write-size",
            require(written as usize == bytes.len()),
        )?;
        at("binding-public-mode", chmod(raw, 0o644))?;
        let binding_identity = at("binding-policy", BindingIdentity::capture(raw))?;
        let binding_file = at(
            "binding-read-open",
            open_at(stage.as_raw_fd(), c"binding.json", false, true),
        )?;
        at(
            "binding-read-identity",
            require(BindingIdentity::capture(binding_file.as_raw_fd())? == binding_identity),
        )?;
        let mut private = Vec::new();
        for (index, name) in [
            c"state",
            c"packages",
            c"admissions",
            c"openings-v1",
            c"endpoints-v1",
            c"sessions-v1",
        ]
        .into_iter()
        .enumerate()
        {
            let parent = if index == 0 {
                stage.as_raw_fd()
            } else {
                directory_fd(&private[0])
            };
            at("private-create", mkdir(parent, name, 0o700))?;
            let fd = at("private-open", open_at(parent, name, true, true))?;
            at("private-mode", chmod(fd.as_raw_fd(), 0o700))?;
            private.push(at("private-policy", capture(fd, index + 7, (0, 0)))?);
        }
        at(
            "checkpoint",
            checkpoint(ProvisionStage::BeforeOwnershipHandoff),
        )?;
        at(
            "stage-pre-handoff",
            require(Identity::directory(stage.as_raw_fd(), 7, (0, 0))? == stage_identity),
        )?;
        for (index, directory) in private.iter().enumerate() {
            at(
                "private-pre-handoff",
                validate_directory(directory, index + 7, (0, 0)),
            )?;
            // SAFETY: fchown changes only the retained newly created directory; selectors exclude the sentinel.
            if unsafe { libc::fchown(directory_fd(directory), uid, gid) } < 0 {
                return at("private-owner", Err(io::Error::last_os_error()));
            }
        }
        at("checkpoint", checkpoint(ProvisionStage::OwnershipHandedOff))?;
        for (index, directory) in private.iter_mut().enumerate() {
            let identity = at(
                "private-final-policy",
                Identity::directory(directory_fd(directory), index + 7, (uid, gid)),
            )?;
            at(
                "private-final-object",
                require(
                    identity.device == directory.identity.device
                        && identity.inode == directory.identity.inode
                        && identity.mount == directory.identity.mount,
                ),
            )?;
            directory.identity = identity;
        }
        at(
            "binding-before-sync",
            require(BindingIdentity::capture(raw)? == binding_identity),
        )?;
        at("checkpoint", checkpoint(ProvisionStage::BeforeBindingSync))?;
        at("binding-sync", sync(raw))?;
        for (index, seam) in [
            (1, ProvisionStage::BeforePackagesSync),
            (2, ProvisionStage::BeforeAdmissionsSync),
            (3, ProvisionStage::BeforeOpeningsSync),
            (4, ProvisionStage::BeforeEndpointsSync),
            (5, ProvisionStage::BeforeSessionsSync),
            (0, ProvisionStage::BeforeStateSync),
        ] {
            at("checkpoint", checkpoint(seam))?;
            at("private-sync", sync(directory_fd(&private[index])))?;
        }
        at(
            "stage-before-final-mode",
            require(Identity::directory(stage.as_raw_fd(), 7, (0, 0))? == stage_identity),
        )?;
        at("stage-final-mode", chmod(stage.as_raw_fd(), 0o755))?;
        let mut directories = vec![at("stage-final-policy", capture(stage, 6, (uid, gid)))?];
        directories.extend(private);
        let slot = Slot {
            directories,
            binding_file,
            binding_identity,
            bytes,
            ids: (uid, gid),
        };
        at("stage-validation", slot.validate(users, &name))?;
        drop(writable);
        at("checkpoint", checkpoint(ProvisionStage::BeforeStageSync))?;
        at("stage-sync", sync(directory_fd(&slot.directories[0])))?;
        at("checkpoint", checkpoint(ProvisionStage::BeforeRename))?;
        at("publication-anchors", scaffold_validate(&scaffold))?;
        at(
            "publication-original-anchors",
            scaffold_validate(&self.directories),
        )?;
        at("publication-stage", slot.validate(users, &name))?;
        let final_name = CString::new(uid.to_string()).expect("numeric selector");
        // SAFETY: both terminated single names are relative to the same retained users directory.
        if unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                users,
                name.as_ptr(),
                users,
                final_name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } < 0
        {
            return at("publication-rename", Err(io::Error::last_os_error()));
        }
        *published = true;
        at("checkpoint", checkpoint(ProvisionStage::Renamed))?;
        at("published-anchors", scaffold_validate(&scaffold))?;
        at("published-slot", slot.validate(users, &final_name))?;
        at("checkpoint", checkpoint(ProvisionStage::BeforeParentSync))?;
        at("parent-sync", sync(users))?;
        at("checkpoint", checkpoint(ProvisionStage::ParentSynced))?;
        at("final-anchors", scaffold_validate(&scaffold))?;
        at(
            "final-original-anchors",
            scaffold_validate(&self.directories),
        )?;
        at("final-slot", slot.validate(users, &final_name))?;
        at("final-admin", admin())?;
        let binding = at("final-binding-codec", Binding::parse(&slot.bytes))?;
        Ok(Publication { binding })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_identifier_preserves_all_128_bits_and_binding_codec() {
        for (bytes, expected) in [
            ([0; 16], "00000000000000000000000000"),
            ([255; 16], "7zzzzzzzzzzzzzzzzzzzzzzzzz"),
            (1u128.to_be_bytes(), "00000000000000000000000001"),
            ((1u128 << 127).to_be_bytes(), "40000000000000000000000000"),
        ] {
            let id = installation_id(bytes);
            assert_eq!(id, expected);
            let binding = Binding {
                version: 1,
                installation_id: id,
                owner_uid: 1000,
                owner_gid: 1001,
                profile_revision: 1,
            };
            assert_eq!(
                Binding::parse(&binding.canonical_bytes().unwrap()).unwrap(),
                binding
            );
        }
    }

    #[test]
    fn selectors_refuse_root_and_chown_sentinel() {
        for pair in [(0, 1000), (1000, 0), (u32::MAX, 1000), (1000, u32::MAX)] {
            assert!(selectors(pair.0, pair.1).is_err());
        }
        assert!(selectors(1, u32::MAX - 1).is_ok());
    }

    #[test]
    fn visibility_boundary_controls_failure_classification() {
        assert!(matches!(
            classify(false, io::Error::from_raw_os_error(libc::EIO)),
            ProvisionError::Unpublished(_)
        ));
        assert!(matches!(
            classify(true, io::Error::from_raw_os_error(libc::EIO)),
            ProvisionError::PublishedDurabilityUnknown(_)
        ));
    }
}
