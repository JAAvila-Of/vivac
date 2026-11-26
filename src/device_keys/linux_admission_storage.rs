#[path = "linux_admission_storage/metadata.rs"]
mod metadata;

pub use metadata::Metadata;
use metadata::{identifier, invalid, require};

use std::ffi::{CStr, CString};
use std::fmt;
use std::fs::File;
use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;

const MAX_RECORD: usize = 2048;
const RECORD_NAME: &CStr = c"record.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishStage {
    FileSynced,
    StagingSynced,
    BeforeRename,
    Renamed,
    BeforeParentSync,
    ParentSynced,
}

#[derive(Debug)]
pub enum PublishError {
    Unpublished(io::Error),
    PublishedDurabilityUnknown(io::Error),
}

impl fmt::Display for PublishError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unpublished(error) => write!(output, "metadata was not published: {error}"),
            Self::PublishedDurabilityUnknown(error) => write!(
                output,
                "metadata was published but durability is unknown: {error}"
            ),
        }
    }
}

impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unpublished(error) | Self::PublishedDurabilityUnknown(error) => Some(error),
        }
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
    require(uid == euid && gid == egid)?;
    Ok((uid, gid))
}

fn duplicate(fd: i32) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC duplicates the descriptor atomically.
    let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fcntl returned a fresh owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn descriptor(fd: i32) -> io::Result<()> {
    require(fd >= 3)?;
    // SAFETY: fcntl only queries the descriptor flags.
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
    // SAFETY: fstatfs initializes the entire statfs value on success.
    if unsafe { libc::fstatfs(fd, value.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the preceding fstatfs succeeded.
    require(unsafe { value.assume_init() }.f_type == 0xEF53)
}

fn acl(fd: i32) -> io::Result<()> {
    for name in [c"system.posix_acl_access", c"system.posix_acl_default"] {
        // SAFETY: the fixed attribute name is terminated; a zero buffer queries its size.
        let result = unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0) };
        if result >= 0 {
            return Err(invalid());
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::ENODATA | libc::EOPNOTSUPP)) {
            return Err(error);
        }
    }
    Ok(())
}

fn metadata(fd: i32) -> io::Result<std::fs::Metadata> {
    File::from(duplicate(fd)?).metadata()
}

fn mount_identity(fd: i32) -> io::Result<u64> {
    let mut value = std::mem::MaybeUninit::<libc::statx>::uninit();
    let mask = libc::STATX_BASIC_STATS | libc::STATX_MNT_ID;
    // SAFETY: AT_EMPTY_PATH selects the retained descriptor; statx initializes value.
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
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the preceding statx call succeeded.
    let value = unsafe { value.assume_init() };
    require(value.stx_mask & mask == mask)?;
    Ok(value.stx_mnt_id)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DirectoryIdentity {
    dev: u64,
    mount: u64,
    ino: u64,
    uid: u32,
    gid: u32,
    mode: u32,
}

impl DirectoryIdentity {
    fn capture(fd: i32, ids: (u32, u32)) -> io::Result<Self> {
        descriptor(fd)?;
        filesystem(fd)?;
        acl(fd)?;
        let value = metadata(fd)?;
        require(
            value.is_dir()
                && value.uid() == ids.0
                && value.gid() == ids.1
                && value.mode() & 0o7777 == 0o700,
        )?;
        Ok(Self {
            dev: value.dev(),
            mount: mount_identity(fd)?,
            ino: value.ino(),
            uid: value.uid(),
            gid: value.gid(),
            mode: value.mode(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    dev: u64,
    mount: u64,
    ino: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    links: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DirectoryStamp {
    size: u64,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl DirectoryStamp {
    fn capture(fd: i32) -> io::Result<Self> {
        let value = metadata(fd)?;
        require(value.nlink() == 2)?;
        Ok(Self {
            size: value.len(),
            links: value.nlink(),
            modified: (value.mtime(), value.mtime_nsec()),
            changed: (value.ctime(), value.ctime_nsec()),
        })
    }
}

impl FileIdentity {
    fn capture(fd: i32, ids: (u32, u32), device: u64, mount: u64) -> io::Result<Self> {
        descriptor(fd)?;
        filesystem(fd)?;
        acl(fd)?;
        let value = metadata(fd)?;
        let actual_mount = mount_identity(fd)?;
        require(
            value.is_file()
                && value.dev() == device
                && actual_mount == mount
                && value.uid() == ids.0
                && value.gid() == ids.1
                && value.mode() & 0o7777 == 0o600
                && value.nlink() == 1
                && value.len() <= MAX_RECORD as u64,
        )?;
        Ok(Self {
            dev: value.dev(),
            mount: actual_mount,
            ino: value.ino(),
            uid: value.uid(),
            gid: value.gid(),
            mode: value.mode(),
            links: value.nlink(),
            size: value.len(),
            modified: (value.mtime(), value.mtime_nsec()),
            changed: (value.ctime(), value.ctime_nsec()),
        })
    }
}

fn open_at(directory: i32, name: &CStr, flags: i32) -> io::Result<OwnedFd> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: (flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK) as u64,
        mode: if flags & libc::O_CREAT != 0 { 0o600 } else { 0 },
        resolve: libc::RESOLVE_BENEATH
            | libc::RESOLVE_NO_SYMLINKS
            | libc::RESOLVE_NO_MAGICLINKS
            | libc::RESOLVE_NO_XDEV,
    };
    // SAFETY: name is terminated and how has the kernel open_how layout.
    let raw = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory,
            name.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat2 returned one fresh descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw as i32) })
}

fn read_bytes(fd: i32, expected: u64) -> io::Result<Vec<u8>> {
    require(expected <= MAX_RECORD as u64)?;
    let mut bytes = [0u8; MAX_RECORD + 1];
    // SAFETY: bytes is a live writable array; pread does not change the shared offset.
    let count = unsafe { libc::pread(fd, bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    require(count as u64 == expected)?;
    Ok(bytes[..count as usize].to_vec())
}

fn sync(fd: i32) -> io::Result<()> {
    // SAFETY: fsync only synchronizes this owned descriptor.
    if unsafe { libc::fsync(fd) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn shape(directory: i32) -> io::Result<()> {
    let opened = open_at(directory, c".", libc::O_RDONLY | libc::O_DIRECTORY)?;
    // SAFETY: fdopendir consumes the fresh descriptor only on success.
    let raw = unsafe { libc::fdopendir(opened.as_raw_fd()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    std::mem::forget(opened);
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            // SAFETY: this object owns the successful fdopendir result.
            unsafe { libc::closedir(self.0) };
        }
    }
    let stream = Directory(raw);
    let mut seen = [false; 3];
    for _ in 0..4 {
        // SAFETY: errno belongs to this thread; stream is an owned directory stream.
        let entry = unsafe {
            *libc::__errno_location() = 0;
            libc::readdir(stream.0)
        };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error);
            }
            return require(seen == [true; 3]);
        }
        // SAFETY: readdir returns a live, NUL-terminated d_name until the next call.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let index = if name == c"." {
            0
        } else if name == c".." {
            1
        } else if name == RECORD_NAME {
            2
        } else {
            return Err(invalid());
        };
        require(!seen[index])?;
        seen[index] = true;
    }
    Err(invalid())
}

pub(crate) mod record_seal {
    pub trait Sealed {}
}

pub trait StoredRecord: Eq + record_seal::Sealed {
    fn credential_id(&self) -> &str;
    fn canonical_bytes(&self) -> io::Result<Vec<u8>>;
    fn parse(bytes: &[u8]) -> io::Result<Self>
    where
        Self: Sized;
}

impl record_seal::Sealed for Metadata {}

impl StoredRecord for Metadata {
    fn credential_id(&self) -> &str {
        &self.credential_id
    }

    fn canonical_bytes(&self) -> io::Result<Vec<u8>> {
        Metadata::canonical_bytes(self)
    }

    fn parse(bytes: &[u8]) -> io::Result<Self> {
        Metadata::parse(bytes)
    }
}

pub type Storage = TypedStorage<Metadata>;
pub type Snapshot = TypedSnapshot<Metadata>;

/// An anchored storage mechanism, not an admission or authority source.
pub struct TypedStorage<T: StoredRecord> {
    root: OwnedFd,
    identity: DirectoryIdentity,
    ids: (u32, u32),
    _record: PhantomData<T>,
}

impl<T: StoredRecord> TypedStorage<T> {
    pub fn from_anchored(root: OwnedFd) -> io::Result<Self> {
        let ids = stable_ids()?;
        let identity = DirectoryIdentity::capture(root.as_raw_fd(), ids)?;
        let value = Self {
            root,
            identity,
            ids,
            _record: PhantomData,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> io::Result<()> {
        require(stable_ids()? == self.ids)?;
        require(DirectoryIdentity::capture(self.root.as_raw_fd(), self.ids)? == self.identity)
    }

    pub fn read(&self, credential_id: &str) -> io::Result<TypedSnapshot<T>> {
        self.validate()?;
        require(identifier(credential_id))?;
        let name = CString::new(credential_id).map_err(|_| invalid())?;
        let directory = open_at(
            self.root.as_raw_fd(),
            &name,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        let directory_identity = DirectoryIdentity::capture(directory.as_raw_fd(), self.ids)?;
        require(
            directory_identity.dev == self.identity.dev
                && directory_identity.mount == self.identity.mount,
        )?;
        let directory_stamp = DirectoryStamp::capture(directory.as_raw_fd())?;
        shape(directory.as_raw_fd())?;
        let file = open_at(directory.as_raw_fd(), RECORD_NAME, libc::O_RDONLY)?;
        let file_identity = FileIdentity::capture(
            file.as_raw_fd(),
            self.ids,
            self.identity.dev,
            self.identity.mount,
        )?;
        let bytes = read_bytes(file.as_raw_fd(), file_identity.size)?;
        require(
            FileIdentity::capture(
                file.as_raw_fd(),
                self.ids,
                self.identity.dev,
                self.identity.mount,
            )? == file_identity,
        )?;
        let value = T::parse(&bytes)?;
        require(value.credential_id() == credential_id)?;
        let snapshot = TypedSnapshot {
            root: duplicate(self.root.as_raw_fd())?,
            root_identity: self.identity.clone(),
            ids: self.ids,
            name,
            directory,
            directory_identity,
            directory_stamp,
            file,
            file_identity,
            bytes,
            value,
        };
        snapshot.validate()?;
        self.validate()?;
        Ok(snapshot)
    }

    pub fn publish(&self, value: &T) -> Result<(), PublishError> {
        self.publish_with(value, &mut |_| Ok(()))
    }

    pub fn publish_with(
        &self,
        value: &T,
        checkpoint: &mut impl FnMut(PublishStage) -> io::Result<()>,
    ) -> Result<(), PublishError> {
        let mut renamed = false;
        let result = (|| -> io::Result<()> {
            let bytes = value.canonical_bytes()?;
            self.validate()?;
            let mut nonce = [0u8; 32];
            // SAFETY: getrandom writes only into the complete nonce array.
            let count = unsafe {
                libc::getrandom(nonce.as_mut_ptr().cast(), nonce.len(), libc::GRND_NONBLOCK)
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            require(count as usize == nonce.len())?;
            let mut stage_name = String::from(".pending-");
            for byte in nonce {
                use std::fmt::Write;
                write!(&mut stage_name, "{byte:02x}").map_err(|_| invalid())?;
            }
            let stage_name = CString::new(stage_name).map_err(|_| invalid())?;
            // SAFETY: stage_name is terminated and relative to the owned root.
            if unsafe { libc::mkdirat(self.root.as_raw_fd(), stage_name.as_ptr(), 0o700) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let staged = open_at(
                self.root.as_raw_fd(),
                &stage_name,
                libc::O_RDONLY | libc::O_DIRECTORY,
            )?;
            let staged_identity = DirectoryIdentity::capture(staged.as_raw_fd(), self.ids)?;
            require(
                staged_identity.dev == self.identity.dev
                    && staged_identity.mount == self.identity.mount,
            )?;
            let file = open_at(
                staged.as_raw_fd(),
                RECORD_NAME,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            )?;
            FileIdentity::capture(
                file.as_raw_fd(),
                self.ids,
                self.identity.dev,
                self.identity.mount,
            )?;
            // SAFETY: bytes is live for this bounded write to the new owned file.
            let written =
                unsafe { libc::write(file.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
            if written < 0 {
                return Err(io::Error::last_os_error());
            }
            require(written as usize == bytes.len())?;
            sync(file.as_raw_fd())?;
            checkpoint(PublishStage::FileSynced)?;
            sync(staged.as_raw_fd())?;
            checkpoint(PublishStage::StagingSynced)?;
            let file_identity = FileIdentity::capture(
                file.as_raw_fd(),
                self.ids,
                self.identity.dev,
                self.identity.mount,
            )?;
            require(file_identity.size == bytes.len() as u64)?;
            self.validate()?;
            require(DirectoryIdentity::capture(staged.as_raw_fd(), self.ids)? == staged_identity)?;
            let selected_stage = open_at(
                self.root.as_raw_fd(),
                &stage_name,
                libc::O_RDONLY | libc::O_DIRECTORY,
            )?;
            require(
                DirectoryIdentity::capture(selected_stage.as_raw_fd(), self.ids)?
                    == staged_identity,
            )?;
            shape(staged.as_raw_fd())?;
            let staged_file = open_at(staged.as_raw_fd(), RECORD_NAME, libc::O_RDONLY)?;
            require(
                FileIdentity::capture(
                    staged_file.as_raw_fd(),
                    self.ids,
                    self.identity.dev,
                    self.identity.mount,
                )? == file_identity
                    && read_bytes(staged_file.as_raw_fd(), file_identity.size)? == bytes,
            )?;
            checkpoint(PublishStage::BeforeRename)?;
            let final_name =
                CString::new(value.credential_id().as_bytes()).map_err(|_| invalid())?;
            // SAFETY: both names are terminated single components relative to this root.
            if unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    self.root.as_raw_fd(),
                    stage_name.as_ptr(),
                    self.root.as_raw_fd(),
                    final_name.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
            renamed = true;
            checkpoint(PublishStage::Renamed)?;
            let final_snapshot = self.read(value.credential_id())?;
            require(
                final_snapshot.directory_identity == staged_identity
                    && final_snapshot.file_identity == file_identity
                    && final_snapshot.bytes == bytes
                    && final_snapshot.metadata() == value,
            )?;
            checkpoint(PublishStage::BeforeParentSync)?;
            sync(self.root.as_raw_fd())?;
            checkpoint(PublishStage::ParentSynced)?;
            final_snapshot.validate()?;
            self.validate()
        })();
        result.map_err(|error| {
            if renamed {
                PublishError::PublishedDurabilityUnknown(error)
            } else {
                PublishError::Unpublished(error)
            }
        })
    }
}

/// A retained view of stored public metadata; it confers no admission.
pub struct TypedSnapshot<T: StoredRecord> {
    root: OwnedFd,
    root_identity: DirectoryIdentity,
    ids: (u32, u32),
    name: CString,
    directory: OwnedFd,
    directory_identity: DirectoryIdentity,
    directory_stamp: DirectoryStamp,
    file: OwnedFd,
    file_identity: FileIdentity,
    bytes: Vec<u8>,
    value: T,
}

impl<T: StoredRecord> TypedSnapshot<T> {
    pub fn metadata(&self) -> &T {
        &self.value
    }

    pub fn validate(&self) -> io::Result<()> {
        require(stable_ids()? == self.ids)?;
        require(
            DirectoryIdentity::capture(self.root.as_raw_fd(), self.ids)? == self.root_identity,
        )?;
        require(
            DirectoryIdentity::capture(self.directory.as_raw_fd(), self.ids)?
                == self.directory_identity,
        )?;
        require(DirectoryStamp::capture(self.directory.as_raw_fd())? == self.directory_stamp)?;
        shape(self.directory.as_raw_fd())?;
        require(
            FileIdentity::capture(
                self.file.as_raw_fd(),
                self.ids,
                self.root_identity.dev,
                self.root_identity.mount,
            )? == self.file_identity,
        )?;
        require(read_bytes(self.file.as_raw_fd(), self.file_identity.size)? == self.bytes)?;
        require(
            FileIdentity::capture(
                self.file.as_raw_fd(),
                self.ids,
                self.root_identity.dev,
                self.root_identity.mount,
            )? == self.file_identity,
        )?;
        let current_directory = open_at(
            self.root.as_raw_fd(),
            &self.name,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        require(
            DirectoryIdentity::capture(current_directory.as_raw_fd(), self.ids)?
                == self.directory_identity,
        )?;
        require(DirectoryStamp::capture(current_directory.as_raw_fd())? == self.directory_stamp)?;
        shape(current_directory.as_raw_fd())?;
        let current_file = open_at(current_directory.as_raw_fd(), RECORD_NAME, libc::O_RDONLY)?;
        require(
            FileIdentity::capture(
                current_file.as_raw_fd(),
                self.ids,
                self.root_identity.dev,
                self.root_identity.mount,
            )? == self.file_identity
                && read_bytes(current_file.as_raw_fd(), self.file_identity.size)? == self.bytes,
        )?;
        require(
            FileIdentity::capture(
                current_file.as_raw_fd(),
                self.ids,
                self.root_identity.dev,
                self.root_identity.mount,
            )? == self.file_identity,
        )?;
        require(
            DirectoryIdentity::capture(self.directory.as_raw_fd(), self.ids)?
                == self.directory_identity,
        )?;
        require(DirectoryStamp::capture(self.directory.as_raw_fd())? == self.directory_stamp)?;
        let last_directory = open_at(
            self.root.as_raw_fd(),
            &self.name,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        require(
            DirectoryIdentity::capture(last_directory.as_raw_fd(), self.ids)?
                == self.directory_identity
                && DirectoryStamp::capture(last_directory.as_raw_fd())? == self.directory_stamp,
        )?;
        let last_file = open_at(last_directory.as_raw_fd(), RECORD_NAME, libc::O_RDONLY)?;
        require(
            FileIdentity::capture(
                last_file.as_raw_fd(),
                self.ids,
                self.root_identity.dev,
                self.root_identity.mount,
            )? == self.file_identity,
        )?;
        require(DirectoryIdentity::capture(self.root.as_raw_fd(), self.ids)? == self.root_identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Metadata {
        Metadata {
            version: 1,
            installation_id: "00000000000000000000000001".into(),
            creation_id: "00000000000000000000000002".into(),
            device_id: "00000000000000000000000003".into(),
            credential_id: "00000000000000000000000004".into(),
            public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
            ciphertext_sha256: "11".repeat(32),
            manifest_sha256: "22".repeat(32),
            ciphertext_len: 128,
            manifest_len: 256,
            creation_profile_revision: 1,
            secret_policy_revision: 1,
        }
    }

    #[test]
    fn metadata_requires_exact_canonical_encoding() {
        let value = fixture();
        let bytes = value.canonical_bytes().unwrap();
        assert_eq!(Metadata::parse(&bytes).unwrap(), value);
        let mut padded = bytes.clone();
        padded.push(b'\n');
        assert!(Metadata::parse(&padded).is_err());
        let mut object: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        object["admitted"] = true.into();
        assert!(Metadata::parse(&serde_json::to_vec(&object).unwrap()).is_err());
        assert!(Metadata::parse(&vec![b' '; MAX_RECORD + 1]).is_err());
    }

    #[test]
    fn metadata_refuses_unknown_profiles_and_invalid_identity() {
        for field in 0..5 {
            let mut value = fixture();
            match field {
                0 => value.version = 2,
                1 => value.creation_profile_revision = 2,
                2 => value.secret_policy_revision = 2,
                3 => value.credential_id = "../../another".into(),
                _ => value.installation_id = "80000000000000000000000001".into(),
            }
            assert!(value.canonical_bytes().is_err());
        }
    }

    #[test]
    fn metadata_refuses_weak_keys_noncanonical_digests_and_unbounded_files() {
        let mut value = fixture();
        value.public_key = "01".to_owned() + &"00".repeat(31);
        assert!(value.canonical_bytes().is_err());
        value = fixture();
        value.manifest_sha256 = "AA".repeat(32);
        assert!(value.canonical_bytes().is_err());
        value = fixture();
        value.ciphertext_len = 0;
        assert!(value.canonical_bytes().is_err());
        value = fixture();
        value.manifest_len = 8193;
        assert!(value.canonical_bytes().is_err());
    }
}
