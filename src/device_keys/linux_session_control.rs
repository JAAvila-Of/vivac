//! The keeper's control protocol: one `SOCK_SEQPACKET` socket, `control.sock`,
//! in the session unit's runtime directory. Frames have fixed lengths, so a
//! datagram of any other size is refused instead of interpreted. The client
//! authenticates the keeper before it sends anything: same account, and a
//! process that is a member of exactly the recorded unit cgroup. Same-account
//! processes are inside the profile's trust boundary, so this separates peers
//! outside the user manager (an SSH login session, say) and catches
//! misdirection; it does not stop a hostile process of the same account.
#![allow(
    dead_code,
    reason = "The session open and close commands consume these pieces in later steps."
)]

use super::linux_ipc::registration::live_pid;
use super::linux_ipc::{connected_credentials, connected_pidfd};
use super::linux_session_cgroup::peer_in_cgroup;
use std::ffi::CStr;
use std::fmt;
use std::io;
use std::mem::zeroed;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub(crate) const CONTROL_SOCKET: &CStr = c"control.sock";

const HEADER: usize = 16;
const SECRET: usize = 64;
const SECRET_FRAME: usize = HEADER + SECRET;
const MAGIC: &[u8; 8] = b"VVKCTL\0\x01";
const BACKLOG: libc::c_int = 4;
const CONNECT_RETRY: Duration = Duration::from_millis(10);

const BEGIN: u8 = 0x01;
const SECRET_KIND: u8 = 0x02;
const CANCEL: u8 = 0x03;
const CLOSE: u8 = 0x04;
const SECRET_REQUEST: u8 = 0x11;
const OPENED: u8 = 0x12;
const CLOSING: u8 = 0x13;
const OUTCOME: u8 = 0x14;

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn denied() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}

pub(crate) enum ClientFrame {
    Begin,
    Secret(Zeroizing<[u8; 64]>),
    Cancel,
    Close,
}

impl fmt::Debug for ClientFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Begin => "Begin",
            Self::Secret(_) => "Secret(<redacted>)",
            Self::Cancel => "Cancel",
            Self::Close => "Close",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeeperFrame {
    SecretRequest,
    Opened { reopened: bool },
    Closing,
    Outcome(KeeperOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeeperOutcome {
    Refused,
    Closed,
    Interrupted,
    PublicationUncertain,
}

fn header(kind: u8, flag: u8) -> [u8; HEADER] {
    let mut wire = [0u8; HEADER];
    wire[..8].copy_from_slice(MAGIC);
    wire[8] = kind;
    wire[9] = flag;
    wire
}

/// Checks the fixed header and returns the kind and flag bytes.
fn parse_header(wire: &[u8]) -> io::Result<(u8, u8)> {
    if wire.len() < HEADER || wire[..8] != MAGIC[..] || wire[10..HEADER].iter().any(|b| *b != 0) {
        return Err(invalid());
    }
    Ok((wire[8], wire[9]))
}

fn require_hex(secret: &[u8]) -> io::Result<()> {
    if secret.len() == SECRET
        && secret
            .iter()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn encode_client(frame: &ClientFrame) -> io::Result<Zeroizing<Vec<u8>>> {
    let (kind, secret) = match frame {
        ClientFrame::Begin => (BEGIN, None),
        ClientFrame::Cancel => (CANCEL, None),
        ClientFrame::Close => (CLOSE, None),
        ClientFrame::Secret(value) => {
            require_hex(&value[..])?;
            (SECRET_KIND, Some(value))
        }
    };
    let mut wire = Zeroizing::new(Vec::with_capacity(SECRET_FRAME));
    wire.extend_from_slice(&header(kind, 0));
    if let Some(value) = secret {
        wire.extend_from_slice(&value[..]);
    }
    Ok(wire)
}

fn decode_client(wire: &[u8]) -> io::Result<ClientFrame> {
    let (kind, flag) = parse_header(wire)?;
    if flag != 0 {
        return Err(invalid());
    }
    match (kind, wire.len()) {
        (BEGIN, HEADER) => Ok(ClientFrame::Begin),
        (CANCEL, HEADER) => Ok(ClientFrame::Cancel),
        (CLOSE, HEADER) => Ok(ClientFrame::Close),
        (SECRET_KIND, SECRET_FRAME) => {
            require_hex(&wire[HEADER..])?;
            let mut value = Zeroizing::new([0u8; SECRET]);
            value.copy_from_slice(&wire[HEADER..]);
            Ok(ClientFrame::Secret(value))
        }
        _ => Err(invalid()),
    }
}

fn encode_keeper(frame: KeeperFrame) -> [u8; HEADER] {
    match frame {
        KeeperFrame::SecretRequest => header(SECRET_REQUEST, 0),
        KeeperFrame::Opened { reopened } => header(OPENED, u8::from(reopened)),
        KeeperFrame::Closing => header(CLOSING, 0),
        KeeperFrame::Outcome(outcome) => header(
            OUTCOME,
            match outcome {
                KeeperOutcome::Refused => 0,
                KeeperOutcome::Closed => 1,
                KeeperOutcome::Interrupted => 2,
                KeeperOutcome::PublicationUncertain => 3,
            },
        ),
    }
}

fn decode_keeper(wire: &[u8]) -> io::Result<KeeperFrame> {
    let (kind, flag) = parse_header(wire)?;
    if wire.len() != HEADER {
        return Err(invalid());
    }
    match (kind, flag) {
        (SECRET_REQUEST, 0) => Ok(KeeperFrame::SecretRequest),
        (OPENED, 0 | 1) => Ok(KeeperFrame::Opened {
            reopened: flag == 1,
        }),
        (CLOSING, 0) => Ok(KeeperFrame::Closing),
        (OUTCOME, 0) => Ok(KeeperFrame::Outcome(KeeperOutcome::Refused)),
        (OUTCOME, 1) => Ok(KeeperFrame::Outcome(KeeperOutcome::Closed)),
        (OUTCOME, 2) => Ok(KeeperFrame::Outcome(KeeperOutcome::Interrupted)),
        (OUTCOME, 3) => Ok(KeeperFrame::Outcome(KeeperOutcome::PublicationUncertain)),
        _ => Err(invalid()),
    }
}

fn address(path: &[u8]) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    // SAFETY: an all-zero sockaddr_un is a valid empty address.
    let mut address: libc::sockaddr_un = unsafe { zeroed() };
    if path.is_empty() || path.contains(&0) || path.len() >= address.sun_path.len() {
        return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in address.sun_path.iter_mut().zip(path) {
        *slot = *byte as libc::c_char;
    }
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + path.len() + 1;
    Ok((address, length as libc::socklen_t))
}

fn new_socket() -> io::Result<OwnedFd> {
    // SAFETY: socket takes scalar arguments and returns a fresh descriptor.
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful socket returned this descriptor, owned from here.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// Waits until `fd` reports `events` (or an error condition the next system
/// call will name), retrying interrupted waits, never past `deadline`.
fn wait(fd: RawFd, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        let millis = (deadline - now).as_millis().saturating_add(1);
        let millis = millis.min(i32::MAX as u128) as libc::c_int;
        let mut poll = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: poll is one initialized descriptor entry.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready > 0 && poll.revents != 0 {
            return Ok(());
        }
    }
}

fn current_uid() -> libc::uid_t {
    // SAFETY: getuid only queries the process identity.
    unsafe { libc::getuid() }
}

pub(crate) struct ControlListener {
    socket: OwnedFd,
}

impl ControlListener {
    /// Binds `control.sock` inside `runtime`, owner-only, before listening, so
    /// nobody can connect while the file still has default permissions.
    pub(crate) fn bind(runtime: &OwnedFd) -> io::Result<Self> {
        let socket = new_socket()?;
        let mut full = format!("/proc/self/fd/{}/", runtime.as_raw_fd()).into_bytes();
        full.extend_from_slice(CONTROL_SOCKET.to_bytes());
        let (address, length) = address(&full)?;
        // SAFETY: address is an initialized sockaddr_un of the given length.
        if unsafe { libc::bind(socket.as_raw_fd(), (&raw const address).cast(), length) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the directory descriptor is live and the name is NUL-terminated.
        if unsafe { libc::fchmodat(runtime.as_raw_fd(), CONTROL_SOCKET.as_ptr(), 0o600, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: listen takes the owned socket and a scalar backlog.
        if unsafe { libc::listen(socket.as_raw_fd(), BACKLOG) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { socket })
    }

    /// The next pending connection, or `None` when there is none. A peer of a
    /// different account is dropped and reported as `PermissionDenied`.
    pub(crate) fn accept(&self) -> io::Result<Option<ControlConnection>> {
        loop {
            // SAFETY: null address arguments ask accept4 for no peer address.
            let raw = unsafe {
                libc::accept4(
                    self.socket.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                )
            };
            if raw < 0 {
                let error = io::Error::last_os_error();
                return match error.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock => Ok(None),
                    _ => Err(error),
                };
            }
            // SAFETY: successful accept4 returned this fresh descriptor.
            let socket = unsafe { OwnedFd::from_raw_fd(raw) };
            if connected_credentials(socket.as_raw_fd())?.uid != current_uid() {
                return Err(denied());
            }
            return Ok(Some(ControlConnection { socket }));
        }
    }
}

impl AsRawFd for ControlListener {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

#[derive(Debug)]
pub(crate) struct ControlConnection {
    socket: OwnedFd,
}

impl ControlConnection {
    /// Connects to the keeper and proves who answered before returning: the
    /// account's uid, and a live process whose cgroup is exactly `unit_cgroup`.
    /// The peer is held by pidfd, so the pid read from it cannot be a reused
    /// number; the pidfd must still be alive after the membership read.
    pub(crate) fn connect_keeper(
        socket: &Path,
        unit_cgroup: &str,
        deadline: Instant,
    ) -> io::Result<Self> {
        let owned = connect_socket(socket, deadline)?;
        if connected_credentials(owned.as_raw_fd())?.uid != current_uid() {
            return Err(denied());
        }
        let pidfd = connected_pidfd(owned.as_raw_fd())?;
        let pid = live_pid(&pidfd)?;
        if !peer_in_cgroup(pid, unit_cgroup)? {
            return Err(denied());
        }
        if live_pid(&pidfd)? != pid || !pidfd_alive(&pidfd)? {
            return Err(denied());
        }
        Ok(Self { socket: owned })
    }

    /// Transport only, for tests on kernels without `SO_PEERPIDFD`.
    #[cfg(test)]
    fn connect_unauthenticated(socket: &Path, deadline: Instant) -> io::Result<Self> {
        Ok(Self {
            socket: connect_socket(socket, deadline)?,
        })
    }

    pub(crate) fn send_client(&self, frame: &ClientFrame, deadline: Instant) -> io::Result<()> {
        let wire = encode_client(frame)?;
        self.send(&wire, deadline)
    }

    pub(crate) fn receive_client(&self, deadline: Instant) -> io::Result<ClientFrame> {
        let (wire, length) = self.receive(deadline)?;
        decode_client(&wire[..length])
    }

    pub(crate) fn send_keeper(&self, frame: KeeperFrame, deadline: Instant) -> io::Result<()> {
        self.send(&encode_keeper(frame), deadline)
    }

    pub(crate) fn receive_keeper(&self, deadline: Instant) -> io::Result<KeeperFrame> {
        let (wire, length) = self.receive(deadline)?;
        decode_keeper(&wire[..length])
    }

    fn send(&self, wire: &[u8], deadline: Instant) -> io::Result<()> {
        loop {
            wait(self.socket.as_raw_fd(), libc::POLLOUT, deadline)?;
            // SAFETY: wire is readable for its length; MSG_NOSIGNAL suppresses SIGPIPE.
            let sent = unsafe {
                libc::send(
                    self.socket.as_raw_fd(),
                    wire.as_ptr().cast(),
                    wire.len(),
                    libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                )
            };
            if sent < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                    _ => return Err(error),
                }
            }
            return if sent as usize == wire.len() {
                Ok(())
            } else {
                Err(io::Error::from(io::ErrorKind::WriteZero))
            };
        }
    }

    /// One datagram into a zeroizing buffer one byte longer than the longest
    /// frame. MSG_TRUNC reports the true datagram length, so an oversized one
    /// is refused instead of silently cut to a valid-looking prefix.
    fn receive(&self, deadline: Instant) -> io::Result<(Zeroizing<[u8; SECRET_FRAME + 1]>, usize)> {
        let mut buffer = Zeroizing::new([0u8; SECRET_FRAME + 1]);
        loop {
            wait(self.socket.as_raw_fd(), libc::POLLIN, deadline)?;
            // SAFETY: buffer is writable for its whole length.
            let received = unsafe {
                libc::recv(
                    self.socket.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    libc::MSG_DONTWAIT | libc::MSG_TRUNC,
                )
            };
            if received < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                    _ => return Err(error),
                }
            }
            return match received as usize {
                0 => Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                length if length > SECRET_FRAME => Err(invalid()),
                length => Ok((buffer, length)),
            };
        }
    }
}

impl AsRawFd for ControlConnection {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

fn connect_socket(socket: &Path, deadline: Instant) -> io::Result<OwnedFd> {
    let (address, length) = address(socket.as_os_str().as_bytes())?;
    let owned = new_socket()?;
    loop {
        // SAFETY: address is an initialized sockaddr_un of the given length.
        let result =
            unsafe { libc::connect(owned.as_raw_fd(), (&raw const address).cast(), length) };
        if result == 0 {
            return Ok(owned);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINTR) => {}
            // A full backlog answers EAGAIN on a non-blocking socket.
            Some(libc::EAGAIN) => {
                if Instant::now() >= deadline {
                    return Err(io::Error::from(io::ErrorKind::TimedOut));
                }
                std::thread::sleep(CONNECT_RETRY);
            }
            _ => return Err(error),
        }
    }
}

/// A pidfd becomes readable when its process exits, zombies included.
fn pidfd_alive(pidfd: &OwnedFd) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: poll is one initialized descriptor entry; zero timeout.
        let ready = unsafe { libc::poll(&mut poll, 1, 0) };
        if ready >= 0 {
            return Ok(ready == 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::time::Duration;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let mut template = b"/tmp/vivac-control-XXXXXX\0".to_vec();
            // SAFETY: mkdtemp writes inside this writable NUL-terminated template.
            let created = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
            assert!(!created.is_null());
            template.pop();
            Self(PathBuf::from(std::ffi::OsStr::from_bytes(&template)))
        }
        fn directory(&self) -> OwnedFd {
            std::fs::File::open(&self.0).unwrap().into()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    fn secret() -> Zeroizing<[u8; 64]> {
        let mut value = Zeroizing::new([b'a'; 64]);
        value[0] = b'0';
        value[63] = b'f';
        value
    }

    /// The unified-hierarchy path of this process, or `None` on a host that
    /// still mounts cgroup v1, where no membership check can succeed.
    fn own_cgroup() -> Option<String> {
        let text = std::fs::read_to_string("/proc/self/cgroup").unwrap();
        let line = text.strip_suffix('\n')?;
        line.strip_prefix("0::").map(str::to_owned)
    }

    /// The one decision for a test that needs a kernel feature this host may
    /// lack. With `VIVAC_REQUIRE_LINUX_SESSION_KERNEL=1` (set in the Ubuntu
    /// 24.04 VM) the test fails, so it cannot pass by skipping. Otherwise the
    /// reason goes straight to stderr, which the libtest capture cannot hide.
    fn skip_or_fail(test: &str, reason: &str) {
        use std::io::Write;
        if std::env::var_os("VIVAC_REQUIRE_LINUX_SESSION_KERNEL").is_some_and(|value| value == "1")
        {
            panic!("{test} needs a Linux session kernel: {reason}");
        }
        let _ = writeln!(io::stderr(), "SKIPPED {test}: {reason}");
    }

    #[test]
    fn control_frames_round_trip_with_fixed_lengths() {
        for frame in [ClientFrame::Begin, ClientFrame::Cancel, ClientFrame::Close] {
            let wire = encode_client(&frame).unwrap();
            assert_eq!(wire.len(), 16);
            assert_eq!(&wire[..8], b"VVKCTL\0\x01");
            assert!(matches!(
                (decode_client(&wire).unwrap(), &frame),
                (ClientFrame::Begin, ClientFrame::Begin)
                    | (ClientFrame::Cancel, ClientFrame::Cancel)
                    | (ClientFrame::Close, ClientFrame::Close)
            ));
        }
        let wire = encode_client(&ClientFrame::Secret(secret())).unwrap();
        assert_eq!(wire.len(), 80);
        match decode_client(&wire).unwrap() {
            ClientFrame::Secret(value) => assert_eq!(*value, *secret()),
            _ => panic!("expected a secret"),
        }
        let frames = [
            KeeperFrame::SecretRequest,
            KeeperFrame::Opened { reopened: false },
            KeeperFrame::Opened { reopened: true },
            KeeperFrame::Closing,
            KeeperFrame::Outcome(KeeperOutcome::Refused),
            KeeperFrame::Outcome(KeeperOutcome::Closed),
            KeeperFrame::Outcome(KeeperOutcome::Interrupted),
            KeeperFrame::Outcome(KeeperOutcome::PublicationUncertain),
        ];
        for frame in frames {
            let wire = encode_keeper(frame);
            assert_eq!(wire.len(), 16);
            assert_eq!(decode_keeper(&wire).unwrap(), frame);
        }
    }

    #[test]
    fn control_frames_refuse_unknown_kinds_and_lengths() {
        let begin = encode_client(&ClientFrame::Begin).unwrap();
        let secret_wire = encode_client(&ClientFrame::Secret(secret())).unwrap();
        for length in [15, 17, 79, 81] {
            let mut wire = vec![0u8; length];
            let source = if length > 16 {
                &secret_wire[..]
            } else {
                &begin[..]
            };
            let copied = length.min(source.len());
            wire[..copied].copy_from_slice(&source[..copied]);
            assert!(decode_client(&wire).is_err(), "client length {length}");
            assert!(decode_keeper(&wire).is_err(), "keeper length {length}");
        }
        let mut unknown = begin.to_vec();
        unknown[8] = 0xff;
        assert!(decode_client(&unknown).is_err());
        assert!(decode_keeper(&unknown).is_err());
        for index in 10..16 {
            let mut dirty = begin.to_vec();
            dirty[index] = 1;
            assert!(decode_client(&dirty).is_err(), "reserved byte {index}");
        }
        let mut flagged = begin.to_vec();
        flagged[9] = 1;
        assert!(decode_client(&flagged).is_err());
        let mut wrong_version = begin.to_vec();
        wrong_version[7] = 2;
        assert!(decode_client(&wrong_version).is_err());
        let mut opened = encode_keeper(KeeperFrame::Opened { reopened: true }).to_vec();
        opened[9] = 2;
        assert!(decode_keeper(&opened).is_err());
        let mut outcome = encode_keeper(KeeperFrame::Outcome(KeeperOutcome::Closed)).to_vec();
        outcome[9] = 9;
        assert!(decode_keeper(&outcome).is_err());
        // A client kind is not a keeper kind and the reverse.
        assert!(decode_keeper(&begin).is_err());
        assert!(decode_client(&encode_keeper(KeeperFrame::Closing)).is_err());
    }

    #[test]
    fn secret_frame_requires_lowercase_hex() {
        let wire = encode_client(&ClientFrame::Secret(secret())).unwrap();
        for bad in [b'A', 0x00, b'g', b' '] {
            let mut tampered = wire.to_vec();
            tampered[16 + 10] = bad;
            assert!(decode_client(&tampered).is_err(), "byte {bad:#x}");
        }
        let mut invalid = secret();
        invalid[5] = b'A';
        assert!(encode_client(&ClientFrame::Secret(invalid)).is_err());
        let mut flagged = wire.to_vec();
        flagged[9] = 1;
        assert!(decode_client(&flagged).is_err());
    }

    #[test]
    fn debug_output_never_shows_the_secret() {
        let text = format!("{:?}", ClientFrame::Secret(secret()));
        assert!(!text.contains("aaaa") && !text.contains("97"));
        assert!(text.contains("Secret"));
    }

    #[test]
    fn keeper_authentication_requires_the_recorded_cgroup() {
        const TEST: &str = "keeper_authentication_requires_the_recorded_cgroup";
        let Some(own) = own_cgroup() else {
            skip_or_fail(TEST, "this host has no unified cgroup hierarchy");
            return;
        };
        let scratch = Scratch::new();
        let listener = ControlListener::bind(&scratch.directory()).unwrap();
        let socket = scratch.0.join(CONTROL_SOCKET.to_str().unwrap());
        let client = match ControlConnection::connect_keeper(&socket, &own, soon()) {
            Ok(client) => client,
            Err(error) if error.raw_os_error() == Some(libc::ENOPROTOOPT) => {
                skip_or_fail(
                    TEST,
                    &format!("SO_PEERPIDFD needs Linux 6.5 or newer ({error})"),
                );
                return;
            }
            Err(error) => panic!("the keeper was not accepted: {error}"),
        };
        let error = ControlConnection::connect_keeper(&socket, "/other", soon()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let error = ControlConnection::connect_keeper(&socket, &format!("{own}/session"), soon())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let keeper = listener.accept().unwrap().expect("a pending connection");
        client.send_client(&ClientFrame::Begin, soon()).unwrap();
        assert!(matches!(
            keeper.receive_client(soon()).unwrap(),
            ClientFrame::Begin
        ));
    }

    #[test]
    fn socket_is_owner_only_and_frames_cross_it_both_ways() {
        let scratch = Scratch::new();
        let listener = ControlListener::bind(&scratch.directory()).unwrap();
        let socket = scratch.0.join(CONTROL_SOCKET.to_str().unwrap());
        let mode = std::fs::metadata(&socket).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        assert!(listener.accept().unwrap().is_none());
        let client = ControlConnection::connect_unauthenticated(&socket, soon()).unwrap();
        let keeper = listener.accept().unwrap().expect("a pending connection");
        client.send_client(&ClientFrame::Begin, soon()).unwrap();
        assert!(matches!(
            keeper.receive_client(soon()).unwrap(),
            ClientFrame::Begin
        ));
        keeper
            .send_keeper(KeeperFrame::SecretRequest, soon())
            .unwrap();
        assert_eq!(
            client.receive_keeper(soon()).unwrap(),
            KeeperFrame::SecretRequest
        );
        client
            .send_client(&ClientFrame::Secret(secret()), soon())
            .unwrap();
        match keeper.receive_client(soon()).unwrap() {
            ClientFrame::Secret(value) => assert_eq!(*value, *secret()),
            _ => panic!("expected a secret"),
        }
    }

    #[test]
    fn receiving_honours_the_deadline_and_refuses_oversized_datagrams() {
        let scratch = Scratch::new();
        let listener = ControlListener::bind(&scratch.directory()).unwrap();
        let socket = scratch.0.join("control.sock");
        let client = ControlConnection::connect_unauthenticated(&socket, soon()).unwrap();
        let keeper = listener.accept().unwrap().unwrap();
        let error = keeper
            .receive_client(Instant::now() + Duration::from_millis(30))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let oversized = [0u8; 200];
        // SAFETY: the buffer is readable for its whole length.
        let sent = unsafe {
            libc::send(
                client.as_raw_fd(),
                oversized.as_ptr().cast(),
                oversized.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        assert_eq!(sent, 200);
        assert_eq!(
            keeper.receive_client(soon()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        drop(client);
        assert_eq!(
            keeper.receive_client(soon()).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
