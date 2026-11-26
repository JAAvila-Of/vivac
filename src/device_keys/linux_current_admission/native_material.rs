use super::super::{
    linux_ipc::registration::{verify_live_process_pair, KeeperActiveClosureSender},
    linux_openssh_package,
    linux_reservation::StoreReservation,
    linux_session::{NativeSession, PackageLease},
    linux_spawn::{ExitReceipt, OwnedChildRegistration, Receipt, SpawnOwner, SpawnSpec},
};
use super::{invalid, CurrentAdmission, RetainedKeeperDirectory, Validity};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileExt, FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const REQUIRED_SEALS: libc::c_int =
    libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentSocketIdentity {
    parent_device: u64,
    parent_inode: u64,
    socket_device: u64,
    socket_inode: u64,
}

fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn descriptor(fd: i32) -> io::Result<()> {
    require(fd >= 3)?;
    // SAFETY: fcntl only queries the owned descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    require(flags & libc::FD_CLOEXEC != 0)
}

fn validate_ciphertext(cipher: &OwnedFd, length: u64, hash: &str) -> io::Result<()> {
    descriptor(cipher.as_raw_fd())?;
    require((1..=65_536).contains(&length))?;
    // SAFETY: fcntl only queries the retained sealed descriptor.
    let seals = unsafe { libc::fcntl(cipher.as_raw_fd(), libc::F_GET_SEALS) };
    if seals < 0 {
        return Err(io::Error::last_os_error());
    }
    require(seals & REQUIRED_SEALS == REQUIRED_SEALS)?;
    let file = File::from(cipher.try_clone()?);
    let metadata = file.metadata()?;
    require(metadata.is_file() && metadata.mode() & 0o7777 == 0o600 && metadata.len() == length)?;
    let mut bytes = vec![0; length as usize + 1];
    let mut used = 0;
    while used < bytes.len() {
        let count = match file.read_at(&mut bytes[used..], used as u64) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        used += count;
    }
    require(used as u64 == length)?;
    bytes.truncate(used);
    require(linux_openssh_package::hex(&Sha256::digest(&bytes)) == hash)?;
    descriptor(cipher.as_raw_fd())
}

pub(crate) fn seal_ciphertext(bytes: &[u8]) -> io::Result<OwnedFd> {
    require(!bytes.is_empty() && bytes.len() <= 65_536)?;
    // SAFETY: the fixed name is terminated; flags request a private sealable memfd.
    let raw = unsafe {
        libc::memfd_create(
            c"vivac-admitted-ciphertext".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful memfd_create returned one fresh owned descriptor.
    let initial = unsafe { OwnedFd::from_raw_fd(raw) };
    let cipher = if raw < 3 {
        // SAFETY: fcntl duplicates the owned memfd atomically above stdio.
        let duplicate = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 3) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful fcntl returned one fresh owned descriptor.
        let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
        drop(initial);
        duplicate
    } else {
        initial
    };
    descriptor(cipher.as_raw_fd())?;
    // SAFETY: fchmod changes only this owned anonymous file.
    if unsafe { libc::fchmod(cipher.as_raw_fd(), 0o600) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = File::from(cipher);
    file.write_all(bytes)?;
    file.seek(SeekFrom::Start(0))?;
    // SAFETY: fcntl seals the complete anonymous file against all further writes.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, REQUIRED_SEALS) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let cipher = OwnedFd::from(file);
    validate_ciphertext(
        &cipher,
        bytes.len() as u64,
        &linux_openssh_package::hex(&Sha256::digest(bytes)),
    )?;
    Ok(cipher)
}

pub struct AdmittedSessionMaterial {
    admission: Arc<CurrentAdmission>,
    lease: PackageLease,
    cipher: OwnedFd,
    identity: [u64; 6],
    validity: Validity,
}

pub struct ReservedAdmittedSessionMaterial {
    material: AdmittedSessionMaterial,
    reservation: StoreReservation,
}

pub struct AdmittedLaunchPreparation {
    owner: SpawnOwner,
    material: AdmittedSessionMaterial,
    binding: LaunchBinding,
    pair_attempted: bool,
}

/// A reserved launch preparation holding one opening authority: a consumed
/// permit, or a checked absence of one for a reopening.
pub struct OpeningPermitLaunch {
    preparation: AdmittedLaunchPreparation,
    kind: KeeperLaunchKind,
}

fn secret_bytes(secret: &Zeroizing<Vec<u8>>) -> &[u8] {
    secret
}

impl OpeningPermitLaunch {
    pub fn probe_unlock_registered_active(
        self,
        home: &Path,
        register: impl FnOnce(&AdmittedChildRegistration<'_>) -> io::Result<KeeperActiveClosureSender>,
        read_secret: impl FnOnce() -> io::Result<Zeroizing<Vec<u8>>>,
    ) -> io::Result<(
        NativeSession,
        crate::ubuntu_provider::UbuntuProvider,
        KeeperActiveClosureSender,
    )> {
        let Self { preparation, kind } = self;
        require(kind.opening_authority().is_some())?;
        let (session, provider, sender) = preparation.probe_unlock_openssh(
            home,
            || Ok(()),
            read_secret,
            secret_bytes,
            kind,
            |registration| register(registration).map(Some),
        )?;
        Ok((session, provider, sender.ok_or_else(invalid)?))
    }

    pub fn probe_unlock_attended(
        self,
        home: &Path,
    ) -> io::Result<(NativeSession, crate::ubuntu_provider::UbuntuProvider)> {
        let (session, provider, _) = self.preparation.probe_unlock_openssh(
            home,
            || Ok(()),
            read_attended_unlock,
            crate::attended_terminal::AttendedUnlockInput::as_bytes,
            KeeperLaunchKind::Generic,
            |_| Ok(None),
        )?;
        Ok((session, provider))
    }
}

/// The direct children of the retained reserved owner and their local binding.
pub struct AdmittedChildRegistration<'preparation> {
    children: OwnedChildRegistration<'preparation>,
    material: &'preparation AdmittedSessionMaterial,
    binding: &'preparation LaunchBinding,
    kind: KeeperLaunchKind,
    keeper_bootstrap_attempted: Cell<bool>,
}

#[derive(Eq, PartialEq)]
pub(crate) struct ActiveClosureBinding {
    generation: [u8; 32],
    store: [u64; 6],
    roles: [Receipt; 2],
}

impl ActiveClosureBinding {
    pub(crate) fn consume_closed(self, session: &mut NativeSession) -> io::Result<[u8; 32]> {
        session.consume_active_closure(self.generation, self.store, self.roles)?;
        Ok(self.generation)
    }
}

struct LaunchBinding {
    generation: [u8; 32],
    identity: [u64; 6],
    record_digest: [u8; 32],
}

pub(crate) const KEEPER_BOOTSTRAP_SIZE: usize = 128;
const KEEPER_BOOTSTRAP_HEADER: &[u8; 16] = b"VVKBOOT1\x01\0\0\0\0\0\0\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum KeeperLaunchKind {
    Generic = 0,
    PreUnlockRestricted = 1,
    ActiveRestricted = 2,
    PermitActiveRestricted = 3,
    ReopenedActiveRestricted = 4,
}

/// What lets a supervisor open a session: a consumed permit, or a reopening
/// on the keeper's evidence that no process of the previous session survives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpeningAuthority {
    Permit,
    Reopened,
}

impl KeeperLaunchKind {
    fn is_active(self) -> bool {
        matches!(
            self,
            Self::ActiveRestricted | Self::PermitActiveRestricted | Self::ReopenedActiveRestricted
        )
    }

    fn opening_authority(self) -> Option<OpeningAuthority> {
        match self {
            Self::PermitActiveRestricted => Some(OpeningAuthority::Permit),
            Self::ReopenedActiveRestricted => Some(OpeningAuthority::Reopened),
            Self::Generic | Self::PreUnlockRestricted | Self::ActiveRestricted => None,
        }
    }
}

pub(crate) struct KeeperLaunchExpectation {
    admission: CurrentAdmission,
    binding: LaunchBinding,
    references: [OwnedFd; 2],
    kind: KeeperLaunchKind,
}

impl LaunchBinding {
    fn encode_bootstrap(&self, kind: KeeperLaunchKind) -> [u8; KEEPER_BOOTSTRAP_SIZE] {
        let mut wire = [0; KEEPER_BOOTSTRAP_SIZE];
        wire[..16].copy_from_slice(KEEPER_BOOTSTRAP_HEADER);
        wire[9] = kind as u8;
        wire[16..48].copy_from_slice(&self.generation);
        for (slot, value) in wire[48..96]
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .zip(self.identity)
        {
            *slot = value.to_be_bytes();
        }
        wire[96..].copy_from_slice(&self.record_digest);
        wire
    }

    fn decode_bootstrap(
        wire: &[u8; KEEPER_BOOTSTRAP_SIZE],
    ) -> io::Result<(Self, KeeperLaunchKind)> {
        require(wire[..9] == KEEPER_BOOTSTRAP_HEADER[..9])?;
        require(wire[10..16].iter().all(|byte| *byte == 0))?;
        let kind = match wire[9] {
            0 => KeeperLaunchKind::Generic,
            1 => KeeperLaunchKind::PreUnlockRestricted,
            2 => KeeperLaunchKind::ActiveRestricted,
            3 => KeeperLaunchKind::PermitActiveRestricted,
            4 => KeeperLaunchKind::ReopenedActiveRestricted,
            _ => return Err(invalid()),
        };
        let generation = wire[16..48].try_into().map_err(|_| invalid())?;
        require(generation != [0; 32])?;
        let mut identity = [0; 6];
        for (output, slot) in identity.iter_mut().zip(wire[48..96].as_chunks::<8>().0) {
            *output = u64::from_be_bytes(*slot);
        }
        Ok((
            Self {
                generation,
                identity,
                record_digest: wire[96..].try_into().map_err(|_| invalid())?,
            },
            kind,
        ))
    }
}

impl KeeperLaunchExpectation {
    pub(crate) fn from_authenticated_bootstrap(
        admission: CurrentAdmission,
        wire: &[u8; KEEPER_BOOTSTRAP_SIZE],
        references: [OwnedFd; 2],
    ) -> io::Result<Self> {
        let (binding, kind) = LaunchBinding::decode_bootstrap(wire)?;
        let expectation = Self {
            admission,
            binding,
            references,
            kind,
        };
        expectation.validate()?;
        Ok(expectation)
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        self.admission.checked_store(|record, identity| {
            require(identity == self.binding.identity)?;
            let record_digest: [u8; 32] = Sha256::digest(record.canonical_bytes()?).into();
            require(record_digest == self.binding.record_digest)?;
            verify_live_process_pair(&self.references, &self.references)
        })
    }

    pub(crate) fn commitment(&self, challenge: &[u8; 32]) -> io::Result<[u8; 32]> {
        require(*challenge != [0; 32])?;
        Ok(keeper_admission_commitment(
            &self.binding,
            challenge,
            self.kind,
        ))
    }

    pub(crate) fn restricted_generation(&self) -> Option<[u8; 32]> {
        (self.kind == KeeperLaunchKind::PreUnlockRestricted).then_some(self.binding.generation)
    }

    pub(crate) fn active_generation(&self) -> Option<[u8; 32]> {
        self.kind.is_active().then_some(self.binding.generation)
    }

    pub(crate) fn opening_authority(&self) -> Option<OpeningAuthority> {
        self.kind.opening_authority()
    }

    pub(crate) fn retain_offer(
        self,
        directory: OwnedFd,
        references: &[OwnedFd; 2],
    ) -> io::Result<RetainedKeeperDirectory> {
        self.validate()?;
        verify_live_process_pair(&self.references, references)?;
        let retained = self.admission.retain_keeper_directory(directory)?;
        verify_live_process_pair(&self.references, references)?;
        Ok(retained)
    }
}

fn keeper_admission_commitment(
    binding: &LaunchBinding,
    challenge: &[u8; 32],
    kind: KeeperLaunchKind,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    let domain: &[u8] = match kind {
        KeeperLaunchKind::Generic => b"vivac.linux.keeper-admission.v1\0",
        KeeperLaunchKind::PreUnlockRestricted => b"vivac.linux.keeper-pre-unlock-restricted.v1\0",
        KeeperLaunchKind::ActiveRestricted => b"vivac.linux.keeper-active-restricted.v1\0",
        KeeperLaunchKind::PermitActiveRestricted => {
            b"vivac.linux.keeper-permit-active-restricted.v1\0"
        }
        KeeperLaunchKind::ReopenedActiveRestricted => {
            b"vivac.linux.keeper-reopened-active-restricted.v1\0"
        }
    };
    hash.update(domain);
    hash.update(challenge);
    hash.update(binding.generation);
    for value in binding.identity {
        hash.update(value.to_be_bytes());
    }
    hash.update(binding.record_digest);
    hash.finalize().into()
}

fn validate_launch_binding(expected: &LaunchBinding, current: &LaunchBinding) -> io::Result<()> {
    require(expected.generation == current.generation)?;
    require(expected.identity == current.identity)?;
    require(expected.record_digest == current.record_digest)
}

fn capture_launch_binding(
    material: &AdmittedSessionMaterial,
    session_generation: impl Fn() -> io::Result<[u8; 32]>,
) -> io::Result<LaunchBinding> {
    material.validity.check(|| {
        let generation = session_generation()?;
        let record_digest =
            material.checked(|record| Ok(Sha256::digest(record.canonical_bytes()?).into()))?;
        require(session_generation()? == generation)?;
        Ok(LaunchBinding {
            generation,
            identity: material.identity,
            record_digest,
        })
    })
}

fn validate_retained_launch(
    material: &AdmittedSessionMaterial,
    binding: &LaunchBinding,
    session_generation: impl Fn() -> io::Result<[u8; 32]>,
) -> io::Result<()> {
    material.validity.check(|| {
        validate_launch_binding(
            binding,
            &capture_launch_binding(material, session_generation)?,
        )
    })
}

impl CurrentAdmission {
    pub fn prepare_native_material(self) -> io::Result<AdmittedSessionMaterial> {
        prepare_shared_native_material(Arc::new(self))
    }
}

fn prepare_shared_native_material(
    admission: Arc<CurrentAdmission>,
) -> io::Result<AdmittedSessionMaterial> {
    let (lease, cipher, identity) = admission.checked(|_| {
        let identity = admission.candidate.ipc_identity()?;
        let (directory, cipher) = admission.candidate.native_material()?;
        let lease = PackageLease::capture(directory, &cipher)?;
        require(lease.ipc_identity().map_err(|_| invalid())? == identity)?;
        require(admission.candidate.ipc_identity()? == identity)?;
        Ok((lease, cipher, identity))
    })?;
    let material = AdmittedSessionMaterial {
        admission,
        lease,
        cipher,
        identity,
        validity: Validity::new(),
    };
    material.validate()?;
    Ok(material)
}

impl RetainedKeeperDirectory {
    pub(crate) fn prepare_restricted_successor(
        &self,
        generation: [u8; 32],
    ) -> io::Result<AdmittedLaunchPreparation> {
        self.validate()?;
        let material = prepare_shared_native_material(Arc::clone(&self.admission))?;
        let directory = self.directory.try_clone()?;
        self.validate()?;
        let reservation = StoreReservation::from_retained_directory(
            &material.lease,
            directory,
            Arc::clone(&self.admission),
        )?;
        let owner = SpawnOwner::new_reserved_successor(reservation, generation)?;
        let binding =
            capture_launch_binding(&material, || owner.session_generation(&material.lease))?;
        require(binding.generation == generation)?;
        let preparation = AdmittedLaunchPreparation {
            owner,
            material,
            binding,
            pair_attempted: false,
        };
        preparation.validate()?;
        self.validate()?;
        Ok(preparation)
    }

    pub(crate) fn probe_launch_restricted_successor(
        &self,
        generation: [u8; 32],
        home: &Path,
        observe: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let result = (|| {
            let mut preparation = self.prepare_restricted_successor(generation)?;
            preparation.probe_with_openssh_registration(home, |registration, _| {
                registration.validate()?;
                require(registration.binding.generation == generation)?;
                self.validate()?;
                observe()?;
                self.validate()?;
                registration.validate()
            })
        })();
        self.validate()?;
        result
    }
}

impl AdmittedSessionMaterial {
    fn checked<T>(&self, operation: impl FnOnce(&super::Record) -> io::Result<T>) -> io::Result<T> {
        self.validity.check(|| {
            self.admission.checked_store(|record, identity| {
                require(identity == self.identity)?;
                require(self.lease.ipc_identity().map_err(|_| invalid())? == self.identity)?;
                validate_ciphertext(
                    &self.cipher,
                    record.ciphertext_len,
                    &record.ciphertext_sha256,
                )?;
                let value = operation(record)?;
                require(self.lease.ipc_identity().map_err(|_| invalid())? == self.identity)?;
                Ok(value)
            })
        })
    }

    pub fn validate(&self) -> io::Result<()> {
        self.checked(|_| Ok(()))
    }

    pub fn reserve(self) -> io::Result<ReservedAdmittedSessionMaterial> {
        let reservation = self.validity.check(|| {
            self.validate()?;
            let reservation =
                StoreReservation::acquire_retaining(&self.lease, Arc::clone(&self.admission))?;
            reservation.validate(&self.lease)?;
            self.validate()?;
            Ok(reservation)
        })?;
        let reserved = ReservedAdmittedSessionMaterial {
            material: self,
            reservation,
        };
        reserved.validate()?;
        Ok(reserved)
    }
}

impl ReservedAdmittedSessionMaterial {
    /// Consumes the opening permit under the reservation.
    pub fn prepare_service_opening(self) -> io::Result<OpeningPermitLaunch> {
        self.validate()?;
        let preparation = self.prepare_launch()?;
        preparation.validate()?;
        preparation
            .material
            .validity
            .check(|| preparation.material.admission.consume_opening_permit())?;
        preparation.validate()?;
        Ok(OpeningPermitLaunch {
            preparation,
            kind: KeeperLaunchKind::PermitActiveRestricted,
        })
    }

    /// Opens again without a permit. The keeper decides on kernel evidence
    /// that the previous session is gone; this only refuses, under the
    /// reservation, when a permit is present, and consumes nothing.
    pub fn prepare_service_reopening(self) -> io::Result<OpeningPermitLaunch> {
        self.validate()?;
        let preparation = self.prepare_launch()?;
        preparation.validate()?;
        preparation
            .material
            .validity
            .check(|| require(preparation.material.admission.opening_permit_absent()?))?;
        preparation.validate()?;
        Ok(OpeningPermitLaunch {
            preparation,
            kind: KeeperLaunchKind::ReopenedActiveRestricted,
        })
    }

    pub fn prepare_launch(self) -> io::Result<AdmittedLaunchPreparation> {
        self.validate()?;
        let Self {
            material,
            reservation,
        } = self;
        let owner = material
            .validity
            .check(|| SpawnOwner::new_reserved_dedicated(reservation))?;
        let binding = material.validity.check(|| {
            capture_launch_binding(&material, || owner.session_generation(&material.lease))
        })?;
        let prepared = AdmittedLaunchPreparation {
            owner,
            material,
            binding,
            pair_attempted: false,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    pub fn validate(&self) -> io::Result<()> {
        self.material.validity.check(|| {
            self.material.validate()?;
            self.reservation.validate(&self.material.lease)?;
            self.material.validate()
        })
    }
}

impl AdmittedLaunchPreparation {
    pub(crate) fn probe_openssh_roles(&mut self, home: &Path) -> io::Result<()> {
        self.probe_with_openssh_registration(home, |_, _| Ok(()))
    }

    pub(crate) fn probe_with_openssh_registration<T>(
        &mut self,
        home: &Path,
        operation: impl FnOnce(&AdmittedChildRegistration<'_>, &OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        require(!std::mem::replace(&mut self.pair_attempted, true))?;
        let socket = home.join("agent.sock");
        let Self {
            owner,
            material,
            binding,
            ..
        } = self;
        let mut agent_started = false;
        let mut agent_socket_identity = None;
        let result = material.validity.check(|| {
            validate_retained_launch(material, binding, || {
                owner.session_generation(&material.lease)
            })?;
            let (provider, agent, loader) = start_restricted_openssh(
                owner,
                material.cipher.try_clone()?,
                home,
                &socket,
                &mut agent_started,
                &mut agent_socket_identity,
            )?;
            let roles = [agent, loader.receipt()];
            let children = register_restricted_openssh(owner, &provider, roles)?;
            let registration = AdmittedChildRegistration {
                children,
                material,
                binding,
                kind: KeeperLaunchKind::PreUnlockRestricted,
                keeper_bootstrap_attempted: Cell::new(false),
            };
            validate_retained_launch(material, binding, || {
                registration.children.session_generation(&material.lease)
            })?;
            provider.revalidate()?;
            registration.validate()?;
            require(loader.prompt_seen() && !loader.handed_off())?;
            let master = loader.duplicate_master()?;
            let output = operation(&registration, &master)?;
            registration.validate()?;
            registration.children.validate()?;
            provider.revalidate()?;
            require(loader.prompt_seen() && !loader.handed_off())?;
            Ok((roles, output))
        });
        let exits = owner.shutdown(Duration::from_secs(5))?;
        if agent_started {
            if let Some(identity) = agent_socket_identity {
                remove_agent_socket(&socket, identity)?;
            }
        }
        let (roles, output) = result?;
        require(exits.len() == roles.len())?;
        for role in roles {
            require(exits.iter().any(|exit| {
                exit.token == role.token
                    && exit.pid == role.pid
                    && exit.code.is_none()
                    && exit.signal == Some(libc::SIGKILL)
            }))?;
        }
        Ok(output)
    }

    pub(crate) fn probe_unlock_admitted_openssh(
        self,
        home: &Path,
        secret: &[u8],
    ) -> io::Result<(
        super::super::linux_session::NativeSession,
        crate::ubuntu_provider::UbuntuProvider,
    )> {
        let (session, provider, _) = self.probe_unlock_openssh(
            home,
            || Ok(()),
            || Ok(secret),
            |secret| secret,
            KeeperLaunchKind::Generic,
            |_| Ok(None),
        )?;
        Ok((session, provider))
    }

    pub(crate) fn probe_unlock_attended_openssh(
        self,
        home: &Path,
        validate_keeper: impl Fn() -> io::Result<()>,
    ) -> io::Result<(
        super::super::linux_session::NativeSession,
        crate::ubuntu_provider::UbuntuProvider,
    )> {
        let (session, provider, _) = self.probe_unlock_openssh(
            home,
            validate_keeper,
            read_attended_unlock,
            crate::attended_terminal::AttendedUnlockInput::as_bytes,
            KeeperLaunchKind::Generic,
            |_| Ok(None),
        )?;
        Ok((session, provider))
    }

    pub(crate) fn probe_unlock_registered_active(
        self,
        home: &Path,
        register: impl FnOnce(&AdmittedChildRegistration<'_>) -> io::Result<KeeperActiveClosureSender>,
    ) -> io::Result<(
        NativeSession,
        crate::ubuntu_provider::UbuntuProvider,
        KeeperActiveClosureSender,
    )> {
        let (session, provider, sender) = self.probe_unlock_openssh(
            home,
            || Ok(()),
            read_attended_unlock,
            crate::attended_terminal::AttendedUnlockInput::as_bytes,
            KeeperLaunchKind::ActiveRestricted,
            |registration| register(registration).map(Some),
        )?;
        Ok((session, provider, sender.ok_or_else(invalid)?))
    }

    fn probe_unlock_openssh<T>(
        self,
        home: &Path,
        validate_keeper: impl Fn() -> io::Result<()>,
        read_secret: impl FnOnce() -> io::Result<T>,
        secret_bytes: impl FnOnce(&T) -> &[u8],
        kind: KeeperLaunchKind,
        register: impl FnOnce(
            &AdmittedChildRegistration<'_>,
        ) -> io::Result<Option<KeeperActiveClosureSender>>,
    ) -> io::Result<(
        NativeSession,
        crate::ubuntu_provider::UbuntuProvider,
        Option<KeeperActiveClosureSender>,
    )> {
        require(!self.pair_attempted)?;
        let Self {
            mut owner,
            material,
            binding,
            ..
        } = self;
        let socket = home.join("agent.sock");
        let mut agent_started = false;
        let mut agent_socket_identity = None;
        let result = material.validity.check(|| {
            let check = |owner: &SpawnOwner| {
                validate_keeper()?;
                validate_retained_launch(&material, &binding, || {
                    owner.session_generation(&material.lease)
                })
            };
            check(&owner)?;
            let (provider, agent, mut loader) = start_restricted_openssh(
                &mut owner,
                material.cipher.try_clone()?,
                home,
                &socket,
                &mut agent_started,
                &mut agent_socket_identity,
            )?;
            let (secret, sender) = {
                let registration = AdmittedChildRegistration {
                    children: register_restricted_openssh(
                        &mut owner,
                        &provider,
                        [agent, loader.receipt()],
                    )?,
                    material: &material,
                    binding: &binding,
                    kind,
                    keeper_bootstrap_attempted: Cell::new(false),
                };
                let check_registration = || {
                    validate_keeper()?;
                    registration.validate()?;
                    registration.children.validate()?;
                    provider.revalidate()?;
                    require(loader.prompt_seen() && !loader.handed_off())
                };
                check_registration()?;
                let sender = register(&registration)?;
                require(sender.is_some() == kind.is_active())?;
                if let Some(sender) = &sender {
                    sender.validate_registration(&registration)?;
                }
                let secret = read_secret()?;
                check_registration()?;
                if let Some(sender) = &sender {
                    sender.validate_registration(&registration)?;
                }
                (secret, sender)
            };
            let handoff =
                loader.handoff_stopped(&mut owner, secret_bytes(&secret), Duration::from_secs(5));
            drop(secret);
            handoff?;
            check(&owner)?;
            provider.revalidate()?;
            loader.complete(&mut owner, Duration::from_secs(5))?;
            check(&owner)?;
            provider.revalidate()?;
            owner.enter_endpoint_probe_session(&material.lease)?;
            Ok((provider, agent, loader, sender))
        });
        let (provider, agent, loader, sender) = match result {
            Ok(started) => started,
            Err(error) => {
                owner.shutdown(Duration::from_secs(5))?;
                if agent_started {
                    if let Some(identity) = agent_socket_identity {
                        remove_agent_socket(&socket, identity)?;
                    }
                }
                return Err(error);
            }
        };
        let session = NativeSession::new(
            owner,
            agent.token,
            loader,
            Arc::new(material.lease),
            Vec::new(),
            socket,
        )?;
        Ok((session, provider, sender))
    }

    pub fn keeper_admission_commitment(&self, challenge: &[u8; 32]) -> io::Result<[u8; 32]> {
        require(*challenge != [0; 32])?;
        self.material.validity.check(|| {
            self.validate()?;
            let commitment =
                keeper_admission_commitment(&self.binding, challenge, KeeperLaunchKind::Generic);
            self.validate()?;
            Ok(commitment)
        })
    }

    pub fn validate(&self) -> io::Result<()> {
        validate_retained_launch(&self.material, &self.binding, || {
            self.owner.session_generation(&self.material.lease)
        })
    }

    // The internal assembler must select the process specifications.
    pub(crate) fn launch_pair(
        &mut self,
        specs: [SpawnSpec; 2],
    ) -> io::Result<AdmittedChildRegistration<'_>> {
        require(!std::mem::replace(&mut self.pair_attempted, true))?;
        let Self {
            owner,
            material,
            binding,
            ..
        } = self;
        let material = &*material;
        let binding = &*binding;
        material.validity.check(move || {
            let validate_owner = |owner: &SpawnOwner| {
                validate_retained_launch(material, binding, || {
                    owner.session_generation(&material.lease)
                })
            };
            validate_owner(owner)?;
            for spec in specs {
                owner.spawn(spec)?;
                validate_owner(owner)?;
            }
            let registration = AdmittedChildRegistration {
                children: owner.prepare_child_registration()?,
                material,
                binding,
                kind: KeeperLaunchKind::Generic,
                keeper_bootstrap_attempted: Cell::new(false),
            };
            registration.validate()?;
            Ok(registration)
        })
    }

    pub(crate) fn observe_direct_exits(&mut self) -> io::Result<Option<Vec<ExitReceipt>>> {
        require(self.pair_attempted)?;
        self.owner.observe_direct_exits()
    }
}

fn read_attended_unlock() -> io::Result<crate::attended_terminal::AttendedUnlockInput> {
    crate::attended_terminal::read_existing_unlock()
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "attended-input"))
}

fn start_restricted_openssh(
    owner: &mut SpawnOwner,
    cipher: OwnedFd,
    home: &Path,
    socket: &Path,
    agent_started: &mut bool,
    agent_socket_identity: &mut Option<AgentSocketIdentity>,
) -> io::Result<(
    crate::ubuntu_provider::UbuntuProvider,
    Receipt,
    super::super::linux_loader::PtyLoader,
)> {
    let metadata = std::fs::symlink_metadata(home)?;
    // SAFETY: these calls query the probe's effective credentials.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    require(
        home.is_absolute()
            && metadata.is_dir()
            && metadata.mode() & 0o7777 == 0o700
            && metadata.uid() == uid
            && metadata.gid() == gid,
    )?;
    let parent_identity = (metadata.dev(), metadata.ino());
    match std::fs::symlink_metadata(socket) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => return Err(invalid()),
    }
    let provider = crate::ubuntu_provider::UbuntuProvider::select()?;
    let [agent_image, loader_image] = provider.launch_executables()?;
    let agent = owner.spawn_task_restricted(crate::ubuntu_provider::agent_spec(
        &agent_image,
        socket,
        home,
    )?)?;
    *agent_started = true;
    *agent_socket_identity = Some(wait_empty_openssh_agent(
        owner,
        agent,
        home,
        socket,
        parent_identity,
    )?);
    provider.revalidate()?;
    let mut loader = super::super::linux_loader::PtyLoader::start_task_restricted(
        owner,
        loader_image,
        cipher,
        socket,
        home,
    )?;
    loader.wait_prompt(owner, Duration::from_secs(5))?;
    Ok((provider, agent, loader))
}

fn register_restricted_openssh<'owner>(
    owner: &'owner mut SpawnOwner,
    provider: &crate::ubuntu_provider::UbuntuProvider,
    [agent, loader]: [Receipt; 2],
) -> io::Result<OwnedChildRegistration<'owner>> {
    let [agent_image, loader_image] = provider.launch_executables()?;
    let children = owner.prepare_child_registration()?;
    children.validate_role_order([(agent, &agent_image), (loader, &loader_image)])?;
    children.validate_task_restriction()?;
    Ok(children)
}

fn current_agent_socket_identity(
    home: &Path,
    socket: &Path,
    expected_parent: (u64, u64),
) -> io::Result<Option<AgentSocketIdentity>> {
    let parent = std::fs::symlink_metadata(home)?;
    // SAFETY: these calls query the probe's effective credentials.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    require(
        parent.is_dir()
            && parent.mode() & 0o7777 == 0o700
            && parent.uid() == uid
            && parent.gid() == gid
            && (parent.dev(), parent.ino()) == expected_parent,
    )?;
    let metadata = match std::fs::symlink_metadata(socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    require(
        metadata.file_type().is_socket()
            && metadata.uid() == uid
            && metadata.gid() == gid
            && metadata.dev() == expected_parent.0,
    )?;
    Ok(Some(AgentSocketIdentity {
        parent_device: parent.dev(),
        parent_inode: parent.ino(),
        socket_device: metadata.dev(),
        socket_inode: metadata.ino(),
    }))
}

fn remove_agent_socket(socket: &Path, expected: AgentSocketIdentity) -> io::Result<()> {
    let home = socket.parent().ok_or_else(invalid)?;
    let current = match current_agent_socket_identity(
        home,
        socket,
        (expected.parent_device, expected.parent_inode),
    ) {
        Ok(current) => current,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let Some(current) = current else {
        return Ok(());
    };
    require(current == expected)?;
    std::fs::remove_file(socket)
}

fn wait_empty_openssh_agent(
    owner: &mut SpawnOwner,
    agent: Receipt,
    home: &Path,
    socket: &Path,
    parent_identity: (u64, u64),
) -> io::Result<AgentSocketIdentity> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        require(Instant::now() < deadline && owner.try_wait(agent.token)?.is_none())?;
        let before = match current_agent_socket_identity(home, socket, parent_identity) {
            Ok(Some(identity)) => identity,
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(error),
        };
        match UnixStream::connect(socket) {
            Ok(mut stream) => {
                stream.set_read_timeout(Some(Duration::from_millis(500)))?;
                stream.set_write_timeout(Some(Duration::from_millis(500)))?;
                // SAFETY: a zeroed ucred is valid scalar storage for getsockopt.
                let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
                let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
                // SAFETY: getsockopt writes within the initialized credential buffer.
                let result = unsafe {
                    libc::getsockopt(
                        stream.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_PEERCRED,
                        std::ptr::addr_of_mut!(credentials).cast(),
                        &mut length,
                    )
                };
                // SAFETY: these calls query the probe's effective credentials.
                let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
                require(
                    result == 0
                        && length as usize == std::mem::size_of::<libc::ucred>()
                        && credentials.pid == agent.pid as libc::pid_t
                        && credentials.uid == uid
                        && credentials.gid == gid,
                )?;
                stream.write_all(&[0, 0, 0, 1, 11])?;
                let mut response = [0; 9];
                stream.read_exact(&mut response)?;
                require(response == [0, 0, 0, 5, 12, 0, 0, 0, 0])?;
                let after = current_agent_socket_identity(home, socket, parent_identity)?
                    .ok_or_else(invalid)?;
                require(after == before)?;
                require(Instant::now() < deadline && owner.try_wait(agent.token)?.is_none())?;
                return Ok(after);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

impl AdmittedChildRegistration<'_> {
    pub fn validate(&self) -> io::Result<()> {
        validate_retained_launch(self.material, self.binding, || {
            self.children.session_generation(&self.material.lease)
        })
    }

    pub(crate) fn active_closure_binding(&self) -> io::Result<ActiveClosureBinding> {
        require(self.kind.is_active())?;
        self.validate()?;
        Ok(ActiveClosureBinding {
            generation: self.binding.generation,
            store: self.binding.identity,
            roles: self.children.receipts()?,
        })
    }

    /// Checks that the two launch images match the retained executable objects in order.
    pub fn validate_executable_order(&self, expected: [&OwnedFd; 2]) -> io::Result<()> {
        self.material.validity.check(|| {
            self.validate()?;
            self.children.validate_executable_order(expected)?;
            self.validate()
        })
    }

    pub(crate) fn with_keeper_bootstrap<T>(
        &self,
        operation: impl FnOnce(&[u8; KEEPER_BOOTSTRAP_SIZE], &[OwnedFd; 2]) -> io::Result<T>,
    ) -> io::Result<T> {
        require(!self.keeper_bootstrap_attempted.replace(true))?;
        self.validate()?;
        let wire = self.binding.encode_bootstrap(self.kind);
        let result = self
            .children
            .with_references(|references| operation(&wire, references))?;
        self.validate()?;
        Ok(result)
    }

    pub(crate) fn with_keeper_offer<T>(
        &self,
        challenge: &[u8; 32],
        operation: impl FnOnce([u8; 32], &[OwnedFd; 3]) -> io::Result<T>,
    ) -> io::Result<T> {
        require(*challenge != [0; 32])?;
        self.validate()?;
        let commitment = keeper_admission_commitment(self.binding, challenge, self.kind);
        let references = self
            .children
            .keeper_offer_references(&self.material.lease)?;
        let result = operation(commitment, &references)?;
        self.validate()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    fn temporary_directory() -> PathBuf {
        let mut template = b"/tmp/vivac-session-runtime-XXXXXX\0".to_vec();
        // SAFETY: mkdtemp receives one writable, NUL-terminated template.
        let created = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
        assert!(!created.is_null());
        // SAFETY: successful mkdtemp leaves a NUL-terminated path in template.
        PathBuf::from(std::ffi::OsStr::from_bytes(unsafe {
            CStr::from_ptr(created).to_bytes()
        }))
    }

    fn public_launch_binding() -> LaunchBinding {
        LaunchBinding {
            generation: [1; 32],
            identity: [2, 3, 4, 5, 6, 7],
            record_digest: [8; 32],
        }
    }

    #[test]
    fn session_runtime_preserves_replaced_agent_socket_after_native_teardown() {
        let root = temporary_directory();
        let home = root.join("home");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&home)
            .expect("create private home");
        let socket = home.join("agent.sock");
        let original = UnixListener::bind(&socket).expect("bind original socket");
        let parent = std::fs::symlink_metadata(&home).expect("stat home");
        let parent_identity = (parent.dev(), parent.ino());
        let expected = current_agent_socket_identity(&home, &socket, parent_identity)
            .expect("capture original identity")
            .expect("original socket exists");
        let retained_original = home.join("agent.sock.original");
        std::fs::rename(&socket, &retained_original).expect("retain original inode");
        let replacement = UnixListener::bind(&socket).expect("bind replacement socket");
        let replacement_identity = current_agent_socket_identity(&home, &socket, parent_identity)
            .expect("capture replacement identity")
            .expect("replacement socket exists");
        assert_eq!(
            (
                replacement_identity.parent_device,
                replacement_identity.parent_inode
            ),
            (expected.parent_device, expected.parent_inode)
        );
        assert_ne!(replacement_identity.socket_inode, expected.socket_inode);

        assert!(remove_agent_socket(&socket, expected).is_err());
        assert!(socket.exists());
        drop((original, replacement));
        std::fs::remove_file(&socket).expect("remove replacement socket");
        std::fs::remove_file(retained_original).expect("remove retained original socket");
        std::fs::remove_dir(home).expect("remove home");
        std::fs::remove_dir(root).expect("remove runtime");
    }

    #[test]
    fn keeper_bootstrap_uses_canonical_layout_and_requires_a_generation() {
        let binding = public_launch_binding();
        let wire = binding.encode_bootstrap(KeeperLaunchKind::Generic);
        assert_eq!(&wire[..16], b"VVKBOOT1\x01\0\0\0\0\0\0\0");
        assert_eq!(&wire[16..48], &binding.generation);
        assert_eq!(&wire[48..56], &2u64.to_be_bytes());
        assert_eq!(&wire[88..96], &7u64.to_be_bytes());
        assert_eq!(&wire[96..], &binding.record_digest);
        let (decoded, kind) = LaunchBinding::decode_bootstrap(&wire).unwrap();
        assert_eq!(kind, KeeperLaunchKind::Generic);
        validate_launch_binding(&binding, &decoded).unwrap();
        for (kind, code) in [
            (KeeperLaunchKind::PreUnlockRestricted, 1),
            (KeeperLaunchKind::ActiveRestricted, 2),
            (KeeperLaunchKind::PermitActiveRestricted, 3),
        ] {
            let restricted_wire = binding.encode_bootstrap(kind);
            assert_eq!(restricted_wire[9], code);
            let (decoded, decoded_kind) =
                LaunchBinding::decode_bootstrap(&restricted_wire).unwrap();
            assert_eq!(decoded_kind, kind);
            validate_launch_binding(&binding, &decoded).unwrap();
        }
        for index in 0..16 {
            let mut altered = wire;
            altered[index] = if index == 9 { 5 } else { wire[index] ^ 1 };
            assert!(LaunchBinding::decode_bootstrap(&altered).is_err());
        }
        let mut altered = wire;
        altered[16..48].fill(0);
        assert!(LaunchBinding::decode_bootstrap(&altered).is_err());
    }

    #[test]
    fn keeper_admission_commitment_matches_independent_golden_vector() {
        let challenge = std::array::from_fn(|index| index as u8);
        let binding = LaunchBinding {
            generation: std::array::from_fn(|index| (index + 32) as u8),
            identity: [
                0x4041424344454647,
                0x48494a4b4c4d4e4f,
                0x5051525354555657,
                0x58595a5b5c5d5e5f,
                0x6061626364656667,
                0x68696a6b6c6d6e6f,
            ],
            record_digest: std::array::from_fn(|index| (index + 112) as u8),
        };
        // Computed independently from the domain, NUL and bytes 0 through 143.
        let expected = [
            0x1f, 0x7d, 0x14, 0x1a, 0x60, 0x86, 0x29, 0x8d, 0x32, 0xbc, 0xd7, 0xf9, 0xc0, 0x52,
            0x67, 0xea, 0xf5, 0xdc, 0x5c, 0x80, 0xdb, 0xb6, 0x12, 0xaf, 0x5e, 0xba, 0x95, 0x0e,
            0x5c, 0xf4, 0x00, 0x03,
        ];
        assert_eq!(
            keeper_admission_commitment(&binding, &challenge, KeeperLaunchKind::Generic),
            expected
        );
        let restricted = keeper_admission_commitment(
            &binding,
            &challenge,
            KeeperLaunchKind::PreUnlockRestricted,
        );
        let active =
            keeper_admission_commitment(&binding, &challenge, KeeperLaunchKind::ActiveRestricted);
        assert_ne!(restricted, expected);
        assert_ne!(active, expected);
        assert_ne!(active, restricted);
        let permitted = keeper_admission_commitment(
            &binding,
            &challenge,
            KeeperLaunchKind::PermitActiveRestricted,
        );
        // Computed independently from the permit domain, NUL and bytes 0 through 143.
        assert_eq!(
            permitted,
            [
                0x7b, 0xaf, 0xfc, 0xd9, 0x4f, 0xb5, 0x3a, 0x0d, 0x24, 0xdf, 0xbf, 0x65, 0x51, 0x2f,
                0xd9, 0x22, 0x49, 0x5a, 0xd9, 0xba, 0xef, 0xa6, 0x78, 0x6c, 0xf6, 0xc0, 0xd0, 0x14,
                0xc6, 0x83, 0xeb, 0x60,
            ]
        );
        assert_ne!(permitted, expected);
        assert_ne!(permitted, restricted);
        assert_ne!(permitted, active);
    }

    #[test]
    fn keeper_bootstrap_accepts_the_reopened_kind() {
        let binding = public_launch_binding();
        let mut wire = binding.encode_bootstrap(KeeperLaunchKind::Generic);
        wire[9] = 4;
        let (decoded, kind) = LaunchBinding::decode_bootstrap(&wire).unwrap();
        assert_eq!(kind, KeeperLaunchKind::ReopenedActiveRestricted);
        assert_eq!(kind.opening_authority(), Some(OpeningAuthority::Reopened));
        assert!(kind.is_active());
        validate_launch_binding(&binding, &decoded).unwrap();
        assert_eq!(
            binding.encode_bootstrap(KeeperLaunchKind::ReopenedActiveRestricted),
            wire
        );
        for (kind, authority) in [
            (KeeperLaunchKind::Generic, None),
            (KeeperLaunchKind::PreUnlockRestricted, None),
            (KeeperLaunchKind::ActiveRestricted, None),
            (
                KeeperLaunchKind::PermitActiveRestricted,
                Some(OpeningAuthority::Permit),
            ),
        ] {
            assert_eq!(kind.opening_authority(), authority);
        }
    }

    #[test]
    fn keeper_admission_commitment_separates_reopened_from_permit_kind() {
        let challenge = std::array::from_fn(|index| index as u8);
        let binding = LaunchBinding {
            generation: std::array::from_fn(|index| (index + 32) as u8),
            identity: [
                0x4041424344454647,
                0x48494a4b4c4d4e4f,
                0x5051525354555657,
                0x58595a5b5c5d5e5f,
                0x6061626364656667,
                0x68696a6b6c6d6e6f,
            ],
            record_digest: std::array::from_fn(|index| (index + 112) as u8),
        };
        let reopened = keeper_admission_commitment(
            &binding,
            &challenge,
            KeeperLaunchKind::ReopenedActiveRestricted,
        );
        // Computed independently from the reopened domain, NUL and bytes 0 through 143.
        assert_eq!(
            reopened,
            [
                0xd5, 0x0a, 0x63, 0xaa, 0x46, 0xb4, 0x7f, 0xf6, 0xfd, 0x03, 0x72, 0xde, 0x8b, 0x5b,
                0xca, 0x0e, 0x69, 0xa0, 0xf7, 0xa6, 0xed, 0xea, 0xaf, 0x3f, 0x6a, 0x22, 0xee, 0x74,
                0xa7, 0x6e, 0x9b, 0xec,
            ]
        );
        for kind in [
            KeeperLaunchKind::Generic,
            KeeperLaunchKind::PreUnlockRestricted,
            KeeperLaunchKind::ActiveRestricted,
            KeeperLaunchKind::PermitActiveRestricted,
        ] {
            assert_ne!(
                keeper_admission_commitment(&binding, &challenge, kind),
                reopened
            );
        }
    }

    #[test]
    fn keeper_admission_commitment_changes_with_each_bound_component() {
        let challenge = [9; 32];
        let kind = KeeperLaunchKind::Generic;
        let expected = keeper_admission_commitment(&public_launch_binding(), &challenge, kind);
        let mut changed_challenge = challenge;
        changed_challenge[31] ^= 1;
        assert_ne!(
            keeper_admission_commitment(&public_launch_binding(), &changed_challenge, kind),
            expected
        );
        for component in 0..8 {
            let mut changed = public_launch_binding();
            match component {
                0 => changed.generation[31] ^= 1,
                1..=6 => changed.identity[component - 1] ^= 1,
                _ => changed.record_digest[31] ^= 1,
            }
            assert_ne!(
                keeper_admission_commitment(&changed, &challenge, kind),
                expected
            );
        }
    }

    #[test]
    fn admitted_launch_binding_rejects_each_changed_component() {
        let expected = public_launch_binding();
        validate_launch_binding(&expected, &public_launch_binding()).unwrap();
        for component in 0..3 {
            let mut changed = public_launch_binding();
            match component {
                0 => changed.generation[0] ^= 1,
                1 => changed.identity[0] ^= 1,
                _ => changed.record_digest[0] ^= 1,
            }
            assert_eq!(
                validate_launch_binding(&expected, &changed)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn admitted_launch_binding_failure_remains_invalid_after_restoration() {
        let expected = public_launch_binding();
        let mut changed = public_launch_binding();
        changed.record_digest[31] ^= 1;
        let validity = Validity::new();
        assert!(validity
            .check(|| validate_launch_binding(&expected, &changed))
            .is_err());
        let called = std::cell::Cell::new(false);
        let restored = validity.check(|| {
            called.set(true);
            validate_launch_binding(&expected, &public_launch_binding())
        });
        assert_eq!(restored.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(!called.get());
    }

    #[test]
    fn admitted_ciphertext_preserves_bytes_and_refuses_mutation() {
        let bytes = b"public synthetic admitted ciphertext";
        let cipher = seal_ciphertext(bytes).expect("seal retained ciphertext");
        let mut file = std::fs::File::from(cipher);
        assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600);
        assert_eq!(file.stream_position().unwrap(), 0);
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        // SAFETY: these operations query or attempt mutation on the owned memfd.
        let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
        assert_eq!(seals & 15, 15);
        file.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(
            file.write(b"x").unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            file.set_len(0).unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            file.set_len(bytes.len() as u64 + 1)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
    }

    #[test]
    fn admitted_ciphertext_refuses_empty_and_oversized_inputs() {
        assert_eq!(
            seal_ciphertext(&[]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            seal_ciphertext(&vec![1; 65_537]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn admitted_ciphertext_rejects_record_and_metadata_mismatches() {
        let bytes = b"public synthetic admitted ciphertext";
        let hash = linux_openssh_package::hex(&Sha256::digest(bytes));
        let cipher = seal_ciphertext(bytes).unwrap();
        validate_ciphertext(&cipher, bytes.len() as u64, &hash).unwrap();
        for length in [0, bytes.len() as u64 - 1, bytes.len() as u64 + 1, 65_537] {
            assert_eq!(
                validate_ciphertext(&cipher, length, &hash)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        assert_eq!(
            validate_ciphertext(&cipher, bytes.len() as u64, &"00".repeat(32))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        // SAFETY: fchmod changes only this owned synthetic anonymous file.
        assert_eq!(unsafe { libc::fchmod(cipher.as_raw_fd(), 0o640) }, 0);
        assert_eq!(
            validate_ciphertext(&cipher, bytes.len() as u64, &hash)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn admitted_ciphertext_rejects_an_unsealed_memfd() {
        let bytes = b"public synthetic admitted ciphertext";
        // SAFETY: the fixed terminated name and flags create one owned test memfd.
        let raw = unsafe {
            libc::memfd_create(
                c"vivac-unsealed-test".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        assert!(raw >= 0);
        // SAFETY: successful memfd_create returned a fresh owned descriptor.
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut file = File::from(descriptor);
        file.write_all(bytes).unwrap();
        let cipher = OwnedFd::from(file);
        assert_eq!(
            validate_ciphertext(
                &cipher,
                bytes.len() as u64,
                &linux_openssh_package::hex(&Sha256::digest(bytes))
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
