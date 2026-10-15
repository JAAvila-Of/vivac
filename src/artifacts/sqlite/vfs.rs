use crate::failure::Failure;
use libsqlite3_sys as ffi;
use std::cell::Cell;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs::File;
use std::io;
use std::ptr;
use std::sync::{Mutex, MutexGuard, OnceLock};

static ENGINE: Mutex<()> = Mutex::new(());
const HEAP_LIMIT: i64 = 1024 * 1024;
const FILE_LIMIT: u64 = 10 * 1024 * 1024;
const PENDING_BYTE: u64 = 0x4000_0000;
const RESERVED_BYTE: u64 = PENDING_BYTE + 1;
const SHARED_FIRST: u64 = PENDING_BYTE + 2;
const SHARED_SIZE: u64 = 510;

pub(super) fn engine_guard() -> Result<MutexGuard<'static, ()>, Failure> {
    let guard = ENGINE
        .lock()
        .map_err(|_| Failure::usage("SQLite engine lock failed."))?;
    static CONFIGURED: OnceLock<bool> = OnceLock::new();
    // All fixture builders and application connections share this guard. These
    // options must precede even SQLite's implicit initialization.
    let configured = CONFIGURED.get_or_init(|| unsafe {
        ffi::sqlite3_libversion_number() >= 3_053_002
            && ffi::sqlite3_config(ffi::SQLITE_CONFIG_MEMSTATUS, 1) == ffi::SQLITE_OK
            && ffi::sqlite3_config(
                ffi::SQLITE_CONFIG_PAGECACHE,
                ptr::null_mut::<c_void>(),
                0,
                0,
            ) == ffi::SQLITE_OK
            && ffi::sqlite3_hard_heap_limit64(HEAP_LIMIT) >= 0
            && ffi::sqlite3_memory_used() <= HEAP_LIMIT
    });
    if !configured {
        return Err(refused());
    }
    // The internal trusted API never raises the ceiling between operations.
    unsafe { ffi::sqlite3_hard_heap_limit64(HEAP_LIMIT) };
    Ok(guard)
}

pub(super) struct RegisteredVfs {
    state: Box<State>,
    raw: Box<ffi::sqlite3_vfs>,
    name: CString,
}

struct State {
    source: File,
    target: Option<File>,
    max_bytes: u64,
    source_opened: Cell<bool>,
    target_opened: Cell<bool>,
}

impl RegisteredVfs {
    pub(super) fn new(source: File, target: Option<File>, max_bytes: u64) -> Result<Self, Failure> {
        if max_bytes > FILE_LIMIT
            || !source.metadata()?.is_file()
            || source.metadata()?.len() > max_bytes
            || links(&source)? != 1
        {
            return Err(refused());
        }
        if let Some(target) = &target {
            if !target.metadata()?.is_file()
                || target.metadata()?.len() > max_bytes
                || same_file(&source, target)?
            {
                return Err(refused());
            }
        }
        let name =
            CString::new(format!("vivac-handles-{}", crate::id::ulid())).map_err(|_| refused())?;
        hold_source(&source)?;
        let state = Box::new(State {
            source,
            target,
            max_bytes,
            source_opened: Cell::new(false),
            target_opened: Cell::new(false),
        });
        // Zero is valid for every optional callback and pointer field. SQLite
        // retains these stable boxes until unregistering in Drop.
        let mut raw: Box<ffi::sqlite3_vfs> = Box::new(unsafe { std::mem::zeroed() });
        raw.iVersion = 1;
        raw.szOsFile = std::mem::size_of::<HandleFile>() as c_int;
        raw.mxPathname = 32;
        raw.zName = name.as_ptr();
        raw.pAppData = (&*state as *const State).cast_mut().cast();
        raw.xOpen = Some(open);
        raw.xDelete = Some(delete);
        raw.xAccess = Some(access);
        raw.xFullPathname = Some(full_path);
        raw.xDlOpen = Some(dl_open);
        raw.xDlError = Some(dl_error);
        raw.xRandomness = Some(randomness);
        raw.xSleep = Some(sleep);
        raw.xCurrentTime = Some(current_time);
        if unsafe { ffi::sqlite3_vfs_register(&mut *raw, 0) } != ffi::SQLITE_OK {
            release_source(&state.source);
            return Err(refused());
        }
        Ok(Self { state, raw, name })
    }

    pub(super) fn source(&self) -> &File {
        &self.state.source
    }
    pub(super) fn target(&self) -> Option<&File> {
        self.state.target.as_ref()
    }
    pub(super) fn source_connection(&self) -> Result<Database<'_>, Failure> {
        self.connection(c"source", ffi::SQLITE_OPEN_READONLY)
    }
    pub(super) fn target_connection(&self) -> Result<Database<'_>, Failure> {
        self.connection(
            c"target",
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
        )
    }
    fn connection(&self, name: &CStr, flags: c_int) -> Result<Database<'_>, Failure> {
        let mut raw = ptr::null_mut();
        let result = unsafe {
            ffi::sqlite3_open_v2(
                name.as_ptr(),
                &mut raw,
                flags | ffi::SQLITE_OPEN_NOMUTEX | ffi::SQLITE_OPEN_PRIVATECACHE,
                self.name.as_ptr(),
            )
        };
        if result != ffi::SQLITE_OK {
            if !raw.is_null() {
                unsafe { ffi::sqlite3_close(raw) };
            }
            return Err(refused());
        }
        let database = Database {
            raw,
            _owner: std::marker::PhantomData,
        };
        unsafe {
            ffi::sqlite3_limit(raw, ffi::SQLITE_LIMIT_ATTACHED, 0);
            ffi::sqlite3_busy_timeout(raw, 0);
            if ffi::sqlite3_enable_load_extension(raw, 0) != ffi::SQLITE_OK {
                return Err(refused());
            }
        }
        Ok(database)
    }
}

impl Drop for RegisteredVfs {
    fn drop(&mut self) {
        // Database borrows prevent this point being reached with connections.
        unsafe { ffi::sqlite3_vfs_unregister(&mut *self.raw) };
        release_source(&self.state.source);
    }
}

pub(super) struct Database<'a> {
    raw: *mut ffi::sqlite3,
    _owner: std::marker::PhantomData<&'a RegisteredVfs>,
}

impl Database<'_> {
    pub(super) fn raw(&self) -> *mut ffi::sqlite3 {
        self.raw
    }
}

impl Drop for Database<'_> {
    fn drop(&mut self) {
        // raw() is internal trusted code; discard any remaining statements so
        // closing cannot leave a zombie connection referring to this VFS.
        unsafe {
            loop {
                let statement = ffi::sqlite3_next_stmt(self.raw, ptr::null_mut());
                if statement.is_null() {
                    break;
                }
                ffi::sqlite3_finalize(statement);
            }
            ffi::sqlite3_close(self.raw);
        }
    }
}

fn refused() -> Failure {
    Failure::usage("SQLite handle operation does not satisfy the standalone profile.")
}

#[repr(C)]
struct HandleFile {
    base: ffi::sqlite3_file,
    state: *const State,
    target: bool,
    lock: c_int,
}

impl HandleFile {
    unsafe fn state(&self) -> &State {
        &*self.state
    }
    unsafe fn file(&self) -> Option<&File> {
        if self.target {
            self.state().target.as_ref()
        } else {
            Some(&self.state().source)
        }
    }
}

static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(close),
    xRead: Some(read),
    xWrite: Some(write),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(file_size),
    xLock: Some(lock),
    xUnlock: Some(unlock),
    xCheckReservedLock: Some(check_reserved),
    xFileControl: Some(file_control),
    xSectorSize: Some(sector_size),
    xDeviceCharacteristics: Some(characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};

// SQLite supplies valid objects and buffers to these callbacks. No callback
// opens, duplicates or closes an OS handle, delegates to a native VFS, allocates
// an input-sized Rust collection, or unwraps a fallible result.
unsafe extern "C" fn open(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    (*file).pMethods = ptr::null();
    let allowed = ffi::SQLITE_OPEN_MAIN_DB
        | ffi::SQLITE_OPEN_READONLY
        | ffi::SQLITE_OPEN_READWRITE
        | ffi::SQLITE_OPEN_CREATE
        | ffi::SQLITE_OPEN_NOMUTEX
        | ffi::SQLITE_OPEN_PRIVATECACHE
        | ffi::SQLITE_OPEN_EXRESCODE
        | ffi::SQLITE_OPEN_NOFOLLOW;
    if name.is_null() || flags & !allowed != 0 || flags & ffi::SQLITE_OPEN_MAIN_DB == 0 {
        return ffi::SQLITE_CANTOPEN;
    }
    let state = &*(*vfs).pAppData.cast::<State>();
    let target = match CStr::from_ptr(name).to_bytes() {
        b"source"
            if flags & (ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE) == 0
                && flags & ffi::SQLITE_OPEN_READONLY != 0
                && !state.source_opened.replace(true) =>
        {
            false
        }
        b"target"
            if flags & ffi::SQLITE_OPEN_READWRITE != 0
                && flags & ffi::SQLITE_OPEN_READONLY == 0
                && state.target.is_some()
                && !state.target_opened.replace(true) =>
        {
            true
        }
        _ => return ffi::SQLITE_CANTOPEN,
    };
    ptr::write(
        file.cast::<HandleFile>(),
        HandleFile {
            base: ffi::sqlite3_file { pMethods: &METHODS },
            state,
            target,
            lock: ffi::SQLITE_LOCK_NONE,
        },
    );
    if !out.is_null() {
        *out = flags;
    }
    ffi::SQLITE_OK
}

unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    (*file).pMethods = ptr::null();
    ffi::SQLITE_OK
}

fn bounded(offset: i64, amount: c_int, max: u64) -> bool {
    offset >= 0
        && amount >= 0
        && (offset as u64)
            .checked_add(amount as u64)
            .is_some_and(|end| end <= max)
}

unsafe extern "C" fn read(
    file: *mut ffi::sqlite3_file,
    buffer: *mut c_void,
    amount: c_int,
    offset: i64,
) -> c_int {
    let handle = &*file.cast::<HandleFile>();
    if !bounded(offset, amount, handle.state().max_bytes) {
        return ffi::SQLITE_IOERR_READ;
    }
    if amount == 0 {
        return ffi::SQLITE_OK;
    }
    let bytes = std::slice::from_raw_parts_mut(buffer.cast::<u8>(), amount as usize);
    bytes.fill(0);
    let Some(file) = handle.file() else {
        return ffi::SQLITE_IOERR_READ;
    };
    let mut completed = 0;
    while completed < bytes.len() {
        match read_at(
            file,
            &mut bytes[completed..],
            offset as u64 + completed as u64,
        ) {
            Ok(0) => return ffi::SQLITE_IOERR_SHORT_READ,
            Ok(count) => completed += count,
            Err(_) => return ffi::SQLITE_IOERR_READ,
        }
    }
    ffi::SQLITE_OK
}

unsafe extern "C" fn write(
    file: *mut ffi::sqlite3_file,
    buffer: *const c_void,
    amount: c_int,
    offset: i64,
) -> c_int {
    let handle = &*file.cast::<HandleFile>();
    if !handle.target {
        return ffi::SQLITE_READONLY;
    }
    if !bounded(offset, amount, handle.state().max_bytes) {
        return ffi::SQLITE_FULL;
    }
    if amount == 0 {
        return ffi::SQLITE_OK;
    }
    let Some(file) = handle.file() else {
        return ffi::SQLITE_IOERR_WRITE;
    };
    let bytes = std::slice::from_raw_parts(buffer.cast::<u8>(), amount as usize);
    let mut completed = 0;
    while completed < bytes.len() {
        match write_at(file, &bytes[completed..], offset as u64 + completed as u64) {
            Ok(0) | Err(_) => return ffi::SQLITE_IOERR_WRITE,
            Ok(count) => completed += count,
        }
    }
    ffi::SQLITE_OK
}

unsafe extern "C" fn truncate(file: *mut ffi::sqlite3_file, length: i64) -> c_int {
    let handle = &*file.cast::<HandleFile>();
    if !handle.target {
        return ffi::SQLITE_READONLY;
    }
    if length < 0 || length as u64 > handle.state().max_bytes {
        return ffi::SQLITE_FULL;
    }
    match handle.file().map(|file| file.set_len(length as u64)) {
        Some(Ok(())) => ffi::SQLITE_OK,
        _ => ffi::SQLITE_IOERR_TRUNCATE,
    }
}

unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    let handle = &*file.cast::<HandleFile>();
    if !handle.target {
        return ffi::SQLITE_READONLY;
    }
    match handle.file().map(|file| {
        if flags & ffi::SQLITE_SYNC_DATAONLY != 0 {
            file.sync_data()
        } else {
            file.sync_all()
        }
    }) {
        Some(Ok(())) => ffi::SQLITE_OK,
        _ => ffi::SQLITE_IOERR_FSYNC,
    }
}

unsafe extern "C" fn file_size(file: *mut ffi::sqlite3_file, out: *mut i64) -> c_int {
    let handle = &*file.cast::<HandleFile>();
    match handle.file().map(File::metadata) {
        Some(Ok(metadata)) if metadata.len() <= handle.state().max_bytes => {
            *out = metadata.len() as i64;
            ffi::SQLITE_OK
        }
        _ => ffi::SQLITE_IOERR_FSTAT,
    }
}

unsafe extern "C" fn lock(file: *mut ffi::sqlite3_file, requested: c_int) -> c_int {
    let handle = &mut *file.cast::<HandleFile>();
    if !handle.target && requested > ffi::SQLITE_LOCK_SHARED {
        return ffi::SQLITE_READONLY;
    }
    handle.lock = requested;
    ffi::SQLITE_OK
}

unsafe extern "C" fn unlock(file: *mut ffi::sqlite3_file, requested: c_int) -> c_int {
    let handle = &mut *file.cast::<HandleFile>();
    if !handle.target && requested > ffi::SQLITE_LOCK_SHARED {
        return ffi::SQLITE_READONLY;
    }
    // Source SHARED is an outer OS lease; SQLite cannot release it early.
    handle.lock = requested;
    ffi::SQLITE_OK
}

unsafe extern "C" fn check_reserved(file: *mut ffi::sqlite3_file, out: *mut c_int) -> c_int {
    let handle = &*file.cast::<HandleFile>();
    if handle.target {
        *out = i32::from(handle.lock >= ffi::SQLITE_LOCK_RESERVED);
        return ffi::SQLITE_OK;
    }
    match reserved(&handle.state().source) {
        Ok(held) => {
            *out = i32::from(held);
            ffi::SQLITE_OK
        }
        Err(_) => ffi::SQLITE_IOERR_CHECKRESERVEDLOCK,
    }
}

unsafe extern "C" fn file_control(
    file: *mut ffi::sqlite3_file,
    operation: c_int,
    out: *mut c_void,
) -> c_int {
    if operation == ffi::SQLITE_FCNTL_LOCKSTATE && !out.is_null() {
        let handle = &*file.cast::<HandleFile>();
        *out.cast::<c_int>() = if handle.target {
            handle.lock
        } else {
            ffi::SQLITE_LOCK_SHARED
        };
        return ffi::SQLITE_OK;
    }
    ffi::SQLITE_NOTFOUND
}

unsafe extern "C" fn sector_size(_: *mut ffi::sqlite3_file) -> c_int {
    512
}
unsafe extern "C" fn characteristics(_: *mut ffi::sqlite3_file) -> c_int {
    0
}
unsafe extern "C" fn delete(_: *mut ffi::sqlite3_vfs, _: *const c_char, _: c_int) -> c_int {
    ffi::SQLITE_READONLY
}
unsafe extern "C" fn access(
    _: *mut ffi::sqlite3_vfs,
    _: *const c_char,
    _: c_int,
    out: *mut c_int,
) -> c_int {
    // Auxiliary paths are checked through directory capabilities by the caller.
    *out = 0;
    ffi::SQLITE_OK
}
unsafe extern "C" fn full_path(
    _: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    length: c_int,
    out: *mut c_char,
) -> c_int {
    if name.is_null() {
        return ffi::SQLITE_CANTOPEN;
    }
    let name = CStr::from_ptr(name);
    if !matches!(name.to_bytes(), b"source" | b"target") || length <= name.to_bytes().len() as c_int
    {
        return ffi::SQLITE_CANTOPEN;
    }
    ptr::copy_nonoverlapping(name.as_ptr(), out, name.to_bytes_with_nul().len());
    ffi::SQLITE_OK
}
unsafe extern "C" fn dl_open(_: *mut ffi::sqlite3_vfs, _: *const c_char) -> *mut c_void {
    ptr::null_mut()
}
unsafe extern "C" fn dl_error(_: *mut ffi::sqlite3_vfs, length: c_int, out: *mut c_char) {
    if length > 0 {
        *out = 0;
    }
}
unsafe extern "C" fn randomness(
    _: *mut ffi::sqlite3_vfs,
    length: c_int,
    out: *mut c_char,
) -> c_int {
    if length <= 0 {
        return 0;
    }
    let bytes = std::slice::from_raw_parts_mut(out.cast::<u8>(), length as usize);
    if getrandom::getrandom(bytes).is_ok() {
        length
    } else {
        bytes.fill(0);
        0
    }
}
unsafe extern "C" fn sleep(_: *mut ffi::sqlite3_vfs, microseconds: c_int) -> c_int {
    // Connections have no busy handler. Bound a callback even if trusted code
    // accidentally enables one later.
    let bounded = microseconds.clamp(0, 1_000);
    std::thread::sleep(std::time::Duration::from_micros(bounded as u64));
    bounded
}
unsafe extern "C" fn current_time(_: *mut ffi::sqlite3_vfs, out: *mut f64) -> c_int {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => {
            *out = duration.as_secs_f64() / 86400.0 + 2440587.5;
            ffi::SQLITE_OK
        }
        Err(_) => ffi::SQLITE_ERROR,
    }
}

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buffer, offset)
}
#[cfg(unix)]
fn write_at(file: &File, buffer: &[u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(file, buffer, offset)
}
#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buffer, offset)
}
#[cfg(windows)]
fn write_at(file: &File, buffer: &[u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(file, buffer, offset)
}

#[cfg(unix)]
fn links(file: &File) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.nlink())
}
#[cfg(unix)]
fn same_file(first: &File, second: &File) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let first = first.metadata()?;
    let second = second.metadata()?;
    Ok(first.dev() == second.dev() && first.ino() == second.ino())
}
#[cfg(unix)]
fn range(file: &File, start: u64, length: u64, unlock: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = if unlock {
        libc::F_UNLCK as _
    } else {
        libc::F_RDLCK as _
    };
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = start as _;
    lock.l_len = length as _;
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(unix)]
fn hold_source(file: &File) -> Result<(), Failure> {
    range(file, PENDING_BYTE, 1, false).map_err(|_| refused())?;
    let shared = range(file, SHARED_FIRST, SHARED_SIZE, false);
    let pending = range(file, PENDING_BYTE, 1, true);
    if shared.is_err() || pending.is_err() {
        release_source(file);
        return Err(refused());
    }
    Ok(())
}
#[cfg(unix)]
fn release_source(file: &File) {
    let _ = range(file, SHARED_FIRST, SHARED_SIZE, true);
}
#[cfg(unix)]
fn reserved(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = RESERVED_BYTE as _;
    lock.l_len = 1;
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(lock.l_type != libc::F_UNLCK as libc::c_short)
    }
}

#[cfg(windows)]
fn information(
    file: &File,
) -> io::Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(info)
    }
}
#[cfg(windows)]
fn links(file: &File) -> io::Result<u64> {
    Ok(information(file)?.nNumberOfLinks as u64)
}
#[cfg(windows)]
fn same_file(first: &File, second: &File) -> io::Result<bool> {
    let first = information(first)?;
    let second = information(second)?;
    Ok((
        first.dwVolumeSerialNumber,
        first.nFileIndexHigh,
        first.nFileIndexLow,
    ) == (
        second.dwVolumeSerialNumber,
        second.nFileIndexHigh,
        second.nFileIndexLow,
    ))
}
#[cfg(windows)]
fn range(file: &File, start: u64, length: u64, exclusive: bool, unlock: bool) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, UnlockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};
    let mut overlapped = OVERLAPPED {
        Anonymous: OVERLAPPED_0 {
            Anonymous: OVERLAPPED_0_0 {
                Offset: start as u32,
                OffsetHigh: (start >> 32) as u32,
            },
        },
        ..OVERLAPPED::default()
    };
    let result = unsafe {
        if unlock {
            UnlockFileEx(
                file.as_raw_handle(),
                0,
                length as u32,
                (length >> 32) as u32,
                &mut overlapped,
            )
        } else {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_FAIL_IMMEDIATELY
                    | if exclusive {
                        LOCKFILE_EXCLUSIVE_LOCK
                    } else {
                        0
                    },
                0,
                length as u32,
                (length >> 32) as u32,
                &mut overlapped,
            )
        }
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(windows)]
fn hold_source(file: &File) -> Result<(), Failure> {
    range(file, PENDING_BYTE, 1, false, false).map_err(|_| refused())?;
    let shared = range(file, SHARED_FIRST, SHARED_SIZE, false, false);
    let pending = range(file, PENDING_BYTE, 1, false, true);
    if shared.is_err() || pending.is_err() {
        release_source(file);
        return Err(refused());
    }
    Ok(())
}
#[cfg(windows)]
fn release_source(file: &File) {
    let _ = range(file, SHARED_FIRST, SHARED_SIZE, false, true);
}
#[cfg(windows)]
fn reserved(file: &File) -> io::Result<bool> {
    match range(file, RESERVED_BYTE, 1, true, false) {
        Ok(()) => {
            range(file, RESERVED_BYTE, 1, true, true)?;
            Ok(false)
        }
        Err(error)
            if error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION as i32) =>
        {
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs::{self, OpenOptions};
    use std::path::PathBuf;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("vivac-sqlite-vfs-{}", crate::id::ulid()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> PathBuf {
            self.0.join("source.db")
        }
        fn populate(&self) {
            let name = CString::new(self.path().to_str().unwrap()).unwrap();
            let mut db = std::ptr::null_mut();
            assert_eq!(
                unsafe { ffi::sqlite3_open(name.as_ptr(), &mut db) },
                ffi::SQLITE_OK
            );
            assert_eq!(execute(db, "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT); INSERT INTO items VALUES(1,'kept');"), ffi::SQLITE_OK);
            assert_eq!(unsafe { ffi::sqlite3_close(db) }, ffi::SQLITE_OK);
        }
        fn source(&self) -> File {
            File::open(self.path()).unwrap()
        }
        fn target(&self) -> File {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(self.0.join("target.db"))
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn execute(db: *mut ffi::sqlite3, sql: &str) -> i32 {
        let sql = CString::new(sql).unwrap();
        unsafe {
            ffi::sqlite3_exec(
                db,
                sql.as_ptr(),
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        }
    }

    #[test]
    fn engine_guard_enforces_accounted_c_heap_ceiling() {
        let guard = engine_guard().unwrap();
        let allocation = unsafe { ffi::sqlite3_malloc64(2 * 1024 * 1024) };
        let refused = allocation.is_null();
        if !refused {
            unsafe { ffi::sqlite3_free(allocation) };
        }
        let peak = unsafe { ffi::sqlite3_memory_highwater(0) };
        drop(guard);
        assert!(
            refused,
            "SQLite must refuse allocations beyond the hard heap ceiling"
        );
        assert!(peak <= 1024 * 1024);
    }

    #[test]
    fn held_handles_read_source_and_write_only_private_target() {
        let _guard = engine_guard().unwrap();
        let fixture = Fixture::new();
        fixture.populate();
        let before = fs::read(fixture.path()).unwrap();
        let vfs =
            RegisteredVfs::new(fixture.source(), Some(fixture.target()), 10 * 1024 * 1024).unwrap();
        let source = vfs.source_connection().unwrap();
        assert!(vfs.source_connection().is_err());
        assert_eq!(execute(source.raw(), "SELECT * FROM items"), ffi::SQLITE_OK);
        assert_ne!(
            execute(source.raw(), "INSERT INTO items VALUES(2,'refused')"),
            ffi::SQLITE_OK
        );
        assert_ne!(
            execute(source.raw(), "ATTACH ':memory:' AS extra"),
            ffi::SQLITE_OK
        );
        assert_ne!(
            execute(source.raw(), "SELECT load_extension('unavailable')"),
            ffi::SQLITE_OK
        );
        assert_eq!(
            unsafe { ffi::sqlite3_enable_load_extension(source.raw(), 1) },
            ffi::SQLITE_OK
        );
        assert_ne!(
            execute(source.raw(), "SELECT load_extension('unavailable')"),
            ffi::SQLITE_OK
        );
        assert_eq!(
            unsafe { ffi::sqlite3_enable_load_extension(source.raw(), 0) },
            ffi::SQLITE_OK
        );
        let target = vfs.target_connection().unwrap();
        assert!(vfs.target_connection().is_err());
        assert_eq!(execute(target.raw(), "PRAGMA journal_mode=OFF; CREATE TABLE output(id INTEGER PRIMARY KEY); INSERT INTO output VALUES(7)"), ffi::SQLITE_OK);
        drop(target);
        drop(source);
        drop(vfs);
        assert_eq!(fs::read(fixture.path()).unwrap(), before);
        assert!(fs::metadata(fixture.0.join("target.db")).unwrap().len() > 0);
    }

    #[test]
    fn rejects_unknown_uri_temporary_and_auxiliary_opens() {
        let _guard = engine_guard().unwrap();
        let fixture = Fixture::new();
        fixture.populate();
        let vfs = RegisteredVfs::new(fixture.source(), None, FILE_LIMIT).unwrap();
        let raw = (&*vfs.raw as *const ffi::sqlite3_vfs).cast_mut();
        for (name, flags) in [
            (
                c"source".as_ptr(),
                ffi::SQLITE_OPEN_READONLY | ffi::SQLITE_OPEN_MAIN_DB | ffi::SQLITE_OPEN_URI,
            ),
            (
                c"extra".as_ptr(),
                ffi::SQLITE_OPEN_READONLY | ffi::SQLITE_OPEN_MAIN_DB,
            ),
            (
                c"source-journal".as_ptr(),
                ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_MAIN_JOURNAL,
            ),
            (
                c"source-wal".as_ptr(),
                ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_WAL,
            ),
            (
                ptr::null(),
                ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_TEMP_DB,
            ),
            (
                c"source".as_ptr(),
                ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_MAIN_DB,
            ),
        ] {
            let mut file = std::mem::MaybeUninit::<HandleFile>::zeroed();
            let file = file.as_mut_ptr().cast::<ffi::sqlite3_file>();
            assert_eq!(
                unsafe { (*raw).xOpen.unwrap()(raw, name, file, flags, ptr::null_mut()) },
                ffi::SQLITE_CANTOPEN
            );
            assert!(unsafe { (*file).pMethods.is_null() });
        }
        let mut denied = ptr::null_mut();
        let result = unsafe {
            ffi::sqlite3_open_v2(
                c"file:source?immutable=1".as_ptr(),
                &mut denied,
                ffi::SQLITE_OPEN_READONLY | ffi::SQLITE_OPEN_URI,
                vfs.name.as_ptr(),
            )
        };
        if !denied.is_null() {
            assert_eq!(unsafe { ffi::sqlite3_close(denied) }, ffi::SQLITE_OK);
        }
        assert_ne!(result, ffi::SQLITE_OK);
        let source = vfs.source_connection().unwrap();
        assert_eq!(execute(source.raw(), "SELECT * FROM items"), ffi::SQLITE_OK);
    }

    #[test]
    fn held_source_survives_path_replacement() {
        let _guard = engine_guard().unwrap();
        let fixture = Fixture::new();
        fixture.populate();
        let vfs = RegisteredVfs::new(fixture.source(), None, 10 * 1024 * 1024).unwrap();
        fs::rename(fixture.path(), fixture.0.join("original.db")).unwrap();
        fs::write(fixture.path(), b"replacement is not a database").unwrap();
        let source = vfs.source_connection().unwrap();
        assert_eq!(execute(source.raw(), "SELECT * FROM items"), ffi::SQLITE_OK);
    }

    #[test]
    fn source_callbacks_zero_fill_short_reads_and_refuse_overflow_and_writes() {
        let _guard = engine_guard().unwrap();
        let fixture = Fixture::new();
        fixture.populate();
        let vfs = RegisteredVfs::new(fixture.source(), None, 10 * 1024 * 1024).unwrap();
        let source = vfs.source_connection().unwrap();
        let mut file: *mut ffi::sqlite3_file = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                ffi::sqlite3_file_control(
                    source.raw(),
                    c"main".as_ptr(),
                    ffi::SQLITE_FCNTL_FILE_POINTER,
                    (&mut file as *mut *mut ffi::sqlite3_file).cast(),
                )
            },
            ffi::SQLITE_OK
        );
        let methods = unsafe { &*(*file).pMethods };
        let mut buffer = [0xffu8; 8];
        let length = vfs.source().metadata().unwrap().len() as i64;
        assert_eq!(
            unsafe { methods.xRead.unwrap()(file, buffer.as_mut_ptr().cast(), 8, length - 2) },
            ffi::SQLITE_IOERR_SHORT_READ
        );
        assert_eq!(&buffer[2..], &[0; 6]);
        assert_ne!(
            unsafe { methods.xRead.unwrap()(file, buffer.as_mut_ptr().cast(), 8, i64::MAX) },
            ffi::SQLITE_OK
        );
        assert_eq!(
            unsafe { methods.xWrite.unwrap()(file, buffer.as_ptr().cast(), 8, 0) },
            ffi::SQLITE_READONLY
        );
        assert_eq!(
            unsafe { methods.xTruncate.unwrap()(file, 0) },
            ffi::SQLITE_READONLY
        );
        assert_eq!(
            unsafe { methods.xFileControl.unwrap()(file, i32::MAX, std::ptr::null_mut()) },
            ffi::SQLITE_NOTFOUND
        );
    }

    #[test]
    fn refuses_multiple_source_links() {
        let _guard = engine_guard().unwrap();
        let fixture = Fixture::new();
        fixture.populate();
        fs::hard_link(fixture.path(), fixture.0.join("alias.db")).unwrap();
        assert!(RegisteredVfs::new(fixture.source(), None, 10 * 1024 * 1024).is_err());
    }

    #[test]
    fn target_callbacks_enforce_revision_size_limit() {
        let _guard = engine_guard().unwrap();
        let fixture = Fixture::new();
        fixture.populate();
        let vfs = RegisteredVfs::new(fixture.source(), Some(fixture.target()), 8192).unwrap();
        let target = vfs.target_connection().unwrap();
        let mut file: *mut ffi::sqlite3_file = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                ffi::sqlite3_file_control(
                    target.raw(),
                    c"main".as_ptr(),
                    ffi::SQLITE_FCNTL_FILE_POINTER,
                    (&mut file as *mut *mut ffi::sqlite3_file).cast(),
                )
            },
            ffi::SQLITE_OK
        );
        let methods = unsafe { &*(*file).pMethods };
        let buffer = [0u8; 8];
        assert_eq!(
            unsafe { methods.xWrite.unwrap()(file, buffer.as_ptr().cast(), 8, 8190) },
            ffi::SQLITE_FULL
        );
        assert_eq!(
            unsafe { methods.xWrite.unwrap()(file, buffer.as_ptr().cast(), 8, i64::MAX) },
            ffi::SQLITE_FULL
        );
        assert_eq!(
            unsafe { methods.xTruncate.unwrap()(file, 8193) },
            ffi::SQLITE_FULL
        );
        assert_eq!(vfs.target().unwrap().metadata().unwrap().len(), 0);
    }

    fn native_writer(path: &std::path::Path, expected: i32) {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        let output = command
            .args([
                "--exact",
                "artifacts::sqlite::vfs::tests::native_writer_child",
                "--nocapture",
            ])
            .env("VIVAC_SQLITE_WRITER_PATH", path)
            .env("VIVAC_SQLITE_WRITER_EXPECTED", expected.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "native writer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "native writer helper did not execute"
        );
    }

    #[test]
    fn native_writer_child() {
        let Some(path) = std::env::var_os("VIVAC_SQLITE_WRITER_PATH") else {
            return;
        };
        let _guard = engine_guard().unwrap();
        let name = CString::new(PathBuf::from(path).to_str().unwrap()).unwrap();
        let expected: i32 = std::env::var("VIVAC_SQLITE_WRITER_EXPECTED")
            .unwrap()
            .parse()
            .unwrap();
        let mut db = std::ptr::null_mut();
        assert_eq!(
            unsafe { ffi::sqlite3_open(name.as_ptr(), &mut db) },
            ffi::SQLITE_OK
        );
        assert_eq!(
            execute(
                db,
                "BEGIN IMMEDIATE; UPDATE items SET value='writer' WHERE id=1"
            ),
            ffi::SQLITE_OK
        );
        assert_eq!(execute(db, "COMMIT"), expected);
        if expected != ffi::SQLITE_OK {
            assert_eq!(execute(db, "ROLLBACK"), ffi::SQLITE_OK);
        }
        assert_eq!(unsafe { ffi::sqlite3_close(db) }, ffi::SQLITE_OK);
    }

    #[test]
    fn native_writer_commit_waits_for_outer_lease_even_after_connection_close() {
        let _guard = engine_guard().unwrap();
        let fixture = Fixture::new();
        fixture.populate();
        let vfs = RegisteredVfs::new(fixture.source(), None, 10 * 1024 * 1024).unwrap();
        let source = vfs.source_connection().unwrap();
        assert_eq!(execute(source.raw(), "SELECT * FROM items"), ffi::SQLITE_OK);
        drop(source);
        native_writer(&fixture.path(), ffi::SQLITE_BUSY);
        drop(vfs);
        native_writer(&fixture.path(), ffi::SQLITE_OK);
    }
}
