mod schema;
pub(super) mod vfs;

use crate::failure::Failure;

#[cfg(test)]
pub(super) fn test_engine_guard() -> Result<std::sync::MutexGuard<'static, ()>, Failure> {
    vfs::engine_guard()
}

#[derive(Debug)]
pub(super) struct Snapshot {
    pub source_bytes: u64,
    pub source_sha256: String,
    pub bytes: u64,
    pub sha256: String,
}

pub(super) fn normalize(
    _source: &cap_std::fs::File,
    _target: &cap_std::fs::File,
    _check_aux: impl Fn() -> Result<(), Failure>,
) -> Result<Snapshot, Failure> {
    Err(refused())
}

pub(super) fn inspect(
    _source: &cap_std::fs::File,
    _check_aux: impl Fn() -> Result<(), Failure>,
) -> Result<(), Failure> {
    Err(refused())
}

fn refused() -> Failure {
    Failure::usage("SQLite snapshot does not satisfy the standalone profile.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsqlite3_sys as ffi;
    use std::ffi::CString;
    use std::ptr;

    struct Temporary(std::path::PathBuf);

    impl Temporary {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(sql: &str) -> (Temporary, cap_std::fs::File, cap_std::fs::File) {
        let dir = Temporary(
            std::env::temp_dir().join(format!("vivac-sqlite-logical-{}", crate::id::ulid())),
        );
        std::fs::create_dir(&dir.0).unwrap();
        let path = dir.path().join("source.db");
        {
            let _guard = vfs::engine_guard().unwrap();
            let path = CString::new(path.to_str().unwrap()).unwrap();
            let sql = CString::new(format!("PRAGMA cache_size=-64; {sql}")).unwrap();
            let mut db = ptr::null_mut();
            unsafe {
                assert_eq!(ffi::sqlite3_open(path.as_ptr(), &mut db), ffi::SQLITE_OK);
                let result =
                    ffi::sqlite3_exec(db, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut());
                assert_eq!(result, ffi::SQLITE_OK, "Synthetic fixture must be valid.");
                assert_eq!(ffi::sqlite3_close(db), ffi::SQLITE_OK);
            }
        }
        let source = cap_std::fs::File::from_std(std::fs::File::open(path).unwrap());
        let target = cap_std::fs::File::from_std(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(dir.path().join("target.db"))
                .unwrap(),
        );
        (dir, source, target)
    }

    #[test]
    fn logical_export_reopens_preserving_schema_data_and_counters() {
        let (dir, source, target) = fixture(
            "PRAGMA user_version=23; PRAGMA application_id=42;
             CREATE TABLE parent(id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL DEFAULT 'ready' UNIQUE);
             CREATE TABLE child(id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent(id), amount REAL DEFAULT 1.5);
             CREATE INDEX child_parent ON child(parent_id);
             CREATE TABLE pair(a TEXT, b INTEGER, PRIMARY KEY(a,b)) WITHOUT ROWID;
             INSERT INTO parent(id,label) VALUES(41,'sample');
             DELETE FROM parent; INSERT INTO parent(label) VALUES('retained');
             INSERT INTO child VALUES(7,42,2.5); INSERT INTO pair VALUES('key',9);"
        );
        let calls = std::cell::Cell::new(0);
        let snapshot = normalize(&source, &target, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(snapshot.source_bytes, source.metadata().unwrap().len());
        assert_eq!(snapshot.bytes, target.metadata().unwrap().len());
        assert_eq!(snapshot.sha256.len(), 64);
        assert_eq!(snapshot.source_sha256.len(), 64);
        inspect(&target, || Ok(())).unwrap();
        let _guard = vfs::engine_guard().unwrap();
        let path = CString::new(dir.path().join("target.db").to_str().unwrap()).unwrap();
        unsafe {
            let mut db = ptr::null_mut();
            assert_eq!(ffi::sqlite3_open(path.as_ptr(), &mut db), ffi::SQLITE_OK);
            for query in [
                "SELECT id=42 AND label='retained' FROM parent",
                "SELECT id=7 AND parent_id=42 AND amount=2.5 FROM child",
                "SELECT a='key' AND b=9 FROM pair",
                "SELECT seq=42 FROM sqlite_sequence WHERE name='parent'",
                "SELECT count(*)=1 FROM pragma_index_list('child') WHERE name='child_parent'",
                "SELECT count(*)=1 FROM pragma_foreign_key_list('child') WHERE \"table\"='parent'",
                "SELECT user_version=23 FROM pragma_user_version",
                "SELECT application_id=42 FROM pragma_application_id",
            ] {
                let query = CString::new(query).unwrap();
                let mut statement = ptr::null_mut();
                assert_eq!(
                    ffi::sqlite3_prepare_v2(
                        db,
                        query.as_ptr(),
                        -1,
                        &mut statement,
                        ptr::null_mut()
                    ),
                    ffi::SQLITE_OK
                );
                assert_eq!(ffi::sqlite3_step(statement), ffi::SQLITE_ROW);
                assert_eq!(ffi::sqlite3_column_int(statement, 0), 1);
                assert_eq!(ffi::sqlite3_finalize(statement), ffi::SQLITE_OK);
            }
            assert_eq!(ffi::sqlite3_close(db), ffi::SQLITE_OK);
        }
    }

    #[test]
    fn deleted_sensitive_content_is_not_retained() {
        let (_dir, source, mut target) = fixture(
            "PRAGMA secure_delete=OFF; CREATE TABLE item(id INTEGER PRIMARY KEY, value TEXT);
             INSERT INTO item VALUES(1,'person@example.com'); DELETE FROM item;
             INSERT INTO item VALUES(2,'safe');",
        );
        normalize(&source, &target, || Ok(())).unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut target, &mut bytes).unwrap();
        assert!(!bytes
            .windows(b"person@example.com".len())
            .any(|value| value == b"person@example.com"));
    }

    #[test]
    fn rejects_sensitive_values_metadata_and_unsupported_storage() {
        for sql in [
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO item VALUES(1,'person@example.com')",
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value INTEGER); INSERT INTO item VALUES(1,'person@example.com')",
            "CREATE TABLE \"person@example.com\"(id INTEGER PRIMARY KEY)",
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT DEFAULT 'person@example.com')",
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value BLOB); INSERT INTO item VALUES(1,x'00')",
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO item VALUES(1,char(0))",
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value REAL); INSERT INTO item VALUES(1,1e999)",
            "CREATE TABLE item(id INTEGER PRIMARY KEY DESC,value TEXT)",
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); CREATE VIEW hidden AS SELECT * FROM item",
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); CREATE INDEX expression ON item(lower(value))",
            "PRAGMA encoding='UTF-16'; CREATE TABLE item(id INTEGER PRIMARY KEY)",
        ] {
            let (_dir, source, target) = fixture(sql);
            let error = normalize(&source, &target, || Ok(())).unwrap_err();
            assert!(!error.message().contains("person@example.com"));
            assert_eq!(target.metadata().unwrap().len(), 0);
        }
    }

    #[test]
    fn logical_limits_refuse_complete_capture() {
        for sql in [
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO item VALUES(1,printf('%.*c',65537,65))".to_string(),
            (0..17).map(|index| format!("CREATE TABLE item{index}(id INTEGER PRIMARY KEY);")).collect(),
            format!("CREATE TABLE item(id INTEGER PRIMARY KEY,{});", (0..32).map(|index| format!("value{index} TEXT")).collect::<Vec<_>>().join(",")),
            "CREATE TABLE item(id INTEGER PRIMARY KEY); WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<100001) INSERT INTO item SELECT i FROM n".to_string(),
        ] {
            let (_dir, source, target) = fixture(&sql);
            assert!(normalize(&source, &target, || Ok(())).is_err());
        }
    }

    #[test]
    fn serialized_row_limit_refuses_individually_bounded_cells() {
        let (_dir, source, target) = fixture(
            "CREATE TABLE item(id INTEGER PRIMARY KEY,a TEXT,b TEXT,c TEXT,d TEXT,e TEXT);
             INSERT INTO item VALUES(1,printf('%.*c',61440,65),printf('%.*c',61440,66),printf('%.*c',61440,67),printf('%.*c',61440,68),printf('%.*c',61440,69))"
        );
        assert!(normalize(&source, &target, || Ok(())).is_err());
    }

    #[test]
    fn malformed_header_and_auxiliary_guard_refuse() {
        let (_dir, source, target) = fixture("CREATE TABLE item(id INTEGER PRIMARY KEY)");
        let called = std::cell::Cell::new(false);
        assert!(normalize(&source, &target, || {
            called.set(true);
            Err(Failure::usage("Synthetic auxiliary refusal."))
        })
        .is_err());
        assert!(called.get());
        assert_eq!(target.metadata().unwrap().len(), 0);
        let path = _dir.path().join("invalid.db");
        std::fs::write(&path, b"not a database").unwrap();
        let source = cap_std::fs::File::from_std(std::fs::File::open(path).unwrap());
        assert!(inspect(&source, || Ok(())).is_err());
    }

    #[test]
    fn sqlite_and_rust_peak_fit_combined_budget() {
        let (_dir, source, target) = fixture(
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO item VALUES(1,printf('%.*c',65536,65))"
        );
        let (result, rust_peak) =
            crate::test_alloc::measure(|| normalize(&source, &target, || Ok(())));
        result.unwrap();
        let _guard = vfs::engine_guard().unwrap();
        let mut current = 0;
        let mut c_peak = 0;
        unsafe {
            assert_eq!(
                ffi::sqlite3_status64(ffi::SQLITE_STATUS_MEMORY_USED, &mut current, &mut c_peak, 0),
                ffi::SQLITE_OK
            );
        }
        assert!(c_peak <= 1024 * 1024);
        assert!(
            rust_peak + c_peak as usize + 128 * 1024 <= 2 * 1024 * 1024,
            "Rust peak {rust_peak}, SQLite peak {c_peak}, stack allowance 131072"
        );
    }
}
