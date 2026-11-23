//! The keeper of a Linux custody session: the main process of the user
//! service `vivac-custody-<credential>-<generation>.service`.
//!
//! Before it reports READY the keeper checks that it is the binary that
//! started the unit, creates the session cgroup, retires an old session record
//! on kernel evidence, publishes its own record and binds its control socket.
//! After the client's `Begin` it runs the opening, serves the active session
//! and, on every way out, kills the session cgroup, waits for the kernel to
//! report it empty and only then publishes the opening permit again.
//!
//! The keeper is single-threaded: the spawn owner of the session requires it.

#[path = "linux_session_keeper/active.rs"]
mod active;
#[path = "linux_session_keeper/closing.rs"]
mod closing;
#[path = "linux_session_keeper/opening.rs"]
mod opening;

use crate::linux_admission_storage::PublishError;
use crate::linux_current_admission::{CurrentAdmission, OpeningAuthority};
use crate::linux_endpoint_runtime::{EndpointRuntime, RuntimeRoot};
use crate::linux_ipc::registration::KeeperAdoptedOffer;
use crate::linux_reservation::StoreReservation;
use crate::linux_retained_removal::{
    entry_identity, remove_retained_entry, sync_directory, EntryIdentity,
};
use crate::linux_session_cgroup::{boot_id, SessionCgroup, UnitCgroup};
use crate::linux_session_control::{
    ClientFrame, ControlConnection, ControlListener, KeeperFrame, KeeperOutcome,
};
use crate::linux_session_record::{
    observe_evidence, SessionRecord, SessionRecords, StoredSessionRecord,
};
use crate::linux_spawn::{Receipt, SpawnOwner};
use crate::linux_user_manager::{SessionLimits, SessionUnit};
use std::ffi::{CStr, OsStr, OsString};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The client sends `Begin` within this budget after READY, or the keeper ends.
const BEGIN_TIMEOUT: Duration = Duration::from_secs(10);
/// A connected client sends its one request within this budget.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// The unit allows 30 seconds to stop; closing keeps five of them in reserve.
const CLOSE_BUDGET: Duration = Duration::from_secs(25);
const HANGUP: libc::c_short = libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;

fn refused() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}

pub(crate) fn run(arguments: impl Iterator<Item = OsString>) -> i32 {
    let keeper = match start(arguments) {
        Ok(keeper) => keeper,
        Err(_) => {
            // The journal keeps this line; it carries no detail of the store.
            let _ = io::stderr()
                .lock()
                .write_all(b"Linux custody startup refused.\n");
            return 1;
        }
    };
    match keeper.serve() {
        KeeperOutcome::Closed => 0,
        _ => 1,
    }
}

struct KeeperArguments {
    unit: SessionUnit,
    identity: (u64, u64),
    limits: SessionLimits,
}

/// `<credential> <generation> <dev>:<ino> <idle-seconds> <maximum-seconds>`,
/// each in its one canonical form.
fn parse_arguments(arguments: impl Iterator<Item = OsString>) -> io::Result<KeeperArguments> {
    let values = arguments
        .take(6)
        .map(|value| value.into_string().map_err(|_| refused()))
        .collect::<io::Result<Vec<_>>>()?;
    let [credential, generation, identity, idle, maximum] =
        <[String; 5]>::try_from(values).map_err(|_| refused())?;
    let unit = SessionUnit::from_parts(&credential, &generation).map_err(|_| refused())?;
    let (device, inode) = identity.split_once(':').ok_or_else(refused)?;
    let identity = (canonical_number(device)?, canonical_number(inode)?);
    let limits = SessionLimits::from_values(Some(&idle), Some(&maximum)).map_err(|_| refused())?;
    Ok(KeeperArguments {
        unit,
        identity,
        limits,
    })
}

fn canonical_number(text: &str) -> io::Result<u64> {
    let value = text.parse::<u64>().map_err(|_| refused())?;
    if value.to_string() != text {
        return Err(refused());
    }
    Ok(value)
}

/// Refuses a binary replaced between `open-linux` and the unit start.
fn same_executable(expected: (u64, u64)) -> io::Result<()> {
    let metadata = std::fs::File::open("/proc/self/exe")?.metadata()?;
    if (metadata.dev(), metadata.ino()) != expected {
        return Err(refused());
    }
    Ok(())
}

/// Sends `READY=1` to the service manager's socket: a path, or a name with a
/// leading `@`.
fn notify_ready(socket: &OsStr) -> io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    let address = match socket.as_bytes().split_first() {
        Some((b'@', name)) => SocketAddr::from_abstract_name(name)?,
        Some((b'/', _)) => SocketAddr::from_pathname(Path::new(socket))?,
        _ => return Err(refused()),
    };
    let message = b"READY=1";
    if UnixDatagram::unbound()?.send_to_addr(message, &address)? != message.len() {
        return Err(io::Error::from(io::ErrorKind::WriteZero));
    }
    Ok(())
}

/// Blocks the stop signals and returns a descriptor that reports them.
/// Children start with a reset mask.
fn block_signals() -> io::Result<OwnedFd> {
    let mut set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: sigemptyset initializes the complete signal set.
    if unsafe { libc::sigemptyset(set.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful sigemptyset initialized the complete value.
    let mut set = unsafe { set.assume_init() };
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: set is initialized and every listed signal is valid.
        if unsafe { libc::sigaddset(&mut set, signal) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // SAFETY: the signal set is initialized and no previous mask is requested.
    let code = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code));
    }
    // SAFETY: the initialized mask requests a new owned signal descriptor.
    let raw = unsafe { libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful signalfd returned a fresh exclusively owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// Reads every pending signal and reports whether a stop was requested.
/// A hangup is consumed and ignored: the keeper has no terminal.
fn stop_requested(signals: &OwnedFd) -> io::Result<bool> {
    let mut stop = false;
    loop {
        // SAFETY: an all-zero siginfo record is a valid buffer.
        let mut information: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::signalfd_siginfo>();
        // SAFETY: the record is writable for its whole size.
        let count = unsafe { libc::read(signals.as_raw_fd(), (&raw mut information).cast(), size) };
        if count < 0 {
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(stop),
                _ => return Err(error),
            }
        }
        if count as usize != size {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let signal = information.ssi_signo as libc::c_int;
        stop |= signal == libc::SIGTERM || signal == libc::SIGINT;
    }
}

fn poll_timeout(deadline: Instant) -> libc::c_int {
    deadline
        .saturating_duration_since(Instant::now())
        .saturating_add(Duration::from_millis(1))
        .as_millis()
        .min(libc::c_int::MAX as u128) as libc::c_int
}

/// Waits until one of `descriptors` is ready or `deadline` passes.
fn poll_until(descriptors: &mut [libc::pollfd], deadline: Instant) -> io::Result<()> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        for descriptor in descriptors.iter_mut() {
            descriptor.revents = 0;
        }
        // SAFETY: every entry is an initialized pollfd in this writable slice.
        let ready = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                poll_timeout(deadline),
            )
        };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready > 0 {
            return Ok(());
        }
    }
}

fn watched(fd: RawFd) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }
}

/// Waits until `fd` has input or an error condition, never past `deadline`.
fn wait_readable(fd: RawFd, deadline: Instant) -> io::Result<()> {
    poll_until(&mut [watched(fd)], deadline)
}

/// The poll events `fd` reports right now, or zero.
fn ready_now(fd: RawFd) -> io::Result<libc::c_short> {
    let mut descriptor = watched(fd);
    loop {
        // SAFETY: descriptor is one initialized pollfd; zero timeout.
        let ready = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if ready >= 0 {
            return Ok(if ready > 0 { descriptor.revents } else { 0 });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn short_request() -> Instant {
    Instant::now() + REQUEST_TIMEOUT
}

fn request_deadline(limit: Instant) -> Instant {
    short_request().min(limit)
}

/// The unit's runtime directory: exactly the given path, a directory the
/// account owns with mode 0700, opened without following a link.
fn open_runtime_directory(
    given: Option<&OsStr>,
    expected: &Path,
    uid: libc::uid_t,
) -> io::Result<OwnedFd> {
    if given != Some(expected.as_os_str()) {
        return Err(refused());
    }
    let directory: OwnedFd = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(expected)?
        .into();
    let metadata = std::fs::File::from(directory.try_clone()?).metadata()?;
    if !metadata.is_dir() || metadata.mode() & 0o7777 != 0o700 || metadata.uid() != uid {
        return Err(refused());
    }
    Ok(directory)
}

/// Open from the permit when there is one. Without it, only a session record
/// retired on kernel evidence allows a reopening.
fn opening_authority(retired_record: bool, permit_absent: bool) -> io::Result<OpeningAuthority> {
    match (retired_record, permit_absent) {
        (_, false) => Ok(OpeningAuthority::Permit),
        (true, true) => Ok(OpeningAuthority::Reopened),
        (false, true) => Err(refused()),
    }
}

fn process_ids() -> (u32, u32) {
    // SAFETY: these calls only query this process's credentials.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

/// A stale endpoint is a socket of this account with one link that refuses
/// connections: nobody listens on it any more.
fn stale_endpoint(
    identity: &EntryIdentity,
    ids: (u32, u32),
    connected: io::Result<()>,
) -> io::Result<()> {
    let socket = identity.mode & libc::S_IFMT == libc::S_IFSOCK
        && (identity.uid, identity.gid) == ids
        && identity.links == 1;
    match connected {
        Err(error) if socket && error.raw_os_error() == Some(libc::ECONNREFUSED) => Ok(()),
        _ => Err(refused()),
    }
}

/// Removes the old generation's endpoint socket, if any, after checking that
/// it is stale and still the object that was checked.
fn remove_stale_endpoint(directory: &OwnedFd, name: &CStr) -> io::Result<()> {
    let Some(identity) = entry_identity(directory, name)? else {
        return Ok(());
    };
    let connected = EndpointRuntime::from_named_directory(directory.try_clone()?, name)?
        .connect_once(Instant::now() + REQUEST_TIMEOUT)
        .map(drop);
    stale_endpoint(&identity, process_ids(), connected)?;
    remove_retained_entry(directory, name, &identity)?;
    sync_directory(directory)
}

/// Retires the record of a lost session. The stale endpoint goes first, so a
/// refusal on it leaves the record, and with it the evidence, in place.
fn retire(
    admission: &CurrentAdmission,
    records: &SessionRecords<'_>,
    old: &StoredSessionRecord,
) -> io::Result<()> {
    observe_evidence(&old.record)?.ok_or_else(refused)?;
    admission.with_endpoint_location(|name, directory| remove_stale_endpoint(&directory, name))?;
    records.remove(old)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn start(arguments: impl Iterator<Item = OsString>) -> io::Result<Keeper> {
    let KeeperArguments {
        unit,
        identity,
        limits,
    } = parse_arguments(arguments)?;
    same_executable(identity)?;
    let signals = block_signals()?;
    let unit_cgroup = UnitCgroup::current(&unit.name())?;
    let session = unit_cgroup.create_session()?;
    let (uid, _) = process_ids();
    let runtime = PathBuf::from(format!("/run/user/{uid}/{}", unit.runtime_name()));
    let runtime_directory = open_runtime_directory(
        std::env::var_os("RUNTIME_DIRECTORY").as_deref(),
        &runtime,
        uid,
    )?;
    let admission = CurrentAdmission::open_fixed_namespace(unit.credential())?;
    // Activation binds the endpoint in `endpoints-v1`; that collection must exist.
    admission.with_endpoint_location(|_, _| Ok(()))?;
    // Everything the new record needs is read before an old one is retired,
    // so that little can fail between the retirement and the publication.
    let record = SessionRecord {
        version: 1,
        credential_id: unit.credential().to_owned(),
        generation: unit.generation().to_owned(),
        unit: unit.name(),
        unit_cgroup: unit_cgroup.path().to_owned(),
        unit_cgroup_inode: unit_cgroup.identity().inode,
        session_cgroup_inode: session.identity().inode,
        boot_id: boot_id()?,
        admission_sha256: hex(&admission.admission_digest()?),
    };
    // Retiring the old record and publishing this one happen under the
    // credential reservation: inode numbers are reused, so an identity check
    // alone cannot tell an old record from a new one.
    let reservation = admission.reserve_package_directory()?;
    let records = SessionRecords::new(&admission);
    let retired = match records.read()? {
        Some(old) => {
            retire(&admission, &records, &old)?;
            true
        }
        None => false,
    };
    let authority = opening_authority(retired, admission.opening_permit_absent()?)?;
    if authority == OpeningAuthority::Permit {
        admission.observe_opening_permit()?;
    }
    let stored = records.publish(&record).map_err(|error| match error {
        PublishError::Unpublished(error) | PublishError::PublishedDurabilityUnknown(error) => error,
    })?;
    let ready = ControlListener::bind(&runtime_directory).and_then(|listener| {
        notify_ready(&std::env::var_os("NOTIFY_SOCKET").ok_or_else(refused)?)?;
        Ok(listener)
    });
    let mut keeper = Keeper {
        unit,
        limits,
        session,
        runtime,
        admission,
        record: stored,
        authority,
        reservation: Some(reservation),
        signals,
        listener: None,
    };
    match ready {
        Ok(listener) => {
            keeper.listener = Some(listener);
            Ok(keeper)
        }
        Err(error) => {
            keeper.abandon();
            Err(error)
        }
    }
}

/// What the keeper found at startup and holds for the whole session.
pub(super) struct Keeper {
    unit: SessionUnit,
    limits: SessionLimits,
    session: SessionCgroup,
    runtime: PathBuf,
    admission: CurrentAdmission,
    record: StoredSessionRecord,
    authority: OpeningAuthority,
    /// Held from before the record was published until the supervisor starts.
    reservation: Option<StoreReservation>,
    signals: OwnedFd,
    listener: Option<ControlListener>,
}

/// What the opening has started, kept for closing or for failure.
#[derive(Default)]
pub(super) struct Children {
    owner: Option<SpawnOwner>,
    supervisor: Option<(Receipt, OwnedFd)>,
    verifier: Option<(Receipt, OwnedFd)>,
    runtime: Option<RuntimeRoot>,
    adopted: Option<KeeperAdoptedOffer>,
}

impl Keeper {
    fn serve(mut self) -> KeeperOutcome {
        let Some(client) = self.await_begin() else {
            self.abandon();
            return KeeperOutcome::Refused;
        };
        // The supervisor takes the reservation itself, without waiting.
        self.reservation = None;
        let mut children = Children::default();
        if opening::open(&self, &client, &mut children).is_err() {
            return closing::fail_opening(&self, &mut children, Some(client));
        }
        drop(client);
        let (reason, client) = active::serve(&self, &mut children);
        closing::close_session(reason, client, &self, &mut children)
    }

    /// Ends before `Begin`. Nothing was consumed: a record beside its permit
    /// is removed, while a reopening keeps its record so that the next opening
    /// still finds the evidence once this unit has ended.
    fn abandon(&self) {
        if self.authority == OpeningAuthority::Permit {
            let _ = self.remove_record();
        }
    }

    /// Removes this keeper's record through a fresh admission: a failed
    /// publication leaves the admission that tried it invalid.
    fn remove_record(&self) -> io::Result<()> {
        let admission = CurrentAdmission::open_fixed_namespace(self.unit.credential())?;
        SessionRecords::new(&admission).remove(&self.record)
    }

    fn listener(&self) -> io::Result<&ControlListener> {
        self.listener.as_ref().ok_or_else(refused)
    }

    /// Accepts connections until one sends `Begin`. Any other request is
    /// answered `Refused`; a peer of another account, or one that fails, is
    /// dropped and the keeper keeps waiting.
    fn await_begin(&self) -> Option<ControlConnection> {
        let deadline = Instant::now() + BEGIN_TIMEOUT;
        let listener = self.listener().ok()?;
        loop {
            let mut descriptors = [
                watched(self.signals.as_raw_fd()),
                watched(listener.as_raw_fd()),
            ];
            poll_until(&mut descriptors, deadline).ok()?;
            if descriptors[0].revents != 0 && stop_requested(&self.signals).unwrap_or(true) {
                return None;
            }
            if descriptors[1].revents == 0 {
                continue;
            }
            while let Some(connection) = next_connection(listener) {
                match connection.receive_client(request_deadline(deadline)) {
                    Ok(ClientFrame::Begin) => return Some(connection),
                    Ok(_) => refuse_request(&connection, deadline),
                    Err(_) => {}
                }
            }
        }
    }
}

/// The next pending connection of this account. A foreign peer is dropped
/// and the next one taken; any other accept failure ends this round, and the
/// keeper keeps serving.
fn next_connection(listener: &ControlListener) -> Option<ControlConnection> {
    loop {
        match listener.accept() {
            Ok(connection) => return connection,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => continue,
            Err(_) => return None,
        }
    }
}

fn refuse_request(connection: &ControlConnection, deadline: Instant) {
    let _ = connection.send_keeper(
        KeeperFrame::Outcome(KeeperOutcome::Refused),
        request_deadline(deadline),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_retained_removal::EntryIdentity;
    use std::os::unix::net::UnixDatagram;
    use std::path::PathBuf;

    const CREDENTIAL: &str = "00000000000000000000000001";
    const GENERATION: &str = "00000000000000000000000002";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "vivac-keeper-{label}-{}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn arguments(values: &[&str]) -> impl Iterator<Item = OsString> {
        values
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn keeper_arguments_accept_only_canonical_values() {
        let parsed =
            parse_arguments(arguments(&[CREDENTIAL, GENERATION, "7:42", "43200", "60"])).unwrap();
        assert_eq!(parsed.unit.credential(), CREDENTIAL);
        assert_eq!(parsed.unit.generation(), GENERATION);
        assert_eq!(parsed.identity, (7, 42));
        assert_eq!(parsed.limits.idle, Duration::from_secs(43_200));
        assert_eq!(parsed.limits.maximum, Duration::from_secs(60));
        for values in [
            &[][..],
            &[CREDENTIAL, GENERATION, "7:42", "43200"][..],
            &[CREDENTIAL, GENERATION, "7:42", "43200", "604800", "x"][..],
            &[CREDENTIAL, "notanid", "7:42", "43200", "604800"][..],
            &[CREDENTIAL, GENERATION, "7", "43200", "604800"][..],
            &[CREDENTIAL, GENERATION, "07:42", "43200", "604800"][..],
            &[CREDENTIAL, GENERATION, "7:42:1", "43200", "604800"][..],
            &[CREDENTIAL, GENERATION, "7:+42", "43200", "604800"][..],
            &[CREDENTIAL, GENERATION, "7:42", "43201", "604800"][..],
            &[CREDENTIAL, GENERATION, "7:42", "43200", "604801"][..],
            &[CREDENTIAL, GENERATION, "7:42", "59", "604800"][..],
            &[CREDENTIAL, GENERATION, "7:42", "0600", "604800"][..],
        ] {
            assert!(parse_arguments(arguments(values)).is_err(), "{values:?}");
        }
    }

    #[test]
    fn keeper_refuses_a_different_executable_identity() {
        assert!(same_executable((0, 0)).is_err());
        let metadata = std::fs::File::open("/proc/self/exe")
            .unwrap()
            .metadata()
            .unwrap();
        use std::os::unix::fs::MetadataExt;
        same_executable((metadata.dev(), metadata.ino())).unwrap();
    }

    #[test]
    fn readiness_reaches_a_path_notification_socket() {
        let scratch = Scratch::new("notify");
        let path = scratch.0.join("notify");
        let receiver = UnixDatagram::bind(&path).unwrap();
        notify_ready(path.as_os_str()).unwrap();
        let mut buffer = [0u8; 32];
        let count = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..count], b"READY=1");
        assert!(notify_ready(OsStr::new("relative")).is_err());
        assert!(notify_ready(OsStr::new("")).is_err());
    }

    #[test]
    fn readiness_reaches_an_abstract_notification_socket() {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::SocketAddr;
        let name = format!("vivac-keeper-notify-{}", std::process::id());
        let receiver =
            UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap())
                .unwrap();
        notify_ready(OsStr::new(&format!("@{name}"))).unwrap();
        let mut buffer = [0u8; 32];
        let count = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..count], b"READY=1");
    }

    #[test]
    fn runtime_directory_must_be_the_private_unit_directory() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("runtime");
        let private = scratch.0.join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        // SAFETY: geteuid only reads this process's credentials.
        let uid = unsafe { libc::geteuid() };
        open_runtime_directory(Some(private.as_os_str()), &private, uid).unwrap();
        assert!(open_runtime_directory(None, &private, uid).is_err());
        assert!(open_runtime_directory(Some(scratch.0.as_os_str()), &private, uid).is_err());
        assert!(open_runtime_directory(Some(private.as_os_str()), &private, uid + 1).is_err());
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(open_runtime_directory(Some(private.as_os_str()), &private, uid).is_err());
        let file = scratch.0.join("file");
        std::fs::write(&file, b"").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(open_runtime_directory(Some(file.as_os_str()), &file, uid).is_err());
    }

    #[test]
    fn opening_authority_follows_the_permit_and_the_retired_record() {
        assert_eq!(
            opening_authority(false, false).unwrap(),
            OpeningAuthority::Permit
        );
        assert_eq!(
            opening_authority(true, false).unwrap(),
            OpeningAuthority::Permit
        );
        assert_eq!(
            opening_authority(true, true).unwrap(),
            OpeningAuthority::Reopened
        );
        assert!(opening_authority(false, true).is_err());
    }

    #[test]
    fn only_a_refusing_socket_of_the_account_is_a_stale_endpoint() {
        let ids = (1000, 1000);
        let socket = EntryIdentity {
            device: 1,
            inode: 2,
            mode: libc::S_IFSOCK | 0o600,
            uid: 1000,
            gid: 1000,
            links: 1,
        };
        let refused = || Err(io::Error::from_raw_os_error(libc::ECONNREFUSED));
        stale_endpoint(&socket, ids, refused()).unwrap();
        assert!(stale_endpoint(&socket, ids, Ok(())).is_err());
        assert!(stale_endpoint(
            &socket,
            ids,
            Err(io::Error::from_raw_os_error(libc::ENOENT))
        )
        .is_err());
        assert!(stale_endpoint(&socket, (1000, 1001), refused()).is_err());
        let linked = EntryIdentity { links: 2, ..socket };
        assert!(stale_endpoint(&linked, ids, refused()).is_err());
        let file = EntryIdentity {
            mode: libc::S_IFREG | 0o600,
            ..socket
        };
        assert!(stale_endpoint(&file, ids, refused()).is_err());
    }
}
