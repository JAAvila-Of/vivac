use std::ffi::{c_char, c_int, CStr, CString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr;
use std::sync::OnceLock;

use super::{sql, StoreError, StoreResult, DATABASE, MAX_DATABASE, MAX_JOURNAL};

pub(super) type Root = OwnedFd;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
}

fn identity(stat: &libc::stat) -> Identity {
    Identity {
        device: stat.st_dev,
        inode: stat.st_ino,
    }
}

fn stat_fd(fd: RawFd) -> StoreResult<libc::stat> {
    let mut stat = std::mem::MaybeUninit::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(StoreError::Refused);
    }
    Ok(unsafe { stat.assume_init() })
}

fn owner(stat: &libc::stat) -> bool {
    unsafe {
        libc::getuid() == libc::geteuid()
            && libc::getgid() == libc::getegid()
            && stat.st_uid == libc::geteuid()
            && stat.st_gid == libc::getegid()
    }
}

pub(super) fn validate_root(root: &Root) -> StoreResult<Identity> {
    let fd = root.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    let stat = stat_fd(fd)?;
    let mut filesystem = std::mem::MaybeUninit::uninit();
    if fd < 3
        || flags < 0
        || flags & libc::O_ACCMODE != libc::O_RDONLY
        || flags & libc::O_PATH != 0
        || descriptor_flags < 0
        || descriptor_flags & libc::FD_CLOEXEC == 0
        || stat.st_mode & libc::S_IFMT != libc::S_IFDIR
        || stat.st_mode & 0o7777 != 0o700
        || !owner(&stat)
        || unsafe { libc::fstatfs(fd, filesystem.as_mut_ptr()) } != 0
    {
        return Err(StoreError::Refused);
    }
    if unsafe { filesystem.assume_init() }.f_type != 0xEF53 {
        return Err(StoreError::Refused);
    }
    for name in [c"system.posix_acl_access", c"system.posix_acl_default"] {
        let result = unsafe { libc::fgetxattr(fd, name.as_ptr(), ptr::null_mut(), 0) };
        if result >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ENODATA) {
            return Err(StoreError::Refused);
        }
    }
    Ok(identity(&stat))
}

fn validate_named(
    root: &Root,
    name: &CStr,
    maximum: i64,
    nonempty: bool,
) -> StoreResult<libc::stat> {
    let mut stat = std::mem::MaybeUninit::uninit();
    if unsafe {
        libc::fstatat(
            root.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(StoreError::Refused);
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG
        || stat.st_mode & 0o7777 != 0o600
        || !owner(&stat)
        || stat.st_nlink != 1
        || stat.st_size < i64::from(nonempty)
        || stat.st_size > maximum
    {
        return Err(StoreError::Refused);
    }
    let path = CString::new(format!(
        "/proc/self/fd/{}/{}",
        root.as_raw_fd(),
        name.to_str().map_err(|_| StoreError::Refused)?
    ))
    .map_err(|_| StoreError::Refused)?;
    let result = unsafe {
        libc::lgetxattr(
            path.as_ptr(),
            c"system.posix_acl_access".as_ptr(),
            ptr::null_mut(),
            0,
        )
    };
    if result >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ENODATA) {
        return Err(StoreError::Refused);
    }
    Ok(stat)
}

pub(super) fn validate_database(root: &Root, nonempty: bool) -> StoreResult<Identity> {
    validate_named(root, DATABASE, MAX_DATABASE, nonempty).map(|stat| identity(&stat))
}

pub(super) fn validate_shape(root: &Root, database_required: bool) -> StoreResult<()> {
    let path = format!("/proc/self/fd/{}", root.as_raw_fd());
    let mut found = false;
    for (index, entry) in std::fs::read_dir(path)
        .map_err(|_| StoreError::Refused)?
        .enumerate()
    {
        if index >= 2 {
            return Err(StoreError::Refused);
        }
        let entry = entry.map_err(|_| StoreError::Refused)?;
        match entry.file_name().as_encoded_bytes() {
            b"authority.sqlite3" => {
                validate_database(root, false)?;
                found = true;
            }
            b"authority.sqlite3-journal" if database_required => {
                validate_named(root, c"authority.sqlite3-journal", MAX_JOURNAL, false)?;
            }
            _ => return Err(StoreError::Refused),
        }
    }
    if found != database_required {
        return Err(StoreError::Refused);
    }
    Ok(())
}

pub(super) fn create_database_file(root: &Root) -> StoreResult<()> {
    let descriptor = unsafe {
        libc::openat(
            root.as_raw_fd(),
            DATABASE.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(StoreError::Refused);
    }
    // Closing any auxiliary descriptor would release other connections'
    // POSIX locks. The creation descriptor is closed before SQLite opens.
    let created = unsafe { OwnedFd::from_raw_fd(descriptor) };
    if unsafe { libc::fsync(created.as_raw_fd()) } != 0 {
        return Err(StoreError::Unpublished);
    }
    drop(created);
    Ok(())
}

pub(super) fn sync_root(root: &Root) -> bool {
    unsafe { libc::fsync(root.as_raw_fd()) == 0 }
}

pub(super) fn open_database(root: &Root) -> StoreResult<*mut sql::sqlite3> {
    register_vfs()?;
    let path = CString::new(format!(
        "/proc/self/fd/{}/authority.sqlite3",
        root.as_raw_fd()
    ))
    .map_err(|_| StoreError::Refused)?;
    let mut raw = ptr::null_mut();
    let status = unsafe {
        sql::sqlite3_open_v2(
            path.as_ptr(),
            &mut raw,
            sql::SQLITE_OPEN_READWRITE | sql::SQLITE_OPEN_FULLMUTEX | sql::SQLITE_OPEN_NOFOLLOW,
            c"vivac-authority-procfd-v1".as_ptr(),
        )
    };
    if status != sql::SQLITE_OK {
        if !raw.is_null() {
            unsafe { sql::sqlite3_close(raw) };
        }
        return Err(StoreError::Unavailable);
    }
    Ok(raw)
}

fn register_vfs() -> StoreResult<()> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    if !REGISTERED.get_or_init(|| unsafe {
        let original = sql::sqlite3_vfs_find(c"unix".as_ptr());
        if original.is_null() {
            return false;
        }
        // Every maintained Unix I/O callback remains intact. Path expansion
        // alone preserves our validated, descriptor-rooted namespace.
        let mut wrapper = Box::new(ptr::read(original));
        wrapper.zName = c"vivac-authority-procfd-v1".as_ptr();
        wrapper.pNext = ptr::null_mut();
        wrapper.xFullPathname = Some(full_path);
        let wrapper = Box::into_raw(wrapper);
        if sql::sqlite3_vfs_register(wrapper, 0) != sql::SQLITE_OK {
            drop(Box::from_raw(wrapper));
            return false;
        }
        true
    }) {
        return Err(StoreError::Unavailable);
    }
    Ok(())
}

unsafe extern "C" fn full_path(
    _vfs: *mut sql::sqlite3_vfs,
    input: *const c_char,
    capacity: c_int,
    output: *mut c_char,
) -> c_int {
    if input.is_null() || output.is_null() || capacity <= 0 {
        return sql::SQLITE_CANTOPEN;
    }
    let bytes = unsafe { CStr::from_ptr(input) }.to_bytes();
    let Some(tail) = bytes.strip_prefix(b"/proc/self/fd/") else {
        return sql::SQLITE_CANTOPEN;
    };
    let Some(slash) = tail.iter().position(|byte| *byte == b'/') else {
        return sql::SQLITE_CANTOPEN;
    };
    let descriptor = &tail[..slash];
    let name = &tail[slash + 1..];
    if descriptor.is_empty()
        || descriptor.len() > 10
        || descriptor[0] == b'0'
        || !descriptor.iter().all(u8::is_ascii_digit)
        || !matches!(name, b"authority.sqlite3" | b"authority.sqlite3-journal")
    {
        return sql::SQLITE_CANTOPEN;
    }
    let Ok(descriptor) = std::str::from_utf8(descriptor).unwrap_or("").parse::<i32>() else {
        return sql::SQLITE_CANTOPEN;
    };
    if descriptor < 3 || bytes.len() + 1 > capacity as usize {
        return sql::SQLITE_CANTOPEN;
    }
    unsafe { ptr::copy_nonoverlapping(input, output, bytes.len() + 1) };
    sql::SQLITE_OK
}
