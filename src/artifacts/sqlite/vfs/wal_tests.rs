use super::*;
use std::fs;

#[test]
fn accepts_distinct_readonly_wal_handles() {
    let _guard = engine_guard().unwrap();
    let root = std::env::temp_dir().join(format!("vivac-wal-{}", crate::id::ulid()));
    fs::create_dir(&root).unwrap();
    for name in ["db", "wal", "shm"] {
        fs::write(root.join(name), []).unwrap();
    }
    let result = RegisteredVfs::new_wal(
        File::open(root.join("db")).unwrap(),
        File::open(root.join("wal")).unwrap(),
        File::open(root.join("shm")).unwrap(),
        None,
        FILE_LIMIT,
    );
    let accepted = result.is_ok();
    drop(result);
    fs::remove_dir_all(root).unwrap();
    assert!(
        accepted,
        "distinct readonly handles must acquire the WAL lease"
    );
}

use super::tests::execute;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub(in crate::artifacts) struct Writer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    root: std::path::PathBuf,
}
impl Writer {
    pub(in crate::artifacts) fn start() -> Self {
        Self::start_with_sql("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items VALUES(1); PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO items VALUES(2)")
    }
    pub(in crate::artifacts) fn start_with_sql(sql: &str) -> Self {
        let root = std::env::temp_dir().join(format!("vivac-wal-writer-{}", crate::id::ulid()));
        fs::create_dir(&root).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "artifacts::sqlite::vfs::wal_tests::wal_writer_child",
                "--nocapture",
            ])
            .env("VIVAC_WAL_WRITER_ROOT", &root)
            .env("VIVAC_WAL_FIXTURE_SQL", sql)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut writer = Self {
            child,
            input,
            output,
            root,
        };
        assert_eq!(writer.response(), "ready");
        writer
    }
    fn response(&mut self) -> String {
        loop {
            let mut line = String::new();
            assert_ne!(
                self.output.read_line(&mut line).unwrap(),
                0,
                "writer exited before responding"
            );
            if let Some(message) = line.trim().strip_prefix("WAL-RESULT ") {
                return message.to_owned();
            }
        }
    }
    pub(in crate::artifacts) fn command(&mut self, command: &str) -> i32 {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
        self.response().parse().unwrap()
    }
    pub(in crate::artifacts) fn path(&self, suffix: &str) -> std::path::PathBuf {
        self.root.join(format!("source.db{suffix}"))
    }
    fn exit_without_close(&mut self) {
        writeln!(self.input, "exit-without-close").unwrap();
        self.input.flush().unwrap();
        assert!(self.child.wait().unwrap().success());
    }

    fn vfs(&self) -> Result<RegisteredVfs, Failure> {
        RegisteredVfs::new_wal(
            File::open(self.path("")).unwrap(),
            File::open(self.path("-wal")).unwrap(),
            File::open(self.path("-shm")).unwrap(),
            None,
            FILE_LIMIT,
        )
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "exit");
        let _ = self.input.flush();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn row_count(db: *mut ffi::sqlite3) -> i32 {
    let mut statement = ptr::null_mut();
    assert_eq!(
        unsafe {
            ffi::sqlite3_prepare_v2(
                db,
                c"SELECT count(*) FROM items".as_ptr(),
                -1,
                &mut statement,
                ptr::null_mut(),
            )
        },
        ffi::SQLITE_OK,
        "{}",
        unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(db)) }.to_string_lossy()
    );
    assert_eq!(unsafe { ffi::sqlite3_step(statement) }, ffi::SQLITE_ROW);
    let result = unsafe { ffi::sqlite3_column_int(statement, 0) };
    assert_eq!(unsafe { ffi::sqlite3_finalize(statement) }, ffi::SQLITE_OK);
    result
}

#[test]
fn wal_writer_child() {
    let Some(root) = std::env::var_os("VIVAC_WAL_WRITER_ROOT") else {
        return;
    };
    let _guard = engine_guard().unwrap();
    let path = std::path::PathBuf::from(root).join("source.db");
    let name = CString::new(path.to_str().unwrap()).unwrap();
    let mut db = ptr::null_mut();
    assert_eq!(
        unsafe { ffi::sqlite3_open(name.as_ptr(), &mut db) },
        ffi::SQLITE_OK
    );
    let sql = std::env::var("VIVAC_WAL_FIXTURE_SQL").unwrap();
    assert_eq!(execute(db, &sql), ffi::SQLITE_OK);
    println!("WAL-RESULT ready");
    std::io::stdout().flush().unwrap();
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let result = match line.as_str() {
            "write" => execute(db, "INSERT INTO items VALUES(3)"),
            "begin" => execute(db, "BEGIN IMMEDIATE"),
            "rollback" => execute(db, "ROLLBACK"),
            "checkpoint" => unsafe {
                ffi::sqlite3_wal_checkpoint_v2(
                    db,
                    ptr::null(),
                    ffi::SQLITE_CHECKPOINT_FULL,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            },
            "truncate" => unsafe {
                ffi::sqlite3_wal_checkpoint_v2(
                    db,
                    ptr::null(),
                    ffi::SQLITE_CHECKPOINT_TRUNCATE,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            },
            "exit-without-close" => std::process::exit(0),
            "exit" => break,
            _ => panic!("unknown writer command"),
        };
        println!("WAL-RESULT {result}");
        std::io::stdout().flush().unwrap();
    }
    assert_eq!(unsafe { ffi::sqlite3_close(db) }, ffi::SQLITE_OK);
}

fn bytes(file: &File) -> Vec<u8> {
    let mut bytes = vec![0; file.metadata().unwrap().len() as usize];
    assert_eq!(read_at(file, &mut bytes, 0).unwrap(), bytes.len());
    bytes
}

#[test]
fn live_wal_commit_is_visible_under_persistent_outer_leases() {
    let _guard = engine_guard().unwrap();
    let mut writer = Writer::start();
    let before: Vec<_> = ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| fs::read(writer.path(suffix)).unwrap())
        .collect();
    // An isolated copy of the main database contains only the checkpointed row.
    let copy = writer.root.join("checkpointed.db");
    fs::write(&copy, &before[0]).unwrap();
    let name = CString::new(copy.to_str().unwrap()).unwrap();
    let mut db = ptr::null_mut();
    assert_eq!(
        unsafe { ffi::sqlite3_open(name.as_ptr(), &mut db) },
        ffi::SQLITE_OK
    );
    assert_eq!(row_count(db), 1);
    assert_eq!(unsafe { ffi::sqlite3_close(db) }, ffi::SQLITE_OK);
    let vfs = writer.vfs().unwrap();
    let source = vfs.source_connection().unwrap();
    assert_eq!(row_count(source.raw()), 2);
    assert_ne!(
        execute(source.raw(), "INSERT INTO items VALUES(99)"),
        ffi::SQLITE_OK
    );
    assert_ne!(
        execute(source.raw(), "ATTACH ':memory:' AS extra"),
        ffi::SQLITE_OK
    );
    drop(source);
    let wal = vfs.state.wal.as_ref().unwrap();
    assert_eq!(bytes(&vfs.state.source), before[0]);
    assert_eq!(bytes(&wal.wal), before[1]);
    assert_eq!(bytes(&wal.shm), before[2]);
    assert_eq!(writer.command("write"), ffi::SQLITE_BUSY);
    assert_eq!(writer.command("checkpoint"), ffi::SQLITE_BUSY);
    drop(vfs);
    assert_eq!(writer.command("write"), ffi::SQLITE_OK);
    assert_eq!(writer.command("checkpoint"), ffi::SQLITE_OK);
}

#[test]
fn active_writer_refuses_lease_and_failed_acquisition_releases_db() {
    let _guard = engine_guard().unwrap();
    let mut writer = Writer::start();
    assert_eq!(writer.command("begin"), ffi::SQLITE_OK);
    assert!(writer.vfs().is_err());
    assert_eq!(writer.command("rollback"), ffi::SQLITE_OK);
    assert_eq!(writer.command("checkpoint"), ffi::SQLITE_OK);
    let vfs = writer.vfs().unwrap();
    assert_eq!(writer.command("write"), ffi::SQLITE_BUSY);
    drop(vfs);
    assert_eq!(writer.command("write"), ffi::SQLITE_OK);
}

#[test]
fn wal_callbacks_refuse_mutation_names_and_exclusive_locks() {
    let _guard = engine_guard().unwrap();
    let writer = Writer::start();
    let vfs = writer.vfs().unwrap();
    let raw = (&*vfs.raw as *const ffi::sqlite3_vfs).cast_mut();
    let mut main = std::mem::MaybeUninit::<HandleFile>::zeroed();
    let main = main.as_mut_ptr().cast::<ffi::sqlite3_file>();
    assert_eq!(
        unsafe {
            open(
                raw,
                c"source".as_ptr(),
                main,
                ffi::SQLITE_OPEN_MAIN_DB | ffi::SQLITE_OPEN_READONLY,
                ptr::null_mut(),
            )
        },
        ffi::SQLITE_OK
    );
    let methods = unsafe { &*(*main).pMethods };
    assert_eq!(methods.iVersion, 2);
    for page in [0, 1] {
        for extend in [0, 1] {
            let mut mapped = std::ptr::dangling_mut::<c_void>();
            assert_eq!(
                unsafe { methods.xShmMap.unwrap()(main, page, 32768, extend, &mut mapped) },
                ffi::SQLITE_READONLY_CANTINIT
            );
            assert!(mapped.is_null());
        }
    }
    for offset in 0..8 {
        for action in [ffi::SQLITE_SHM_LOCK, ffi::SQLITE_SHM_UNLOCK] {
            assert_eq!(
                unsafe {
                    methods.xShmLock.unwrap()(main, offset, 1, action | ffi::SQLITE_SHM_SHARED)
                },
                ffi::SQLITE_OK
            );
            assert_eq!(
                unsafe {
                    methods.xShmLock.unwrap()(main, offset, 1, action | ffi::SQLITE_SHM_EXCLUSIVE)
                },
                ffi::SQLITE_BUSY
            );
        }
    }
    for (offset, count, flags) in [
        (-1, 1, 6),
        (0, 0, 6),
        (7, 2, 6),
        (8, 1, 6),
        (0, i32::MAX, 6),
        (0, 1, 0),
        (0, 1, ffi::SQLITE_SHM_SHARED),
        (
            0,
            1,
            ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_UNLOCK | ffi::SQLITE_SHM_SHARED,
        ),
        (
            0,
            1,
            ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_SHARED | ffi::SQLITE_SHM_EXCLUSIVE,
        ),
    ] {
        assert_eq!(
            unsafe { methods.xShmLock.unwrap()(main, offset, count, flags) },
            ffi::SQLITE_IOERR_SHMLOCK
        );
    }
    assert_eq!(
        unsafe { methods.xLock.unwrap()(main, ffi::SQLITE_LOCK_EXCLUSIVE) },
        ffi::SQLITE_READONLY
    );
    assert_eq!(
        unsafe { methods.xLock.unwrap()(main, ffi::SQLITE_LOCK_SHARED) },
        ffi::SQLITE_OK
    );
    unsafe { methods.xShmBarrier.unwrap()(main) };
    assert_eq!(
        unsafe { methods.xShmUnmap.unwrap()(main, 1) },
        ffi::SQLITE_OK
    );
    let wal_flags = ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE | ffi::SQLITE_OPEN_WAL;
    for (name, flags) in [
        (
            c"source-wal".as_ptr(),
            ffi::SQLITE_OPEN_READONLY | ffi::SQLITE_OPEN_WAL,
        ),
        (
            c"source-wal".as_ptr(),
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_WAL,
        ),
        (c"source-wal".as_ptr(), wal_flags | ffi::SQLITE_OPEN_URI),
        (c"source-wal".as_ptr(), wal_flags | ffi::SQLITE_OPEN_MAIN_DB),
        (c"source-shm".as_ptr(), wal_flags),
        (c"source-journal".as_ptr(), wal_flags),
        (
            c"extra".as_ptr(),
            ffi::SQLITE_OPEN_MAIN_DB | ffi::SQLITE_OPEN_READONLY,
        ),
        (
            ptr::null(),
            ffi::SQLITE_OPEN_TEMP_DB | ffi::SQLITE_OPEN_READWRITE,
        ),
    ] {
        let mut denied = std::mem::MaybeUninit::<HandleFile>::zeroed();
        let denied = denied.as_mut_ptr().cast::<ffi::sqlite3_file>();
        assert_eq!(
            unsafe { open(raw, name, denied, flags, ptr::null_mut()) },
            ffi::SQLITE_CANTOPEN
        );
        assert!(unsafe { (*denied).pMethods.is_null() });
    }
    let mut wal = std::mem::MaybeUninit::<HandleFile>::zeroed();
    let wal = wal.as_mut_ptr().cast::<ffi::sqlite3_file>();
    let mut out_flags = 0;
    assert_eq!(
        unsafe { open(raw, c"source-wal".as_ptr(), wal, wal_flags, &mut out_flags) },
        ffi::SQLITE_OK
    );
    assert_eq!(out_flags, ffi::SQLITE_OPEN_READONLY | ffi::SQLITE_OPEN_WAL);
    let wal_methods = unsafe { &*(*wal).pMethods };
    let mut buffer = [0u8; 4];
    assert_eq!(
        unsafe { wal_methods.xRead.unwrap()(wal, buffer.as_mut_ptr().cast(), 4, 0) },
        ffi::SQLITE_OK
    );
    assert!(matches!(buffer, [0x37, 0x7f, 0x06, 0x82 | 0x83]));
    assert_eq!(
        unsafe { wal_methods.xWrite.unwrap()(wal, buffer.as_ptr().cast(), 4, 0) },
        ffi::SQLITE_READONLY
    );
    assert_eq!(
        unsafe { wal_methods.xTruncate.unwrap()(wal, 0) },
        ffi::SQLITE_READONLY
    );
    assert_eq!(
        unsafe { wal_methods.xSync.unwrap()(wal, 0) },
        ffi::SQLITE_READONLY
    );
    assert_eq!(
        unsafe { delete(raw, c"source-wal".as_ptr(), 1) },
        ffi::SQLITE_READONLY
    );
    assert_eq!(unsafe { wal_methods.xClose.unwrap()(wal) }, ffi::SQLITE_OK);
    assert_eq!(unsafe { methods.xClose.unwrap()(main) }, ffi::SQLITE_OK);
}

#[test]
fn wal_constructor_refuses_aliases_links_and_combined_size_over_budget() {
    let _guard = engine_guard().unwrap();
    let root = std::env::temp_dir().join(format!("vivac-wal-validation-{}", crate::id::ulid()));
    fs::create_dir(&root).unwrap();
    for name in ["db", "wal", "shm", "target"] {
        fs::write(root.join(name), [0u8; 4]).unwrap();
    }
    let attempt = |db: &str, wal: &str, shm: &str, target: Option<&str>, max| {
        RegisteredVfs::new_wal(
            File::open(root.join(db)).unwrap(),
            File::open(root.join(wal)).unwrap(),
            File::open(root.join(shm)).unwrap(),
            target.map(|name| File::open(root.join(name)).unwrap()),
            max,
        )
    };
    for (db, wal, shm, target) in [
        ("db", "db", "shm", None),
        ("db", "wal", "db", None),
        ("db", "wal", "wal", None),
        ("db", "wal", "shm", Some("wal")),
        ("db", "wal", "shm", Some("shm")),
        ("db", "wal", "shm", Some("db")),
    ] {
        assert!(attempt(db, wal, shm, target, 8).is_err());
    }
    assert!(attempt("db", "wal", "shm", None, 7).is_err());
    assert!(attempt("db", "wal", "shm", None, FILE_LIMIT + 1).is_err());
    drop(attempt("db", "wal", "shm", None, 8).unwrap());
    fs::write(root.join("shm"), [0u8; 9]).unwrap();
    assert!(attempt("db", "wal", "shm", None, 8).is_err());
    fs::write(root.join("shm"), []).unwrap();
    for name in ["db", "wal", "shm", "target"] {
        fs::hard_link(root.join(name), root.join("alias")).unwrap();
        assert!(attempt("db", "wal", "shm", Some("target"), 8).is_err());
        fs::remove_file(root.join("alias")).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

fn native_recovery_reader(path: &std::path::Path) -> io::Result<std::process::Output> {
    Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "artifacts::sqlite::vfs::wal_tests::native_recovery_reader_child",
            "--nocapture",
        ])
        .env("VIVAC_WAL_RECOVERY_PATH", path)
        .output()
}

fn observed_row_count(db: *mut ffi::sqlite3) -> (i32, Option<i64>) {
    let mut statement = ptr::null_mut();
    let prepared = unsafe {
        ffi::sqlite3_prepare_v2(
            db,
            c"SELECT count(*) FROM items".as_ptr(),
            -1,
            &mut statement,
            ptr::null_mut(),
        )
    };
    if prepared != ffi::SQLITE_OK {
        if !statement.is_null() {
            unsafe {
                ffi::sqlite3_finalize(statement);
            }
        }
        return (prepared, None);
    }
    let step = unsafe { ffi::sqlite3_step(statement) };
    let rows = if step == ffi::SQLITE_ROW {
        Some(unsafe { ffi::sqlite3_column_int64(statement, 0) })
    } else {
        None
    };
    unsafe {
        ffi::sqlite3_finalize(statement);
    }
    (step, rows)
}

#[test]
fn native_recovery_reader_child() {
    let Some(path) = std::env::var_os("VIVAC_WAL_RECOVERY_PATH") else {
        return;
    };
    let _engine = engine_guard().unwrap();
    let name = CString::new(std::path::PathBuf::from(path).to_str().unwrap()).unwrap();
    let mut db = ptr::null_mut();
    let opened = unsafe { ffi::sqlite3_open(name.as_ptr(), &mut db) };
    let selected = if opened == ffi::SQLITE_OK {
        execute(db, "SELECT * FROM items")
    } else {
        -1
    };
    let (step, rows) = if selected == ffi::SQLITE_OK {
        observed_row_count(db)
    } else {
        (-1, None)
    };
    let closed = if db.is_null() {
        ffi::SQLITE_OK
    } else {
        unsafe { ffi::sqlite3_close(db) }
    };
    // Observe every return code and close even when the SELECT is refused.
    println!(
        "WAL-RECOVERY {opened} {selected} {step} {} {closed}",
        rows.unwrap_or(-1)
    );
}

fn recovery_codes(output: io::Result<std::process::Output>) -> [i32; 5] {
    let output = output.unwrap();
    assert!(
        output.status.success(),
        "native recovery reader failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 passed"),
        "native recovery reader helper did not execute"
    );
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix("WAL-RECOVERY "))
        .unwrap();
    let codes: Vec<i32> = line
        .split_whitespace()
        .map(|code| code.parse().unwrap())
        .collect();
    codes.try_into().unwrap()
}

fn observed_bytes(file: &File) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0; file.metadata()?.len() as usize];
    let mut offset = 0;
    while offset < buffer.len() {
        let read = read_at(file, &mut buffer[offset..], offset as u64)?;
        if read == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        offset += read;
    }
    Ok(buffer)
}

#[test]
fn native_shm_initialization_waits_for_outer_lease_and_preserves_inputs() {
    let engine = engine_guard().unwrap();
    let mut writer = Writer::start();
    writer.exit_without_close();
    // An existing but empty SHM requires the native VFS to initialize its index.
    fs::write(writer.path("-shm"), []).unwrap();
    let before: Vec<_> = ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| fs::read(writer.path(suffix)).unwrap())
        .collect();
    let vfs = writer.vfs().unwrap();
    let custom = vfs
        .source_connection()
        .map(|source| observed_row_count(source.raw()));
    let during = native_recovery_reader(&writer.path(""));
    let wal = vfs.state.wal.as_ref().unwrap();
    // Inspect all three held handles before checking any child return code.
    let unchanged: Vec<_> = [
        ("DB", &vfs.state.source, &before[0]),
        ("WAL", &wal.wal, &before[1]),
        ("SHM", &wal.shm, &before[2]),
    ]
    .into_iter()
    .map(|(name, file, previous)| {
        (
            name,
            previous.len(),
            observed_bytes(file).map(|after| (after.len(), after == *previous)),
        )
    })
    .collect();
    drop(vfs);
    let after = native_recovery_reader(&writer.path(""));
    drop(engine);
    // Failures after this point cannot poison the process-wide engine mutex.
    let custom = custom.unwrap();
    assert_eq!(custom, (ffi::SQLITE_ROW, Some(2)));
    let mut differences = Vec::new();
    for (name, previous, observed) in unchanged {
        match observed {
            Ok((length, true)) if length == previous => {}
            Ok((length, _)) => differences.push(format!(
                "{name}: before {previous} bytes, after {length} bytes, content changed"
            )),
            Err(error) => differences.push(format!("{name}: observation failed: {error}")),
        }
    }
    assert!(
        differences.is_empty(),
        "Native initialization changed inputs under lease: {}",
        differences.join("; ")
    );
    let [opened, selected, _, _, closed] = recovery_codes(during);
    assert_eq!(opened, ffi::SQLITE_OK);
    assert!(
        matches!(selected, ffi::SQLITE_BUSY | ffi::SQLITE_PROTOCOL),
        "Native initialization must report documented lock contention, received {selected}"
    );
    assert_eq!(closed, ffi::SQLITE_OK);
    let [opened, selected, step, rows, closed] = recovery_codes(after);
    assert_eq!(
        (opened, selected, closed),
        (ffi::SQLITE_OK, ffi::SQLITE_OK, ffi::SQLITE_OK)
    );
    assert_eq!((step, rows), (ffi::SQLITE_ROW, 2));
}
