use super::linux_creation_binding::LocalCreationChallenge;
use super::linux_gate::{CloseResource, Error, Gate, Request, State};
use super::linux_loader::PtyLoader;
use super::linux_spawn::{ExitReceipt, Receipt, SpawnOwner};
use ed25519_dalek::{Signature, VerifyingKey};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};

#[path = "linux_session/endpoint_witness.rs"]
mod endpoint_witness;
pub use endpoint_witness::EndpointSessionWitness;
#[path = "linux_session/endpoint_attempt.rs"]
mod endpoint_attempt;
pub use endpoint_attempt::EndpointProofAttempt;
#[path = "linux_session/endpoint_identity.rs"]
mod endpoint_identity;
use endpoint_identity::EndpointNativeIdentity;
pub use endpoint_identity::VerifiedEndpointPossession;

const LIMIT: usize = 8192;
const DOMAIN: &[u8] = b"vivac.device.enrollment-proof.v1\0";

fn refused() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}
fn require(ok: bool) -> io::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(refused())
    }
}
fn duplicate(fd: i32) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC duplicates a live descriptor atomically.
    let copied = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if copied < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: fcntl returned a fresh owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(copied) })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    links: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Identity {
    fn from(metadata: &std::fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}
fn read_at(file: &File, limit: usize) -> io::Result<Vec<u8>> {
    require(file.metadata()?.len() <= limit as u64)?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let size = file.read_at(&mut buffer, bytes.len() as u64)?;
        if size == 0 {
            return Ok(bytes);
        }
        require(bytes.len() + size <= limit)?;
        bytes.extend_from_slice(&buffer[..size]);
    }
}
struct Entry {
    file: File,
    identity: Identity,
    bytes: Vec<u8>,
    name: &'static [u8],
    limit: usize,
}
impl Entry {
    fn open(directory: &OwnedFd, name: &'static [u8], limit: usize) -> io::Result<Self> {
        // SAFETY: the fixed name is NUL terminated and contains no path separators.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr().cast(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat transfers a fresh descriptor to File.
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        // SAFETY: geteuid only queries the calling process.
        require(
            metadata.is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0
                && metadata.nlink() == 1,
        )?;
        let identity = Identity::from(&metadata);
        let bytes = read_at(&file, limit)?;
        require(Identity::from(&file.metadata()?) == identity)?;
        Ok(Self {
            file,
            identity,
            bytes,
            name,
            limit,
        })
    }
    fn validate(&self, directory: &OwnedFd) -> io::Result<()> {
        require(
            Identity::from(&self.file.metadata()?) == self.identity
                && read_at(&self.file, self.limit)? == self.bytes,
        )?;
        require(Identity::from(&self.file.metadata()?) == self.identity)?;
        let current = Self::open(directory, self.name, self.limit)?;
        require(current.identity == self.identity && current.bytes == self.bytes)
    }
}
pub struct PackageLease {
    directory: OwnedFd,
    directory_identity: (u64, u64, u32, u32),
    key: Entry,
    manifest: Entry,
}
impl PackageLease {
    fn directory_identity(directory: &OwnedFd) -> io::Result<(u64, u64, u32, u32)> {
        let file = File::from(duplicate(directory.as_raw_fd())?);
        let metadata = file.metadata()?;
        // SAFETY: geteuid only queries the calling process.
        require(
            metadata.is_dir()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0,
        )?;
        Ok((
            metadata.dev(),
            metadata.ino(),
            metadata.uid(),
            metadata.mode(),
        ))
    }
    pub fn capture(directory: OwnedFd, cipher: &OwnedFd) -> io::Result<Self> {
        let directory_identity = Self::directory_identity(&directory)?;
        let key = Entry::open(&directory, b"key.openssh\0", 65536)?;
        let manifest = Entry::open(&directory, b"manifest.json\0", 8192)?;
        let cipher = File::from(duplicate(cipher.as_raw_fd())?);
        require(key.bytes == read_at(&cipher, 65536)?)?;
        let lease = Self {
            directory,
            directory_identity,
            key,
            manifest,
        };
        lease.validate().map_err(|_| refused())?;
        Ok(lease)
    }
    pub fn ipc_identity(&self) -> Result<[u64; 6], Error> {
        self.validate()?;
        Ok([
            self.directory_identity.0,
            self.directory_identity.1,
            self.key.identity.dev,
            self.key.identity.ino,
            self.manifest.identity.dev,
            self.manifest.identity.ino,
        ])
    }
    pub(super) fn duplicate_for_reservation(&self) -> io::Result<Self> {
        let identity = self.ipc_identity().map_err(|_| refused())?;
        let directory = duplicate(self.directory.as_raw_fd())?;
        let cipher = duplicate(self.key.file.as_raw_fd())?;
        let snapshot = Self::capture(directory, &cipher)?;
        require(snapshot.ipc_identity().map_err(|_| refused())? == identity)?;
        require(self.ipc_identity().map_err(|_| refused())? == identity)?;
        Ok(snapshot)
    }
    pub(super) fn open_independent_directory(&self) -> io::Result<OwnedFd> {
        self.validate().map_err(|_| refused())?;
        // SAFETY: the fixed relative name is NUL terminated. openat creates a
        // new open file description rather than sharing one through dup.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat returned a fresh owned descriptor.
        let directory = unsafe { OwnedFd::from_raw_fd(fd) };
        self.validate_reservation_directory(&directory)?;
        self.validate().map_err(|_| refused())?;
        Ok(directory)
    }
    pub(super) fn validate_reservation_directory(&self, directory: &OwnedFd) -> io::Result<()> {
        require(Self::directory_identity(directory)? == self.directory_identity)
    }
    pub fn validate(&self) -> Result<(), Error> {
        let validate = || -> io::Result<()> {
            require(Self::directory_identity(&self.directory)? == self.directory_identity)?;
            self.key.validate(&self.directory)?;
            self.manifest.validate(&self.directory)
        };
        validate().map_err(|_| Error::LeaseInvalidated)
    }
}

pub struct SigningIdentity {
    blob: Vec<u8>,
    public_key: [u8; 32],
    challenge: Vec<u8>,
    _comment: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreationIdentityError {
    Unavailable,
    InvalidReply,
    IdentityMismatch,
}

impl SigningIdentity {
    pub fn for_creation_binding(
        blob: Vec<u8>,
        native_comment: Vec<u8>,
        challenge: &LocalCreationChallenge,
    ) -> io::Result<Self> {
        require(native_comment == challenge.comment())?;
        let mut input = blob.as_slice();
        require(
            take_string(&mut input)? == b"ssh-ed25519"
                && take_string(&mut input)? == challenge.public_key()
                && input.is_empty(),
        )?;
        Ok(Self {
            blob,
            public_key: *challenge.public_key(),
            challenge: challenge.bytes().to_vec(),
            _comment: native_comment,
        })
    }

    pub fn new(
        blob: Vec<u8>,
        public_key: [u8; 32],
        challenge: Vec<u8>,
        comment: Vec<u8>,
    ) -> io::Result<Self> {
        require(challenge.starts_with(DOMAIN) && challenge.len() <= 4096)?;
        require(
            comment.len() <= 128
                && comment.starts_with(b"vivac.device-key.v1:")
                && comment.is_ascii()
                && !comment.contains(&0),
        )?;
        let mut input = blob.as_slice();
        require(
            take_string(&mut input)? == b"ssh-ed25519"
                && take_string(&mut input)? == public_key
                && input.is_empty(),
        )?;
        VerifyingKey::from_bytes(&public_key).map_err(|_| refused())?;
        Ok(Self {
            blob,
            public_key,
            challenge,
            _comment: comment,
        })
    }
}

fn ssh_string(bytes: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
    require(bytes.len() <= LIMIT)?;
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

fn take_string<'a>(input: &mut &'a [u8]) -> io::Result<&'a [u8]> {
    require(input.len() >= 4)?;
    let size = u32::from_be_bytes(input[..4].try_into().map_err(|_| refused())?) as usize;
    *input = &input[4..];
    require(size <= LIMIT && size <= input.len())?;
    let value = &input[..size];
    *input = &input[size..];
    Ok(value)
}

fn parse_signature(response: &[u8]) -> io::Result<[u8; 64]> {
    require(response.first() == Some(&14))?;
    let mut outer = &response[1..];
    let mut signature = take_string(&mut outer)?;
    require(outer.is_empty() && take_string(&mut signature)? == b"ssh-ed25519")?;
    let value = take_string(&mut signature)?
        .try_into()
        .map_err(|_| refused())?;
    require(signature.is_empty())?;
    Ok(value)
}

fn parse_creation_identity(
    response: &[u8],
    challenge: &LocalCreationChallenge,
) -> Result<SigningIdentity, CreationIdentityError> {
    let invalid = || CreationIdentityError::InvalidReply;
    if response.len() < 5 || response.len() > LIMIT || response[0] != 12 {
        return Err(invalid());
    }
    let count = u32::from_be_bytes(response[1..5].try_into().map_err(|_| invalid())?);
    if count != 1 {
        return Err(invalid());
    }
    let mut input = &response[5..];
    let blob = take_string(&mut input).map_err(|_| invalid())?;
    let comment = take_string(&mut input).map_err(|_| invalid())?;
    if !input.is_empty() {
        return Err(invalid());
    }
    let mut identity = blob;
    let algorithm = take_string(&mut identity).map_err(|_| invalid())?;
    let key = take_string(&mut identity).map_err(|_| invalid())?;
    if algorithm != b"ssh-ed25519" || key.len() != 32 || !identity.is_empty() {
        return Err(invalid());
    }
    if key != challenge.public_key() || comment != challenge.comment() {
        return Err(CreationIdentityError::IdentityMismatch);
    }
    SigningIdentity::for_creation_binding(blob.to_vec(), comment.to_vec(), challenge)
        .map_err(|_| CreationIdentityError::IdentityMismatch)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignStage {
    BeforeNativeSend,
    NativeResponsePending,
    AfterNativeReply,
    BeforePublication,
}
impl SignStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeNativeSend => "before-native-send",
            Self::NativeResponsePending => "native-response-pending",
            Self::AfterNativeReply => "after-native-reply",
            Self::BeforePublication => "before-publication",
        }
    }
}

pub struct NativeSigner {
    socket: PathBuf,
    peer: [u32; 3],
    identity: SigningIdentity,
    lease: Arc<PackageLease>,
}
impl NativeSigner {
    pub fn sign(
        self,
        request: Request,
        mut hook: impl FnMut(SignStage) -> Result<(), Error>,
    ) -> Result<[u8; 64], Error> {
        let mut checkpoint = |point| -> Result<(), Error> {
            hook(point)?;
            request.check()?;
            self.lease.validate()
        };
        checkpoint(SignStage::BeforeNativeSend)?;
        let mut connection = UnixStream::connect(&self.socket).map_err(|_| Error::Backend)?;
        if !peer_matches(
            self.peer,
            peer_credentials(&connection).map_err(|_| Error::Backend)?,
        ) {
            return Err(Error::Backend);
        }
        connection
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| Error::Backend)?;
        connection
            .set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| Error::Backend)?;
        let mut packet = vec![13];
        ssh_string(&self.identity.blob, &mut packet).map_err(|_| Error::Backend)?;
        ssh_string(&self.identity.challenge, &mut packet).map_err(|_| Error::Backend)?;
        packet.extend_from_slice(&0u32.to_be_bytes());
        request.check()?;
        self.lease.validate()?;
        connection
            .write_all(&(packet.len() as u32).to_be_bytes())
            .and_then(|_| connection.write_all(&packet))
            .map_err(|_| Error::Backend)?;
        request.native_sent()?;
        checkpoint(SignStage::NativeResponsePending)?;
        let mut length = [0u8; 4];
        connection
            .read_exact(&mut length)
            .map_err(|_| Error::Backend)?;
        let size = u32::from_be_bytes(length) as usize;
        if size == 0 || size > LIMIT {
            return Err(Error::Backend);
        }
        let mut response = vec![0; size];
        connection
            .read_exact(&mut response)
            .map_err(|_| Error::Backend)?;
        let signature = parse_signature(&response).map_err(|_| Error::Backend)?;
        request.native_reply()?;
        checkpoint(SignStage::AfterNativeReply)?;
        VerifyingKey::from_bytes(&self.identity.public_key)
            .map_err(|_| Error::Backend)?
            .verify_strict(&self.identity.challenge, &Signature::from_bytes(&signature))
            .map_err(|_| Error::Backend)?;
        request.verified()?;
        checkpoint(SignStage::BeforePublication)?;
        Ok(signature)
    }
}

enum Operation {
    Stop,
    Release,
}
struct ResourceMessage {
    operation: Operation,
    deadline: Instant,
    reply: mpsc::SyncSender<Result<(), Error>>,
}
struct ProxyCloseResource {
    sender: mpsc::SyncSender<ResourceMessage>,
}
impl ProxyCloseResource {
    fn invoke(&self, operation: Operation, deadline: Instant) -> Result<(), Error> {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let (reply, result) = mpsc::sync_channel(1);
        self.sender
            .try_send(ResourceMessage {
                operation,
                deadline,
                reply,
            })
            .map_err(|_| Error::Uncertain)?;
        result
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => Error::Timeout,
                mpsc::RecvTimeoutError::Disconnected => Error::Uncertain,
            })?
    }
}
impl CloseResource for ProxyCloseResource {
    fn stop(&self, deadline: Instant) -> Result<(), Error> {
        self.invoke(Operation::Stop, deadline)
    }
    fn release(&self, deadline: Instant) -> Result<(), Error> {
        self.invoke(Operation::Release, deadline)
    }
}

struct Resources {
    owner: SpawnOwner,
    _loader: PtyLoader,
    agent: Receipt,
    loader_receipt: Receipt,
    lease: Arc<PackageLease>,
    _retained: Vec<OwnedFd>,
    gate: Weak<Gate>,
    socket: PathBuf,
    peer: [u32; 3],
}

fn peer_matches(expected: [u32; 3], observed: [u32; 3]) -> bool {
    expected[0] != 0 && observed == expected
}

fn peer_credentials(connection: &UnixStream) -> io::Result<[u32; 3]> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes only the initialized ucred and its length.
    let result = unsafe {
        libc::getsockopt(
            connection.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    require(length as usize == std::mem::size_of::<libc::ucred>() && credentials.pid > 0)?;
    Ok([credentials.pid as u32, credentials.uid, credentials.gid])
}

fn expected_exits(agent: Receipt, loader: Receipt, exits: &[ExitReceipt]) -> bool {
    exits.len() == 2
        && exits.iter().any(|exit| {
            exit.token == agent.token
                && exit.pid == agent.pid
                && exit.code.is_none()
                && exit.signal == Some(libc::SIGKILL)
        })
        && exits.iter().any(|exit| {
            exit.token == loader.token
                && exit.pid == loader.pid
                && exit.code == Some(0)
                && exit.signal.is_none()
        })
}

fn authorize_release(gate: &Weak<Gate>, lease: &Arc<PackageLease>) -> Result<(), Error> {
    let snapshot = gate.upgrade().ok_or(Error::Uncertain)?.snapshot()?;
    if snapshot.state != State::Closing || snapshot.active || Arc::strong_count(lease) != 1 {
        return Err(Error::Uncertain);
    }
    Ok(())
}

pub struct NativeSession {
    generation: [u8; 32],
    store: [u64; 6],
    roles: [Receipt; 2],
    endpoint_proof_sent: std::cell::Cell<bool>,
    active_closure_taken: bool,
    lifetime: Arc<()>,
    endpoint_attempt_slot: Arc<AtomicBool>,
    resources: Option<Resources>,
    gate: Arc<Gate>,
    receiver: mpsc::Receiver<ResourceMessage>,
    exits: Option<Vec<ExitReceipt>>,
}

impl NativeSession {
    pub fn new(
        mut owner: SpawnOwner,
        agent_token: u64,
        loader: PtyLoader,
        lease: Arc<PackageLease>,
        retained: Vec<OwnedFd>,
        socket: PathBuf,
    ) -> io::Result<Self> {
        owner.validate_reservation(&lease)?;
        let generation = owner.session_generation(&lease)?;
        let store = lease.ipc_identity().map_err(|_| refused())?;
        let agent = owner.receipt(agent_token)?;
        let loader_receipt = owner.receipt(loader.token())?;
        // SAFETY: gettid only queries the calling thread identity.
        let tid: i32 = unsafe { libc::syscall(libc::SYS_gettid) }
            .try_into()
            .map_err(|_| refused())?;
        require(
            agent.token != loader_receipt.token
                && loader.receipt() == loader_receipt
                && agent.creator_tid == tid
                && loader_receipt.creator_tid == tid,
        )?;
        let exit = owner.try_wait(loader_receipt.token)?.ok_or_else(refused)?;
        require(
            exit.code == Some(0) && exit.signal.is_none() && owner.try_wait(agent.token)?.is_none(),
        )?;
        require(retained.len() <= 8)?;
        for descriptor in &retained {
            // SAFETY: F_GETFD queries an owned descriptor without changing it.
            let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
            require(descriptor.as_raw_fd() >= 3 && flags >= 0 && flags & libc::FD_CLOEXEC != 0)?;
        }
        lease.validate().map_err(|_| refused())?;
        require(socket.is_absolute())?;
        let connection = UnixStream::connect(&socket)?;
        // SAFETY: these calls only query the calling process credentials.
        let expected = [agent.pid, unsafe { libc::geteuid() }, unsafe {
            libc::getegid()
        }];
        require(peer_matches(expected, peer_credentials(&connection)?))?;
        drop(connection);
        let (sender, receiver) = mpsc::sync_channel(1);
        let gate = Gate::new(Arc::new(ProxyCloseResource { sender }));
        let resources = Resources {
            owner,
            _loader: loader,
            agent,
            loader_receipt,
            lease,
            _retained: retained,
            gate: Arc::downgrade(&gate),
            socket,
            peer: expected,
        };
        Ok(Self {
            generation,
            store,
            roles: [agent, loader_receipt],
            endpoint_proof_sent: std::cell::Cell::new(false),
            active_closure_taken: false,
            lifetime: Arc::new(()),
            endpoint_attempt_slot: Arc::new(AtomicBool::new(false)),
            resources: Some(resources),
            gate,
            receiver,
            exits: None,
        })
    }

    pub fn gate(&self) -> Arc<Gate> {
        self.gate.clone()
    }

    pub fn endpoint_witness(&self) -> io::Result<EndpointSessionWitness> {
        let resources = self.resources.as_ref().ok_or_else(refused)?;
        resources.lease.validate().map_err(|_| refused())?;
        let store = resources.lease.ipc_identity().map_err(|_| refused())?;
        let witness =
            EndpointSessionWitness::capture(&self.gate, &self.lifetime, self.generation, store)?;
        self.validate_endpoint_witness(&witness)?;
        Ok(witness)
    }

    pub fn validate_endpoint_witness(&self, witness: &EndpointSessionWitness) -> io::Result<()> {
        witness.checked(|| {
            let resources = self.resources.as_ref().ok_or_else(refused)?;
            resources.lease.validate().map_err(|_| refused())?;
            let resource_gate = resources.gate.upgrade().ok_or_else(refused)?;
            require(Arc::ptr_eq(&resource_gate, &self.gate))?;
            let store = resources.lease.ipc_identity().map_err(|_| refused())?;
            witness.validate_binding(&self.gate, self.generation, store)?;
            resources.lease.validate().map_err(|_| refused())?;
            require(resources.lease.ipc_identity().map_err(|_| refused())? == store)?;
            witness.validate_binding(&self.gate, self.generation, store)
        })
    }

    pub fn endpoint_attempt(&self, deadline: Instant) -> io::Result<EndpointProofAttempt> {
        let attempt = EndpointProofAttempt::issue(&self.endpoint_attempt_slot, deadline, || {
            self.endpoint_witness()
        })?;
        self.validate_endpoint_attempt(&attempt)?;
        Ok(attempt)
    }

    pub fn validate_endpoint_attempt(&self, attempt: &EndpointProofAttempt) -> io::Result<()> {
        attempt.validate(&self.endpoint_attempt_slot, |witness| {
            self.validate_endpoint_witness(witness)
        })
    }

    pub fn consume_endpoint_attempt(&self, attempt: &mut EndpointProofAttempt) -> io::Result<()> {
        attempt.consume(&self.endpoint_attempt_slot, |witness| {
            self.validate_endpoint_witness(witness)
        })
    }

    fn validate_endpoint_identity(&self, identity: &mut EndpointNativeIdentity) -> io::Result<()> {
        identity.validate_session(self)
    }

    pub fn creation_identity(
        &self,
        challenge: &LocalCreationChallenge,
    ) -> Result<SigningIdentity, CreationIdentityError> {
        let unavailable = || CreationIdentityError::Unavailable;
        let resources = self.resources.as_ref().ok_or_else(unavailable)?;
        let check = || -> Result<(), CreationIdentityError> {
            if self.exits.is_some()
                || self.gate.snapshot().map_err(|_| unavailable())?.state != State::Ready
            {
                return Err(unavailable());
            }
            challenge.check_live().map_err(|_| unavailable())?;
            resources.lease.validate().map_err(|_| unavailable())
        };
        check()?;
        let mut connection = UnixStream::connect(&resources.socket).map_err(|_| unavailable())?;
        if !peer_matches(
            resources.peer,
            peer_credentials(&connection).map_err(|_| unavailable())?,
        ) {
            return Err(unavailable());
        }
        connection
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| unavailable())?;
        connection
            .set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| unavailable())?;
        connection
            .write_all(&[0, 0, 0, 1, 11])
            .map_err(|_| unavailable())?;
        let mut length = [0; 4];
        connection
            .read_exact(&mut length)
            .map_err(|_| unavailable())?;
        let size = u32::from_be_bytes(length) as usize;
        if size == 0 || size > LIMIT {
            check()?;
            return Err(CreationIdentityError::InvalidReply);
        }
        let mut response = vec![0; size];
        connection
            .read_exact(&mut response)
            .map_err(|_| unavailable())?;
        check()?;
        if !peer_matches(
            resources.peer,
            peer_credentials(&connection).map_err(|_| unavailable())?,
        ) {
            return Err(unavailable());
        }
        parse_creation_identity(&response, challenge)
    }

    pub fn signer(&self, identity: SigningIdentity) -> io::Result<NativeSigner> {
        let resources = self.resources.as_ref().ok_or_else(refused)?;
        Ok(NativeSigner {
            socket: resources.socket.clone(),
            peer: resources.peer,
            identity,
            lease: resources.lease.clone(),
        })
    }

    pub fn service(&mut self, timeout: Duration) -> Result<bool, Error> {
        let message = match self.receiver.recv_timeout(timeout) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Uncertain),
        };
        let result = if Instant::now() >= message.deadline {
            Err(Error::Timeout)
        } else {
            match message.operation {
                Operation::Stop => self.stop(message.deadline),
                Operation::Release => self.release(message.deadline),
            }
        };
        let _ = message.reply.send(result);
        Ok(true)
    }

    /// Closes the gate while its native owner services requests on the creator thread.
    /// A timed-out coordinator retains only the gate and shares this deadline.
    /// The session retains native custody for its existing uncertainty policy.
    pub fn close_on_creator(&mut self, timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now().checked_add(timeout).ok_or(Error::Timeout)?;
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let gate = self.gate();
        let closing = gate.clone();
        let coordinator = thread::Builder::new()
            .spawn(move || {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or(Error::Timeout)?;
                closing.lock(remaining)
            })
            .map_err(|_| Error::Backend)?;
        while !coordinator.is_finished() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or(Error::Timeout)?;
            if let Err(error) = self.service(remaining.min(Duration::from_millis(20))) {
                if coordinator.is_finished() {
                    let _ = coordinator.join();
                }
                return Err(error);
            }
        }
        coordinator.join().map_err(|_| Error::Uncertain)??;
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let state = gate.snapshot()?;
        if !self.released()
            || state.state != State::Closed
            || !state.close_acknowledged
            || !state.resources_closed_after_drain
            || state.active
            || state.drain_waiting
            || !expected_exits(
                self.roles[0],
                self.roles[1],
                self.exits.as_deref().ok_or(Error::Uncertain)?,
            )
        {
            return Err(Error::Uncertain);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        Ok(())
    }

    fn stop(&mut self, deadline: Instant) -> Result<(), Error> {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        if self.exits.is_some() {
            return Ok(());
        }
        let resources = self.resources.as_mut().ok_or(Error::Uncertain)?;
        if resources
            .owner
            .try_wait(resources.agent.token)
            .map_err(|_| Error::Backend)?
            .is_none()
        {
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            resources
                .owner
                .signal(resources.agent.token, libc::SIGKILL)
                .map_err(|_| Error::Backend)?;
        }
        loop {
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            let agent = resources
                .owner
                .try_wait(resources.agent.token)
                .map_err(|_| Error::Backend)?;
            let loader = resources
                .owner
                .try_wait(resources.loader_receipt.token)
                .map_err(|_| Error::Backend)?;
            if let (Some(agent), Some(loader)) = (agent, loader) {
                if agent.signal != Some(libc::SIGKILL)
                    || agent.code.is_some()
                    || loader.code != Some(0)
                    || loader.signal.is_some()
                {
                    return Err(Error::Uncertain);
                }
                self.exits = Some(vec![agent, loader]);
                return if Instant::now() < deadline {
                    Ok(())
                } else {
                    Err(Error::Timeout)
                };
            }
            thread::sleep(
                Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    fn release(&mut self, deadline: Instant) -> Result<(), Error> {
        if self.resources.is_none() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let resources = self.resources.as_mut().ok_or(Error::Uncertain)?;
        authorize_release(&resources.gate, &resources.lease)?;
        let expected = self.exits.as_ref().ok_or(Error::Uncertain)?;
        if !expected_exits(resources.agent, resources.loader_receipt, expected) {
            return Err(Error::Uncertain);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let observed = resources
            .owner
            .shutdown(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| Error::Uncertain)?;
        if observed.len() != 2 || !expected.iter().all(|exit| observed.contains(exit)) {
            return Err(Error::Uncertain);
        }
        drop(self.resources.take());
        Ok(())
    }

    pub fn released(&self) -> bool {
        self.resources.is_none()
    }

    pub(crate) fn consume_active_closure(
        &mut self,
        generation: [u8; 32],
        store: [u64; 6],
        roles: [Receipt; 2],
    ) -> io::Result<()> {
        require(!std::mem::replace(&mut self.active_closure_taken, true))?;
        require(self.generation == generation && self.store == store && self.roles == roles)?;
        let state = self.gate.snapshot().map_err(|_| refused())?;
        require(
            self.released()
                && state.state == State::Closed
                && !state.active
                && !state.drain_waiting
                && state.close_acknowledged
                && state.resources_closed_after_drain
                && state.workers_joined > 0
                && state.proofs_verified > 0
                && state.proofs_published > 0
                && self.endpoint_proof_sent.get(),
        )?;
        let exits = self.exits.as_deref().ok_or_else(refused)?;
        require(expected_exits(roles[0], roles[1], exits))
    }
    pub fn exits(&self) -> Option<&[ExitReceipt]> {
        self.exits.as_deref()
    }
}

impl Drop for NativeSession {
    fn drop(&mut self) {
        if let Some(mut resources) = self.resources.take() {
            resources.owner.abandon();
            // Abandonment is not lock or recovery. Retain the complete bundle
            // and occupied owner slot rather than drop leases beneath an
            // unjoined worker. Process-wide admission was closed first.
            std::mem::forget(resources);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::linux_creation_record::Record;
    use super::*;
    use ed25519_dalek::SigningKey;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn signing_identity() -> SigningIdentity {
        let key = [
            0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
            0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
            0x66, 0x66, 0x66, 0x66,
        ];
        let mut blob = Vec::new();
        ssh_string(b"ssh-ed25519", &mut blob).unwrap();
        ssh_string(&key, &mut blob).unwrap();
        SigningIdentity::new(
            blob,
            key,
            [DOMAIN, b"public challenge"].concat(),
            b"vivac.device-key.v1:public-fixture".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn identity_rejects_wrong_key_domain_comment_or_extra_blob_bytes() {
        for changed in ["key", "domain", "comment", "trailing"] {
            let mut identity = signing_identity();
            match changed {
                "key" => identity.public_key[0] ^= 1,
                "domain" => identity.challenge = b"wrong-domain".to_vec(),
                "comment" => identity._comment.push(0),
                "trailing" => identity.blob.push(0),
                _ => unreachable!(),
            }
            assert!(SigningIdentity::new(
                identity.blob,
                identity.public_key,
                identity.challenge,
                identity._comment
            )
            .is_err());
        }
    }

    fn creation_identity_fixture() -> (LocalCreationChallenge, Vec<u8>) {
        let identity = signing_identity();
        let record = Record {
            version: 2,
            installation_id: "00000000000000000000000001".to_owned(),
            creation_id: "00000000000000000000000002".to_owned(),
            device_id: "00000000000000000000000003".to_owned(),
            credential_id: "00000000000000000000000004".to_owned(),
            public_key: identity
                .public_key
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            ciphertext_sha256: "11".repeat(32),
            manifest_sha256: "22".repeat(32),
            ciphertext_len: 128,
            manifest_len: 256,
            creation_profile_revision: 1,
            secret_policy_revision: 1,
        };
        (
            LocalCreationChallenge::issue(&record).unwrap(),
            identity.blob,
        )
    }

    #[test]
    fn creation_binding_identity_uses_only_the_typed_challenge_and_exact_native_comment() {
        let (challenge, blob) = creation_identity_fixture();
        let identity = SigningIdentity::for_creation_binding(
            blob.clone(),
            challenge.comment().to_vec(),
            &challenge,
        )
        .unwrap();
        assert_eq!(identity.blob, blob);
        assert_eq!(&identity.public_key, challenge.public_key());
        assert_eq!(identity.challenge, challenge.bytes());
        assert_eq!(identity._comment, challenge.comment());
    }

    #[test]
    fn creation_binding_identity_refuses_different_or_malformed_native_comments() {
        let (challenge, blob) = creation_identity_fixture();
        let mut trailing = challenge.comment().to_vec();
        trailing.push(0);
        for comment in [
            b"vivac.device-key.v1:00000000000000000000000005:00000000000000000000000004".to_vec(),
            b"vivac.device-key.v1:00000000000000000000000003:00000000000000000000000005".to_vec(),
            b"vivac.device-key.v1:public-fixture".to_vec(),
            Vec::new(),
            vec![b'x'; 129],
            vec![0xff],
            trailing,
        ] {
            assert!(
                SigningIdentity::for_creation_binding(blob.clone(), comment, &challenge).is_err()
            );
        }
    }

    #[test]
    fn creation_binding_identity_refuses_other_keys_algorithms_and_blob_boundaries() {
        let (challenge, blob) = creation_identity_fixture();
        let other_key = SigningKey::from_bytes(&[8; 32]).verifying_key();
        let mut another_key = Vec::new();
        ssh_string(b"ssh-ed25519", &mut another_key).unwrap();
        ssh_string(other_key.as_bytes(), &mut another_key).unwrap();
        let mut another_algorithm = Vec::new();
        ssh_string(b"ssh-rsa", &mut another_algorithm).unwrap();
        ssh_string(challenge.public_key(), &mut another_algorithm).unwrap();
        let mut trailing = blob.clone();
        trailing.push(0);
        let mut truncated = blob;
        truncated.pop();
        for changed in [another_key, another_algorithm, trailing, truncated] {
            assert!(SigningIdentity::for_creation_binding(
                changed,
                challenge.comment().to_vec(),
                &challenge,
            )
            .is_err());
        }
    }

    #[test]
    fn enrollment_identity_constructor_refuses_the_local_creation_purpose() {
        let (challenge, blob) = creation_identity_fixture();
        assert!(SigningIdentity::new(
            blob,
            *challenge.public_key(),
            challenge.bytes().to_vec(),
            challenge.comment().to_vec(),
        )
        .is_err());
    }

    fn creation_identity_reply(blob: &[u8], comment: &[u8]) -> Vec<u8> {
        let mut reply = vec![12];
        reply.extend_from_slice(&1u32.to_be_bytes());
        ssh_string(blob, &mut reply).unwrap();
        ssh_string(comment, &mut reply).unwrap();
        reply
    }

    #[test]
    fn creation_identity_reply_requires_one_exact_native_identity() {
        let (challenge, blob) = creation_identity_fixture();
        let reply = creation_identity_reply(&blob, challenge.comment());
        let identity = parse_creation_identity(&reply, &challenge).unwrap();
        assert_eq!(identity.blob, blob);
        assert_eq!(&identity.public_key, challenge.public_key());
        assert_eq!(identity.challenge, challenge.bytes());
        assert_eq!(identity._comment, challenge.comment());
    }

    #[test]
    fn creation_identity_reply_refuses_wrong_type_count_and_packet_boundaries() {
        let (challenge, blob) = creation_identity_fixture();
        let reply = creation_identity_reply(&blob, challenge.comment());
        let mut wrong_type = reply.clone();
        wrong_type[0] = 13;
        let mut empty_count = reply.clone();
        empty_count[1..5].copy_from_slice(&0u32.to_be_bytes());
        let mut extra_count = reply.clone();
        extra_count[1..5].copy_from_slice(&2u32.to_be_bytes());
        let mut trailing = reply.clone();
        trailing.push(0);
        let mut truncated = reply;
        truncated.pop();
        for packet in [
            Vec::new(),
            vec![12],
            wrong_type,
            empty_count,
            extra_count,
            trailing,
            truncated,
            vec![12; LIMIT + 1],
        ] {
            assert!(matches!(
                parse_creation_identity(&packet, &challenge),
                Err(CreationIdentityError::InvalidReply)
            ));
        }
    }

    #[test]
    fn creation_identity_reply_refuses_substituted_key_and_comment() {
        let (challenge, blob) = creation_identity_fixture();
        let other_key = SigningKey::from_bytes(&[8; 32]).verifying_key();
        let mut another_blob = Vec::new();
        ssh_string(b"ssh-ed25519", &mut another_blob).unwrap();
        ssh_string(other_key.as_bytes(), &mut another_blob).unwrap();
        let wrong_key = creation_identity_reply(&another_blob, challenge.comment());
        assert!(matches!(
            parse_creation_identity(&wrong_key, &challenge),
            Err(CreationIdentityError::IdentityMismatch)
        ));
        let wrong_comment = creation_identity_reply(
            &blob,
            b"vivac.device-key.v1:00000000000000000000000003:00000000000000000000000005",
        );
        assert!(matches!(
            parse_creation_identity(&wrong_comment, &challenge),
            Err(CreationIdentityError::IdentityMismatch)
        ));
    }

    #[test]
    fn creation_identity_reply_classifies_malformed_nested_blobs_as_invalid() {
        let (challenge, blob) = creation_identity_fixture();
        let mut wrong_algorithm = Vec::new();
        ssh_string(b"ssh-rsa", &mut wrong_algorithm).unwrap();
        ssh_string(challenge.public_key(), &mut wrong_algorithm).unwrap();
        let mut short_key = Vec::new();
        ssh_string(b"ssh-ed25519", &mut short_key).unwrap();
        ssh_string(&challenge.public_key()[..31], &mut short_key).unwrap();
        let mut trailing = blob.clone();
        trailing.push(0);
        let mut truncated = blob;
        truncated.pop();
        for blob in [wrong_algorithm, short_key, trailing, truncated] {
            let reply = creation_identity_reply(&blob, challenge.comment());
            assert!(matches!(
                parse_creation_identity(&reply, &challenge),
                Err(CreationIdentityError::InvalidReply)
            ));
        }
    }

    #[test]
    fn signature_packet_refuses_trailing_bytes_wrong_algorithm_and_wrong_length() {
        let mut inner = Vec::new();
        ssh_string(b"ssh-ed25519", &mut inner).unwrap();
        ssh_string(&[7; 64], &mut inner).unwrap();
        let mut packet = vec![14];
        ssh_string(&inner, &mut packet).unwrap();
        assert_eq!(parse_signature(&packet).unwrap(), [7; 64]);
        let mut trailing = packet.clone();
        trailing.push(0);
        assert!(parse_signature(&trailing).is_err());
        packet[9] ^= 1;
        assert!(parse_signature(&packet).is_err());
        assert!(parse_signature(&[14, 255, 255, 255, 255]).is_err());
    }

    #[test]
    fn expired_proxy_requests_do_not_enqueue_owner_side_effects() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let resource = ProxyCloseResource { sender };
        assert_eq!(resource.stop(Instant::now()), Err(Error::Timeout));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(receiver);
        assert_eq!(
            resource.stop(Instant::now() + Duration::from_secs(1)),
            Err(Error::Uncertain)
        );
    }

    #[test]
    fn lost_owner_response_is_uncertain_and_never_a_close_ack() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let resource = ProxyCloseResource { sender };
        let worker =
            thread::spawn(move || resource.release(Instant::now() + Duration::from_secs(2)));
        let message = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(message.operation, Operation::Release));
        drop(message);
        assert_eq!(worker.join().unwrap(), Err(Error::Uncertain));
    }

    #[test]
    fn package_capability_survives_until_the_last_signer_borrow_is_dropped() {
        let (_directory, lease) = public_lease();
        let lease = Arc::new(lease);
        let observed = Arc::downgrade(&lease);
        let signer = NativeSigner {
            socket: PathBuf::from("/private/unused.sock"),
            peer: [101, 1000, 1000],
            identity: signing_identity(),
            lease: lease.clone(),
        };
        assert_eq!(Arc::strong_count(&lease), 2);
        drop(lease);
        assert!(observed.upgrade().unwrap().validate().is_ok());
        drop(signer);
        assert!(observed.upgrade().is_none());
    }

    #[test]
    fn release_guard_vetoes_a_live_borrow_after_closing_and_drain() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Mutex;

        struct GuardedResource {
            gate: Mutex<Weak<Gate>>,
            lease: Mutex<Option<Arc<PackageLease>>>,
            saw_closing: AtomicBool,
            saw_inactive: AtomicBool,
            released: AtomicBool,
        }
        impl CloseResource for GuardedResource {
            fn stop(&self, _: Instant) -> Result<(), Error> {
                Ok(())
            }

            fn release(&self, _: Instant) -> Result<(), Error> {
                let gate = self.gate.lock().unwrap();
                let snapshot = gate.upgrade().ok_or(Error::Uncertain)?.snapshot()?;
                self.saw_closing
                    .store(snapshot.state == State::Closing, Ordering::SeqCst);
                self.saw_inactive.store(!snapshot.active, Ordering::SeqCst);
                let mut lease = self.lease.lock().unwrap();
                authorize_release(&gate, lease.as_ref().ok_or(Error::Uncertain)?)?;
                drop(lease.take());
                self.released.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        for borrowed in [true, false] {
            let (_directory, lease) = public_lease();
            let lease = Arc::new(lease);
            let extra = borrowed.then(|| lease.clone());
            let resource = Arc::new(GuardedResource {
                gate: Mutex::new(Weak::new()),
                lease: Mutex::new(Some(lease)),
                saw_closing: AtomicBool::new(false),
                saw_inactive: AtomicBool::new(false),
                released: AtomicBool::new(false),
            });
            let gate = Gate::new(resource.clone());
            *resource.gate.lock().unwrap() = Arc::downgrade(&gate);
            assert_eq!(
                gate.lock(Duration::from_secs(1)),
                if borrowed {
                    Err(Error::Uncertain)
                } else {
                    Ok(())
                }
            );
            assert!(resource.saw_closing.load(Ordering::SeqCst));
            assert!(resource.saw_inactive.load(Ordering::SeqCst));
            let snapshot = gate.snapshot().unwrap();
            assert!(!snapshot.active);
            assert_eq!(resource.released.load(Ordering::SeqCst), !borrowed);
            assert_eq!(snapshot.close_acknowledged, !borrowed);
            assert_eq!(snapshot.resources_closed_after_drain, !borrowed);
            assert_eq!(snapshot.lock_acknowledgments, u64::from(!borrowed));
            assert_eq!(
                snapshot.state,
                if borrowed {
                    State::Closing
                } else {
                    State::Closed
                }
            );
            drop(extra);
            if borrowed {
                assert_eq!(gate.lock(Duration::from_secs(1)), Err(Error::Uncertain));
                assert!(!resource.released.load(Ordering::SeqCst));
                assert!(!gate.snapshot().unwrap().close_acknowledged);
            }
        }
    }

    #[test]
    fn release_receipts_must_match_both_real_tokens_pids_and_expected_exits() {
        let agent = Receipt {
            token: 1,
            pid: 101,
            creator_tid: 7,
        };
        let loader = Receipt {
            token: 2,
            pid: 102,
            creator_tid: 7,
        };
        let correct = [
            ExitReceipt {
                token: 1,
                pid: 101,
                code: None,
                signal: Some(libc::SIGKILL),
            },
            ExitReceipt {
                token: 2,
                pid: 102,
                code: Some(0),
                signal: None,
            },
        ];
        assert!(expected_exits(agent, loader, &correct));
        for changed in ["token", "pid", "signal", "loader-code"] {
            let mut exits = correct;
            match changed {
                "token" => exits[0].token = 3,
                "pid" => exits[0].pid = 103,
                "signal" => exits[0].signal = Some(libc::SIGTERM),
                "loader-code" => exits[1].code = Some(1),
                _ => unreachable!(),
            }
            assert!(!expected_exits(agent, loader, &exits));
        }
        assert!(!expected_exits(agent, loader, &correct[..1]));
    }

    #[test]
    fn active_closure_refuses_a_gate_result_without_a_successful_endpoint_exchange() {
        struct EmptyResource;
        impl CloseResource for EmptyResource {
            fn stop(&self, _: Instant) -> Result<(), Error> {
                Ok(())
            }
            fn release(&self, _: Instant) -> Result<(), Error> {
                Ok(())
            }
        }
        let gate = Gate::new(Arc::new(EmptyResource));
        let id = gate
            .start(|request| {
                request.native_sent()?;
                request.native_reply()?;
                request.verified()?;
                Ok([1; 64])
            })
            .unwrap();
        gate.wait_request(id, Duration::from_secs(2)).unwrap();
        gate.lock(Duration::from_secs(2)).unwrap();
        let state = gate.snapshot().unwrap();
        assert_eq!(state.state, State::Closed);
        assert_eq!(state.proofs_published, 1);
        let generation = [1; 32];
        let store = [2; 6];
        let roles = [1, 2].map(|id| Receipt {
            token: u64::from(id),
            pid: id,
            creator_tid: 7,
        });
        let (_sender, receiver) = mpsc::sync_channel(1);
        let mut session = NativeSession {
            generation,
            store,
            roles,
            endpoint_proof_sent: std::cell::Cell::new(false),
            active_closure_taken: false,
            lifetime: Arc::new(()),
            endpoint_attempt_slot: Arc::new(AtomicBool::new(false)),
            resources: None,
            gate,
            receiver,
            exits: Some(vec![
                ExitReceipt {
                    token: roles[0].token,
                    pid: roles[0].pid,
                    code: None,
                    signal: Some(libc::SIGKILL),
                },
                ExitReceipt {
                    token: roles[1].token,
                    pid: roles[1].pid,
                    code: Some(0),
                    signal: None,
                },
            ]),
        };
        assert!(session
            .consume_active_closure(generation, store, roles)
            .is_err());
        assert!(session.active_closure_taken);
        session.endpoint_proof_sent.set(true);
        assert!(session
            .consume_active_closure(generation, store, roles)
            .is_err());
    }

    #[test]
    fn socket_peer_requires_the_owned_agent_pid_uid_and_gid() {
        let expected = [101, 1000, 1000];
        assert!(peer_matches(expected, expected));
        for observed in [[102, 1000, 1000], [101, 1001, 1000], [101, 1000, 1001]] {
            assert!(!peer_matches(expected, observed));
        }
        assert!(!peer_matches([0, 1000, 1000], [0, 1000, 1000]));
    }

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
    struct TestDirectory(std::path::PathBuf);
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn public_lease() -> (TestDirectory, PackageLease) {
        let directory = TestDirectory(std::env::temp_dir().join(format!(
            "vivac-concurrent-lease-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::create_dir(&directory.0).expect("private test directory");
        std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700))
            .expect("private test directory mode");
        for (name, bytes) in [
            ("key.openssh", b"public ciphertext fixture".as_slice()),
            ("manifest.json", b"public manifest fixture".as_slice()),
        ] {
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.0.join(name))
                .expect("public test fixture");
            file.write_all(bytes).expect("public test fixture bytes");
        }
        let capability: OwnedFd = File::open(&directory.0)
            .expect("directory capability")
            .into();
        let cipher: OwnedFd = File::open(directory.0.join("key.openssh"))
            .expect("cipher fixture")
            .into();
        let lease = PackageLease::capture(capability, &cipher).expect("public baseline");
        (directory, lease)
    }

    #[test]
    fn endpoint_session_witness_refuses_to_retain_a_package_lease() {
        struct EmptyResource;
        impl CloseResource for EmptyResource {
            fn stop(&self, _: Instant) -> Result<(), Error> {
                Ok(())
            }
            fn release(&self, _: Instant) -> Result<(), Error> {
                Ok(())
            }
        }
        let (_directory, lease) = public_lease();
        let lease = Arc::new(lease);
        let observed = Arc::downgrade(&lease);
        let gate = Gate::new(Arc::new(EmptyResource));
        let generation = endpoint_witness::new_generation().unwrap();
        let store = lease.ipc_identity().unwrap();
        let lifetime = Arc::new(());
        let witness = EndpointSessionWitness::capture(&gate, &lifetime, generation, store).unwrap();
        assert_eq!(Arc::strong_count(&lease), 1);
        assert_eq!(witness.generation().unwrap(), generation);
        assert_eq!(witness.store_identity().unwrap(), store);
        drop(lease);
        assert!(observed.upgrade().is_none());
        assert!(witness.check_live().is_ok());
        gate.lock(Duration::from_secs(1)).unwrap();
        assert!(witness.check_live().is_err());
    }

    #[test]
    fn endpoint_attempt_keeps_only_slot_and_weak_session_lifetime_without_a_lease() {
        struct EmptyResource;
        impl CloseResource for EmptyResource {
            fn stop(&self, _: Instant) -> Result<(), Error> {
                Ok(())
            }
            fn release(&self, _: Instant) -> Result<(), Error> {
                Ok(())
            }
        }
        let (_directory, lease) = public_lease();
        let lease = Arc::new(lease);
        let observed = Arc::downgrade(&lease);
        let gate = Gate::new(Arc::new(EmptyResource));
        let lifetime = Arc::new(());
        let slot = Arc::new(AtomicBool::new(false));
        let store = lease.ipc_identity().unwrap();
        let attempt =
            EndpointProofAttempt::issue(&slot, Instant::now() + Duration::from_secs(10), || {
                EndpointSessionWitness::capture(&gate, &lifetime, [7; 32], store)
            })
            .unwrap();
        assert_eq!(Arc::strong_count(&lease), 1);
        assert_eq!(Arc::strong_count(&lifetime), 1);
        assert_eq!(Arc::strong_count(&slot), 2);
        drop(lease);
        assert!(observed.upgrade().is_none());
        assert!(attempt.check_live().is_ok());
        gate.lock(Duration::from_secs(1)).unwrap();
        assert!(attempt.check_live().is_err());
        drop(attempt);
        assert_eq!(Arc::strong_count(&slot), 1);
    }

    #[test]
    fn reservation_snapshot_keeps_valid_capabilities_after_original_is_dropped() {
        let (_directory, lease) = public_lease();
        let identity = lease.ipc_identity().unwrap();
        let snapshot = lease.duplicate_for_reservation().unwrap();
        drop(lease);
        assert_eq!(snapshot.ipc_identity().unwrap(), identity);
        let directory = snapshot.open_independent_directory().unwrap();
        assert!(snapshot.validate_reservation_directory(&directory).is_ok());
    }

    #[test]
    fn reservation_snapshot_detects_mutation_and_refuses_another_directory() {
        let (directory, lease) = public_lease();
        let snapshot = lease.duplicate_for_reservation().unwrap();
        let (_other_directory, other) = public_lease();
        let other_directory = other.open_independent_directory().unwrap();
        assert!(snapshot
            .validate_reservation_directory(&other_directory)
            .is_err());
        File::options()
            .write(true)
            .open(directory.0.join("manifest.json"))
            .unwrap()
            .write_at(b"X", 0)
            .unwrap();
        assert!(snapshot.validate().is_err());
        assert!(snapshot.open_independent_directory().is_err());
        assert!(lease.duplicate_for_reservation().is_err());
    }

    #[test]
    fn anchored_lease_refuses_changed_bytes_permissions_links_and_replacements() {
        for mutation in [
            "bytes",
            "mode",
            "hardlink",
            "replacement",
            "symlink",
            "manifest",
        ] {
            let (directory, lease) = public_lease();
            assert!(lease.validate().is_ok());
            let key = directory.0.join("key.openssh");
            match mutation {
                "bytes" => {
                    File::options()
                        .write(true)
                        .open(&key)
                        .expect("public key fixture")
                        .write_at(b"X", 0)
                        .expect("changed ciphertext fixture");
                }
                "mode" => std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644))
                    .expect("changed fixture permissions"),
                "hardlink" => std::fs::hard_link(&key, directory.0.join("extra-link"))
                    .expect("extra fixture link"),
                "replacement" => {
                    std::fs::rename(&key, directory.0.join("previous"))
                        .expect("retain previous fixture");
                    let mut replacement = File::options()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&key)
                        .expect("replacement fixture");
                    replacement
                        .write_all(b"public ciphertext fixture")
                        .expect("replacement bytes");
                }
                "symlink" => {
                    std::fs::rename(&key, directory.0.join("previous"))
                        .expect("retain previous fixture");
                    std::os::unix::fs::symlink("previous", &key).expect("fixture symbolic link");
                }
                "manifest" => {
                    File::options()
                        .write(true)
                        .open(directory.0.join("manifest.json"))
                        .expect("public manifest fixture")
                        .write_at(b"X", 0)
                        .expect("changed manifest fixture");
                }
                _ => unreachable!(),
            }
            assert_eq!(lease.validate(), Err(Error::LeaseInvalidated));
        }
    }
}
