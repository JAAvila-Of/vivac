use std::collections::BTreeSet;
use std::ffi::{c_int, CStr, CString};
#[cfg(test)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr;

use libsqlite3_sys as sql;
use sha2::{Digest, Sha256};

use crate::team_authority::signed::{
    DeviceRequest, KnownDevice, SignedAuthority, SignedRequest, MAX_ENVELOPE_BYTES,
};
use crate::team_authority::{
    Admission, OrganizationId, Outcome, PrincipalId, ProjectId, ProjectPermission,
};

#[path = "durable/linux.rs"]
mod platform;

use platform::{validate_database, validate_root, validate_shape, Identity, Root};

#[path = "durable/pending.rs"]
pub(crate) mod pending;

#[path = "durable/registered_assertion.rs"]
pub(crate) mod registered_assertion;

#[path = "durable/profiles.rs"]
mod profiles;

#[cfg(feature = "coordinator-probe")]
pub(crate) use profiles::EnrollmentProfile;
pub(crate) use profiles::{Profile, RegisteredProfile, V1Profile, V2Profile};

const DATABASE: &CStr = c"authority.sqlite3";
const MAX_DATABASE: i64 = 2 * 1024 * 1024;
const MAX_JOURNAL: i64 = 4 * 1024 * 1024;
const MAX_OPERATIONS: u64 = 1024;
const METADATA_SCHEMA: &str = "CREATE TABLE meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL CHECK(version=2), organization BLOB NOT NULL CHECK(length(organization)=26), initial_principal BLOB NOT NULL CHECK(length(initial_principal)=26), trust BLOB NOT NULL CHECK(length(trust)=32), revision INTEGER NOT NULL CHECK(revision BETWEEN 0 AND 1024), head BLOB NOT NULL CHECK(length(head)=32)) STRICT";
const ENTRY_SCHEMA: &str = "CREATE TABLE entries (seq INTEGER PRIMARY KEY CHECK(seq BETWEEN 1 AND 1024), operation BLOB NOT NULL UNIQUE CHECK(length(operation)=26), request BLOB NOT NULL CHECK(length(request) BETWEEN 186 AND 213), signature BLOB NOT NULL CHECK(length(signature)=64)) STRICT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Checkpoint {
    pub revision: u64,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug)]
pub(crate) struct FixtureTrust {
    pub organization: OrganizationId,
    pub initial_principal: PrincipalId,
    pub known: Vec<KnownDevice>,
}

#[derive(Clone, Debug)]
pub(crate) struct FixtureTrustV2 {
    pub(crate) base: FixtureTrust,
    pub(crate) policy_admin: PrincipalId,
}

impl FixtureTrustV2 {
    pub(crate) fn bootstrap_checkpoint(&self) -> StoreResult<Checkpoint> {
        if self.policy_admin != self.base.initial_principal {
            return Err(StoreError::Refused);
        }
        let base = self.base.bootstrap_checkpoint()?;
        let mut digest = Sha256::new();
        digest.update(b"vivac.team.authority.fixture.v2\0");
        digest.update(base.digest);
        digest.update(self.policy_admin.as_str().as_bytes());
        Ok(Checkpoint {
            revision: 0,
            digest: digest.finalize().into(),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FixtureRegisteredTrust {
    pub(crate) base: FixtureTrust,
    pub(crate) policy_admin: PrincipalId,
}

impl FixtureRegisteredTrust {
    pub(crate) fn bootstrap_checkpoint(&self) -> StoreResult<Checkpoint> {
        if self.base.known.is_empty() || self.base.known.len() > 128 {
            return Err(StoreError::Refused);
        }
        crate::team_authority::signed::v2::registry::RegisteredAuthority::bootstrap_fixture(
            self.base.organization.clone(),
            self.base.initial_principal.clone(),
            self.policy_admin.clone(),
            self.base.known.clone(),
        )
        .map_err(|_| StoreError::Refused)?;
        let base = self.base.bootstrap_checkpoint()?;
        let mut digest = Sha256::new();
        digest.update(b"vivac.team.authority.registered-fixture.v1\0");
        digest.update(base.digest);
        digest.update(self.policy_admin.as_str().as_bytes());
        Ok(Checkpoint {
            revision: 0,
            digest: digest.finalize().into(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RegisteredRequest {
    Policy(crate::team_authority::signed::v2::SignedRequestV2),
    Revoke(crate::team_authority::signed::v2::registry::SignedRevocation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StoreError {
    Refused,
    Unavailable,
    Unpublished,
    CommitUnknown,
}

type StoreResult<T> = std::result::Result<T, StoreError>;

impl FixtureTrust {
    pub(crate) fn bootstrap_checkpoint(&self) -> StoreResult<Checkpoint> {
        if self.known.len() > 128 {
            return Err(StoreError::Refused);
        }
        let mut identifiers = BTreeSet::new();
        let mut bindings = Vec::with_capacity(self.known.len());
        for device in &self.known {
            let (organization, principal, device, credential, key) = device.binding_fields();
            if organization != &self.organization
                || !identifiers.insert((device.as_str(), credential.as_str()))
            {
                return Err(StoreError::Refused);
            }
            let mut binding = Vec::with_capacity(136);
            for identifier in [
                organization.as_str(),
                principal.as_str(),
                device.as_str(),
                credential.as_str(),
            ] {
                binding.extend_from_slice(identifier.as_bytes());
            }
            binding.extend_from_slice(&key);
            bindings.push(binding);
        }
        bindings.sort_unstable();
        let mut digest = Sha256::new();
        digest.update(b"vivac.team.authority.fixture.v1\0");
        digest.update(self.organization.as_str().as_bytes());
        digest.update(self.initial_principal.as_str().as_bytes());
        digest.update((bindings.len() as u16).to_be_bytes());
        for binding in bindings {
            digest.update(binding);
        }
        Ok(Checkpoint {
            revision: 0,
            digest: digest.finalize().into(),
        })
    }

    fn device(&self, request: &DeviceRequest) -> StoreResult<&KnownDevice> {
        self.known
            .iter()
            .find(|binding| {
                let (organization, principal, device, credential, _) = binding.binding_fields();
                organization == &request.change.organization
                    && principal == &request.change.actor
                    && device == &request.device
                    && credential == &request.credential
            })
            .ok_or(StoreError::Refused)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FaultPoint {
    AfterInsert,
    BeforeCommit,
    AfterCommit,
}

pub(crate) type AuthorityStore = ProfileStore<V1Profile>;
pub(crate) type AuthorityStoreV2 = ProfileStore<V2Profile>;
pub(crate) type AuthorityStoreRegistered = ProfileStore<RegisteredProfile>;
#[cfg(feature = "coordinator-probe")]
pub(crate) type AuthorityStoreEnrollment = ProfileStore<EnrollmentProfile>;
#[cfg(feature = "coordinator-probe")]
pub(crate) type FixtureEnrollmentTrust = profiles::enrollment::FixtureEnrollmentTrust;
#[cfg(feature = "coordinator-probe")]
pub(crate) type EnrollmentRequest = profiles::enrollment::EnrollmentRequest;

pub(crate) struct ProfileStore<P: Profile> {
    // SQLite releases its native descriptors before the root descriptor.
    database: Connection,
    root: Root,
    root_identity: Identity,
    database_identity: Identity,
    trust: P::Trust,
    expected: Checkpoint,
    poisoned: bool,
    fault: Option<fn(FaultPoint) -> bool>,
}

impl<P: Profile> ProfileStore<P> {
    pub(crate) fn create_new(root: Root, trust: P::Trust) -> StoreResult<Self> {
        let expected = P::bootstrap_checkpoint(&trust)?;
        let root_identity = validate_root(&root)?;
        validate_shape(&root, false)?;
        platform::create_database_file(&root)?;
        let database_identity = validate_database(&root, false)?;
        let database = Connection::open(&root)?;
        let mut store = Self {
            database,
            root,
            root_identity,
            database_identity,
            trust,
            expected,
            poisoned: false,
            fault: None,
        };
        store.validate_files()?;
        store.database.configure(true)?;
        store.database.execute("BEGIN IMMEDIATE")?;
        let mut transaction = Transaction::new(&store.database);
        let initialize = (|| {
            store.database.execute(P::METADATA_SCHEMA)?;
            store.database.execute(P::ENTRY_SCHEMA)?;
            P::create_auxiliary(&store)?;
            let mut statement = store.database.prepare(P::INSERT_METADATA)?;
            statement.bind_blob(1, P::base(&store.trust).organization.as_str().as_bytes())?;
            statement.bind_blob(
                2,
                P::base(&store.trust).initial_principal.as_str().as_bytes(),
            )?;
            statement.bind_blob(3, &expected.digest)?;
            statement.done()
        })();
        if initialize.is_err() {
            store.rollback()?;
            return Err(StoreError::Unpublished);
        }
        transaction.disarm();
        if store.database.execute("COMMIT").is_err() || !platform::sync_root(&store.root) {
            store.poisoned = true;
            return Err(StoreError::CommitUnknown);
        }
        if store.validate_files().is_err() || store.checkpoint().is_err() {
            store.poisoned = true;
            return Err(StoreError::CommitUnknown);
        }
        Ok(store)
    }

    pub(crate) fn open_existing(
        root: Root,
        trust: P::Trust,
        expected: Checkpoint,
    ) -> StoreResult<Self> {
        P::bootstrap_checkpoint(&trust)?;
        let root_identity = validate_root(&root)?;
        validate_shape(&root, true)?;
        let database_identity = validate_database(&root, true)?;
        let database = Connection::open(&root)?;
        database.validate_existing_header()?;
        let mut store = Self {
            database,
            root,
            root_identity,
            database_identity,
            trust,
            expected,
            poisoned: false,
            fault: None,
        };
        store.validate_files()?;
        store.database.configure(false)?;
        store.checkpoint()?;
        Ok(store)
    }

    pub(crate) fn set_fault_hook(&mut self, fault: Option<fn(FaultPoint) -> bool>) {
        self.fault = fault;
    }

    fn fault(&self, point: FaultPoint) -> bool {
        self.fault.is_some_and(|fault| fault(point))
    }

    pub(crate) fn admit(&mut self, request: &P::SignedRequest) -> StoreResult<Outcome> {
        self.validate_files()?;
        self.database
            .execute("BEGIN IMMEDIATE")
            .map_err(|_| StoreError::Unpublished)?;
        let mut transaction = Transaction::new(&self.database);
        match self.admit_in_transaction(request) {
            Ok((outcome, checkpoint)) => {
                if self.fault(FaultPoint::BeforeCommit) {
                    self.rollback()?;
                    return Err(StoreError::Unpublished);
                }
                transaction.disarm();
                if self.database.execute("COMMIT").is_err() {
                    self.poisoned = true;
                    return Err(StoreError::CommitUnknown);
                }
                if self.fault(FaultPoint::AfterCommit) || self.validate_files().is_err() {
                    self.poisoned = true;
                    return Err(StoreError::CommitUnknown);
                }
                self.expected = checkpoint;
                Ok(outcome)
            }
            Err(error) => {
                self.rollback()?;
                Err(error)
            }
        }
    }

    fn admit_in_transaction(
        &mut self,
        request: &P::SignedRequest,
    ) -> StoreResult<(Outcome, Checkpoint)> {
        self.validate_files()?;
        let (mut authority, current) = self.replay()?;
        self.expected = current;
        let outcome = P::apply_live(&self.trust, &mut authority, request, current)?;
        if outcome.admission == Admission::Historical {
            return Ok((outcome, current));
        }
        let bytes = P::canonical_bytes(request);
        let checkpoint = advance(current, &bytes, P::signature(request));
        let mut insert = self
            .database
            .prepare("INSERT INTO entries VALUES(?1,?2,?3,?4)")?;
        insert.bind_integer(1, checkpoint.revision)?;
        insert.bind_blob(2, P::operation(request))?;
        insert.bind_blob(3, &bytes)?;
        insert.bind_blob(4, P::signature(request))?;
        insert.done().map_err(|_| StoreError::Unpublished)?;
        drop(insert);
        if self.fault(FaultPoint::AfterInsert) {
            return Err(StoreError::Unpublished);
        }
        let mut update = self
            .database
            .prepare("UPDATE meta SET revision=?1, head=?2 WHERE singleton=1")?;
        update.bind_integer(1, checkpoint.revision)?;
        update.bind_blob(2, &checkpoint.digest)?;
        update.done().map_err(|_| StoreError::Unpublished)?;
        drop(update);
        self.validate_files()?;
        Ok((outcome, checkpoint))
    }

    pub(crate) fn checkpoint(&mut self) -> StoreResult<Checkpoint> {
        self.snapshot().map(|(_, checkpoint)| checkpoint)
    }

    pub(crate) fn allows(
        &mut self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        project: &ProjectId,
        permission: ProjectPermission,
    ) -> StoreResult<bool> {
        let (authority, _) = self.snapshot()?;
        Ok(P::allows(
            &authority,
            organization,
            principal,
            project,
            permission,
        ))
    }
}

impl<P: Profile> ProfileStore<P> {
    fn snapshot(&mut self) -> StoreResult<(P::Authority, Checkpoint)> {
        self.validate_files()?;
        self.database.execute("BEGIN")?;
        let mut transaction = Transaction::new(&self.database);
        let result = self.replay().and_then(|snapshot| {
            self.validate_files()?;
            Ok(snapshot)
        });
        match result {
            Ok(snapshot) => {
                transaction.disarm();
                if self.database.execute("COMMIT").is_err() {
                    self.poisoned = true;
                    return Err(StoreError::Unavailable);
                }
                self.validate_files()?;
                self.expected = snapshot.1;
                Ok(snapshot)
            }
            Err(error) => {
                self.rollback()?;
                Err(error)
            }
        }
    }

    fn rollback(&mut self) -> StoreResult<()> {
        if self.database.execute("ROLLBACK").is_err()
            || unsafe { sql::sqlite3_get_autocommit(self.database.0) } != 1
        {
            self.poisoned = true;
            return Err(StoreError::CommitUnknown);
        }
        Ok(())
    }

    fn validate_files(&self) -> StoreResult<()> {
        if self.poisoned {
            return Err(StoreError::CommitUnknown);
        }
        if validate_root(&self.root)? != self.root_identity
            || validate_database(&self.root, false)? != self.database_identity
        {
            return Err(StoreError::Refused);
        }
        validate_shape(&self.root, true)?;
        let mut moved: c_int = 1;
        if unsafe {
            sql::sqlite3_file_control(
                self.database.0,
                c"main".as_ptr(),
                sql::SQLITE_FCNTL_HAS_MOVED,
                (&mut moved as *mut c_int).cast(),
            )
        } != sql::SQLITE_OK
            || moved != 0
        {
            return Err(StoreError::Refused);
        }
        Ok(())
    }

    fn replay(&self) -> StoreResult<(P::Authority, Checkpoint)> {
        self.validate_schema()?;
        let mut check = self.database.prepare("PRAGMA quick_check(1)")?;
        if !check.row()? || check.text(0, 2)? != b"ok" || check.row()? {
            return Err(StoreError::Refused);
        }
        drop(check);
        let bootstrap = P::bootstrap_checkpoint(&self.trust)?;
        let mut metadata_row = self.database.prepare(
            "SELECT singleton,version,organization,initial_principal,trust,revision,head FROM meta",
        )?;
        if !metadata_row.row()?
            || metadata_row.integer(0)? != 1
            || metadata_row.integer(1)? != P::VERSION
            || metadata_row.blob(2, 26)? != P::base(&self.trust).organization.as_str().as_bytes()
            || metadata_row.blob(3, 26)?
                != P::base(&self.trust).initial_principal.as_str().as_bytes()
            || metadata_row.blob(4, 32)? != bootstrap.digest
        {
            return Err(StoreError::Refused);
        }
        let metadata = Checkpoint {
            revision: metadata_row.integer(5)?,
            digest: metadata_row
                .blob(6, 32)?
                .try_into()
                .map_err(|_| StoreError::Refused)?,
        };
        if metadata.revision > MAX_OPERATIONS || metadata_row.row()? {
            return Err(StoreError::Refused);
        }
        drop(metadata_row);
        let mut authority = P::bootstrap(&self.trust)?;
        let mut checkpoint = bootstrap;
        let mut matched = checkpoint == self.expected;
        let mut entries = self
            .database
            .prepare("SELECT seq,operation,request,signature FROM entries ORDER BY seq")?;
        while entries.row()? {
            if checkpoint.revision >= MAX_OPERATIONS
                || entries.integer(0)? != checkpoint.revision + 1
            {
                return Err(StoreError::Refused);
            }
            let operation = entries.blob(1, 26)?;
            let bytes = entries.blob(2, P::MAX_ENVELOPE_BYTES)?;
            let signature = entries
                .blob(3, 64)?
                .try_into()
                .map_err(|_| StoreError::Refused)?;
            let signed = P::parse_signed(&bytes, signature)?;
            if operation != P::operation(&signed) {
                return Err(StoreError::Refused);
            }
            let outcome = P::apply_at(&self.trust, &mut authority, &signed, checkpoint)?;
            if outcome.admission != Admission::NewlyAdmitted
                || outcome.revision != checkpoint.revision + 1
            {
                return Err(StoreError::Refused);
            }
            checkpoint = advance(checkpoint, &bytes, &signature);
            matched |= checkpoint == self.expected;
        }
        if !matched || metadata != checkpoint || P::revision(&authority) != checkpoint.revision {
            return Err(StoreError::Refused);
        }
        P::validate_auxiliary(self, checkpoint.revision)?;
        Ok((authority, checkpoint))
    }

    fn validate_schema(&self) -> StoreResult<()> {
        let mut schema = self
            .database
            .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY name")?;
        let expected = P::schema();
        for &(kind, name, table, definition) in expected {
            if !schema.row()?
                || schema.text(0, 16)? != kind
                || schema.text(1, 64)? != name
                || schema.text(2, 64)? != table
            {
                return Err(StoreError::Refused);
            }
            match definition {
                Some(definition) if schema.text(3, 2048)? == definition => {}
                None if unsafe { sql::sqlite3_column_type(schema.0, 3) } == sql::SQLITE_NULL => {}
                _ => return Err(StoreError::Refused),
            }
        }
        if schema.row()? {
            return Err(StoreError::Refused);
        }
        Ok(())
    }
}

fn advance(previous: Checkpoint, request: &[u8], signature: &[u8; 64]) -> Checkpoint {
    let mut digest = Sha256::new();
    digest.update(previous.digest);
    digest.update(request);
    digest.update(signature);
    Checkpoint {
        revision: previous.revision + 1,
        digest: digest.finalize().into(),
    }
}

struct Connection(*mut sql::sqlite3);

struct Transaction {
    database: *mut sql::sqlite3,
    active: bool,
}

impl Transaction {
    fn new(database: &Connection) -> Self {
        Self {
            database: database.0,
            active: true,
        }
    }
    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.active && unsafe { sql::sqlite3_get_autocommit(self.database) } == 0 {
            unsafe {
                sql::sqlite3_exec(
                    self.database,
                    c"ROLLBACK".as_ptr(),
                    None,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe { sql::sqlite3_close(self.0) };
    }
}

impl Connection {
    fn validate_existing_header(&self) -> StoreResult<()> {
        let mut file: *mut sql::sqlite3_file = ptr::null_mut();
        if unsafe {
            sql::sqlite3_file_control(
                self.0,
                c"main".as_ptr(),
                sql::SQLITE_FCNTL_FILE_POINTER,
                (&mut file as *mut *mut sql::sqlite3_file).cast(),
            )
        } != sql::SQLITE_OK
            || file.is_null()
        {
            return Err(StoreError::Refused);
        }
        // Use SQLite's own native handle before any statement can activate
        // a WAL-mode header. No auxiliary descriptor or private Unix layout
        // participates in this fixed profile check.
        let methods = unsafe { (*file).pMethods };
        if methods.is_null() {
            return Err(StoreError::Refused);
        }
        let read = unsafe { (*methods).xRead }.ok_or(StoreError::Refused)?;
        let mut header = [0_u8; 20];
        if unsafe { read(file, header.as_mut_ptr().cast(), header.len() as c_int, 0) }
            != sql::SQLITE_OK
            || &header[..16] != b"SQLite format 3\0"
            || header[18] != 1
            || header[19] != 1
        {
            return Err(StoreError::Refused);
        }
        Ok(())
    }

    fn open(root: &Root) -> StoreResult<Self> {
        let raw = platform::open_database(root)?;
        let connection = Self(raw);
        if unsafe { sql::sqlite3_db_readonly(raw, c"main".as_ptr()) } != 0 {
            return Err(StoreError::Unavailable);
        }
        unsafe {
            sql::sqlite3_limit(raw, sql::SQLITE_LIMIT_LENGTH, 8192);
            sql::sqlite3_limit(raw, sql::SQLITE_LIMIT_SQL_LENGTH, 4096);
            sql::sqlite3_limit(raw, sql::SQLITE_LIMIT_COLUMN, 16);
            sql::sqlite3_limit(raw, sql::SQLITE_LIMIT_EXPR_DEPTH, 32);
            sql::sqlite3_limit(raw, sql::SQLITE_LIMIT_COMPOUND_SELECT, 1);
            sql::sqlite3_limit(raw, sql::SQLITE_LIMIT_ATTACHED, 0);
            sql::sqlite3_limit(raw, sql::SQLITE_LIMIT_VARIABLE_NUMBER, 8);
            sql::sqlite3_limit(raw, sql::SQLITE_LIMIT_TRIGGER_DEPTH, 0);
        }
        let mut result: c_int = -1;
        for (option, value) in [
            (sql::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION, 0),
            (sql::SQLITE_DBCONFIG_TRUSTED_SCHEMA, 0),
            (sql::SQLITE_DBCONFIG_DEFENSIVE, 1),
        ] {
            if unsafe {
                sql::sqlite3_db_config(raw, option, value as c_int, &mut result as *mut c_int)
            } != sql::SQLITE_OK
                || result != value
            {
                return Err(StoreError::Unavailable);
            }
        }
        if unsafe { sql::sqlite3_busy_timeout(raw, 0) } != sql::SQLITE_OK {
            return Err(StoreError::Unavailable);
        }
        Ok(connection)
    }

    fn configure(&self, creating: bool) -> StoreResult<()> {
        if creating {
            self.execute("PRAGMA page_size=4096")?;
        }
        if !creating {
            let mut mode = self.prepare("PRAGMA journal_mode")?;
            if !mode.row()? || mode.text(0, 6)? != b"delete" || mode.row()? {
                return Err(StoreError::Refused);
            }
            let mut size = self.prepare("PRAGMA page_size")?;
            if !size.row()? || size.integer(0)? != 4096 || size.row()? {
                return Err(StoreError::Refused);
            }
        }
        for pragma in [
            "PRAGMA journal_mode=DELETE",
            "PRAGMA synchronous=FULL",
            "PRAGMA mmap_size=0",
            "PRAGMA cache_size=-1024",
            "PRAGMA temp_store=MEMORY",
            "PRAGMA max_page_count=512",
        ] {
            self.execute(pragma)?;
        }
        for (pragma, expected) in [
            ("PRAGMA page_size", 4096),
            ("PRAGMA synchronous", 2),
            ("PRAGMA mmap_size", 0),
            ("PRAGMA cache_size", -1024),
            ("PRAGMA temp_store", 2),
            ("PRAGMA max_page_count", 512),
        ] {
            let mut statement = self.prepare(pragma)?;
            if !statement.row()? || statement.signed_integer(0)? != expected || statement.row()? {
                return Err(StoreError::Refused);
            }
        }
        let mut mode = self.prepare("PRAGMA journal_mode")?;
        if !mode.row()? || mode.text(0, 6)? != b"delete" || mode.row()? {
            return Err(StoreError::Refused);
        }
        Ok(())
    }

    fn execute(&self, text: &str) -> StoreResult<()> {
        let mut statement = self.prepare(text)?;
        while statement.row()? {}
        Ok(())
    }

    fn prepare(&self, text: &str) -> StoreResult<Statement> {
        let text = CString::new(text).map_err(|_| StoreError::Unavailable)?;
        let mut raw = ptr::null_mut();
        if unsafe { sql::sqlite3_prepare_v2(self.0, text.as_ptr(), -1, &mut raw, ptr::null_mut()) }
            != sql::SQLITE_OK
        {
            if !raw.is_null() {
                unsafe { sql::sqlite3_finalize(raw) };
            }
            return Err(StoreError::Unavailable);
        }
        Ok(Statement(raw))
    }
}

struct Statement(*mut sql::sqlite3_stmt);

impl Drop for Statement {
    fn drop(&mut self) {
        unsafe { sql::sqlite3_finalize(self.0) };
    }
}

impl Statement {
    fn bind_blob(&mut self, index: c_int, bytes: &[u8]) -> StoreResult<()> {
        if unsafe {
            sql::sqlite3_bind_blob(
                self.0,
                index,
                bytes.as_ptr().cast(),
                bytes.len() as c_int,
                sql::SQLITE_TRANSIENT(),
            )
        } != sql::SQLITE_OK
        {
            return Err(StoreError::Unavailable);
        }
        Ok(())
    }

    fn bind_integer(&mut self, index: c_int, value: u64) -> StoreResult<()> {
        let value = i64::try_from(value).map_err(|_| StoreError::Refused)?;
        if unsafe { sql::sqlite3_bind_int64(self.0, index, value) } != sql::SQLITE_OK {
            return Err(StoreError::Unavailable);
        }
        Ok(())
    }

    fn row(&mut self) -> StoreResult<bool> {
        match unsafe { sql::sqlite3_step(self.0) } {
            sql::SQLITE_ROW => Ok(true),
            sql::SQLITE_DONE => Ok(false),
            _ => Err(StoreError::Unavailable),
        }
    }

    fn done(&mut self) -> StoreResult<()> {
        if self.row()? {
            Err(StoreError::Unavailable)
        } else {
            Ok(())
        }
    }

    fn signed_integer(&self, column: c_int) -> StoreResult<i64> {
        if unsafe { sql::sqlite3_column_type(self.0, column) } != sql::SQLITE_INTEGER {
            return Err(StoreError::Refused);
        }
        Ok(unsafe { sql::sqlite3_column_int64(self.0, column) })
    }

    fn integer(&self, column: c_int) -> StoreResult<u64> {
        u64::try_from(self.signed_integer(column)?).map_err(|_| StoreError::Refused)
    }

    fn blob(&self, column: c_int, maximum: usize) -> StoreResult<Vec<u8>> {
        self.bytes(column, maximum, sql::SQLITE_BLOB)
    }
    fn text(&self, column: c_int, maximum: usize) -> StoreResult<Vec<u8>> {
        self.bytes(column, maximum, sql::SQLITE_TEXT)
    }

    fn bytes(&self, column: c_int, maximum: usize, kind: c_int) -> StoreResult<Vec<u8>> {
        if unsafe { sql::sqlite3_column_type(self.0, column) } != kind {
            return Err(StoreError::Refused);
        }
        let size = unsafe { sql::sqlite3_column_bytes(self.0, column) };
        if size < 0 || size as usize > maximum {
            return Err(StoreError::Refused);
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        let bytes = unsafe { sql::sqlite3_column_blob(self.0, column) };
        if bytes.is_null() {
            return Err(StoreError::Unavailable);
        }
        Ok(unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), size as usize) }.to_vec())
    }
}

#[cfg(test)]
#[path = "durable/tests.rs"]
mod tests;
