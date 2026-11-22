pub use super::linux_custody_anchor::Binding;
use super::linux_custody_anchor::{duplicate, namespace_root, MAX_BINDING};
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

#[path = "linux_custody_root/endpoint_directory.rs"]
pub(crate) mod endpoint_directory;

const DIRECTORY_STAGES: [&str; 10] = [
    "directory-0",
    "directory-1",
    "directory-2",
    "directory-3",
    "directory-4",
    "directory-5",
    "directory-6",
    "directory-7",
    "directory-8",
    "directory-9",
];
const OPEN_STAGES: [&str; 10] = [
    "open-0", "open-1", "open-2", "open-3", "open-4", "open-5", "open-6", "open-7", "open-8",
    "open-9",
];

fn at<T>(stage: &'static str, result: io::Result<T>) -> io::Result<T> {
    result.map_err(|error| {
        let detail = match error.raw_os_error() {
            Some(errno) => format!("errno {errno}"),
            None => error.to_string(),
        };
        io::Error::new(error.kind(), format!("{stage}: {detail}"))
    })
}

fn invalid() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn stable_ids() -> io::Result<(u32, u32)> {
    // SAFETY: these calls only query this process's credentials.
    let (uid, euid, gid, egid) = unsafe {
        (
            libc::getuid(),
            libc::geteuid(),
            libc::getgid(),
            libc::getegid(),
        )
    };
    require(uid != 0 && gid != 0 && uid == euid && gid == egid)?;
    Ok((uid, gid))
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

/// A read-only candidate over a caller-supplied synthetic filesystem root.
/// It establishes neither administrative origin nor credential admission.
pub struct CandidateRoot {
    directories: Vec<Directory>,
    ids: (u32, u32),
    binding_file: OwnedFd,
    binding_identity: BindingIdentity,
    bytes: Vec<u8>,
    binding: Binding,
}

impl CandidateRoot {
    pub fn open_fixed_namespace() -> io::Result<Self> {
        Self::from_fixture_filesystem(namespace_root()?)
    }

    pub fn from_fixture_filesystem(root: OwnedFd) -> io::Result<Self> {
        let ids = at("constructor-ids", stable_ids())?;
        let root_identity = at(
            DIRECTORY_STAGES[0],
            Identity::directory(root.as_raw_fd(), 0, ids),
        )?;
        let mut directories = vec![Directory {
            descriptor: root,
            identity: root_identity,
        }];
        for (index, (parent, name)) in Self::components(ids.0).into_iter().enumerate() {
            let index = index + 1;
            let fd = at(
                OPEN_STAGES[index],
                open_at(
                    directories[parent].descriptor.as_raw_fd(),
                    &name,
                    true,
                    index > 4,
                ),
            )?;
            let identity = at(
                DIRECTORY_STAGES[index],
                Identity::directory(fd.as_raw_fd(), index, ids),
            )?;
            directories.push(Directory {
                descriptor: fd,
                identity,
            });
        }
        let binding_file = at(
            "open-binding",
            open_at(
                directories[6].descriptor.as_raw_fd(),
                c"binding.json",
                false,
                true,
            ),
        )?;
        let binding_identity = at(
            "constructor-binding",
            BindingIdentity::capture(binding_file.as_raw_fd()),
        )?;
        let bytes = at(
            "binding-bytes",
            read_bytes(binding_file.as_raw_fd(), binding_identity.size),
        )?;
        let binding = at("binding-codec", Binding::parse(&bytes))?;
        at(
            "binding-owner",
            require((binding.owner_uid, binding.owner_gid) == ids),
        )?;
        let root = Self {
            directories,
            ids,
            binding_file,
            binding_identity,
            bytes,
            binding,
        };
        at("constructor-validate", root.validate())?;
        Ok(root)
    }

    fn components(uid: u32) -> Vec<(usize, CString)> {
        [
            (0, "var".to_owned()),
            (1, "lib".to_owned()),
            (2, "vivac".to_owned()),
            (3, "custody-v1".to_owned()),
            (4, "users".to_owned()),
            (5, uid.to_string()),
            (6, "state".to_owned()),
            (7, "packages".to_owned()),
            (7, "admissions".to_owned()),
        ]
        .into_iter()
        .map(|(parent, name)| (parent, CString::new(name).expect("fixed component")))
        .collect()
    }

    fn validate_binding(&self, fd: i32) -> io::Result<()> {
        at(
            "binding-snapshot",
            require(BindingIdentity::capture(fd)? == self.binding_identity),
        )?;
        at(
            "binding-bytes",
            require(at("binding-read", read_bytes(fd, self.binding_identity.size))? == self.bytes),
        )?;
        at(
            "binding-snapshot",
            require(BindingIdentity::capture(fd)? == self.binding_identity),
        )
    }

    fn validate_retained(&self) -> io::Result<()> {
        for (index, directory) in self.directories.iter().enumerate() {
            at(
                DIRECTORY_STAGES[index],
                require(
                    at(
                        DIRECTORY_STAGES[index],
                        Identity::directory(directory.descriptor.as_raw_fd(), index, self.ids),
                    )? == directory.identity,
                ),
            )?;
        }
        self.validate_binding(self.binding_file.as_raw_fd())
    }

    fn validate_named(&self) -> io::Result<()> {
        let mut current = vec![at(
            "duplicate-root",
            duplicate(self.directories[0].descriptor.as_raw_fd()),
        )?];
        at(
            DIRECTORY_STAGES[0],
            require(
                at(
                    DIRECTORY_STAGES[0],
                    Identity::directory(current[0].as_raw_fd(), 0, self.ids),
                )? == self.directories[0].identity,
            ),
        )?;
        for (index, (parent, name)) in Self::components(self.ids.0).into_iter().enumerate() {
            let index = index + 1;
            let fd = at(
                OPEN_STAGES[index],
                open_at(current[parent].as_raw_fd(), &name, true, index > 4),
            )?;
            at(
                DIRECTORY_STAGES[index],
                require(
                    at(
                        DIRECTORY_STAGES[index],
                        Identity::directory(fd.as_raw_fd(), index, self.ids),
                    )? == self.directories[index].identity,
                ),
            )?;
            current.push(fd);
        }
        let binding = at(
            "open-binding",
            open_at(current[6].as_raw_fd(), c"binding.json", false, true),
        )?;
        self.validate_binding(binding.as_raw_fd())
    }

    pub fn validate(&self) -> io::Result<()> {
        at("validate-ids", require(stable_ids()? == self.ids))?;
        at("retained", self.validate_retained())?;
        at("named", self.validate_named())?;
        at("retained", self.validate_retained())?;
        at("named", self.validate_named())?;
        at("validate-ids", require(stable_ids()? == self.ids))
    }

    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    fn child_descriptor(&self, index: usize) -> io::Result<OwnedFd> {
        self.validate()?;
        let fd = duplicate(self.directories[index].descriptor.as_raw_fd())?;
        require(
            Identity::directory(fd.as_raw_fd(), index, self.ids)?
                == self.directories[index].identity,
        )?;
        self.validate()?;
        Ok(fd)
    }

    pub fn admissions_descriptor(&self) -> io::Result<OwnedFd> {
        self.child_descriptor(9)
    }

    pub fn packages_descriptor(&self) -> io::Result<OwnedFd> {
        self.child_descriptor(8)
    }

    /// Runs storage work beneath the checked account opening collection.
    pub fn with_opening_directory<T>(
        &self,
        operation: impl FnOnce(&OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let state = self.directories[7].descriptor.as_raw_fd();
        let directory = open_at(state, c"openings-v1", true, true)?;
        let identity = Identity::directory(directory.as_raw_fd(), 8, self.ids)?;
        self.validate()?;
        require(Identity::directory(directory.as_raw_fd(), 8, self.ids)? == identity)?;
        let result = operation(&directory);
        require(Identity::directory(directory.as_raw_fd(), 8, self.ids)? == identity)?;
        let current = open_at(state, c"openings-v1", true, true)?;
        require(Identity::directory(current.as_raw_fd(), 8, self.ids)? == identity)?;
        self.validate()?;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn namespace_root_retains_current_root_with_safe_descriptor_flags() {
        let root = namespace_root().unwrap();
        let fd = root.as_raw_fd();
        assert!(fd >= 3);
        // SAFETY: both fcntl calls only query the retained live descriptor.
        let (flags, status) = unsafe {
            (
                libc::fcntl(fd, libc::F_GETFD),
                libc::fcntl(fd, libc::F_GETFL),
            )
        };
        assert!(flags >= 0);
        assert!(status >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        assert_eq!(status & libc::O_PATH, 0);
        assert_eq!(status & libc::O_ACCMODE, libc::O_RDONLY);
        assert_ne!(status & libc::O_DIRECTORY, 0);
        assert_ne!(status & libc::O_NOFOLLOW, 0);
        assert_ne!(status & libc::O_NONBLOCK, 0);

        let retained = std::fs::File::from(root).metadata().unwrap();
        let current = std::fs::File::open("/").unwrap().metadata().unwrap();
        assert!(retained.is_dir());
        assert_eq!(retained.dev(), current.dev());
        assert_eq!(retained.ino(), current.ino());
    }

    const CANONICAL: &[u8] = br#"{"version":1,"installation_id":"00000000000000000000000001","owner_uid":1000,"owner_gid":1000,"profile_revision":1}"#;

    fn fixture() -> Binding {
        Binding {
            version: 1,
            installation_id: "00000000000000000000000001".into(),
            owner_uid: 1000,
            owner_gid: 1000,
            profile_revision: 1,
        }
    }

    #[test]
    fn binding_round_trips_exact_canonical_bytes() {
        assert_eq!(Binding::parse(CANONICAL).unwrap(), fixture());
        assert_eq!(fixture().canonical_bytes().unwrap(), CANONICAL);
    }

    #[test]
    fn binding_rejects_noncanonical_or_unknown_input() {
        for bytes in [
            [CANONICAL, b"\n"].concat(),
            b"{}".to_vec(),
            vec![b' '; 513],
            String::from_utf8(CANONICAL.to_vec())
                .unwrap()
                .replace("\"version\":1", "\"version\":1,\"extra\":true")
                .into_bytes(),
            String::from_utf8(CANONICAL.to_vec())
                .unwrap()
                .replace("\"version\":1", "\"version\":1,\"version\":1")
                .into_bytes(),
            String::from_utf8(CANONICAL.to_vec())
                .unwrap()
                .replace("1000", "4294967296")
                .into_bytes(),
        ] {
            assert!(Binding::parse(&bytes).is_err());
        }
        let mut reordered: serde_json::Value = serde_json::from_slice(CANONICAL).unwrap();
        reordered["owner_uid"] = 1000.into();
        assert!(Binding::parse(&serde_json::to_vec(&reordered).unwrap()).is_err());
    }

    #[test]
    fn binding_rejects_unsupported_revisions_and_invalid_selectors() {
        for field in 0..4 {
            let mut value = fixture();
            match field {
                0 => value.version = 2,
                1 => value.profile_revision = 2,
                2 => value.owner_uid = 0,
                _ => value.owner_gid = 0,
            }
            assert!(value.canonical_bytes().is_err());
            assert!(Binding::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }

    #[test]
    fn binding_rejects_chown_unchanged_selector() {
        for uid in [true, false] {
            let mut value = fixture();
            if uid {
                value.owner_uid = u32::MAX;
            } else {
                value.owner_gid = u32::MAX;
            }
            assert!(value.canonical_bytes().is_err());
            assert!(Binding::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }

    #[test]
    fn binding_rejects_noncanonical_identifiers() {
        for identifier in [
            "",
            "../../another",
            "80000000000000000000000001",
            "0000000000000000000000000I",
            "0000000000000000000000000l",
            "0000000000000000000000000u",
            "0000000000000000000000000A",
        ] {
            let mut value = fixture();
            value.installation_id = identifier.into();
            assert!(value.canonical_bytes().is_err());
            assert!(Binding::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }
}
