use super::linux_creation_record::Record;
use super::linux_custody_root::CandidateRoot;
use super::linux_openssh_package::{self as package, Manifest};
use sha2::{Digest, Sha256};
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

const OPENING_PERMIT_SIZE: usize = 48;

/// State observed by the disposable reader. This cannot permit an unlock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpeningPermitKind {
    FirstOpen,
    CleanClose,
}

/// Encodes public storage metadata without granting publication authority.
pub fn opening_permit_frame(
    kind: OpeningPermitKind,
    canonical_record: &[u8],
) -> [u8; OPENING_PERMIT_SIZE] {
    let mut frame = [0; OPENING_PERMIT_SIZE];
    frame[..8].copy_from_slice(b"VVCOPEN\0");
    frame[8] = 1;
    frame[9] = match kind {
        OpeningPermitKind::FirstOpen => 1,
        OpeningPermitKind::CleanClose => 2,
    };
    frame[16..].copy_from_slice(&Sha256::digest(canonical_record));
    frame
}

fn decode_opening_permit(bytes: &[u8], canonical_record: &[u8]) -> io::Result<OpeningPermitKind> {
    let frame: &[u8; OPENING_PERMIT_SIZE] = bytes.try_into().map_err(|_| invalid())?;
    let digest: [u8; 32] = Sha256::digest(canonical_record).into();
    require(
        &frame[..8] == b"VVCOPEN\0"
            && frame[8] == 1
            && frame[10..16] == [0; 6]
            && frame[16..] == digest,
    )?;
    match frame[9] {
        1 => Ok(OpeningPermitKind::FirstOpen),
        2 => Ok(OpeningPermitKind::CleanClose),
        _ => Err(invalid()),
    }
}

pub(crate) fn identifier(value: &str) -> bool {
    value.len() == 26
        && matches!(value.as_bytes()[0], b'0'..=b'7')
        && value
            .bytes()
            .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&byte))
}

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn stable_ids() -> io::Result<(u32, u32)> {
    // SAFETY: these calls only query process credentials.
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
    // SAFETY: both operations only query this live descriptor.
    let (flags, status) = unsafe {
        (
            libc::fcntl(fd, libc::F_GETFD),
            libc::fcntl(fd, libc::F_GETFL),
        )
    };
    if flags < 0 || status < 0 {
        return Err(io::Error::last_os_error());
    }
    require(
        flags & libc::FD_CLOEXEC != 0
            && status & libc::O_PATH == 0
            && status & libc::O_ACCMODE == libc::O_RDONLY
            && status & libc::O_NONBLOCK != 0
            && status & libc::O_NOFOLLOW != 0,
    )
}

fn open_at(parent: i32, name: &CStr, directory: bool) -> io::Result<OwnedFd> {
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
            | libc::RESOLVE_NO_MAGICLINKS
            | libc::RESOLVE_NO_XDEV,
    };
    // SAFETY: name is terminated and how matches the kernel open_how layout.
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
    // SAFETY: successful openat2 returned one fresh owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    descriptor(fd.as_raw_fd())?;
    Ok(fd)
}

fn create_opening_stage(parent: i32, name: &CStr) -> io::Result<std::fs::File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: (libc::O_WRONLY
            | libc::O_CREAT
            | libc::O_EXCL
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | libc::O_NOFOLLOW) as u64,
        mode: 0o600,
        resolve: libc::RESOLVE_BENEATH
            | libc::RESOLVE_NO_SYMLINKS
            | libc::RESOLVE_NO_MAGICLINKS
            | libc::RESOLVE_NO_XDEV,
    };
    // SAFETY: the initialized open_how and terminated component stay live.
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
    Ok(std::fs::File::from(unsafe {
        OwnedFd::from_raw_fd(raw as i32)
    }))
}

fn stat(fd: i32, directory: bool, ids: (u32, u32)) -> io::Result<libc::statx> {
    stat_with_links(fd, directory, ids, 1)
}

fn stat_with_links(
    fd: i32,
    directory: bool,
    ids: (u32, u32),
    links: u32,
) -> io::Result<libc::statx> {
    descriptor(fd)?;
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs initializes filesystem on success.
    if unsafe { libc::fstatfs(fd, filesystem.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatfs succeeded.
    require(unsafe { filesystem.assume_init() }.f_type == 0xEF53)?;
    for name in [c"system.posix_acl_access", c"system.posix_acl_default"] {
        if !directory && name == c"system.posix_acl_default" {
            continue;
        }
        // SAFETY: fixed terminated names and null buffer query attribute size.
        if unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0) } >= 0 {
            return Err(invalid());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENODATA) {
            return Err(error);
        }
    }
    let mut value = std::mem::MaybeUninit::<libc::statx>::uninit();
    let mask = libc::STATX_BASIC_STATS | libc::STATX_MNT_ID;
    // SAFETY: AT_EMPTY_PATH selects the live fd and statx initializes value.
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
    // SAFETY: statx succeeded.
    let value = unsafe { value.assume_init() };
    require(
        value.stx_mask & mask == mask
            && (value.stx_uid, value.stx_gid) == ids
            && u32::from(value.stx_mode) & libc::S_IFMT
                == if directory {
                    libc::S_IFDIR
                } else {
                    libc::S_IFREG
                }
            && value.stx_mode & 0o7777 == if directory { 0o700 } else { 0o600 }
            && (directory || value.stx_nlink == links),
    )?;
    Ok(value)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Identity {
    device: (u32, u32),
    mount: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u16,
}

impl Identity {
    fn capture(value: &libc::statx) -> Self {
        Self {
            device: (value.stx_dev_major, value.stx_dev_minor),
            mount: value.stx_mnt_id,
            inode: value.stx_ino,
            uid: value.stx_uid,
            gid: value.stx_gid,
            mode: value.stx_mode,
        }
    }

    fn same_filesystem(&self, other: &Self) -> io::Result<()> {
        require(self.device == other.device && self.mount == other.mount)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Stamp {
    size: u64,
    links: u32,
    changed: (i64, u32),
    modified: (i64, u32),
}

impl Stamp {
    fn capture(value: &libc::statx) -> Self {
        Self {
            size: value.stx_size,
            links: value.stx_nlink,
            changed: (value.stx_ctime.tv_sec, value.stx_ctime.tv_nsec),
            modified: (value.stx_mtime.tv_sec, value.stx_mtime.tv_nsec),
        }
    }
}

struct Directory {
    fd: OwnedFd,
    identity: Identity,
    stamp: Option<Stamp>,
}

impl Directory {
    fn capture(fd: OwnedFd, ids: (u32, u32), slot: bool) -> io::Result<Self> {
        let value = stat(fd.as_raw_fd(), true, ids)?;
        Ok(Self {
            fd,
            identity: Identity::capture(&value),
            stamp: slot.then(|| Stamp::capture(&value)),
        })
    }

    fn validate(&self, fd: i32, ids: (u32, u32)) -> io::Result<()> {
        let value = stat(fd, true, ids)?;
        require(Identity::capture(&value) == self.identity)?;
        if let Some(stamp) = &self.stamp {
            require(Stamp::capture(&value) == *stamp)?;
        }
        Ok(())
    }
}

fn read_bytes(fd: i32, size: u64, limit: usize) -> io::Result<Vec<u8>> {
    require(size != 0 && size <= limit as u64)?;
    let mut bytes = vec![0; size as usize + 1];
    let mut used = 0;
    while used < bytes.len() {
        // SAFETY: the remaining buffer is writable and offset is bounded.
        let count = unsafe {
            libc::pread(
                fd,
                bytes[used..].as_mut_ptr().cast(),
                bytes.len() - used,
                used as libc::off_t,
            )
        };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if count == 0 {
            break;
        }
        used += count as usize;
    }
    require(used as u64 == size)?;
    bytes.truncate(used);
    Ok(bytes)
}

struct FileSnapshot {
    fd: OwnedFd,
    identity: Identity,
    stamp: Stamp,
    bytes: Vec<u8>,
    limit: usize,
}

impl FileSnapshot {
    fn capture(parent: &Directory, name: &CStr, ids: (u32, u32), limit: usize) -> io::Result<Self> {
        let fd = open_at(parent.fd.as_raw_fd(), name, false)?;
        let before = stat(fd.as_raw_fd(), false, ids)?;
        let identity = Identity::capture(&before);
        parent.identity.same_filesystem(&identity)?;
        let stamp = Stamp::capture(&before);
        let bytes = read_bytes(fd.as_raw_fd(), stamp.size, limit)?;
        let result = Self {
            fd,
            identity,
            stamp,
            bytes,
            limit,
        };
        result.validate(result.fd.as_raw_fd(), ids)?;
        Ok(result)
    }

    fn metadata(&self, fd: i32, ids: (u32, u32)) -> io::Result<()> {
        let value = stat(fd, false, ids)?;
        require(Identity::capture(&value) == self.identity && Stamp::capture(&value) == self.stamp)
    }

    fn validate(&self, fd: i32, ids: (u32, u32)) -> io::Result<()> {
        self.metadata(fd, ids)?;
        require(read_bytes(fd, self.stamp.size, self.limit)? == self.bytes)?;
        self.metadata(fd, ids)
    }

    fn validate_unlinked(&self, ids: (u32, u32)) -> io::Result<()> {
        let check = || {
            let value = stat_with_links(self.fd.as_raw_fd(), false, ids, 0)?;
            require(Identity::capture(&value) == self.identity && value.stx_size == self.stamp.size)
        };
        check()?;
        require(read_bytes(self.fd.as_raw_fd(), self.stamp.size, self.limit)? == self.bytes)?;
        check()
    }

    fn commit_opening(
        &self,
        directory: &Directory,
        name: &CStr,
        credential: &CStr,
        ids: (u32, u32),
        authorize_rename: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let parent = directory.fd.as_raw_fd();
        let named_stage = open_at(parent, name, false)?;
        self.validate(named_stage.as_raw_fd(), ids)?;
        if let Err(cancelled) = authorize_rename() {
            let named_stage = open_at(parent, name, false)?;
            self.validate(named_stage.as_raw_fd(), ids)?;
            // SAFETY: this is our validated staging name in the retained directory.
            if unsafe { libc::unlinkat(parent, name.as_ptr(), 0) } < 0 {
                return Err(io::Error::last_os_error());
            }
            return Err(cancelled);
        }
        // SAFETY: both terminated components are relative to the same retained directory.
        if unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                parent,
                name.as_ptr(),
                parent,
                credential.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

fn shape(directory: i32, expected: &[&CStr]) -> io::Result<()> {
    let fd = open_at(directory, c".", true)?;
    let raw = fd.into_raw_fd();
    // SAFETY: fdopendir takes ownership of this fresh directory descriptor on success.
    let stream = unsafe { libc::fdopendir(raw) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: fdopendir failed and did not take ownership of raw.
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
    let mut seen = vec![false; expected.len()];
    for _ in 0..expected.len() + 3 {
        // SAFETY: this thread's errno is writable; stream remains exclusively live.
        unsafe {
            *libc::__errno_location() = 0;
        }
        // SAFETY: readdir returns an entry valid until the next call on stream.
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error);
            }
            return require(seen.into_iter().all(|value| value));
        }
        // SAFETY: the successful readdir entry contains a terminated d_name.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name == c"." || name == c".." {
            continue;
        }
        let index = expected
            .iter()
            .position(|value| *value == name)
            .ok_or_else(invalid)?;
        require(!seen[index])?;
        seen[index] = true;
    }
    Err(invalid())
}

fn compare_package(
    record: &Record,
    installation_id: &str,
    credential_id: &str,
    key: &[u8],
    manifest_bytes: &[u8],
) -> io::Result<()> {
    let manifest: Manifest = package::validate_bytes(key, manifest_bytes).map_err(|_| invalid())?;
    require(
        record.installation_id == installation_id
            && record.credential_id == credential_id
            && manifest.device_id == record.device_id
            && manifest.credential_id == record.credential_id
            && manifest.public_key == record.public_key
            && manifest.key_size == record.ciphertext_len
            && manifest.key_sha256 == record.ciphertext_sha256
            && key.len() as u64 == record.ciphertext_len
            && manifest_bytes.len() as u64 == record.manifest_len
            && package::hex(&Sha256::digest(key)) == record.ciphertext_sha256
            && package::hex(&Sha256::digest(manifest_bytes)) == record.manifest_sha256,
    )
}

/// Whether no entry of any kind, a dangling link included, holds `name`.
/// Only `ENOENT` is absence; any other failure is an error.
fn entry_absent(directory: libc::c_int, name: &CStr) -> io::Result<bool> {
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the terminated name is one component below the directory,
    // links are not followed and status is writable for one stat object.
    let found = unsafe {
        libc::fstatat(
            directory,
            name.as_ptr(),
            status.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if found == 0 {
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOENT) {
        Ok(true)
    } else {
        Err(error)
    }
}

/// Read-only resident candidate; it supplies no producer evidence or admission.
pub struct CandidateResident {
    root: CandidateRoot,
    ids: (u32, u32),
    credential: CString,
    admissions: Directory,
    packages: Directory,
    admission: Directory,
    package: Directory,
    record_file: FileSnapshot,
    key_file: FileSnapshot,
    manifest_file: FileSnapshot,
    record: Record,
}

impl CandidateResident {
    pub fn open_fixed_namespace(credential_id: &str) -> io::Result<Self> {
        require(identifier(credential_id))?;
        let credential = CString::new(credential_id).map_err(|_| invalid())?;
        let root = CandidateRoot::open_fixed_namespace()?;
        let ids = stable_ids()?;
        require((root.binding().owner_uid, root.binding().owner_gid) == ids)?;
        let admissions = Directory::capture(root.admissions_descriptor()?, ids, false)?;
        let admission = Directory::capture(
            open_at(admissions.fd.as_raw_fd(), &credential, true)?,
            ids,
            true,
        )?;
        admissions.identity.same_filesystem(&admission.identity)?;
        shape(admission.fd.as_raw_fd(), &[c"record.json"])?;
        let record_file = FileSnapshot::capture(&admission, c"record.json", ids, 2048)?;
        let record = Record::parse(&record_file.bytes)?;
        require(
            record.installation_id == root.binding().installation_id
                && record.credential_id == credential_id,
        )?;
        let packages = Directory::capture(root.packages_descriptor()?, ids, false)?;
        admissions.identity.same_filesystem(&packages.identity)?;
        let package = Directory::capture(
            open_at(packages.fd.as_raw_fd(), &credential, true)?,
            ids,
            true,
        )?;
        packages.identity.same_filesystem(&package.identity)?;
        shape(package.fd.as_raw_fd(), &[c"key.openssh", c"manifest.json"])?;
        let key_file = FileSnapshot::capture(&package, c"key.openssh", ids, 65536)?;
        let manifest_file = FileSnapshot::capture(&package, c"manifest.json", ids, 2048)?;
        compare_package(
            &record,
            &root.binding().installation_id,
            credential_id,
            &key_file.bytes,
            &manifest_file.bytes,
        )?;
        let candidate = Self {
            root,
            ids,
            credential,
            admissions,
            packages,
            admission,
            package,
            record_file,
            key_file,
            manifest_file,
            record,
        };
        candidate.validate()?;
        Ok(candidate)
    }

    pub fn record(&self) -> &Record {
        &self.record
    }

    pub fn with_endpoint_location<T>(
        &self,
        operation: impl FnOnce(&CStr, OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self
            .root
            .with_endpoint_directory(|directory| operation(&self.credential, directory));
        self.validate()?;
        result
    }

    pub fn has_session_directory(&self) -> io::Result<bool> {
        self.validate()?;
        let present = self.root.session_directory()?.is_some();
        self.validate()?;
        Ok(present)
    }

    pub fn with_session_location<T>(
        &self,
        operation: impl FnOnce(&CStr, OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self
            .root
            .with_session_directory(|directory| operation(&self.credential, directory));
        self.validate()?;
        result
    }

    pub(super) fn observe_opening_permit(&self) -> io::Result<OpeningPermitKind> {
        self.with_opening_permit(|_, _, kind| Ok(kind))
    }

    fn with_opening_permit<T>(
        &self,
        operation: impl FnOnce(&Directory, &FileSnapshot, OpeningPermitKind) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self.root.with_opening_directory(|descriptor| {
            let directory = Directory::capture(descriptor.try_clone()?, self.ids, false)?;
            self.admission
                .identity
                .same_filesystem(&directory.identity)?;
            let permit =
                FileSnapshot::capture(&directory, &self.credential, self.ids, OPENING_PERMIT_SIZE)?;
            let kind = decode_opening_permit(&permit.bytes, &self.record_file.bytes)?;
            let named = open_at(directory.fd.as_raw_fd(), &self.credential, false)?;
            permit.validate(named.as_raw_fd(), self.ids)?;
            directory.validate(directory.fd.as_raw_fd(), self.ids)?;
            let result = operation(&directory, &permit, kind);
            directory.validate(directory.fd.as_raw_fd(), self.ids)?;
            result
        });
        self.validate()?;
        result
    }

    /// Reports whether no entry of any kind holds the permit name. A present
    /// entry, valid or not, is never absence.
    pub(super) fn opening_permit_absent(&self) -> io::Result<bool> {
        self.validate()?;
        let result = self.root.with_opening_directory(|descriptor| {
            let directory = Directory::capture(descriptor.try_clone()?, self.ids, false)?;
            self.admission
                .identity
                .same_filesystem(&directory.identity)?;
            let absent = entry_absent(directory.fd.as_raw_fd(), &self.credential)?;
            directory.validate(directory.fd.as_raw_fd(), self.ids)?;
            Ok(absent)
        });
        self.validate()?;
        result
    }

    pub(super) fn consume_opening_permit(&self) -> io::Result<()> {
        self.with_opening_permit(|directory, permit, _| {
            let absent = || match open_at(directory.fd.as_raw_fd(), &self.credential, false) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
                Ok(_) => Err(invalid()),
            };
            self.validate()?;
            let named = open_at(directory.fd.as_raw_fd(), &self.credential, false)?;
            permit.validate(named.as_raw_fd(), self.ids)?;
            // SAFETY: the terminated credential is one validated component below
            // the retained collection and its current file was checked above.
            if unsafe { libc::unlinkat(directory.fd.as_raw_fd(), self.credential.as_ptr(), 0) } < 0
            {
                return Err(io::Error::last_os_error());
            }
            permit.validate_unlinked(self.ids)?;
            absent()?;
            // SAFETY: fsync synchronizes the retained collection after removal.
            if unsafe { libc::fsync(directory.fd.as_raw_fd()) } < 0 {
                return Err(io::Error::last_os_error());
            }
            permit.validate_unlinked(self.ids)?;
            absent()?;
            self.validate()
        })
    }

    pub(super) fn publish_clean_opening_permit(
        &self,
        validate_reservation: impl Fn() -> io::Result<()>,
        authorize_rename: impl FnOnce() -> io::Result<()>,
        published: &mut bool,
    ) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::MetadataExt;

        self.validate()?;
        validate_reservation()?;
        let bytes = opening_permit_frame(OpeningPermitKind::CleanClose, &self.record_file.bytes);
        let result = self.root.with_opening_directory(|descriptor| {
            let directory = Directory::capture(descriptor.try_clone()?, self.ids, false)?;
            self.admission
                .identity
                .same_filesystem(&directory.identity)?;
            validate_reservation()?;
            let mut nonce = [0; 16];
            // SAFETY: getrandom writes only into the bounded initialized nonce.
            let count = unsafe {
                libc::getrandom(nonce.as_mut_ptr().cast(), nonce.len(), libc::GRND_NONBLOCK)
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            require(count as usize == nonce.len())?;
            let mut name = String::from(".pending-opening-");
            for byte in nonce {
                use std::fmt::Write;
                write!(&mut name, "{byte:02x}").map_err(|_| invalid())?;
            }
            let name = CString::new(name).map_err(|_| invalid())?;
            let mut file = create_opening_stage(directory.fd.as_raw_fd(), &name)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            let staged = FileSnapshot::capture(&directory, &name, self.ids, OPENING_PERMIT_SIZE)?;
            let written = file.metadata()?;
            let retained = std::fs::File::from(staged.fd.try_clone()?).metadata()?;
            require(
                written.dev() == retained.dev()
                    && written.ino() == retained.ino()
                    && staged.bytes == bytes,
            )?;
            drop(file);
            self.validate()?;
            validate_reservation()?;
            directory.validate(directory.fd.as_raw_fd(), self.ids)?;
            staged.commit_opening(
                &directory,
                &name,
                &self.credential,
                self.ids,
                authorize_rename,
            )?;
            *published = true;
            let published =
                FileSnapshot::capture(&directory, &self.credential, self.ids, OPENING_PERMIT_SIZE)?;
            require(published.identity == staged.identity && published.bytes == bytes)?;
            validate_reservation()?;
            // SAFETY: fsync synchronizes only the retained collection after publication.
            if unsafe { libc::fsync(directory.fd.as_raw_fd()) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let named = open_at(directory.fd.as_raw_fd(), &self.credential, false)?;
            published.validate(named.as_raw_fd(), self.ids)?;
            directory.validate(directory.fd.as_raw_fd(), self.ids)?;
            self.validate()?;
            validate_reservation()
        });
        self.validate()?;
        validate_reservation()?;
        result
    }

    /// A fresh descriptor for the retained package directory, which anchors
    /// the credential reservation.
    pub(super) fn package_directory(&self) -> io::Result<OwnedFd> {
        self.validate()?;
        // SAFETY: fcntl duplicates the retained package capability atomically.
        let raw = unsafe { libc::fcntl(self.package.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful fcntl returned a fresh owned descriptor.
        let directory = unsafe { OwnedFd::from_raw_fd(raw) };
        descriptor(directory.as_raw_fd())?;
        self.validate()?;
        Ok(directory)
    }

    pub(super) fn native_material(&self) -> io::Result<(OwnedFd, OwnedFd)> {
        let directory = self.package_directory()?;
        let cipher = super::linux_current_admission::seal_ciphertext(&self.key_file.bytes)?;
        self.validate()?;
        Ok((directory, cipher))
    }

    pub fn ipc_identity(&self) -> io::Result<[u64; 6]> {
        self.checked_ipc_identity(|_, identity| Ok(identity))
    }

    pub(super) fn checked_ipc_identity<T>(
        &self,
        operation: impl FnOnce(&Record, [u64; 6]) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let mut identity = [0; 6];
        for (descriptor, words) in [
            self.package.fd.as_raw_fd(),
            self.key_file.fd.as_raw_fd(),
            self.manifest_file.fd.as_raw_fd(),
        ]
        .into_iter()
        .zip(identity.as_chunks_mut::<2>().0)
        {
            let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: fstat queries the retained descriptor and initializes value.
            if unsafe { libc::fstat(descriptor, value.as_mut_ptr()) } < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fstat succeeded.
            let value = unsafe { value.assume_init() };
            words[0] = value.st_dev;
            words[1] = value.st_ino;
        }
        let result = operation(self.record(), identity)?;
        self.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> io::Result<()> {
        require(stable_ids()? == self.ids)?;
        self.root.validate()?;
        for _ in 0..2 {
            self.validate_retained()?;
            self.validate_named()?;
        }
        self.root.validate()?;
        require(stable_ids()? == self.ids)
    }

    fn validate_retained(&self) -> io::Result<()> {
        for directory in [
            &self.admissions,
            &self.packages,
            &self.admission,
            &self.package,
        ] {
            directory.validate(directory.fd.as_raw_fd(), self.ids)?;
        }
        shape(self.admission.fd.as_raw_fd(), &[c"record.json"])?;
        shape(
            self.package.fd.as_raw_fd(),
            &[c"key.openssh", c"manifest.json"],
        )?;
        for file in [&self.record_file, &self.key_file, &self.manifest_file] {
            file.validate(file.fd.as_raw_fd(), self.ids)?;
        }
        self.admission
            .validate(self.admission.fd.as_raw_fd(), self.ids)?;
        self.package.validate(self.package.fd.as_raw_fd(), self.ids)
    }

    fn validate_named(&self) -> io::Result<()> {
        let admissions = self.root.admissions_descriptor()?;
        let packages = self.root.packages_descriptor()?;
        self.admissions.validate(admissions.as_raw_fd(), self.ids)?;
        self.packages.validate(packages.as_raw_fd(), self.ids)?;
        let admission = open_at(admissions.as_raw_fd(), &self.credential, true)?;
        let package = open_at(packages.as_raw_fd(), &self.credential, true)?;
        self.admission.validate(admission.as_raw_fd(), self.ids)?;
        self.package.validate(package.as_raw_fd(), self.ids)?;
        shape(admission.as_raw_fd(), &[c"record.json"])?;
        shape(package.as_raw_fd(), &[c"key.openssh", c"manifest.json"])?;
        for (parent, name, snapshot) in [
            (admission.as_raw_fd(), c"record.json", &self.record_file),
            (package.as_raw_fd(), c"key.openssh", &self.key_file),
            (package.as_raw_fd(), c"manifest.json", &self.manifest_file),
        ] {
            let fd = open_at(parent, name, false)?;
            snapshot.validate(fd.as_raw_fd(), self.ids)?;
        }
        self.admission.validate(admission.as_raw_fd(), self.ids)?;
        self.package.validate(package.as_raw_fd(), self.ids)
    }
}

#[cfg(test)]
mod opening_permit_tests {
    use super::*;

    fn fixture(record: &[u8]) -> [u8; OPENING_PERMIT_SIZE] {
        opening_permit_frame(OpeningPermitKind::FirstOpen, record)
    }

    #[test]
    fn shared_encoder_preserves_the_fixed_storage_format() {
        let frame = opening_permit_frame(OpeningPermitKind::CleanClose, b"");
        assert_eq!(&frame[..16], b"VVCOPEN\0\x01\x02\0\0\0\0\0\0");
        assert_eq!(
            package::hex(&frame[16..]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn accepts_only_known_opening_kinds_for_the_exact_record() {
        let record = b"canonical admission bytes";
        let mut frame = fixture(record);
        assert_eq!(
            decode_opening_permit(&frame, record).unwrap(),
            OpeningPermitKind::FirstOpen
        );
        frame[9] = 2;
        assert_eq!(
            decode_opening_permit(&frame, record).unwrap(),
            OpeningPermitKind::CleanClose
        );
        assert!(decode_opening_permit(&frame, b"different admission bytes").is_err());
    }

    #[test]
    fn refuses_unknown_or_noncanonical_opening_frames() {
        let record = b"canonical admission bytes";
        let frame = fixture(record);
        assert!(decode_opening_permit(&frame[..47], record).is_err());
        let longer = [frame.as_slice(), b"\0"].concat();
        assert!(decode_opening_permit(&longer, record).is_err());
        for (index, value) in [
            (0, 0),
            (7, 1),
            (8, 2),
            (9, 0),
            (9, 3),
            (10, 1),
            (15, 1),
            (47, 0),
        ] {
            let mut damaged = frame;
            damaged[index] = value;
            assert!(decode_opening_permit(&damaged, record).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = include_bytes!("../../tests/data/linux-resident/key.openssh");
    const MANIFEST: &[u8] = include_bytes!("../../tests/data/linux-resident/manifest.json");
    const INSTALLATION: &str = "00000000000000000000000001";

    fn fixture_record() -> Record {
        let manifest = package::validate_bytes(KEY, MANIFEST).unwrap();
        let record = Record {
            version: 2,
            installation_id: INSTALLATION.into(),
            creation_id: "00000000000000000000000002".into(),
            device_id: manifest.device_id,
            credential_id: manifest.credential_id,
            public_key: manifest.public_key,
            ciphertext_sha256: package::hex(&Sha256::digest(KEY)),
            manifest_sha256: package::hex(&Sha256::digest(MANIFEST)),
            ciphertext_len: KEY.len() as u64,
            manifest_len: MANIFEST.len() as u64,
            creation_profile_revision: 1,
            secret_policy_revision: 1,
        };
        Record::parse(&record.canonical_bytes().unwrap()).unwrap()
    }

    #[test]
    #[ignore = "requires an ext4 TMPDIR"]
    fn cancellation_before_rename_removes_only_the_staged_opening() {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

        let path =
            std::env::temp_dir().join(format!("vivac-opening-cancel-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        let result = (|| -> io::Result<_> {
            let metadata = std::fs::metadata(&path)?;
            let ids = (metadata.uid(), metadata.gid());
            let directory = std::fs::File::options()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_NOFOLLOW)
                .open(&path)?;
            let directory = Directory::capture(directory.into(), ids, false)?;
            let pending = c".pending-opening-test";
            let published = c"opening";
            let pending_path = path.join(".pending-opening-test");
            let published_path = path.join("opening");
            let other_path = path.join("unrelated");
            let mut file = create_opening_stage(directory.fd.as_raw_fd(), pending)?;
            let bytes = opening_permit_frame(OpeningPermitKind::CleanClose, b"test record");
            file.write_all(&bytes)?;
            file.sync_all()?;
            let snapshot = FileSnapshot::capture(&directory, pending, ids, OPENING_PERMIT_SIZE)?;
            std::fs::write(&other_path, b"untouched")?;

            let refused = snapshot.commit_opening(&directory, pending, published, ids, || {
                require(std::fs::read(&pending_path)? == bytes && !published_path.try_exists()?)?;
                Err(io::Error::from(io::ErrorKind::Interrupted))
            });
            Ok((
                refused,
                pending_path.try_exists()?,
                published_path.try_exists()?,
                std::fs::read(other_path)?,
            ))
        })();
        std::fs::remove_dir_all(&path).unwrap();
        let (refused, staged_exists, published_exists, unrelated) = result.unwrap();
        assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(!staged_exists);
        assert!(!published_exists);
        assert_eq!(unrelated, b"untouched");
    }

    #[test]
    fn joint_expectation_requires_independent_installation_slot_and_exact_package() {
        let record = fixture_record();
        assert!(
            compare_package(&record, INSTALLATION, &record.credential_id, KEY, MANIFEST).is_ok()
        );
        assert!(compare_package(
            &record,
            "00000000000000000000000003",
            &record.credential_id,
            KEY,
            MANIFEST
        )
        .is_err());
        assert!(compare_package(
            &record,
            INSTALLATION,
            "00000000000000000000000003",
            KEY,
            MANIFEST
        )
        .is_err());
        for field in 0..7 {
            let mut changed = record.clone();
            match field {
                0 => changed.device_id = "00000000000000000000000005".into(),
                1 => changed.credential_id = "00000000000000000000000005".into(),
                2 => changed.public_key = "11".repeat(32),
                3 => changed.ciphertext_sha256 = "11".repeat(32),
                4 => changed.manifest_sha256 = "11".repeat(32),
                5 => changed.ciphertext_len += 1,
                _ => changed.manifest_len += 1,
            }
            assert!(
                compare_package(&changed, INSTALLATION, &record.credential_id, KEY, MANIFEST)
                    .is_err(),
                "accepted mismatch field {field}"
            );
        }
        let mut key = KEY.to_vec();
        key[0] ^= 1;
        assert!(
            compare_package(&record, INSTALLATION, &record.credential_id, &key, MANIFEST).is_err()
        );
        let mut manifest = MANIFEST.to_vec();
        manifest.push(b' ');
        assert!(
            compare_package(&record, INSTALLATION, &record.credential_id, KEY, &manifest).is_err()
        );
    }

    #[test]
    fn credential_selector_accepts_only_canonical_128_bit_identifiers() {
        for value in [
            "00000000000000000000000000",
            "7zzzzzzzzzzzzzzzzzzzzzzzzz",
            "0123456789abcdefghjkmnpqrs",
        ] {
            assert!(identifier(value), "refused canonical selector {value}");
        }
        for value in [
            "",
            ".",
            "..",
            "../00000000000000000000000001",
            "/00000000000000000000000001",
            "0000000000000000000000000",
            "000000000000000000000000000",
            "80000000000000000000000000",
            "0000000000000000000000000i",
            "0000000000000000000000000l",
            "0000000000000000000000000o",
            "0000000000000000000000000u",
            "0000000000000000000000000A",
            "0000000000000000000000000/",
            "0000000000000000000000000\0",
            "0000000000000000000000000é",
        ] {
            assert!(!identifier(value), "accepted unsafe selector {value:?}");
        }
    }

    #[test]
    fn only_a_missing_name_is_an_absent_permit() {
        use std::os::unix::fs::{symlink, DirBuilderExt};

        let path = std::env::temp_dir().join(format!("vivac-permit-absent-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        let result = (|| -> io::Result<()> {
            let directory: OwnedFd = std::fs::File::open(&path)?.into();
            assert!(entry_absent(directory.as_raw_fd(), c"missing")?);
            std::fs::write(path.join("permit"), b"")?;
            assert!(!entry_absent(directory.as_raw_fd(), c"permit")?);
            std::fs::create_dir(path.join("directory"))?;
            assert!(!entry_absent(directory.as_raw_fd(), c"directory")?);
            symlink(path.join("nowhere"), path.join("dangling"))?;
            assert!(!entry_absent(directory.as_raw_fd(), c"dangling")?);
            let file: OwnedFd = std::fs::File::open(path.join("permit"))?.into();
            assert!(entry_absent(file.as_raw_fd(), c"permit").is_err());
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&path);
        result.unwrap();
    }
}
