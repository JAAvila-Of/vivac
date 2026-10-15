use crate::failure::Failure;
use libsqlite3_sys as ffi;
use std::fs::File;
use std::sync::{Mutex, MutexGuard};

static ENGINE: Mutex<()> = Mutex::new(());

pub(super) fn engine_guard() -> Result<MutexGuard<'static, ()>, Failure> {
    ENGINE
        .lock()
        .map_err(|_| Failure::usage("SQLite engine lock failed."))
}

pub(super) struct RegisteredVfs {
    source: File,
    target: Option<File>,
}

impl RegisteredVfs {
    pub(super) fn new(source: File, target: Option<File>, _: u64) -> Result<Self, Failure> {
        Ok(Self { source, target })
    }

    pub(super) fn source(&self) -> &File {
        &self.source
    }
    pub(super) fn target(&self) -> Option<&File> {
        self.target.as_ref()
    }
    pub(super) fn source_connection(&self) -> Result<Database<'_>, Failure> {
        Err(Failure::usage("SQLite handle backend unavailable."))
    }
    pub(super) fn target_connection(&self) -> Result<Database<'_>, Failure> {
        Err(Failure::usage("SQLite handle backend unavailable."))
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
