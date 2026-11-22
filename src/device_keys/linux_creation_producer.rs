use crate::creation_input::ConfirmedGeneratedSecret;
use crate::linux_admission_storage::{PublishError, PublishStage};
use crate::linux_creation_binding::{LocalCreationChallenge, VerifiedCreationBinding};
use crate::linux_creation_record::Record;
use crate::linux_creation_storage::Storage;
use crate::linux_current_admission::CurrentAdmission;
use crate::linux_gate::{Error, Gate, RequestOutcome, Snapshot, State};
use crate::linux_loader::PtyLoader;
use crate::linux_reservation::StoreReservation;
use crate::linux_session::{NativeSession, PackageLease, SignStage};
use crate::linux_spawn::{ExitReceipt, Receipt, SpawnOwner};
use crate::opening_publication::{self, OpeningPublication};
use crate::package::{CreateError, CreationDraft, Location, PackagePublishStage, PreparedPackage};
use crate::ubuntu_provider::UbuntuProvider;
use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, CreateError>;

fn refused<T>(_: T) -> CreateError {
    CreateError::Refused
}

pub(crate) fn prepare_package(
    location: Location,
    comment: &str,
) -> Result<(ConfirmedGeneratedSecret, PreparedPackage)> {
    let ciphertext = crate::creation_ciphertext::AttendedCiphertext::read(comment)?;
    let (confirmation, encoded) = ciphertext.into_parts();
    let package = crate::package::prepare(location, comment, encoded.as_bytes())?;
    Ok((confirmation, package))
}

fn require(value: bool) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(CreateError::Refused)
    }
}

pub(crate) struct CreationRuntime {
    path: PathBuf,
    directory: File,
    identity: (u64, u64),
    cleanup_on_drop: bool,
}

pub(crate) struct AgentSocket {
    runtime_identity: (u64, u64),
    socket_identity: (u64, u64),
}

impl CreationRuntime {
    pub(crate) fn create() -> Result<Self> {
        let mut template = b"/tmp/vivac-create-XXXXXX\0".to_vec();
        // SAFETY: mkdtemp receives the writable, NUL-terminated template.
        let created = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
        require(!created.is_null())?;
        // SAFETY: successful mkdtemp retains a NUL-terminated name in template.
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(unsafe {
            CStr::from_ptr(created).to_bytes()
        }));
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(refused)?;
        let metadata = directory.metadata().map_err(refused)?;
        let runtime = Self {
            path,
            directory,
            identity: (metadata.dev(), metadata.ino()),
            cleanup_on_drop: true,
        };
        runtime.validate()?;
        Ok(runtime)
    }

    pub(crate) fn create_session() -> Result<Self> {
        let mut runtime = Self::create()?;
        runtime.cleanup_on_drop = false;
        Ok(runtime)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn validate(&self) -> Result<()> {
        private_directory(&self.path)?;
        for metadata in [
            self.directory.metadata().map_err(refused)?,
            std::fs::symlink_metadata(&self.path).map_err(refused)?,
        ] {
            require((metadata.dev(), metadata.ino()) == self.identity)?;
        }
        Ok(())
    }

    pub(crate) fn cleanup(&self) -> Result<()> {
        self.validate()?;
        for (name, directory) in [("agent.sock", false), ("home", true)] {
            self.validate()?;
            let path = self.path.join(name);
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(refused(error)),
            };
            require(metadata.uid() == unsafe { libc::geteuid() })?;
            require(if directory {
                metadata.is_dir() && metadata.mode() & 0o7777 == 0o700
            } else {
                metadata.file_type().is_socket()
            })?;
            let name = CString::new(name).map_err(refused)?;
            self.validate()?;
            // SAFETY: the descriptor is retained and the name is one fixed child.
            require(unsafe {
                libc::unlinkat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    if directory { libc::AT_REMOVEDIR } else { 0 },
                ) == 0
            })?;
        }
        self.validate()?;
        std::fs::remove_dir(&self.path).map_err(refused)
    }

    pub(crate) fn capture_agent_socket(&self) -> Result<AgentSocket> {
        self.validate()?;
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the fixed name is relative to the retained runtime directory,
        // and fstatat initializes the complete output on success.
        require(unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                c"agent.sock".as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            ) == 0
        })?;
        // SAFETY: successful fstatat initialized metadata.
        let metadata = unsafe { metadata.assume_init() };
        require(
            metadata.st_mode & libc::S_IFMT == libc::S_IFSOCK
                && metadata.st_dev == self.identity.0
                && metadata.st_uid == unsafe { libc::geteuid() }
                && metadata.st_gid == unsafe { libc::getegid() },
        )?;
        self.validate()?;
        Ok(AgentSocket {
            runtime_identity: self.identity,
            socket_identity: (metadata.st_dev, metadata.st_ino),
        })
    }

    pub(crate) fn cleanup_after_session(&self, expected: AgentSocket) -> Result<()> {
        self.validate()?;
        let current = self.capture_agent_socket()?;
        require(
            expected.runtime_identity == self.identity
                && current.runtime_identity == expected.runtime_identity
                && current.socket_identity == expected.socket_identity,
        )?;
        // SAFETY: this removes only the fixed socket name from the retained
        // directory after its identity check and the caller's session closure.
        require(unsafe {
            libc::unlinkat(self.directory.as_raw_fd(), c"agent.sock".as_ptr(), 0) == 0
        })?;
        self.validate()?;
        std::fs::remove_dir(&self.path).map_err(refused)
    }

    pub(crate) fn cleanup_after_native_teardown(&self) -> Result<()> {
        require(!self.cleanup_on_drop && !SpawnOwner::admission_uncertain())?;
        self.validate()?;
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: this queries the fixed name without following links and
        // supplies writable storage for a complete stat object.
        let result = unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                c"agent.sock".as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        require(result < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT))?;
        self.validate()?;
        std::fs::remove_dir(&self.path).map_err(refused)
    }
}

impl Drop for CreationRuntime {
    fn drop(&mut self) {
        if self.cleanup_on_drop && !SpawnOwner::admission_uncertain() {
            // Preserve unknown children and changed identities for inspection.
            let _ = self.cleanup();
        }
    }
}

pub(crate) fn create_attended(device: &str, credential: &str) -> Result<PublicationResult> {
    require(!SpawnOwner::admission_uncertain())?;
    // Check the required collection before secret delivery or package staging.
    crate::linux_custody_root::CandidateRoot::open_fixed_namespace()
        .and_then(|root| root.with_opening_directory(|_| Ok(())))
        .map_err(refused)?;
    let runtime = CreationRuntime::create()?;
    let timeout = Duration::from_secs(15);
    let (creation, session) =
        AttendedCreation::start(&runtime.path, device, credential, timeout, |_| Ok(()))?;
    let proof = creation.prove_and_close(session, timeout, |_| Ok(()), |_, _| Ok(()))?;
    let closed = proof.draft.map_err(refused)?;
    Ok(closed.publish_with(PackageCheckpoints::Disabled, true, |_| Ok(())))
}

fn check_closed_session_state(released: bool, snapshot: Snapshot) -> Result<()> {
    check_proof_session_state(released, snapshot, true)
}

fn check_proof_session_state(released: bool, snapshot: Snapshot, published: bool) -> Result<()> {
    require(
        released
            && snapshot.state == State::Closed
            && !snapshot.active
            && snapshot.close_acknowledged
            && snapshot.resources_closed_after_drain
            && snapshot.proofs_published == u64::from(published)
            && snapshot.native_requests_sent == 1
            && snapshot.native_replies_received == 1
            && snapshot.proofs_verified == 1
            && snapshot.workers_joined == 1
            && snapshot.lock_callers == 1
            && snapshot.lock_acknowledgments == 1,
    )
}

fn matches_session_gate(expected: &Weak<Gate>, actual: &Arc<Gate>) -> bool {
    expected
        .upgrade()
        .is_some_and(|gate| Arc::ptr_eq(&gate, actual))
}

fn check_closed_exits(exits: &[ExitReceipt], [agent, loader]: [Receipt; 2]) -> Result<()> {
    require(exits.len() == 2)?;
    let loader_exit = exits
        .iter()
        .find(|exit| exit.token == loader.token && exit.pid == loader.pid)
        .ok_or(CreateError::Refused)?;
    require(loader_exit.code == Some(0) && loader_exit.signal.is_none())?;
    let agent_exit = exits
        .iter()
        .find(|exit| exit.token == agent.token && exit.pid == agent.pid)
        .ok_or(CreateError::Refused)?;
    require(agent_exit.code.is_none() && agent_exit.signal == Some(libc::SIGKILL))
}

fn duplicate(descriptor: RawFd) -> Result<OwnedFd> {
    let result = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
    require(result >= 3)?;
    // SAFETY: successful fcntl returned a fresh owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(result) })
}

fn private_directory(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(refused)?;
    require(
        metadata.is_dir()
            && metadata.mode() & 0o7777 == 0o700
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.gid() == unsafe { libc::getegid() },
    )
}

fn native_credentials(pid: u32) -> Result<()> {
    let file = File::open(format!("/proc/{pid}/status")).map_err(refused)?;
    let mut bytes = Vec::new();
    file.take(16385).read_to_end(&mut bytes).map_err(refused)?;
    require(bytes.len() <= 16384)?;
    let status = std::str::from_utf8(&bytes).map_err(refused)?;
    for (key, expected) in [
        ("Uid:", unsafe { libc::geteuid() }),
        ("Gid:", unsafe { libc::getegid() }),
    ] {
        let values = status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .ok_or(())?
            .split_whitespace()
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(refused)?;
        require(values.len() == 4 && values.iter().all(|value| *value == expected))?;
    }
    require(status.lines().any(|line| {
        line.strip_prefix("NoNewPrivs:")
            .is_some_and(|value| value.trim() == "1")
    }))
}

fn empty_agent(
    owner: &mut SpawnOwner,
    agent: Receipt,
    socket: &Path,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        require(
            Instant::now() < deadline && owner.try_wait(agent.token).map_err(refused)?.is_none(),
        )?;
        match UnixStream::connect(socket) {
            Ok(mut connection) => {
                connection
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .map_err(refused)?;
                connection
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .map_err(refused)?;
                connection.write_all(&[0, 0, 0, 1, 11]).map_err(refused)?;
                let mut length = [0; 4];
                connection.read_exact(&mut length).map_err(refused)?;
                require(u32::from_be_bytes(length) == 5)?;
                let mut response = [0; 5];
                connection.read_exact(&mut response).map_err(refused)?;
                require(response == [12, 0, 0, 0, 0])?;
                native_credentials(agent.pid)?;
                return require(owner.try_wait(agent.token).map_err(refused)?.is_none());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                thread::sleep(Duration::from_millis(10))
            }
            Err(_) => return Err(CreateError::Refused),
        }
    }
}

struct SessionSetup {
    owner: SpawnOwner,
    agent_token: u64,
    loader: PtyLoader,
    lease: Arc<PackageLease>,
    retained: Vec<OwnedFd>,
    socket: PathBuf,
}

pub(crate) enum StartupEvent<'a> {
    LoaderReady {
        master: &'a OwnedFd,
        record: &'a Record,
        agent: Receipt,
        loader: Receipt,
    },
    LoaderHandoff,
}

struct ProducerContext {
    draft: CreationDraft,
    provider: UbuntuProvider,
}

impl ProducerContext {
    fn validate(&self) -> std::result::Result<(), ()> {
        self.provider.revalidate().map_err(|_| ())?;
        self.draft.validate()?;
        self.provider.revalidate().map_err(|_| ())
    }
}

fn validate_context(context: &Arc<Mutex<ProducerContext>>) -> std::result::Result<(), Error> {
    context
        .try_lock()
        .map_err(|_| Error::LeaseInvalidated)?
        .validate()
        .map_err(|_| Error::LeaseInvalidated)
}

pub(crate) enum ProofEvent<'a> {
    BeforePublication,
    Verified {
        challenge: &'a [u8],
        signature: &'a [u8; 64],
    },
    LeaseInvalidated,
}

pub(crate) struct LeaseInvalidated;

pub(crate) struct ProofResult {
    pub(crate) draft: std::result::Result<ClosedCreationDraft, LeaseInvalidated>,
    pub(crate) snapshot: Snapshot,
}

pub(crate) struct AttendedCreation {
    confirmation: Option<ConfirmedGeneratedSecret>,
    context: Arc<Mutex<ProducerContext>>,
    handoff_completed: bool,
    registered_roles: Option<[Receipt; 2]>,
    session_attempted: bool,
    session_gate: Option<Weak<Gate>>,
}

impl AttendedCreation {
    pub(crate) fn start(
        runtime: &Path,
        device: &str,
        credential: &str,
        timeout: Duration,
        mut observe: impl FnMut(StartupEvent<'_>) -> Result<()>,
    ) -> Result<(Self, NativeSession)> {
        private_directory(runtime)?;
        let home = runtime.join("home");
        let socket = runtime.join("agent.sock");
        require(
            matches!(std::fs::symlink_metadata(&socket), Err(error) if error.kind() == io::ErrorKind::NotFound),
        )?;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&home)
            .map_err(refused)?;
        private_directory(&home)?;
        let mut creation = Self::prepare(device, credential)?;
        let [agent_fd, loader_fd] = creation.provider_executables()?;
        let (directory, cipher) = creation.native_material()?;
        let record = creation.record()?;
        let context = creation.context();
        context.try_lock().map_err(refused)?.validate()?;
        let lease = Arc::new(PackageLease::capture(directory, &cipher).map_err(refused)?);
        let reservation =
            StoreReservation::acquire_retaining(&lease, context.clone()).map_err(refused)?;
        let mut owner = SpawnOwner::new_reserved(reservation).map_err(refused)?;
        let agent = owner
            .spawn(crate::ubuntu_provider::agent_spec(&agent_fd, &socket, &home).map_err(refused)?)
            .map_err(refused)?;
        empty_agent(&mut owner, agent, &socket, timeout)?;
        let mut loader = PtyLoader::start(
            &mut owner,
            duplicate(loader_fd.as_raw_fd())?,
            duplicate(cipher.as_raw_fd())?,
            &socket,
            &home,
        )
        .map_err(refused)?;
        loader.wait_prompt(&mut owner, timeout).map_err(refused)?;
        let loader_receipt = loader.receipt();
        native_credentials(loader_receipt.pid)?;
        let master = loader.duplicate_master().map_err(refused)?;
        observe(StartupEvent::LoaderReady {
            master: &master,
            record: &record,
            agent,
            loader: loader_receipt,
        })?;
        context.try_lock().map_err(refused)?.validate()?;
        creation.handoff_stopped(&mut owner, agent, &mut loader, timeout)?;
        observe(StartupEvent::LoaderHandoff)?;
        context.try_lock().map_err(refused)?.validate()?;
        loader.complete(&mut owner, timeout).map_err(refused)?;
        context.try_lock().map_err(refused)?.validate()?;
        let session = creation.native_session(SessionSetup {
            owner,
            agent_token: agent.token,
            loader,
            lease,
            retained: vec![master, agent_fd, loader_fd, cipher],
            socket,
        })?;
        Ok((creation, session))
    }

    fn prepare(device: &str, credential: &str) -> Result<Self> {
        require(
            crate::creation_ciphertext::valid_identifier(device)
                && crate::creation_ciphertext::valid_identifier(credential),
        )?;
        let provider = UbuntuProvider::select().map_err(refused)?;
        let location = Location::fixed_credential(credential)?;
        let comment = format!("vivac.device-key.v1:{device}:{credential}");
        let (confirmation, package) = prepare_package(location, &comment)?;
        let draft = package.into_creation_draft()?;
        let context = ProducerContext { draft, provider };
        context.validate()?;
        Ok(Self {
            confirmation: Some(confirmation),
            context: Arc::new(Mutex::new(context)),
            handoff_completed: false,
            registered_roles: None,
            session_attempted: false,
            session_gate: None,
        })
    }

    fn record(&self) -> Result<Record> {
        let context = self.context.try_lock().map_err(refused)?;
        context.validate()?;
        Ok(context.draft.record().clone())
    }

    fn native_material(&self) -> Result<(OwnedFd, OwnedFd)> {
        let context = self.context.try_lock().map_err(refused)?;
        context.validate()?;
        let material = context.draft.native_material()?.into_parts();
        context.validate()?;
        Ok(material)
    }

    fn provider_executables(&self) -> Result<[OwnedFd; 2]> {
        let context = self.context.try_lock().map_err(refused)?;
        context.validate()?;
        let executables = context.provider.launch_executables().map_err(refused)?;
        context.validate()?;
        Ok(executables)
    }

    fn context(&self) -> Arc<Mutex<ProducerContext>> {
        self.context.clone()
    }

    fn handoff_stopped(
        &mut self,
        owner: &mut SpawnOwner,
        agent: Receipt,
        loader: &mut PtyLoader,
        timeout: Duration,
    ) -> Result<()> {
        require(!self.handoff_completed && !self.session_attempted)?;
        let loader_receipt = loader.receipt();
        {
            let context = self.context.try_lock().map_err(refused)?;
            context.validate()?;
            let [agent_image, loader_image] =
                context.provider.launch_executables().map_err(refused)?;
            let registration = owner.prepare_child_registration().map_err(refused)?;
            registration
                .validate_role_order([(agent, &agent_image), (loader_receipt, &loader_image)])
                .map_err(refused)?;
            context.validate()?;
            registration.validate().map_err(refused)?;
        }
        let confirmation = self.confirmation.take().ok_or(CreateError::Refused)?;
        let result = loader.handoff_stopped(owner, confirmation.as_bytes(), timeout);
        drop(confirmation);
        let validation = self.context.try_lock().map_err(refused)?.validate();
        result.map_err(refused)?;
        validation?;
        self.registered_roles = Some([agent, loader_receipt]);
        self.handoff_completed = true;
        Ok(())
    }

    fn native_session(&mut self, setup: SessionSetup) -> Result<NativeSession> {
        let [agent, loader] = self.registered_roles.ok_or(CreateError::Refused)?;
        require(
            self.handoff_completed
                && self.confirmation.is_none()
                && !self.session_attempted
                && self.session_gate.is_none()
                && loader == setup.loader.receipt()
                && agent == setup.owner.receipt(setup.agent_token).map_err(refused)?,
        )?;
        self.context.try_lock().map_err(refused)?.validate()?;
        self.session_attempted = true;
        let session = NativeSession::new(
            setup.owner,
            setup.agent_token,
            setup.loader,
            setup.lease,
            setup.retained,
            setup.socket,
        );
        let validation = self.context.try_lock().map_err(refused)?.validate();
        let session = session.map_err(refused)?;
        validation?;
        self.session_gate = Some(Arc::downgrade(&session.gate()));
        self.context.try_lock().map_err(refused)?.validate()?;
        Ok(session)
    }

    pub(crate) fn prove_and_close(
        self,
        mut session: NativeSession,
        timeout: Duration,
        observe: impl Fn(ProofEvent<'_>) -> Result<()> + Send + Sync + 'static,
        observe_closed: impl FnOnce(&NativeSession, Snapshot) -> Result<()>,
    ) -> Result<ProofResult> {
        require(self.handoff_completed && self.confirmation.is_none() && self.session_attempted)?;
        let gate = session.gate();
        require(matches_session_gate(
            self.session_gate.as_ref().ok_or(CreateError::Refused)?,
            &gate,
        ))?;
        let record = self.record()?;
        let challenge = LocalCreationChallenge::issue(&record).map_err(refused)?;
        let context = self.context();
        validate_context(&context).map_err(refused)?;
        let identity = session.creation_identity(&challenge).map_err(refused)?;
        validate_context(&context).map_err(refused)?;
        let signer = session.signer(identity).map_err(refused)?;
        let coordinator = thread::spawn(move || -> Result<_> {
            let observe = Arc::new(observe);
            let operation = (|| -> Result<Option<VerifiedCreationBinding>> {
                let hook_context = context.clone();
                let worker_observe = observe.clone();
                let id = gate
                    .start(move |request| {
                        signer.sign(request, |stage| {
                            if stage == SignStage::BeforePublication {
                                worker_observe(ProofEvent::BeforePublication)
                                    .map_err(|_| Error::Backend)?;
                            }
                            validate_context(&hook_context)
                        })
                    })
                    .map_err(refused)?;
                let outcome = gate.wait_request(id, timeout).map_err(refused)?;
                let proof = gate.take_proof(id).map_err(refused)?;
                match (outcome, proof) {
                    (RequestOutcome::Published, Some(signature)) => {
                        let message = challenge.bytes().to_vec();
                        let binding = challenge.verify(&signature).map_err(refused)?;
                        require(binding.matches_record(&record).map_err(refused)?)?;
                        validate_context(&context).map_err(refused)?;
                        observe(ProofEvent::Verified {
                            challenge: &message,
                            signature: &signature,
                        })?;
                        Ok(Some(binding))
                    }
                    (RequestOutcome::LeaseInvalidated, None) => {
                        observe(ProofEvent::LeaseInvalidated)?;
                        Ok(None)
                    }
                    _ => Err(CreateError::Refused),
                }
            })();
            // Refused observations still stop admission, drain and release.
            gate.lock(timeout).map_err(refused)?;
            let snapshot = gate.snapshot().map_err(refused)?;
            let verified = operation?;
            Ok((verified, snapshot))
        });
        let deadline = Instant::now() + Duration::from_secs(60);
        while !coordinator.is_finished() || !session.released() {
            require(Instant::now() < deadline)?;
            session
                .service(Duration::from_millis(20))
                .map_err(refused)?;
        }
        let (verified, snapshot) = coordinator.join().map_err(refused)??;
        check_proof_session_state(session.released(), snapshot, verified.is_some())?;
        check_closed_exits(
            session.exits().ok_or(CreateError::Refused)?,
            self.registered_roles.ok_or(CreateError::Refused)?,
        )?;
        observe_closed(&session, snapshot)?;
        match verified {
            Some(verified) => Ok(ProofResult {
                draft: Ok(self.close(session, verified)?),
                snapshot,
            }),
            None => {
                drop(session);
                drop(self);
                Ok(ProofResult {
                    draft: Err(LeaseInvalidated),
                    snapshot,
                })
            }
        }
    }

    fn close(
        self,
        session: NativeSession,
        verified: VerifiedCreationBinding,
    ) -> Result<ClosedCreationDraft> {
        require(self.handoff_completed && self.confirmation.is_none() && self.session_attempted)?;
        let gate = session.gate();
        require(matches_session_gate(
            self.session_gate.as_ref().ok_or(CreateError::Refused)?,
            &gate,
        ))?;
        check_closed_session_state(session.released(), gate.snapshot().map_err(refused)?)?;
        check_closed_exits(
            session.exits().ok_or(CreateError::Refused)?,
            self.registered_roles.ok_or(CreateError::Refused)?,
        )?;
        require(verified.matches_record(&self.record()?).map_err(refused)?)?;
        drop(gate);
        drop(session);
        require(Arc::strong_count(&self.context) == 1)?;
        let context = Arc::try_unwrap(self.context)
            .map_err(refused)?
            .into_inner()
            .map_err(refused)?;
        context.validate()?;
        require(
            verified
                .matches_record(context.draft.record())
                .map_err(refused)?,
        )?;
        Ok(ClosedCreationDraft { context, verified })
    }
}

pub(crate) struct ClosedCreationDraft {
    context: ProducerContext,
    verified: VerifiedCreationBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationCheckpoint {
    Package(PackagePublishStage),
    PackageDurable,
    OpeningPermitDurable,
    Record(PublishStage),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackageCheckpoints {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationOutcome {
    Unpublished,
    PackageDurabilityUnknown,
    PackageOnly,
    RecordDurabilityUnknown,
    Durable,
}

impl PublicationOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unpublished => "unpublished",
            Self::PackageDurabilityUnknown => "package-durability-unknown",
            Self::PackageOnly => "package-only",
            Self::RecordDurabilityUnknown => "record-durability-unknown",
            Self::Durable => "durable",
        }
    }
}

struct PublishedCreation {
    package: crate::package::PublishedDraft,
    verified: VerifiedCreationBinding,
    provider: UbuntuProvider,
    resident: CurrentAdmission,
    opening: Option<OpeningPublication>,
}

impl PublishedCreation {
    fn validate(&self) -> Result<()> {
        for _ in 0..2 {
            self.provider.revalidate().map_err(refused)?;
            self.package.validate()?;
            self.resident.validate().map_err(refused)?;
            require(
                self.resident.store_identity().map_err(refused)? == self.package.ipc_identity()?,
            )?;
            require(
                self.resident.canonical_record().map_err(refused)?
                    == self.package.record().canonical_bytes().map_err(refused)?,
            )?;
            require(
                crate::linux_openssh_package::hex(
                    &self.resident.expected_public_key().map_err(refused)?,
                ) == self.package.record().public_key,
            )?;
            require(
                self.resident
                    .matches_record(self.package.record())
                    .map_err(refused)?,
            )?;
            require(
                self.verified
                    .matches_record(self.package.record())
                    .map_err(refused)?,
            )?;
        }
        self.resident.validate().map_err(refused)?;
        self.package.validate()?;
        if let Some(opening) = &self.opening {
            opening.validate(&self.package).map_err(refused)?;
        }
        self.provider.revalidate().map_err(refused)
    }
}

enum PublicationFailure {
    Unpublished,
    PackageDurabilityUnknown,
    PackageOnly,
    RecordDurabilityUnknown,
}

pub(crate) struct PublicationResult {
    result: std::result::Result<PublishedCreation, PublicationFailure>,
}

impl PublicationResult {
    fn failed(failure: PublicationFailure) -> Self {
        Self {
            result: Err(failure),
        }
    }

    pub(crate) fn outcome(&self) -> PublicationOutcome {
        match &self.result {
            Ok(published) if published.validate().is_ok() => PublicationOutcome::Durable,
            Ok(_) | Err(PublicationFailure::RecordDurabilityUnknown) => {
                PublicationOutcome::RecordDurabilityUnknown
            }
            Err(PublicationFailure::Unpublished) => PublicationOutcome::Unpublished,
            Err(PublicationFailure::PackageDurabilityUnknown) => {
                PublicationOutcome::PackageDurabilityUnknown
            }
            Err(PublicationFailure::PackageOnly) => PublicationOutcome::PackageOnly,
        }
    }

    pub(crate) fn record(&self) -> Result<&Record> {
        let published = self.result.as_ref().map_err(refused)?;
        published.validate()?;
        Ok(published.package.record())
    }
}

impl ClosedCreationDraft {
    pub(crate) fn publish_with(
        self,
        package_checkpoints: PackageCheckpoints,
        initial_opening_permit: bool,
        mut checkpoint: impl FnMut(PublicationCheckpoint) -> io::Result<()>,
    ) -> PublicationResult {
        if self.context.validate().is_err() {
            return PublicationResult::failed(PublicationFailure::Unpublished);
        }
        let record = self.context.draft.record().clone();
        let ProducerContext { draft, provider } = self.context;
        let verified = self.verified;
        if !matches!(verified.matches_record(&record), Ok(true)) {
            return PublicationResult::failed(PublicationFailure::Unpublished);
        }
        let package: std::result::Result<crate::package::PublishedDraft, CreateError> =
            match package_checkpoints {
                PackageCheckpoints::Enabled => draft.publish_package_with(&mut |stage| {
                    provider.revalidate().map_err(|_| ())?;
                    if !verified.matches_record(&record).map_err(|_| ())? {
                        return Err(());
                    }
                    let result = checkpoint(PublicationCheckpoint::Package(stage));
                    provider.revalidate().map_err(|_| ())?;
                    result.map_err(|_| ())
                }),
                PackageCheckpoints::Disabled => draft.publish_package(),
            };
        let package = match package {
            Err(CreateError::Refused) => {
                return PublicationResult::failed(PublicationFailure::Unpublished);
            }
            Err(CreateError::CommittedIndeterminate) => {
                return PublicationResult::failed(PublicationFailure::PackageDurabilityUnknown);
            }
            Ok(package) => package,
        };
        if provider.revalidate().is_err()
            || !matches!(verified.matches_record(package.record()), Ok(true))
        {
            return PublicationResult::failed(PublicationFailure::PackageOnly);
        }
        let durable_checkpoint = checkpoint(PublicationCheckpoint::PackageDurable);
        if provider.revalidate().is_err() || durable_checkpoint.is_err() {
            return PublicationResult::failed(PublicationFailure::PackageOnly);
        }
        let opening = if initial_opening_permit {
            let operation = (|| {
                let opening = opening_publication::prepare(&package)?;
                checkpoint(PublicationCheckpoint::OpeningPermitDurable)?;
                opening.validate(&package)?;
                Ok::<_, io::Error>(opening)
            })();
            match operation {
                Ok(opening) => Some(opening),
                Err(_) => return PublicationResult::failed(PublicationFailure::PackageOnly),
            }
        } else {
            None
        };
        let operation = (|| -> std::result::Result<CurrentAdmission, PublishError> {
            let unpublished =
                |_: ()| PublishError::Unpublished(io::Error::from(io::ErrorKind::InvalidData));
            provider.revalidate().map_err(PublishError::Unpublished)?;
            package.validate().map_err(unpublished)?;
            let descriptor = package
                .admissions_descriptor()
                .map_err(|_| unpublished(()))?;
            let storage = Storage::from_anchored(descriptor).map_err(PublishError::Unpublished)?;
            storage.publish_with(package.record(), &mut |stage| {
                provider.revalidate()?;
                package
                    .validate()
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
                if !verified.matches_record(package.record())? {
                    return Err(io::Error::from(io::ErrorKind::InvalidData));
                }
                if let Some(opening) = &opening {
                    opening.validate(&package)?;
                }
                let result = checkpoint(PublicationCheckpoint::Record(stage));
                provider.revalidate()?;
                result
            })?;
            let final_error = |_: ()| {
                PublishError::PublishedDurabilityUnknown(io::Error::from(
                    io::ErrorKind::InvalidData,
                ))
            };
            provider
                .revalidate()
                .map_err(PublishError::PublishedDurabilityUnknown)?;
            let snapshot = storage
                .read(&package.record().credential_id)
                .map_err(PublishError::PublishedDurabilityUnknown)?;
            package.validate().map_err(final_error)?;
            if snapshot.metadata() != package.record() {
                return Err(final_error(()));
            }
            snapshot
                .validate()
                .map_err(PublishError::PublishedDurabilityUnknown)?;
            package.validate().map_err(final_error)?;
            provider
                .revalidate()
                .map_err(PublishError::PublishedDurabilityUnknown)?;
            let resident = CurrentAdmission::open_fixed_namespace(&package.record().credential_id)
                .map_err(PublishError::PublishedDurabilityUnknown)?;
            resident
                .validate()
                .map_err(PublishError::PublishedDurabilityUnknown)?;
            if !resident
                .matches_record(package.record())
                .map_err(PublishError::PublishedDurabilityUnknown)?
                || !verified
                    .matches_record(package.record())
                    .map_err(PublishError::PublishedDurabilityUnknown)?
            {
                return Err(final_error(()));
            }
            snapshot
                .validate()
                .map_err(PublishError::PublishedDurabilityUnknown)?;
            package.validate().map_err(final_error)?;
            provider
                .revalidate()
                .map_err(PublishError::PublishedDurabilityUnknown)?;
            Ok(resident)
        })();
        match operation {
            Ok(resident) => {
                let published = PublishedCreation {
                    package,
                    verified,
                    provider,
                    resident,
                    opening,
                };
                if published.validate().is_err() {
                    return PublicationResult::failed(PublicationFailure::RecordDurabilityUnknown);
                }
                PublicationResult {
                    result: Ok(published),
                }
            }
            Err(PublishError::Unpublished(_)) => {
                PublicationResult::failed(PublicationFailure::PackageOnly)
            }
            Err(PublishError::PublishedDurabilityUnknown(_)) => {
                PublicationResult::failed(PublicationFailure::RecordDurabilityUnknown)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_gate::RequestOutcome;
    use std::time::Instant;

    #[test]
    fn runtime_cleanup_preserves_replaced_directory_and_unknown_members() {
        let runtime = CreationRuntime::create().expect("create private runtime");
        let moved = runtime.path.with_extension("retained");
        std::fs::rename(&runtime.path, &moved).expect("move retained runtime");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&runtime.path)
            .expect("create replacement");
        assert!(runtime.cleanup().is_err());
        assert!(runtime.path.is_dir());
        assert!(moved.is_dir());
        std::fs::remove_dir(&runtime.path).expect("remove empty replacement");
        std::fs::rename(&moved, &runtime.path).expect("restore retained runtime");
        let unknown = runtime.path.join("unexpected");
        std::fs::write(&unknown, b"preserved").expect("create unknown child");
        assert!(runtime.cleanup().is_err());
        assert_eq!(std::fs::read(&unknown).unwrap(), b"preserved");
        std::fs::remove_file(unknown).expect("remove owned test child");
        runtime.cleanup().expect("remove empty owned runtime");
    }

    #[test]
    fn session_runtime_preserves_a_replaced_agent_socket_after_refusal_and_drop() {
        let runtime = CreationRuntime::create_session().expect("create session runtime");
        let path = runtime.path().to_owned();
        let agent = path.join("agent.sock");
        let original = std::os::unix::net::UnixListener::bind(&agent).expect("bind original");
        let token = runtime.capture_agent_socket().expect("capture original");
        std::fs::remove_file(&agent).expect("unlink original name");
        let replacement = std::os::unix::net::UnixListener::bind(&agent).expect("bind replacement");
        assert!(runtime.cleanup_after_session(token).is_err());
        drop(runtime);
        assert!(agent.exists());
        drop((original, replacement));
        std::fs::remove_file(agent).expect("remove test replacement");
        std::fs::remove_dir(path).expect("remove retained test directory");
    }

    #[test]
    fn session_runtime_removes_only_the_captured_agent_and_preserves_an_introduced_home() {
        let runtime = CreationRuntime::create_session().expect("create session runtime");
        let path = runtime.path().to_owned();
        let agent = std::os::unix::net::UnixListener::bind(path.join("agent.sock"))
            .expect("bind owned agent");
        let token = runtime.capture_agent_socket().expect("capture owned agent");
        runtime
            .cleanup_after_session(token)
            .expect("remove owned runtime");
        assert!(!path.exists());
        drop(agent);

        let runtime = CreationRuntime::create_session().expect("create retained session runtime");
        let path = runtime.path().to_owned();
        let agent = std::os::unix::net::UnixListener::bind(path.join("agent.sock"))
            .expect("bind session agent");
        let token = runtime
            .capture_agent_socket()
            .expect("capture session agent");
        let introduced_home = path.join("home");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&introduced_home)
            .expect("introduce an unrelated home directory");
        assert!(runtime.cleanup_after_session(token).is_err());
        drop(runtime);
        assert!(introduced_home.is_dir());
        assert!(!path.join("agent.sock").exists());
        drop(agent);
        std::fs::remove_dir(introduced_home).expect("remove introduced test home");
        std::fs::remove_dir(path).expect("remove retained test runtime");
    }

    #[test]
    fn session_runtime_after_teardown_requires_the_agent_name_to_be_absent() {
        let runtime = CreationRuntime::create_session().expect("create closed session runtime");
        let path = runtime.path().to_owned();
        runtime
            .cleanup_after_native_teardown()
            .expect("remove runtime after native teardown");
        assert!(!path.exists());

        let runtime = CreationRuntime::create_session().expect("create retained session runtime");
        let path = runtime.path().to_owned();
        let agent = path.join("agent.sock");
        std::fs::write(&agent, b"preserved").expect("create substituted agent name");
        assert!(runtime.cleanup_after_native_teardown().is_err());
        assert_eq!(std::fs::read(&agent).unwrap(), b"preserved");
        std::fs::remove_file(&agent).expect("remove substituted test name");
        std::os::unix::fs::symlink("missing-target", &agent).expect("create dangling replacement");
        assert!(runtime.cleanup_after_native_teardown().is_err());
        assert!(std::fs::symlink_metadata(&agent)
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_file(&agent).expect("remove dangling test replacement");
        let introduced_home = path.join("home");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&introduced_home)
            .expect("introduce an unrelated home directory");
        assert!(runtime.cleanup_after_native_teardown().is_err());
        assert!(introduced_home.is_dir());
        let unknown = path.join("unexpected");
        std::fs::write(&unknown, b"unknown").expect("create unknown member");
        assert!(runtime.cleanup_after_native_teardown().is_err());
        drop(runtime);
        assert_eq!(std::fs::read(&unknown).unwrap(), b"unknown");
        std::fs::remove_file(unknown).expect("remove owned test member");
        std::fs::remove_dir(introduced_home).expect("remove introduced test home");
        std::fs::remove_dir(path).expect("remove retained test runtime");
    }

    #[test]
    fn failed_publication_cannot_expose_a_record_or_promote_its_outcome() {
        for (failure, expected) in [
            (
                PublicationFailure::Unpublished,
                PublicationOutcome::Unpublished,
            ),
            (
                PublicationFailure::PackageDurabilityUnknown,
                PublicationOutcome::PackageDurabilityUnknown,
            ),
            (
                PublicationFailure::PackageOnly,
                PublicationOutcome::PackageOnly,
            ),
            (
                PublicationFailure::RecordDurabilityUnknown,
                PublicationOutcome::RecordDurabilityUnknown,
            ),
        ] {
            let result = PublicationResult::failed(failure);
            assert_eq!(result.outcome(), expected);
            assert!(result.record().is_err());
            assert_eq!(result.outcome(), expected);
        }
    }

    struct EmptyResource;

    impl crate::linux_gate::CloseResource for EmptyResource {
        fn stop(&self, _: Instant) -> std::result::Result<(), crate::linux_gate::Error> {
            Ok(())
        }

        fn release(&self, _: Instant) -> std::result::Result<(), crate::linux_gate::Error> {
            Ok(())
        }
    }

    #[test]
    fn creation_session_gate_rejects_another_session_and_expired_association() {
        let original = crate::linux_gate::Gate::new(Arc::new(EmptyResource));
        let another = crate::linux_gate::Gate::new(Arc::new(EmptyResource));
        assert_eq!(original.snapshot().unwrap(), another.snapshot().unwrap());
        let association = Arc::downgrade(&original);
        assert!(matches_session_gate(&association, &original));
        assert!(!matches_session_gate(&association, &another));
        drop(original);
        assert!(!matches_session_gate(&association, &another));
    }

    fn closed_snapshot() -> Snapshot {
        Snapshot {
            state: State::Closed,
            epoch: 1,
            id: 1,
            active: false,
            native_requests_sent: 1,
            native_replies_received: 1,
            proofs_verified: 1,
            proofs_published: 1,
            lock_callers: 1,
            lock_acknowledgments: 1,
            busy_refused: 0,
            closed_refused: 0,
            workers_joined: 1,
            close_acknowledged: true,
            resources_closed_after_drain: true,
            drain_waiting: false,
            outcome: Some(RequestOutcome::Published),
        }
    }

    #[test]
    fn closed_creation_requires_released_resources_and_exact_completed_work() {
        let accepted = closed_snapshot();
        assert!(check_closed_session_state(true, accepted).is_ok());
        assert!(check_closed_session_state(false, accepted).is_err());
        for state in [State::Ready, State::Closing] {
            let mut changed = accepted;
            changed.state = state;
            assert!(check_closed_session_state(true, changed).is_err());
        }
        for field in 0..10 {
            let mut changed = accepted;
            match field {
                0 => changed.active = true,
                1 => changed.close_acknowledged = false,
                2 => changed.resources_closed_after_drain = false,
                3 => changed.proofs_published = 0,
                4 => changed.native_requests_sent = 0,
                5 => changed.native_replies_received = 0,
                6 => changed.proofs_verified = 0,
                7 => changed.workers_joined = 0,
                8 => changed.lock_callers = 0,
                9 => changed.lock_acknowledgments = 0,
                _ => unreachable!(),
            }
            assert!(check_closed_session_state(true, changed).is_err());
        }
        for field in 0..7 {
            let mut changed = accepted;
            match field {
                0 => changed.proofs_published = 2,
                1 => changed.native_requests_sent = 2,
                2 => changed.native_replies_received = 2,
                3 => changed.proofs_verified = 2,
                4 => changed.workers_joined = 2,
                5 => changed.lock_callers = 2,
                6 => changed.lock_acknowledgments = 2,
                _ => unreachable!(),
            }
            assert!(check_closed_session_state(true, changed).is_err());
        }
    }
}
