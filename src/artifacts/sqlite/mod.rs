mod schema;
pub(super) mod vfs;

use crate::failure::Failure;
use libsqlite3_sys as ffi;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::ffi::CString;
use std::io;
use std::ptr;
use std::time::{Duration, Instant};

const MAX_BYTES: u64 = 10 * 1024 * 1024;
const MAX_VALUE: usize = 65536;
const MAX_STEPS: u64 = 20_000_000;

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

#[derive(Debug)]
pub(super) struct WalSnapshot {
    pub snapshot: Snapshot,
    pub wal_bytes: u64,
    pub wal_sha256: String,
}

pub(super) fn normalize_wal(
    source: &cap_std::fs::File,
    wal: &cap_std::fs::File,
    shm: &cap_std::fs::File,
    target: &cap_std::fs::File,
    check_aux: impl Fn() -> Result<(), Failure>,
) -> Result<WalSnapshot, Failure> {
    if target.metadata().map_err(|_| refused())?.len() != 0 {
        return Err(refused());
    }
    let _engine = vfs::engine_guard()?;
    // Clone every descriptor before acquiring any process-wide POSIX lock.
    let source = source.try_clone().map_err(|_| refused())?.into_std();
    let wal = wal.try_clone().map_err(|_| refused())?.into_std();
    let shm = shm.try_clone().map_err(|_| refused())?.into_std();
    let target = target.try_clone().map_err(|_| refused())?.into_std();
    let work = Work::new(MAX_STEPS);
    let registered = vfs::RegisteredVfs::new_wal(source, wal, shm, Some(target), MAX_BYTES)?;
    check_aux()?;
    let result = (|| {
        header_with_version(registered.source(), 2)?;
        let wal = registered.wal().ok_or_else(refused)?;
        let wal_bytes = wal.metadata().map_err(|_| refused())?.len();
        if (1..32).contains(&wal_bytes) {
            return Err(refused());
        }
        let snapshot = normalize_registered(&registered, &work)?;
        let (wal_bytes, wal_sha256) = hash(wal, &work)?;
        Ok(WalSnapshot {
            snapshot,
            wal_bytes,
            wal_sha256,
        })
    })();
    let checked = check_aux();
    result.and_then(|value| {
        checked?;
        work.check()?;
        Ok(value)
    })
}

fn refused() -> Failure {
    Failure::usage("SQLite snapshot does not satisfy the standalone profile.")
}

fn checked_text(value: &str) -> Result<(), Failure> {
    if value.len() > MAX_VALUE || value.contains('\0') {
        return Err(refused());
    }
    super::guard::check_text(value).map_err(|_| refused())
}

struct Work {
    start: Instant,
    steps: Cell<u64>,
    maximum: u64,
}

impl Work {
    fn new(maximum: u64) -> Self {
        Self {
            start: Instant::now(),
            steps: Cell::new(0),
            maximum,
        }
    }
    fn check(&self) -> Result<(), Failure> {
        if self.start.elapsed() > Duration::from_secs(2) || self.steps.get() > self.maximum {
            Err(refused())
        } else {
            Ok(())
        }
    }
}

unsafe extern "C" fn progress(context: *mut std::ffi::c_void) -> i32 {
    // SQLite invokes this only while the borrowed Work remains alive.
    let work = unsafe { &*(context as *const Work) };
    work.steps.set(work.steps.get().saturating_add(1000));
    i32::from(work.check().is_err())
}

unsafe extern "C" fn authorize(
    context: *mut std::ffi::c_void,
    action: i32,
    first: *const std::ffi::c_char,
    _second: *const std::ffi::c_char,
    _database: *const std::ffi::c_char,
    _trigger: *const std::ffi::c_char,
) -> i32 {
    let target = !context.is_null();
    let allowed = match action {
        ffi::SQLITE_SELECT | ffi::SQLITE_READ | ffi::SQLITE_RECURSIVE => true,
        ffi::SQLITE_PRAGMA => {
            if first.is_null() {
                false
            } else {
                // SQLite owns the terminated authorization argument for this callback.
                let name = unsafe { std::ffi::CStr::from_ptr(first) }.to_bytes();
                [
                    b"integrity_check".as_slice(),
                    b"foreign_key_check",
                    b"table_xinfo",
                    b"table_list",
                    b"index_list",
                    b"index_xinfo",
                    b"foreign_key_list",
                    b"user_version",
                    b"application_id",
                ]
                .iter()
                .any(|allowed| name.eq_ignore_ascii_case(allowed))
            }
        }
        ffi::SQLITE_CREATE_TABLE
        | ffi::SQLITE_CREATE_INDEX
        | ffi::SQLITE_REINDEX
        | ffi::SQLITE_INSERT
        | ffi::SQLITE_UPDATE
        | ffi::SQLITE_DELETE
        | ffi::SQLITE_TRANSACTION => target,
        _ => false,
    };
    if allowed {
        ffi::SQLITE_OK
    } else {
        ffi::SQLITE_DENY
    }
}

struct Statement(*mut ffi::sqlite3_stmt);

impl Drop for Statement {
    fn drop(&mut self) {
        // This statement owns exactly one successful SQLite prepare result.
        unsafe {
            ffi::sqlite3_finalize(self.0);
        }
    }
}

impl Statement {
    fn new(db: *mut ffi::sqlite3, sql: &str) -> Result<Self, Failure> {
        let sql = CString::new(sql).map_err(|_| refused())?;
        let mut statement = ptr::null_mut();
        // Queries are fixed statements or DDL emitted by the strict AST.
        let result = unsafe {
            ffi::sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut statement, ptr::null_mut())
        };
        if result != ffi::SQLITE_OK || statement.is_null() {
            if !statement.is_null() {
                unsafe {
                    ffi::sqlite3_finalize(statement);
                }
            }
            return Err(refused());
        }
        Ok(Self(statement))
    }

    fn next(&mut self, work: &Work) -> Result<bool, Failure> {
        work.check()?;
        let previous = work.steps.get();
        let result = unsafe { ffi::sqlite3_step(self.0) };
        let steps = unsafe { ffi::sqlite3_stmt_status(self.0, ffi::SQLITE_STMTSTATUS_VM_STEP, 1) };
        work.steps.set(previous.saturating_add(steps.max(0) as u64));
        work.check()?;
        match result {
            ffi::SQLITE_ROW => Ok(true),
            ffi::SQLITE_DONE => Ok(false),
            _ => Err(refused()),
        }
    }

    fn kind(&self, column: i32) -> i32 {
        unsafe { ffi::sqlite3_column_type(self.0, column) }
    }
    fn integer(&self, column: i32) -> Result<i64, Failure> {
        if self.kind(column) != ffi::SQLITE_INTEGER {
            return Err(refused());
        }
        let value = unsafe { ffi::sqlite3_column_int64(self.0, column) };
        checked_text(&value.to_string())?;
        Ok(value)
    }
    fn text(&self, column: i32) -> Result<&str, Failure> {
        if self.kind(column) != ffi::SQLITE_TEXT {
            return Err(refused());
        }
        let bytes = unsafe { ffi::sqlite3_column_bytes(self.0, column) };
        if bytes < 0 || bytes as usize > MAX_VALUE {
            return Err(refused());
        }
        let pointer = unsafe { ffi::sqlite3_column_text(self.0, column) };
        if pointer.is_null() {
            return Err(refused());
        }
        // The statement remains on its current row throughout this borrow.
        let value =
            std::str::from_utf8(unsafe { std::slice::from_raw_parts(pointer, bytes as usize) })
                .map_err(|_| refused())?;
        checked_text(value)?;
        Ok(value)
    }
}

fn execute(db: *mut ffi::sqlite3, sql: &str, work: &Work) -> Result<(), Failure> {
    let mut statement = Statement::new(db, sql)?;
    if statement.next(work)? {
        return Err(refused());
    }
    Ok(())
}

fn configure(
    db: *mut ffi::sqlite3,
    target: bool,
    work: &Work,
    require_source_utf8: bool,
) -> Result<(), Failure> {
    if target {
        // The destination is still empty. Select its construction journal mode
        // before defensive mode forbids changing it to OFF.
        let mut query = Statement::new(db, "PRAGMA journal_mode=OFF")?;
        if !query.next(work)? || query.text(0)? != "off" || query.next(work)? {
            return Err(refused());
        }
    }
    unsafe {
        for (option, value) in [
            (ffi::SQLITE_DBCONFIG_DEFENSIVE, 1),
            (ffi::SQLITE_DBCONFIG_TRUSTED_SCHEMA, 0),
            (ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION, 0),
        ] {
            if ffi::sqlite3_db_config(db, option, value, ptr::null_mut::<i32>()) != ffi::SQLITE_OK {
                return Err(refused());
            }
        }
        for (limit, value) in [
            (ffi::SQLITE_LIMIT_LENGTH, 262144),
            (ffi::SQLITE_LIMIT_SQL_LENGTH, 65536),
            (ffi::SQLITE_LIMIT_COLUMN, 32),
            (ffi::SQLITE_LIMIT_EXPR_DEPTH, 32),
            (ffi::SQLITE_LIMIT_COMPOUND_SELECT, 0),
            (ffi::SQLITE_LIMIT_ATTACHED, 0),
            (ffi::SQLITE_LIMIT_VDBE_OP, 100000),
            (ffi::SQLITE_LIMIT_FUNCTION_ARG, 16),
            (ffi::SQLITE_LIMIT_VARIABLE_NUMBER, 32),
            (ffi::SQLITE_LIMIT_TRIGGER_DEPTH, 0),
            (ffi::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, 128),
        ] {
            ffi::sqlite3_limit(db, limit, value);
        }
        ffi::sqlite3_busy_timeout(db, 0);
        ffi::sqlite3_progress_handler(
            db,
            1000,
            Some(progress),
            work as *const Work as *mut std::ffi::c_void,
        );
    }
    let mut query = Statement::new(db, "PRAGMA mmap_size=0")?;
    while query.next(work)? {}
    drop(query);
    for sql in [
        "PRAGMA cache_size=-32",
        "PRAGMA temp_store=MEMORY",
        "PRAGMA trusted_schema=OFF",
    ] {
        execute(db, sql, work)?;
    }
    if target {
        for sql in ["PRAGMA synchronous=OFF", "PRAGMA encoding='UTF-8'"] {
            let mut query = Statement::new(db, sql)?;
            while query.next(work)? {}
        }
    } else {
        execute(db, "PRAGMA query_only=ON", work)?;
    }
    if require_source_utf8 {
        let mut query = Statement::new(db, "PRAGMA encoding")?;
        if !query.next(work)? || query.text(0)? != "UTF-8" || query.next(work)? {
            return Err(refused());
        }
    }
    let context = if target {
        std::ptr::dangling_mut::<u8>().cast()
    } else {
        ptr::null_mut()
    };
    if unsafe { ffi::sqlite3_set_authorizer(db, Some(authorize), context) } != ffi::SQLITE_OK {
        return Err(refused());
    }
    Ok(())
}

#[cfg(unix)]
fn read_at(file: &std::fs::File, bytes: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, bytes, offset)
}
#[cfg(windows)]
fn read_at(file: &std::fs::File, bytes: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, bytes, offset)
}
#[cfg(not(any(unix, windows)))]
fn read_at(_file: &std::fs::File, _bytes: &mut [u8], _offset: u64) -> io::Result<usize> {
    Err(io::Error::other("Unsupported SQLite handle platform."))
}

fn header(file: &std::fs::File) -> Result<(), Failure> {
    header_with_version(file, 1)
}

fn header_with_version(file: &std::fs::File, version: u8) -> Result<(), Failure> {
    let size = file.metadata().map_err(|_| refused())?.len();
    if !(100..=MAX_BYTES).contains(&size) {
        return Err(refused());
    }
    let mut bytes = [0u8; 100];
    if read_at(file, &mut bytes, 0).map_err(|_| refused())? != bytes.len()
        || &bytes[..16] != b"SQLite format 3\0"
        || bytes[18] != version
        || bytes[19] != version
        || bytes[20] != 0
        || bytes[21..24] != [64, 32, 32]
        || (bytes[56..60] != [0, 0, 0, 1] && !(version == 2 && bytes[56..60] == [0; 4]))
        || bytes[72..92] != [0; 20]
        || bytes[24..28] != bytes[92..96]
    {
        return Err(refused());
    }
    let encoded = u16::from_be_bytes([bytes[16], bytes[17]]);
    let page_size = if encoded == 1 {
        65536
    } else {
        u64::from(encoded)
    };
    if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() || size % page_size != 0
    {
        return Err(refused());
    }
    let pages = u32::from_be_bytes(bytes[28..32].try_into().map_err(|_| refused())?);
    if pages == 0 || size != u64::from(pages) * page_size {
        return Err(refused());
    }
    Ok(())
}

fn hash(file: &std::fs::File, work: &Work) -> Result<(u64, String), Failure> {
    let expected = file.metadata().map_err(|_| refused())?.len();
    if expected > MAX_BYTES {
        return Err(refused());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 16384];
    let mut offset = 0;
    while offset < expected {
        work.check()?;
        let limit = buffer.len().min((expected - offset) as usize);
        let count = read_at(file, &mut buffer[..limit], offset).map_err(|_| refused())?;
        if count == 0 {
            return Err(refused());
        }
        digest.update(&buffer[..count]);
        offset += count as u64;
    }
    Ok((offset, format!("{:x}", digest.finalize())))
}

struct Model {
    tables: Vec<schema::Table>,
    indexes: Vec<schema::Index>,
    sequence: bool,
    user_version: i64,
    application_id: i64,
}

fn scalar_integer(db: *mut ffi::sqlite3, query: &str, work: &Work) -> Result<i64, Failure> {
    let mut statement = Statement::new(db, query)?;
    if !statement.next(work)? {
        return Err(refused());
    }
    let value = statement.integer(0)?;
    if statement.next(work)? {
        return Err(refused());
    }
    Ok(value)
}

fn integrity(db: *mut ffi::sqlite3, work: &Work) -> Result<(), Failure> {
    let mut query = Statement::new(db, "PRAGMA integrity_check")?;
    if !query.next(work)? || query.text(0)? != "ok" || query.next(work)? {
        return Err(refused());
    }
    let mut query = Statement::new(db, "PRAGMA foreign_key_check")?;
    if query.next(work)? {
        return Err(refused());
    }
    Ok(())
}

fn model(db: *mut ffi::sqlite3, work: &Work) -> Result<Model, Failure> {
    let user_version = scalar_integer(db, "PRAGMA user_version", work)?;
    let application_id = scalar_integer(db, "PRAGMA application_id", work)?;
    let mut query = Statement::new(
        db,
        "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY rowid",
    )?;
    let mut tables = Vec::new();
    let mut indexes = Vec::new();
    let mut sequence = false;
    let mut schema_bytes = 0;
    let mut columns = 0;
    let mut objects = 0;
    while query.next(work)? {
        objects += 1;
        if objects > 80 {
            return Err(refused());
        }
        let kind = query.text(0)?;
        let name = query.text(1)?;
        let table_name = query.text(2)?;
        if name.len() > 128 || table_name.len() > 128 {
            return Err(refused());
        }
        if kind == "index" && query.kind(3) == ffi::SQLITE_NULL {
            if !name.starts_with("sqlite_autoindex_") {
                return Err(refused());
            }
            continue;
        }
        let sql = query.text(3)?;
        schema_bytes += sql.len();
        if schema_bytes > 65536 {
            return Err(refused());
        }
        if kind == "table" && name == "sqlite_sequence" {
            if sequence || sql != "CREATE TABLE sqlite_sequence(name,seq)" {
                return Err(refused());
            }
            sequence = true;
        } else if kind == "table" {
            if tables.len() == 16 {
                return Err(refused());
            }
            let table = schema::table(sql)?;
            if table.name != name || table_name != name {
                return Err(refused());
            }
            columns += table.columns.len();
            if columns > 256 {
                return Err(refused());
            }
            tables.push(table);
        } else if kind == "index" {
            if indexes.len() == 32 {
                return Err(refused());
            }
            let index = schema::index(sql)?;
            if index.name != name || !index.table.eq_ignore_ascii_case(table_name) {
                return Err(refused());
            }
            indexes.push(index);
        } else {
            return Err(refused());
        }
    }
    if sequence != tables.iter().any(|table| table.autoincrement) {
        return Err(refused());
    }
    let model = Model {
        tables,
        indexes,
        sequence,
        user_version,
        application_id,
    };
    metadata(db, &model, work)?;
    integrity(db, work)?;
    Ok(model)
}

fn metadata(db: *mut ffi::sqlite3, model: &Model, work: &Work) -> Result<(), Failure> {
    let mut index_count = 0;
    for table in &model.tables {
        work.check()?;
        let mut query = Statement::new(
            db,
            &format!("PRAGMA table_xinfo({})", schema::quote(&table.name)),
        )?;
        let mut position = 0;
        while query.next(work)? {
            let column = table.definitions.get(position).ok_or_else(refused)?;
            if query.integer(0)? != position as i64
                || query.text(1)? != column.name
                || schema::type_name(query.text(2)?)? != column.declared_type
                || query.integer(3)?
                    != i64::from(column.not_null || (table.without_rowid && column.pk != 0))
                || query.integer(5)? != column.pk as i64
                || query.integer(6)? != 0
            {
                return Err(refused());
            }
            match &column.default {
                None if query.kind(4) == ffi::SQLITE_NULL => {}
                Some(expected)
                    if query.kind(4) == ffi::SQLITE_TEXT
                        && schema::default_literal(query.text(4)?)? == *expected => {}
                _ => return Err(refused()),
            }
            position += 1;
        }
        if position != table.columns.len() {
            return Err(refused());
        }
        let mut query = Statement::new(
            db,
            &format!("PRAGMA table_list({})", schema::quote(&table.name)),
        )?;
        if !query.next(work)?
            || query.text(0)? != "main"
            || query.text(1)? != table.name
            || query.text(2)? != "table"
            || query.integer(3)? != table.columns.len() as i64
            || query.integer(4)? != i64::from(table.without_rowid)
            || query.integer(5)? != 0
            || query.next(work)?
        {
            return Err(refused());
        }
        let mut query = Statement::new(
            db,
            &format!("PRAGMA index_list({})", schema::quote(&table.name)),
        )?;
        let mut primary_index = false;
        while query.next(work)? {
            index_count += 1;
            if index_count > 32 {
                return Err(refused());
            }
            query.integer(0)?;
            let name = query.text(1)?;
            query.integer(2)?;
            let origin = query.text(3)?;
            if !matches!(origin, "c" | "u" | "pk") || query.integer(4)? != 0 {
                return Err(refused());
            }
            if origin == "pk" {
                primary_index = true;
            }
            if origin == "c"
                && !model.indexes.iter().any(|index| {
                    index.name == name && index.table.eq_ignore_ascii_case(&table.name)
                })
            {
                return Err(refused());
            }
            let mut info =
                Statement::new(db, &format!("PRAGMA index_xinfo({})", schema::quote(name)))?;
            while info.next(work)? {
                info.integer(0)?;
                let cid = info.integer(1)?;
                let key = info.integer(5)?;
                if cid < -1 || !matches!(key, 0 | 1) {
                    return Err(refused());
                }
                if cid >= 0 {
                    let name = info.text(2)?;
                    if table
                        .columns
                        .get(cid as usize)
                        .is_none_or(|column| column != name)
                    {
                        return Err(refused());
                    }
                } else if info.kind(2) != ffi::SQLITE_NULL {
                    return Err(refused());
                }
                if !matches!(info.integer(3)?, 0 | 1) || info.text(4)? != "BINARY" {
                    return Err(refused());
                }
            }
        }
        if primary_index != table.without_rowid {
            return Err(refused());
        }
        let mut query = Statement::new(
            db,
            &format!("PRAGMA foreign_key_list({})", schema::quote(&table.name)),
        )?;
        while query.next(work)? {
            query.integer(0)?;
            query.integer(1)?;
            for column in 2..8 {
                query.text(column)?;
            }
        }
    }
    Ok(())
}

fn rows(
    db: *mut ffi::sqlite3,
    target: Option<*mut ffi::sqlite3>,
    model: &Model,
    work: &Work,
) -> Result<(), Failure> {
    let mut count = 0;
    for table in &model.tables {
        let names = table
            .columns
            .iter()
            .map(|name| schema::quote(name))
            .collect::<Vec<_>>()
            .join(",");
        let mut select = Statement::new(
            db,
            &format!("SELECT {names} FROM {}", schema::quote(&table.name)),
        )?;
        let mut insert = target
            .map(|target| {
                Statement::new(
                    target,
                    &format!(
                        "INSERT INTO {}({names}) VALUES({})",
                        schema::quote(&table.name),
                        vec!["?"; table.columns.len()].join(",")
                    ),
                )
            })
            .transpose()?;
        while select.next(work)? {
            count += 1;
            if count > 100000 {
                return Err(refused());
            }
            for (column, _) in table.columns.iter().enumerate() {
                work.check()?;
                let column = column as i32;
                let result = match select.kind(column) {
                    ffi::SQLITE_NULL => insert
                        .as_ref()
                        .map(|insert| unsafe { ffi::sqlite3_bind_null(insert.0, column + 1) }),
                    ffi::SQLITE_INTEGER => {
                        let value = select.integer(column)?;
                        insert.as_ref().map(|insert| unsafe {
                            ffi::sqlite3_bind_int64(insert.0, column + 1, value)
                        })
                    }
                    ffi::SQLITE_FLOAT => {
                        let value = unsafe { ffi::sqlite3_column_double(select.0, column) };
                        if !value.is_finite() {
                            return Err(refused());
                        }
                        checked_text(&value.to_string())?;
                        insert.as_ref().map(|insert| unsafe {
                            ffi::sqlite3_bind_double(insert.0, column + 1, value)
                        })
                    }
                    ffi::SQLITE_TEXT => {
                        let value = select.text(column)?;
                        insert.as_ref().map(|insert| unsafe {
                            ffi::sqlite3_bind_text(
                                insert.0,
                                column + 1,
                                value.as_ptr().cast(),
                                value.len() as i32,
                                ffi::SQLITE_TRANSIENT(),
                            )
                        })
                    }
                    _ => return Err(refused()),
                };
                if result.is_some_and(|result| result != ffi::SQLITE_OK) {
                    return Err(refused());
                }
            }
            if let Some(insert) = &mut insert {
                if insert.next(work)? {
                    return Err(refused());
                }
                if unsafe { ffi::sqlite3_reset(insert.0) } != ffi::SQLITE_OK
                    || unsafe { ffi::sqlite3_clear_bindings(insert.0) } != ffi::SQLITE_OK
                {
                    return Err(refused());
                }
            }
        }
    }
    Ok(())
}

fn sequences(
    db: *mut ffi::sqlite3,
    target: Option<*mut ffi::sqlite3>,
    model: &Model,
    work: &Work,
) -> Result<(), Failure> {
    if !model.sequence {
        return Ok(());
    }
    if let Some(target) = target {
        execute(target, "DELETE FROM sqlite_sequence", work)?;
    }
    let mut select = Statement::new(db, "SELECT name,seq FROM sqlite_sequence")?;
    let mut insert = target
        .map(|target| Statement::new(target, "INSERT INTO sqlite_sequence(name,seq) VALUES(?,?)"))
        .transpose()?;
    let mut names = Vec::new();
    while select.next(work)? {
        let name = select.text(0)?;
        let value = select.integer(1)?;
        if value < 0
            || names.iter().any(|prior| prior == name)
            || !model
                .tables
                .iter()
                .any(|table| table.autoincrement && table.name == name)
        {
            return Err(refused());
        }
        names.push(name.to_owned());
        if let Some(insert) = &mut insert {
            if unsafe {
                ffi::sqlite3_bind_text(
                    insert.0,
                    1,
                    name.as_ptr().cast(),
                    name.len() as i32,
                    ffi::SQLITE_TRANSIENT(),
                )
            } != ffi::SQLITE_OK
                || unsafe { ffi::sqlite3_bind_int64(insert.0, 2, value) } != ffi::SQLITE_OK
                || insert.next(work)?
                || unsafe { ffi::sqlite3_reset(insert.0) } != ffi::SQLITE_OK
                || unsafe { ffi::sqlite3_clear_bindings(insert.0) } != ffi::SQLITE_OK
            {
                return Err(refused());
            }
        }
    }
    Ok(())
}

fn verify_data(
    source: *mut ffi::sqlite3,
    target: *mut ffi::sqlite3,
    model: &Model,
    work: &Work,
) -> Result<(), Failure> {
    for table in &model.tables {
        let columns = table
            .columns
            .iter()
            .map(|name| schema::quote(name))
            .collect::<Vec<_>>()
            .join(",");
        let mut keys = table
            .definitions
            .iter()
            .filter(|column| column.pk != 0)
            .collect::<Vec<_>>();
        keys.sort_by_key(|column| column.pk);
        let order = keys
            .iter()
            .map(|column| schema::quote(&column.name))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT {columns} FROM {} ORDER BY {order}",
            schema::quote(&table.name)
        );
        let mut before = Statement::new(source, &sql)?;
        let mut after = Statement::new(target, &sql)?;
        loop {
            let source_row = before.next(work)?;
            if source_row != after.next(work)? {
                return Err(refused());
            }
            if !source_row {
                break;
            }
            for column in 0..table.columns.len() as i32 {
                work.check()?;
                let kind = before.kind(column);
                if kind != after.kind(column) {
                    return Err(refused());
                }
                let same = match kind {
                    ffi::SQLITE_NULL => true,
                    ffi::SQLITE_INTEGER => before.integer(column)? == after.integer(column)?,
                    ffi::SQLITE_FLOAT => unsafe {
                        ffi::sqlite3_column_double(before.0, column)
                            == ffi::sqlite3_column_double(after.0, column)
                    },
                    ffi::SQLITE_TEXT => before.text(column)? == after.text(column)?,
                    _ => false,
                };
                if !same {
                    return Err(refused());
                }
            }
        }
    }
    Ok(())
}

fn operation<T>(
    source: &cap_std::fs::File,
    target: Option<&cap_std::fs::File>,
    check_aux: impl Fn() -> Result<(), Failure>,
    maximum: u64,
    action: impl FnOnce(&vfs::RegisteredVfs, &Work) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let _engine = vfs::engine_guard()?;
    // Clones precede the actual POSIX lock: closing any source descriptor later
    // could otherwise release process-wide fcntl locks.
    let source = source.try_clone().map_err(|_| refused())?.into_std();
    let target = target
        .map(|file| file.try_clone().map(cap_std::fs::File::into_std))
        .transpose()
        .map_err(|_| refused())?;
    let work = Work::new(maximum);
    let registered = vfs::RegisteredVfs::new(source, target, MAX_BYTES)?;
    check_aux()?;
    let result = (|| {
        header(registered.source())?;
        action(&registered, &work)
    })();
    let checked = check_aux();
    result.and_then(|value| {
        checked?;
        work.check()?;
        Ok(value)
    })
}

pub(super) fn normalize(
    source: &cap_std::fs::File,
    target: &cap_std::fs::File,
    check_aux: impl Fn() -> Result<(), Failure>,
) -> Result<Snapshot, Failure> {
    if target.metadata().map_err(|_| refused())?.len() != 0 {
        return Err(refused());
    }
    operation(
        source,
        Some(target),
        check_aux,
        MAX_STEPS,
        normalize_registered,
    )
}

fn normalize_registered(registered: &vfs::RegisteredVfs, work: &Work) -> Result<Snapshot, Failure> {
    let source = registered.source_connection()?;
    configure(source.raw(), false, work, registered.wal().is_some())?;
    let model = model(source.raw(), work)?;
    rows(source.raw(), None, &model, work)?;
    sequences(source.raw(), None, &model, work)?;
    let (source_bytes, source_sha256) = hash(registered.source(), work)?;
    {
        let target = registered.target_connection()?;
        configure(target.raw(), true, work, false)?;
        execute(target.raw(), "BEGIN", work)?;
        for table in &model.tables {
            execute(target.raw(), &table.sql, work)?;
        }
        rows(source.raw(), Some(target.raw()), &model, work)?;
        sequences(source.raw(), Some(target.raw()), &model, work)?;
        for index in &model.indexes {
            execute(target.raw(), &index.sql, work)?;
        }
        execute(
            target.raw(),
            &format!("PRAGMA user_version={}", model.user_version),
            work,
        )?;
        execute(
            target.raw(),
            &format!("PRAGMA application_id={}", model.application_id),
            work,
        )?;
        execute(target.raw(), "COMMIT", work)?;
        integrity(target.raw(), work)?;
        metadata(target.raw(), &model, work)?;
        verify_data(source.raw(), target.raw(), &model, work)?;
    }
    drop(source);
    let target = registered.target().ok_or_else(refused)?;
    header(target)?;
    let (bytes, sha256) = hash(target, work)?;
    Ok(Snapshot {
        source_bytes,
        source_sha256,
        bytes,
        sha256,
    })
}

pub(super) fn inspect(
    source: &cap_std::fs::File,
    check_aux: impl Fn() -> Result<(), Failure>,
) -> Result<super::guard::Receipt, Failure> {
    operation(source, None, check_aux, MAX_STEPS, |registered, work| {
        let source = registered.source_connection()?;
        configure(source.raw(), false, work, false)?;
        let model = model(source.raw(), work)?;
        rows(source.raw(), None, &model, work)?;
        sequences(source.raw(), None, &model, work)?;
        let (size, digest) = hash(registered.source(), work)?;
        Ok(super::guard::Receipt { size, digest })
    })
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
             CREATE TABLE pair(a VARCHAR(0080), b INTEGER, PRIMARY KEY(a,b)) WITHOUT ROWID;
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
        let inspected = inspect(&target, || Ok(())).unwrap();
        assert_eq!(inspected.size, snapshot.bytes);
        assert_eq!(inspected.digest, snapshot.sha256);
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
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO item VALUES(1,x'00')",
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO item VALUES(1,CAST(x'ff' AS TEXT))",
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

    #[test]
    fn aggregate_vm_budget_and_elapsed_deadline_refuse() {
        let (_dir, source, _target) = fixture(
            "CREATE TABLE item(id INTEGER PRIMARY KEY,value TEXT);
             WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<1000) INSERT INTO item SELECT i,'safe' FROM n"
        );
        assert!(operation(
            &source,
            None,
            || Ok(()),
            1000,
            |registered, work| {
                let source = registered.source_connection()?;
                configure(source.raw(), false, work, false)?;
                let model = model(source.raw(), work)?;
                rows(source.raw(), None, &model, work)
            }
        )
        .is_err());
        let expired = Work {
            start: Instant::now() - Duration::from_secs(3),
            steps: Cell::new(0),
            maximum: MAX_STEPS,
        };
        assert!(expired.check().is_err());
    }

    #[test]
    fn destination_must_be_empty_before_any_auxiliary_callback() {
        let (_dir, source, mut target) = fixture("CREATE TABLE item(id INTEGER PRIMARY KEY)");
        std::io::Write::write_all(&mut target, b"previous content").unwrap();
        let called = Cell::new(false);
        assert!(normalize(&source, &target, || {
            called.set(true);
            Ok(())
        })
        .is_err());
        assert!(!called.get());
        assert_eq!(target.metadata().unwrap().len(), 16);
    }

    #[test]
    fn complete_unaccounted_page_refuses_without_destination_writes() {
        let (dir, source, target) = fixture("CREATE TABLE item(id INTEGER PRIMARY KEY)");
        let path = dir.path().join("source.db");
        let mut bytes = std::fs::read(&path).unwrap();
        let encoded = u16::from_be_bytes([bytes[16], bytes[17]]);
        let page_size = if encoded == 1 {
            65536
        } else {
            encoded as usize
        };
        bytes.resize(bytes.len() + page_size, 0);
        std::fs::write(&path, bytes).unwrap();
        assert!(normalize(&source, &target, || Ok(())).is_err());
        assert_eq!(target.metadata().unwrap().len(), 0);
    }

    #[test]
    fn auxiliary_callbacks_run_under_native_source_lock() {
        let (dir, source, target) = fixture(
            "CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO items VALUES(1,'safe')"
        );
        let calls = Cell::new(0);
        let writer = |expected: i32| {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "artifacts::sqlite::vfs::tests::native_writer_child",
                    "--nocapture",
                ])
                .env("VIVAC_SQLITE_WRITER_PATH", dir.path().join("source.db"))
                .env("VIVAC_SQLITE_WRITER_EXPECTED", expected.to_string())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Synthetic writer failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        };
        normalize(&source, &target, || {
            calls.set(calls.get() + 1);
            writer(ffi::SQLITE_BUSY);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.get(), 2);
        inspect(&source, || {
            calls.set(calls.get() + 1);
            writer(ffi::SQLITE_BUSY);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.get(), 4);
        writer(ffi::SQLITE_OK);
    }

    #[test]
    fn low_heap_child() {
        if std::env::var_os("VIVAC_SQLITE_LOW_HEAP_CHILD").is_none() {
            return;
        }
        let (_dir, source, target) = fixture("CREATE TABLE item(id INTEGER PRIMARY KEY)");
        let allocation = {
            let _guard = vfs::engine_guard().unwrap();
            let used = unsafe { ffi::sqlite3_memory_used() };
            let allocation = unsafe { ffi::sqlite3_malloc64((1024 * 1024 - used - 8192) as u64) };
            assert!(!allocation.is_null());
            allocation
        };
        let normalized = normalize(&source, &target, || Ok(()));
        let inspected = inspect(&source, || Ok(()));
        {
            let _guard = vfs::engine_guard().unwrap();
            unsafe {
                ffi::sqlite3_free(allocation);
            }
        }
        assert!(normalized.is_err());
        assert!(inspected.is_err());
        assert_eq!(target.metadata().unwrap().len(), 0);
    }

    #[test]
    fn sqlite_oom_refuses_in_isolated_process() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "artifacts::sqlite::tests::low_heap_child",
                "--nocapture",
            ])
            .env("VIVAC_SQLITE_LOW_HEAP_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Synthetic low-heap child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

#[cfg(test)]
mod wal_tests;
