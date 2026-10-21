use super::*;
use std::cell::RefCell;
use std::fs;

#[test]
fn policy_permissive_wal_keeps_native_locks_and_preserves_source_bytes() {
    let fixture = Fixture::new("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO item VALUES(1,'person@example.com')");
    let paths = [
        fixture.writer.borrow().path(""),
        fixture.writer.borrow().path("-wal"),
        fixture.writer.borrow().path("-shm"),
    ];
    let before: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
    let calls = Cell::new(0);
    let captured = normalize_wal_with_policy(
        &fixture.source,
        &fixture.wal,
        &fixture.shm,
        &fixture.target,
        || {
            calls.set(calls.get() + 1);
            assert_eq!(
                fixture.writer.borrow_mut().command("checkpoint"),
                ffi::SQLITE_BUSY
            );
            Ok(())
        },
        CapturePolicy::AllowInsecure,
    )
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(
        captured.snapshot.inspection.sensitivity,
        super::super::policy::Sensitivity::Detected
    );
    let observed =
        inspect_with_policy(&fixture.target, || Ok(()), CapturePolicy::AllowInsecure).unwrap();
    assert_eq!(observed.inspection, captured.snapshot.inspection);
    assert_eq!(observed.receipt.digest, captured.snapshot.sha256);
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn logical_wal_export_retains_pending_commit_without_mutating_inputs() {
    let writer = RefCell::new(vfs::wal_tests::Writer::start());
    let path = writer.borrow().path("");
    let paths = [
        path.clone(),
        writer.borrow().path("-wal"),
        writer.borrow().path("-shm"),
    ];
    let before: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
    let source = cap_std::fs::File::from_std(fs::File::open(&paths[0]).unwrap());
    let wal = cap_std::fs::File::from_std(fs::File::open(&paths[1]).unwrap());
    let shm = cap_std::fs::File::from_std(fs::File::open(&paths[2]).unwrap());
    let target_path = path.with_file_name("normalized.db");
    let target = cap_std::fs::File::from_std(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&target_path)
            .unwrap(),
    );
    let calls = Cell::new(0);
    let captured = normalize_wal(&source, &wal, &shm, &target, || {
        calls.set(calls.get() + 1);
        assert_eq!(writer.borrow_mut().command("write"), ffi::SQLITE_BUSY);
        assert_eq!(writer.borrow_mut().command("checkpoint"), ffi::SQLITE_BUSY);
        Ok(())
    })
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(captured.snapshot.source_bytes, before[0].len() as u64);
    assert_eq!(
        captured.snapshot.source_sha256,
        format!("{:x}", Sha256::digest(&before[0]))
    );
    assert_eq!(captured.wal_bytes, before[1].len() as u64);
    assert_eq!(
        captured.wal_sha256,
        format!("{:x}", Sha256::digest(&before[1]))
    );
    for (path, bytes) in paths.iter().zip(&before) {
        assert_eq!(&fs::read(path).unwrap(), bytes);
    }
    let normalized = fs::read(&target_path).unwrap();
    assert_eq!(&normalized[18..20], &[1, 1]);
    let inspected = inspect(&target, || Ok(())).unwrap();
    assert_eq!(inspected.size, captured.snapshot.bytes);
    assert_eq!(inspected.digest, captured.snapshot.sha256);
    {
        let _guard = vfs::engine_guard().unwrap();
        let registered =
            vfs::RegisteredVfs::new(target.try_clone().unwrap().into_std(), None, MAX_BYTES)
                .unwrap();
        let db = registered.source_connection().unwrap();
        let work = Work::new(MAX_STEPS);
        assert_eq!(
            scalar_integer(db.raw(), "SELECT count(*) FROM items", &work).unwrap(),
            2
        );
        assert_eq!(
            scalar_integer(db.raw(), "SELECT sum(id) FROM items", &work).unwrap(),
            3
        );
    }
    assert_eq!(writer.borrow_mut().command("write"), ffi::SQLITE_OK);
    assert_eq!(writer.borrow_mut().command("checkpoint"), ffi::SQLITE_OK);
}

struct Fixture {
    writer: RefCell<vfs::wal_tests::Writer>,
    source: cap_std::fs::File,
    wal: cap_std::fs::File,
    shm: cap_std::fs::File,
    target: cap_std::fs::File,
}
impl Fixture {
    fn new(sql: &str) -> Self {
        let writer = vfs::wal_tests::Writer::start_with_sql(sql);
        let source = cap_std::fs::File::from_std(fs::File::open(writer.path("")).unwrap());
        let wal = cap_std::fs::File::from_std(fs::File::open(writer.path("-wal")).unwrap());
        let shm = cap_std::fs::File::from_std(fs::File::open(writer.path("-shm")).unwrap());
        let target = cap_std::fs::File::from_std(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(writer.path("").with_file_name("normalized.db"))
                .unwrap(),
        );
        Self {
            writer: RefCell::new(writer),
            source,
            wal,
            shm,
            target,
        }
    }
    fn capture(&self) -> Result<WalSnapshot, Failure> {
        normalize_wal(&self.source, &self.wal, &self.shm, &self.target, || Ok(()))
    }
    fn output_bytes(&self) -> Vec<u8> {
        fs::read(
            self.writer
                .borrow()
                .path("")
                .with_file_name("normalized.db"),
        )
        .unwrap()
    }
}

#[test]
fn empty_wal_after_truncate_exports_checkpointed_database() {
    let fixture = Fixture::new(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items VALUES(1);
        PRAGMA wal_checkpoint(TRUNCATE)",
    );
    assert_eq!(fixture.wal.metadata().unwrap().len(), 0);
    let captured = fixture.capture().unwrap();
    assert_eq!(captured.wal_bytes, 0);
    assert_eq!(captured.wal_sha256, format!("{:x}", Sha256::digest([])));
    assert_eq!(
        inspect(&fixture.target, || Ok(())).unwrap().digest,
        captured.snapshot.sha256
    );
}

#[test]
fn partial_wal_header_refuses_before_destination_writes() {
    for length in [1, 16, 31] {
        let fixture = Fixture::new(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items VALUES(1);
            PRAGMA wal_checkpoint(TRUNCATE)",
        );
        fs::write(fixture.writer.borrow().path("-wal"), vec![0; length]).unwrap();
        assert!(fixture.capture().is_err());
        assert_eq!(fixture.target.metadata().unwrap().len(), 0);
    }
}

#[test]
fn wal_schema_values_and_encoding_keep_standalone_guards() {
    let prefix = "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items VALUES(1);
        PRAGMA wal_checkpoint(TRUNCATE); ";
    for sql in [
        "CREATE TABLE extra(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO extra VALUES(1,'person@example.com')",
        "CREATE TABLE extra(id INTEGER PRIMARY KEY,value INTEGER); INSERT INTO extra VALUES(1,'person@example.com')",
        "CREATE TABLE extra(id INTEGER PRIMARY KEY,value TEXT DEFAULT 'person@example.com')",
        "CREATE TABLE extra(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO extra VALUES(1,CAST(x'ff' AS TEXT))",
        "CREATE TABLE extra(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO extra VALUES(1,char(0))",
        "CREATE TABLE extra(id INTEGER PRIMARY KEY,value BLOB); INSERT INTO extra VALUES(1,x'00')",
        "CREATE TABLE extra(id INTEGER PRIMARY KEY DESC,value TEXT)",
        "CREATE TABLE extra(value TEXT)",
        "CREATE VIEW hidden AS SELECT * FROM items",
        "CREATE INDEX expression ON items(abs(id))",
    ] {
        let fixture = Fixture::new(&format!("{prefix}{sql}"));
        let error = fixture.capture().unwrap_err();
        assert!(!error.message().contains("person@example.com"));
        assert_eq!(fixture.target.metadata().unwrap().len(), 0);
    }
    let fixture = Fixture::new(
        "PRAGMA encoding='UTF-16'; PRAGMA journal_mode=WAL;
        PRAGMA wal_autocheckpoint=0; CREATE TABLE items(id INTEGER PRIMARY KEY);
        INSERT INTO items VALUES(1); PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO items VALUES(2)",
    );
    assert!(fixture.capture().is_err());
    assert_eq!(fixture.target.metadata().unwrap().len(), 0);
}

#[test]
fn deleted_wal_content_is_not_copied_into_logical_export() {
    let fixture = Fixture::new(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        PRAGMA secure_delete=OFF; CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT);
        INSERT INTO items VALUES(1,'safe'); PRAGMA wal_checkpoint(TRUNCATE);
        INSERT INTO items VALUES(2,'person@example.com'); DELETE FROM items WHERE id=2;
        INSERT INTO items VALUES(3,'retained')",
    );
    let input = fs::read(fixture.writer.borrow().path("-wal")).unwrap();
    assert!(input
        .windows(b"person@example.com".len())
        .any(|bytes| bytes == b"person@example.com"));
    fixture.capture().unwrap();
    assert!(!fixture
        .output_bytes()
        .windows(b"person@example.com".len())
        .any(|bytes| bytes == b"person@example.com"));
    inspect(&fixture.target, || Ok(())).unwrap();
}

#[test]
fn wal_auxiliary_failure_refuses_under_lease_and_releases_it() {
    let fixture = Fixture::new(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items VALUES(1);
        PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO items VALUES(2)",
    );
    let calls = Cell::new(0);
    let result = normalize_wal(
        &fixture.source,
        &fixture.wal,
        &fixture.shm,
        &fixture.target,
        || {
            calls.set(calls.get() + 1);
            assert_eq!(
                fixture.writer.borrow_mut().command("write"),
                ffi::SQLITE_BUSY
            );
            assert_eq!(
                fixture.writer.borrow_mut().command("checkpoint"),
                ffi::SQLITE_BUSY
            );
            if calls.get() == 2 {
                Err(Failure::usage("Synthetic auxiliary refusal."))
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(calls.get(), 2);
    assert_eq!(fixture.writer.borrow_mut().command("write"), ffi::SQLITE_OK);
}

#[test]
fn wal_normalization_preserves_combined_memory_budget() {
    let fixture = Fixture::new(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO items VALUES(1,'safe');
        PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO items VALUES(2,printf('%.*c',65536,65))",
    );
    let (result, rust_peak) = crate::test_alloc::measure(|| fixture.capture());
    result.unwrap();
    let _guard = vfs::engine_guard().unwrap();
    let mut current = 0;
    let mut c_peak = 0;
    assert_eq!(
        unsafe {
            ffi::sqlite3_status64(ffi::SQLITE_STATUS_MEMORY_USED, &mut current, &mut c_peak, 0)
        },
        ffi::SQLITE_OK
    );
    assert!(c_peak <= 1024 * 1024);
    assert!(
        rust_peak + c_peak as usize + 128 * 1024 <= 2 * 1024 * 1024,
        "Rust peak {rust_peak}, SQLite peak {c_peak}, stack allowance 131072"
    );
}

#[test]
fn fresh_wal_schema_and_first_commit_export_before_any_checkpoint() {
    let fixture = Fixture::new(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT);
        INSERT INTO items VALUES(1,'pending')",
    );
    let main = fs::read(fixture.writer.borrow().path("")).unwrap();
    let encoding = u32::from_be_bytes(main[56..60].try_into().unwrap());
    let pages = u32::from_be_bytes(main[28..32].try_into().unwrap());
    {
        let _engine = vfs::engine_guard().unwrap();
        let source = fixture.source.try_clone().unwrap().into_std();
        let wal = fixture.wal.try_clone().unwrap().into_std();
        let shm = fixture.shm.try_clone().unwrap().into_std();
        let registered = vfs::RegisteredVfs::new_wal(source, wal, shm, None, MAX_BYTES).unwrap();
        let source = registered.source_connection().unwrap();
        let work = Work::new(MAX_STEPS);
        let mut query = Statement::new(source.raw(), "PRAGMA encoding").unwrap();
        assert!(query.next(&work).unwrap());
        assert_eq!(query.text(0, &work).unwrap(), "UTF-8");
        drop(query);
        assert_eq!(
            scalar_integer(
                source.raw(),
                "SELECT count(*) FROM items WHERE id=1 AND value='pending'",
                &work
            )
            .unwrap(),
            1
        );
    }
    let result = fixture.capture();
    assert!(result.is_ok(), "A valid uncheckpointed UTF-8 WAL must export: main encoding={encoding}, pages={pages}, bytes={}, result={result:?}", main.len());
    inspect(&fixture.target, || Ok(())).unwrap();
    let _engine = vfs::engine_guard().unwrap();
    let registered = vfs::RegisteredVfs::new(
        fixture.target.try_clone().unwrap().into_std(),
        None,
        MAX_BYTES,
    )
    .unwrap();
    let output = registered.source_connection().unwrap();
    let work = Work::new(MAX_STEPS);
    assert_eq!(
        scalar_integer(
            output.raw(),
            "SELECT count(*) FROM items WHERE id=1 AND value='pending'",
            &work
        )
        .unwrap(),
        1
    );
}

#[test]
fn fresh_utf16_wal_with_unset_physical_encoding_refuses_before_export() {
    for encoding in ["UTF-16le", "UTF-16be"] {
        let fixture = Fixture::new(&format!(
            "PRAGMA encoding='{encoding}'; PRAGMA journal_mode=WAL;
            PRAGMA wal_autocheckpoint=0; CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT);
            INSERT INTO items VALUES(1,'pending')"
        ));
        let main = fs::read(fixture.writer.borrow().path("")).unwrap();
        assert_eq!(
            &main[56..60],
            &[0; 4],
            "Fresh WAL fixture must leave physical encoding unset."
        );
        let result = fixture.capture();
        assert!(
            result.is_err(),
            "A fresh {encoding} WAL must not pass the logical UTF-8 profile."
        );
        assert_eq!(fixture.target.metadata().unwrap().len(), 0);
    }
}
