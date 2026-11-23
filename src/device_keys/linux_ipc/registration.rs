use super::super::linux_admission_storage::PublishError;
use super::keeper_codec::{
    ActiveClosureConfirmationFrame, ActiveClosureFrame, ChallengeFrame, ConfirmationFrame,
    OfferFrame,
};
use super::{
    deadline_check, denied, invalid, require_cloexec, socket_option, ExpectedPeer, SpawnOwner,
    SCM_PIDFD,
};
use super::{packet_pair, stable_ids};
use std::io;
use std::marker::PhantomData;
use std::mem::{size_of, size_of_val, zeroed, ManuallyDrop};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

use super::super::linux_current_admission::{
    ActiveClosureBinding, AdmittedChildRegistration, CurrentAdmission, KeeperLaunchExpectation,
    OpeningAuthority, RetainedKeeperDirectory, KEEPER_BOOTSTRAP_SIZE,
};
use super::super::linux_session::NativeSession;
use super::super::linux_spawn::{
    new_session_generation, retained_process_exited, running_executable, OwnedChildRegistration,
};

#[path = "pidfd_identity.rs"]
mod pidfd_identity;
#[allow(
    unused_imports,
    reason = "Only the session control protocol reads a pid from a process reference."
)]
pub use pidfd_identity::live_pid;
use pidfd_identity::validate_process_reference;
pub use pidfd_identity::{verify_live_process, verify_live_process_pair};

const WIRE_SIZE: usize = 48;
const TAG: &[u8; 8] = b"vv-proc\0";
const CHALLENGE: u8 = 1;
const RESPONSE: u8 = 2;
const ACK: u8 = 3;
const PROBE_PTY_FRAME: &[u8; 1] = b"F";
const SUPERVISOR_START: &[u8; 16] = b"VVKSTART\x01\0\0\0\0\0\0\0";
const SUPERVISOR_READY: &[u8; 16] = b"VVKREADY\x01\0\0\0\0\0\0\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointStartupRole {
    Supervisor,
    Verifier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointSessionStep {
    Prepared,
    Start,
    Register,
    Unlocked,
    Prove,
    Proved,
    Close,
    Closed,
    Activate,
    Active,
    /// The supervisor served one proof; the keeper's idle limit counts from it.
    Served,
    /// Opens the session again without a permit, on the keeper's evidence.
    Reopen,
}

impl EndpointSessionStep {
    const ALL: [Self; 12] = [
        Self::Prepared,
        Self::Start,
        Self::Register,
        Self::Unlocked,
        Self::Prove,
        Self::Proved,
        Self::Close,
        Self::Closed,
        Self::Activate,
        Self::Active,
        Self::Served,
        Self::Reopen,
    ];

    fn sent_by_keeper(self) -> bool {
        matches!(
            self,
            Self::Start | Self::Prove | Self::Close | Self::Activate | Self::Reopen
        )
    }
}

fn endpoint_session_frame(step: EndpointSessionStep, keeper: bool) -> io::Result<[u8; 16]> {
    if step.sent_by_keeper() != keeper {
        return Err(denied());
    }
    let mut wire = *b"VVKEPSS\0\x01\0\0\0\0\0\0\0";
    wire[9] = if keeper { 1 } else { 2 };
    wire[10] = match step {
        EndpointSessionStep::Prepared => 1,
        EndpointSessionStep::Start => 2,
        EndpointSessionStep::Register => 3,
        EndpointSessionStep::Unlocked => 4,
        EndpointSessionStep::Prove => 5,
        EndpointSessionStep::Proved => 6,
        EndpointSessionStep::Close => 7,
        EndpointSessionStep::Closed => 8,
        EndpointSessionStep::Activate => 9,
        EndpointSessionStep::Active => 10,
        EndpointSessionStep::Served => 11,
        EndpointSessionStep::Reopen => 12,
    };
    Ok(wire)
}

fn decode_session_child_step(wire: &[u8]) -> io::Result<EndpointSessionStep> {
    for step in EndpointSessionStep::ALL {
        if !step.sent_by_keeper() && wire == endpoint_session_frame(step, false)? {
            return Ok(step);
        }
    }
    Err(invalid())
}

fn decode_session_keeper_opening(wire: &[u8]) -> io::Result<OpeningAuthority> {
    for (step, authority) in [
        (EndpointSessionStep::Start, OpeningAuthority::Permit),
        (EndpointSessionStep::Reopen, OpeningAuthority::Reopened),
    ] {
        if wire == endpoint_session_frame(step, true)? {
            return Ok(authority);
        }
    }
    Err(invalid())
}

const SESSION_SECRET_HEADER: &[u8; 16] = b"VVKEPSU\0\x01\0\0\0\0\0\0\0";
const SESSION_SECRET_LENGTH: usize = 64;
const SESSION_SECRET_FRAME: usize = SESSION_SECRET_HEADER.len() + SESSION_SECRET_LENGTH;

fn encode_session_secret(
    secret: &[u8; SESSION_SECRET_LENGTH],
) -> Zeroizing<[u8; SESSION_SECRET_FRAME]> {
    let mut wire = Zeroizing::new([0; SESSION_SECRET_FRAME]);
    wire[..SESSION_SECRET_HEADER.len()].copy_from_slice(SESSION_SECRET_HEADER);
    wire[SESSION_SECRET_HEADER.len()..].copy_from_slice(secret);
    wire
}

fn decode_session_secret(wire: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
    if wire.len() != SESSION_SECRET_FRAME
        || wire[..SESSION_SECRET_HEADER.len()] != *SESSION_SECRET_HEADER
    {
        return Err(invalid());
    }
    Ok(Zeroizing::new(wire[SESSION_SECRET_HEADER.len()..].to_vec()))
}

fn validate_endpoint_session_frame(
    wire: &[u8],
    step: EndpointSessionStep,
    keeper: bool,
) -> io::Result<()> {
    if wire != endpoint_session_frame(step, keeper)? {
        return Err(invalid());
    }
    Ok(())
}

fn endpoint_start_frame(role: EndpointStartupRole) -> [u8; 16] {
    let mut wire = *b"VVKEPST\0\x01\0\0\0\0\0\0\0";
    wire[9] = match role {
        EndpointStartupRole::Supervisor => 1,
        EndpointStartupRole::Verifier => 2,
    };
    wire
}

fn validate_endpoint_start_frame(wire: &[u8], role: EndpointStartupRole) -> io::Result<()> {
    if wire != endpoint_start_frame(role) {
        return Err(invalid());
    }
    Ok(())
}

const CHILD_PIDFD: u8 = 1;

pub(crate) struct ChildPidfdChannel {
    parent: OwnedFd,
    child: OwnedFd,
    uid: u32,
    gid: u32,
}

impl ChildPidfdChannel {
    pub(crate) fn new() -> io::Result<Self> {
        let (uid, gid) = stable_ids()?;
        let (parent, child) = packet_pair()?;
        Ok(Self {
            parent: above_stdio(parent)?,
            child: above_stdio(child)?,
            uid,
            gid,
        })
    }

    pub(crate) fn child_socket(&self) -> libc::c_int {
        self.child.as_raw_fd()
    }

    pub(crate) fn receive(self, pid: u32) -> io::Result<OwnedFd> {
        let expected = libc::ucred {
            pid: i32::try_from(pid).map_err(|_| invalid())?,
            uid: self.uid,
            gid: self.gid,
        };
        drop(self.child);
        let mut wire = [0u8; 1];
        let mut control = [0usize; 32];
        let mut iov = libc::iovec {
            iov_base: wire.as_mut_ptr().cast(),
            iov_len: wire.len(),
        };
        // SAFETY: zero initializes unused fields and the assigned buffers are writable.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = size_of_val(&control);
        // SAFETY: the buffers are live and sized as specified by message.
        let count = unsafe {
            libc::recvmsg(
                self.parent.as_raw_fd(),
                &mut message,
                libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC | libc::MSG_TRUNC,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let references = parse_control_credentials(&control, message.msg_controllen, &expected, 1)?;
        if count != 1 || wire != [CHILD_PIDFD] || message.msg_flags & !libc::MSG_CMSG_CLOEXEC != 0 {
            return Err(invalid());
        }
        let [pidfd]: [OwnedFd; 1] = references.try_into().map_err(|_| invalid())?;
        above_stdio(pidfd)
    }
}

fn above_stdio(fd: OwnedFd) -> io::Result<OwnedFd> {
    if fd.as_raw_fd() >= 3 {
        return Ok(fd);
    }
    // SAFETY: fcntl allocates a fresh close-on-exec descriptor above stdio.
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fcntl transfers exclusive ownership of the duplicate.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

/// Called in the child before exec. Uses only fixed buffers and native calls.
pub(crate) fn send_own_pidfd(socket: libc::c_int) -> io::Result<()> {
    // SAFETY: getpid and pidfd_open take no pointer arguments.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0u32) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pidfd_open returned a fresh descriptor that stays close-on-exec.
    let pidfd = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
    let mut wire = [CHILD_PIDFD];
    let mut control = [0usize; 8];
    let mut iov = libc::iovec {
        iov_base: wire.as_mut_ptr().cast(),
        iov_len: wire.len(),
    };
    // SAFETY: zero initializes unused fields and assigned buffers stay live.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    let size = size_of::<libc::c_int>() as libc::c_uint;
    // SAFETY: control is aligned and large enough for one descriptor word.
    let count = unsafe {
        message.msg_controllen = libc::CMSG_SPACE(size) as usize;
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size) as usize;
        libc::CMSG_DATA(header)
            .cast::<libc::c_int>()
            .write_unaligned(pidfd.as_raw_fd());
        libc::sendmsg(socket, &message, libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL)
    };
    match count {
        1 => Ok(()),
        -1 => Err(io::Error::last_os_error()),
        _ => Err(invalid()),
    }
}

struct Origin {
    process: libc::pid_t,
    thread: libc::pid_t,
    _local: PhantomData<Rc<()>>,
}

impl Origin {
    fn current() -> Self {
        Self {
            // SAFETY: these calls only query the current process and thread.
            process: unsafe { libc::getpid() },
            thread: unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t },
            _local: PhantomData,
        }
    }

    fn validate(&self) -> io::Result<()> {
        // SAFETY: these calls only query the current process and thread.
        if unsafe { libc::getpid() } != self.process
            || unsafe { libc::syscall(libc::SYS_gettid) } != libc::c_long::from(self.thread)
        {
            return Err(denied());
        }
        Ok(())
    }
}

struct Transport {
    socket: OwnedFd,
    socket_identity: (libc::dev_t, libc::ino_t),
    peer: ExpectedPeer,
    deadline: Instant,
    origin: Origin,
    attempted: bool,
}

impl Transport {
    fn new(socket: OwnedFd, peer: ExpectedPeer, deadline: Instant) -> io::Result<Self> {
        Self::with_budget(socket, peer, deadline, Duration::from_secs(3))
    }

    fn with_budget(
        socket: OwnedFd,
        peer: ExpectedPeer,
        deadline: Instant,
        budget: Duration,
    ) -> io::Result<Self> {
        let deadline = deadline.min(Instant::now() + budget);
        deadline_check(deadline)?;
        require_cloexec(socket.as_raw_fd())?;
        if socket_option(socket.as_raw_fd(), libc::SO_DOMAIN)? != libc::AF_UNIX
            || socket_option(socket.as_raw_fd(), libc::SO_TYPE)? != libc::SOCK_SEQPACKET
            || socket_option(socket.as_raw_fd(), libc::SO_PASSCRED)? != 1
        {
            return Err(invalid());
        }
        let socket_identity = socket_identity(socket.as_raw_fd())?;
        let result = Self {
            socket,
            socket_identity,
            peer,
            deadline,
            origin: Origin::current(),
            attempted: false,
        };
        result.check()?;
        Ok(result)
    }

    fn begin(&mut self) -> io::Result<()> {
        if self.attempted {
            return Err(invalid());
        }
        self.attempted = true;
        self.check()
    }

    fn check(&self) -> io::Result<()> {
        self.check_live()?;
        deadline_check(self.deadline)
    }

    fn check_live(&self) -> io::Result<()> {
        self.origin.validate()?;
        require_cloexec(self.socket.as_raw_fd())?;
        if socket_identity(self.socket.as_raw_fd())? != self.socket_identity
            || socket_option(self.socket.as_raw_fd(), libc::SO_DOMAIN)? != libc::AF_UNIX
            || socket_option(self.socket.as_raw_fd(), libc::SO_TYPE)? != libc::SOCK_SEQPACKET
            || socket_option(self.socket.as_raw_fd(), libc::SO_PASSCRED)? != 1
        {
            return Err(invalid());
        }
        self.peer.alive()?;
        let mut descriptor = libc::pollfd {
            fd: self.socket.as_raw_fd(),
            events: 0,
            revents: 0,
        };
        // SAFETY: descriptor is one initialized poll descriptor.
        if unsafe { libc::poll(&mut descriptor, 1, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if descriptor.revents != 0 {
            return Err(denied());
        }
        self.origin.validate()?;
        self.peer.alive()
    }

    fn renew(&mut self, deadline: Instant) -> io::Result<()> {
        self.check_live()?;
        self.deadline = deadline.min(Instant::now() + Duration::from_secs(3));
        self.attempted = false;
        deadline_check(self.deadline)
    }

    fn close_write(&self) -> io::Result<()> {
        self.check()?;
        // SAFETY: the retained socket is live and SHUT_WR closes only its write half.
        if unsafe { libc::shutdown(self.socket.as_raw_fd(), libc::SHUT_WR) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wait_write_closed(&self) -> io::Result<()> {
        self.wait(libc::POLLRDHUP)?;
        let mut byte = 0u8;
        let mut iov = libc::iovec {
            iov_base: (&mut byte as *mut u8).cast(),
            iov_len: 1,
        };
        // SAFETY: zero initializes the header and disables ancillary data delivery.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        // SAFETY: the message and its one-byte buffer are writable. With no control
        // buffer, any packet carrying credentials or descriptors is truncated.
        let count = unsafe {
            libc::recvmsg(
                self.socket.as_raw_fd(),
                &mut message,
                libc::MSG_DONTWAIT | libc::MSG_TRUNC,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count != 0 || message.msg_flags != 0 {
            return Err(invalid());
        }
        self.check()
    }

    fn wait(&self, event: libc::c_short) -> io::Result<()> {
        loop {
            self.check()?;
            let millis = self
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let mut descriptors = [
                libc::pollfd {
                    fd: self.socket.as_raw_fd(),
                    events: event,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.peer.pidfd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: descriptors contains two initialized poll descriptors.
            let count = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, millis) };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            self.check()?;
            if descriptors[1].revents != 0 || descriptors[0].revents & !event != 0 {
                return Err(denied());
            }
            if descriptors[0].revents & event != 0 {
                return Ok(());
            }
        }
    }

    fn send(&self, kind: u8, nonce: &[u8; 32], references: &[OwnedFd]) -> io::Result<()> {
        if !matches!(references.len(), 0 | 2) {
            return Err(invalid());
        }
        self.send_frame(&encode(kind, nonce), references)
    }

    fn send_frame(&self, wire: &[u8], references: &[OwnedFd]) -> io::Result<()> {
        let mut control = [0usize; 8];
        let mut iov = libc::iovec {
            iov_base: wire.as_ptr().cast_mut().cast(),
            iov_len: wire.len(),
        };
        // SAFETY: zero is a valid initial msghdr; assigned buffers remain live below.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        if !references.is_empty() {
            if !(matches!(references.len(), 2 | 3)
                || wire == PROBE_PTY_FRAME && references.len() == 1)
            {
                return Err(invalid());
            }
            let size = size_of::<libc::c_int>() * references.len();
            // SAFETY: size is bounded to three descriptor words.
            message.msg_controllen = unsafe { libc::CMSG_SPACE(size as _) } as usize;
            if message.msg_controllen > size_of_val(&control) {
                return Err(invalid());
            }
            message.msg_control = control.as_mut_ptr().cast();
            // SAFETY: control is aligned and holds the complete rights record.
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(size as _) as usize;
                let data = libc::CMSG_DATA(header).cast::<libc::c_int>();
                for (index, reference) in references.iter().enumerate() {
                    data.add(index).write_unaligned(reference.as_raw_fd());
                }
            }
        }
        loop {
            self.check()?;
            // SAFETY: all message pointers reference initialized readable storage.
            let count = unsafe {
                libc::sendmsg(
                    self.socket.as_raw_fd(),
                    &message,
                    libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock => {
                        self.wait(libc::POLLOUT)?;
                        continue;
                    }
                    _ => return Err(error),
                }
            }
            self.check()?;
            if count as usize != wire.len() {
                return Err(invalid());
            }
            return Ok(());
        }
    }

    fn receive(
        &self,
        kind: u8,
        nonce: Option<&[u8; 32]>,
        expected: usize,
    ) -> io::Result<([u8; 32], Vec<OwnedFd>)> {
        let (wire, references) = self.receive_frame::<WIRE_SIZE>(expected)?;
        let received = decode(&wire, kind, nonce)?;
        Ok((received, references))
    }

    fn receive_frame<const N: usize>(
        &self,
        expected: usize,
    ) -> io::Result<([u8; N], Vec<OwnedFd>)> {
        let mut wire = [0u8; N];
        let references = self.receive_into(&mut wire, expected)?;
        Ok((wire, references))
    }

    /// Receives the secret into zeroizing storage only, never a plain copy.
    fn receive_session_secret(&self) -> io::Result<Zeroizing<Vec<u8>>> {
        let mut wire = Zeroizing::new([0u8; SESSION_SECRET_FRAME]);
        self.receive_into(&mut wire[..], 0)?;
        decode_session_secret(&wire[..])
    }

    /// Fills `wire` with exactly one packet of exactly its length.
    fn receive_into(&self, wire: &mut [u8], expected: usize) -> io::Result<Vec<OwnedFd>> {
        loop {
            self.check()?;
            let mut control = [0usize; 32];
            let mut iov = libc::iovec {
                iov_base: wire.as_mut_ptr().cast(),
                iov_len: wire.len(),
            };
            // SAFETY: zero is a valid initial msghdr; all writable buffers are assigned.
            let mut message: libc::msghdr = unsafe { zeroed() };
            message.msg_iov = &mut iov;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = size_of_val(&control);
            // SAFETY: the message buffers are writable for their supplied bounded lengths.
            let count = unsafe {
                libc::recvmsg(
                    self.socket.as_raw_fd(),
                    &mut message,
                    libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC | libc::MSG_TRUNC,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock => {
                        self.wait(libc::POLLIN)?;
                        continue;
                    }
                    _ => return Err(error),
                }
            }
            // Adopt or close every visible descriptor before checking any other failure.
            let references = parse_control(&control, message.msg_controllen, &self.peer, expected);
            self.check()?;
            let references = references?;
            if message.msg_flags & !libc::MSG_CMSG_CLOEXEC != 0 || count as usize != wire.len() {
                return Err(invalid());
            }
            return Ok(references);
        }
    }
}

fn socket_identity(raw: i32) -> io::Result<(libc::dev_t, libc::ino_t)> {
    // SAFETY: zero initializes writable stat storage before fstat fills it.
    let mut status: libc::stat = unsafe { zeroed() };
    // SAFETY: fstat writes only this initialized stat storage.
    if unsafe { libc::fstat(raw, &mut status) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if status.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return Err(invalid());
    }
    Ok((status.st_dev, status.st_ino))
}

enum EndpointPhase {
    Waiting,
    Ready([u8; 32]),
    Spent,
}

/// One challenge and one proof on the same authenticated local connection.
pub struct EndpointProofConnection {
    transport: Transport,
    phase: EndpointPhase,
}

impl EndpointProofConnection {
    pub fn new(socket: OwnedFd, deadline: Instant) -> io::Result<Self> {
        let peer = ExpectedPeer::connected_identity(&socket)?;
        Ok(Self {
            transport: Transport::with_budget(socket, peer, deadline, Duration::from_secs(30))?,
            phase: EndpointPhase::Waiting,
        })
    }

    /// Binds the connected peer to a retained reference from the local launch context.
    pub fn for_expected_peer(
        socket: OwnedFd,
        expected: &OwnedFd,
        deadline: Instant,
    ) -> io::Result<Self> {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(30));
        deadline_check(deadline)?;
        let peer = ExpectedPeer::connected_identity(&socket)?;
        verify_live_process(expected, &peer.pidfd)?;
        deadline_check(deadline)?;
        let connection = Self {
            transport: Transport::with_budget(socket, peer, deadline, Duration::from_secs(30))?,
            phase: EndpointPhase::Waiting,
        };
        connection.transport.check()?;
        Ok(connection)
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.transport.deadline
    }

    /// A completed exchange retains its connection for the verifier's final checks.
    pub fn response_peer_is_live(&self) -> bool {
        self.transport.check().is_ok()
    }

    pub(crate) fn request_proof(
        &mut self,
    ) -> io::Result<(
        [u8; 32],
        super::super::linux_endpoint_codec::ProofFrame,
        super::ConnectedPeerContext,
    )> {
        use super::super::linux_endpoint_codec::{ChallengeFrame, ProofFrame};

        if !matches!(
            std::mem::replace(&mut self.phase, EndpointPhase::Spent),
            EndpointPhase::Waiting
        ) {
            self.refuse();
            return Err(invalid());
        }
        let result = (|| {
            self.transport.check()?;
            let mut context = super::ConnectedPeerContext::capture(&self.transport.socket)?;
            context.validate()?;
            let challenge = fresh_nonce(self.transport.deadline)?;
            context.validate()?;
            self.transport
                .send_frame(&ChallengeFrame { nonce: challenge }.encode()?, &[])?;
            let (wire, _references) = self.transport.receive_frame::<176>(0)?;
            let proof = ProofFrame::decode(&wire)?;
            if proof.echo != challenge {
                return Err(invalid());
            }
            context.validate()?;
            self.transport.check()?;
            Ok((challenge, proof, context))
        })();
        if result.is_err() {
            self.refuse();
        }
        result
    }

    pub(crate) fn receive_challenge(
        &mut self,
    ) -> io::Result<([u8; 48], super::ConnectedPeerContext)> {
        if !matches!(
            std::mem::replace(&mut self.phase, EndpointPhase::Spent),
            EndpointPhase::Waiting
        ) {
            self.refuse();
            return Err(invalid());
        }
        let result = (|| {
            let (wire, _references) = self.transport.receive_frame::<48>(0)?;
            let challenge = super::super::linux_endpoint_codec::ChallengeFrame::decode(&wire)?;
            let context = super::ConnectedPeerContext::capture(&self.transport.socket)?;
            self.transport.check()?;
            Ok((wire, context, challenge.nonce))
        })();
        match result {
            Ok((wire, context, nonce)) => {
                self.phase = EndpointPhase::Ready(nonce);
                Ok((wire, context))
            }
            Err(error) => {
                self.refuse();
                Err(error)
            }
        }
    }

    pub(crate) fn send_proof(&mut self, wire: &[u8; 176]) -> io::Result<()> {
        let EndpointPhase::Ready(challenge) =
            std::mem::replace(&mut self.phase, EndpointPhase::Spent)
        else {
            self.refuse();
            return Err(invalid());
        };
        let result = (|| {
            let frame = super::super::linux_endpoint_codec::ProofFrame::decode(wire)?;
            if frame.echo != challenge {
                return Err(invalid());
            }
            self.transport.check()?;
            // SAFETY: the fixed frame remains readable for this single
            // nonblocking packet send. No retry can duplicate delivery.
            let count = unsafe {
                libc::send(
                    self.transport.socket.as_raw_fd(),
                    wire.as_ptr().cast(),
                    wire.len(),
                    libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                )
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            if count as usize != wire.len() {
                return Err(invalid());
            }
            self.transport.check()
        })();
        if result.is_err() {
            self.refuse();
        }
        result
    }

    pub(crate) fn refuse(&mut self) {
        self.phase = EndpointPhase::Spent;
        // SAFETY: shutdown affects only the connection owned by this attempt.
        // Refusal remains final even when the peer has already disconnected.
        unsafe { libc::shutdown(self.transport.socket.as_raw_fd(), libc::SHUT_RDWR) };
    }
}

/// Exercises the endpoint transport with disposable frame data.
pub fn probe_endpoint_transport(
    socket: OwnedFd,
    responder: bool,
    deadline: Instant,
) -> io::Result<()> {
    use super::super::linux_endpoint_codec::{ChallengeFrame, ProofFrame};

    let mut connection = EndpointProofConnection::new(socket, deadline)?;
    if responder {
        let (wire, mut context) = connection.receive_challenge()?;
        let challenge = ChallengeFrame::decode(&wire)?;
        let proof = ProofFrame {
            echo: challenge.nonce,
            generation: [2; 32],
            connection_nonce: [3; 32],
            signature: [0; 64],
        }
        .encode()?;
        context.validate()?;
        connection.send_proof(&proof)?;
    } else {
        let (challenge, proof, mut context) = connection.request_proof()?;
        context.validate()?;
        if proof.echo != challenge
            || proof.generation != [2; 32]
            || proof.connection_nonce != [3; 32]
            || proof.signature != [0; 64]
        {
            return Err(invalid());
        }
    }
    Ok(())
}

pub struct KeeperChallengeSender {
    transport: Transport,
}

pub struct KeeperChallengeReceiver {
    transport: Transport,
}

pub struct KeeperBootstrapSender<'registration, 'preparation> {
    transport: Transport,
    registration: &'registration AdmittedChildRegistration<'preparation>,
}

pub struct KeeperBootstrapReceiver {
    transport: Transport,
    expectation: KeeperLaunchExpectation,
}

pub struct KeeperOfferSent<'registration, 'preparation> {
    transport: Transport,
    registration: &'registration AdmittedChildRegistration<'preparation>,
    offer: OfferFrame,
}

/// A confirmation of the exact offer while the registering supervisor is live.
pub struct KeeperConfirmedOffer<'registration, 'preparation>(
    KeeperOfferSent<'registration, 'preparation>,
);

/// A one-use handoff for the closure of an active, signed generation.
pub struct KeeperActiveClosureSender {
    transport: Transport,
    binding: ActiveClosureBinding,
    offer: OfferFrame,
}

/// Keeps the authenticated channel open until the keeper acknowledges receipt.
pub struct KeeperActiveClosureSent {
    transport: Transport,
    offer: OfferFrame,
    generation: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActiveClosureFault {
    Incomplete,
    DifferentGeneration,
}

/// Holds the validated offer pending a separate keeper adoption transition.
pub struct KeeperValidatedOffer {
    transport: Transport,
    resources: KeeperOfferResources,
    offer: OfferFrame,
}

struct KeeperOfferResources {
    directory: RetainedKeeperDirectory,
    references: [OwnedFd; 2],
    restricted_generation: Option<[u8; 32]>,
    active_generation: Option<[u8; 32]>,
    opening_authority: Option<OpeningAuthority>,
}

struct KeeperAdoptedResources {
    offer: KeeperOfferResources,
    supervisor: OwnedFd,
}

/// Each dedicated keeper process retains at most one adopted set until exit.
pub struct KeeperAdoptedOffer {
    transport: Transport,
    resources: ManuallyDrop<KeeperAdoptedResources>,
    offer: OfferFrame,
    confirmation: ConfirmationState,
    exit_observation_failed: bool,
    provider_collection: ProviderCollection,
    retirement: GenerationRetirement,
    active_closure: ActiveClosureState,
    active_closure_confirmation: ConfirmationState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmationState {
    Pending,
    Failed,
    Sent,
}

#[derive(PartialEq, Eq)]
enum ProviderCollection {
    Pending,
    Failed,
    Collected,
}

enum GenerationRetirement {
    Pending,
    Consumed,
    Advanced([u8; 32]),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveClosureState {
    Pending,
    Failed,
    Received,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SuccessorAuthority {
    PreUnlock,
    ActiveClosure,
}

/// A successor identity held under the same keeper reservation in the
/// disposable pre-unlock experiment.
pub struct KeeperPreUnlockSuccessor<'keeper> {
    keeper: &'keeper mut KeeperAdoptedOffer,
    previous: [u8; 32],
    current: [u8; 32],
    authority: SuccessorAuthority,
}

impl KeeperPreUnlockSuccessor<'_> {
    pub fn generations(&self) -> ([u8; 32], [u8; 32]) {
        (self.previous, self.current)
    }

    pub fn validate_generation(&self, generation: &[u8; 32]) -> io::Result<()> {
        self.keeper.transport.origin.validate()?;
        self.keeper.resources.offer.directory.validate()?;
        if *generation != self.current
            || !matches!(self.keeper.retirement, GenerationRetirement::Advanced(current) if current == *generation)
        {
            return Err(invalid());
        }
        self.keeper.transport.origin.validate()
    }

    pub(crate) fn probe_launch_restricted_successor(
        self,
        home: &Path,
        observe: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.validate_generation(&self.current)?;
        self.keeper.retirement = GenerationRetirement::Consumed;
        let result = self
            .keeper
            .resources
            .offer
            .directory
            .probe_launch_restricted_successor(self.current, home, observe);
        self.keeper.transport.origin.validate()?;
        self.keeper.resources.offer.directory.validate()?;
        result
    }

    pub(crate) fn probe_unlock_attended_successor(
        self,
        home: &Path,
    ) -> io::Result<(NativeSession, crate::ubuntu_provider::UbuntuProvider)> {
        self.validate_generation(&self.current)?;
        self.keeper.retirement = GenerationRetirement::Consumed;
        let authorized = match self.authority {
            SuccessorAuthority::PreUnlock => {
                self.keeper.provider_collection == ProviderCollection::Collected
                    && !self.keeper.exit_observation_failed
            }
            SuccessorAuthority::ActiveClosure => {
                self.keeper.active_closure == ActiveClosureState::Received
                    && !self.keeper.exit_observation_failed
            }
        };
        if !authorized {
            return Err(denied());
        }
        let directory = &self.keeper.resources.offer.directory;
        let preparation = directory.prepare_restricted_successor(self.current)?;
        preparation.probe_unlock_attended_openssh(home, || {
            self.keeper.transport.origin.validate()?;
            directory.validate()?;
            self.keeper.transport.origin.validate()
        })
    }
}

static KEEPER_ADOPTED: AtomicBool = AtomicBool::new(false);

pub struct KeeperOfferReceiver {
    transport: Transport,
    challenge: ChallengeFrame,
}

pub struct KeeperOfferSender {
    transport: Transport,
    challenge: ChallengeFrame,
}

/// An authenticated packet with candidate objects awaiting local validation.
pub struct ReceivedKeeperOffer {
    _offer: OfferFrame,
    _references: [OwnedFd; 3],
    _origin: Origin,
}

fn require_same_running_image(peer: &ExpectedPeer) -> io::Result<()> {
    peer.alive()?;
    let local = running_executable(std::process::id())?;
    let remote = running_executable(u32::try_from(peer.pid).map_err(|_| denied())?)?;
    let local_metadata = local.metadata()?;
    let remote_metadata = remote.metadata()?;
    if (local_metadata.dev(), local_metadata.ino())
        != (remote_metadata.dev(), remote_metadata.ino())
    {
        return Err(denied());
    }
    peer.alive()
}

fn require_supervisor_parent(peer: &ExpectedPeer) -> io::Result<()> {
    // SAFETY: getppid queries the live caller's direct parent.
    if unsafe { libc::getppid() } != peer.pid {
        return Err(denied());
    }
    require_same_running_image(peer)
}

/// Authorizes the retained child to begin the supervisor's local admission.
pub fn authorize_supervisor_start(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    deadline: Instant,
) -> io::Result<()> {
    let peer = ExpectedPeer::owned_child(owner, token)?;
    let mut transport = Transport::new(socket.try_clone()?, peer, deadline)?;
    transport.begin()?;
    require_same_running_image(&transport.peer)?;
    transport.send_frame(SUPERVISOR_START, &[])
}

/// Authenticates the direct keeper parent and its start frame before admission.
pub fn receive_supervisor_start(socket: &OwnedFd, deadline: Instant) -> io::Result<()> {
    let peer = ExpectedPeer::connected_identity(socket)?;
    let mut transport = Transport::new(socket.try_clone()?, peer, deadline)?;
    transport.begin()?;
    require_supervisor_parent(&transport.peer)?;
    let (wire, _) = transport.receive_frame::<{ SUPERVISOR_START.len() }>(0)?;
    if &wire != SUPERVISOR_START {
        return Err(invalid());
    }
    require_supervisor_parent(&transport.peer)?;
    transport.check()
}

/// Transfers a sibling's retained process reference and the private runtime directory.
pub fn authorize_endpoint_start(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    target: u64,
    expected_peer: u64,
    runtime: &OwnedFd,
    role: EndpointStartupRole,
    deadline: Instant,
) -> io::Result<()> {
    if target == expected_peer {
        return Err(denied());
    }
    let peer = ExpectedPeer::owned_child(owner, target)?;
    let expected = ExpectedPeer::owned_child(owner, expected_peer)?;
    let references = [expected.pidfd, runtime.try_clone()?];
    validate_process_reference(&references[0])?;
    require_cloexec(references[1].as_raw_fd())?;
    let mut transport = Transport::new(socket.try_clone()?, peer, deadline)?;
    transport.begin()?;
    require_same_running_image(&transport.peer)?;
    transport.send_frame(&endpoint_start_frame(role), &references)?;
    validate_process_reference(&references[0])?;
    transport.check()
}

/// Receives the role-bound peer reference from the authenticated direct keeper.
pub fn receive_endpoint_start(
    socket: &OwnedFd,
    role: EndpointStartupRole,
    deadline: Instant,
) -> io::Result<[OwnedFd; 2]> {
    let peer = ExpectedPeer::connected_identity(socket)?;
    let mut transport = Transport::new(socket.try_clone()?, peer, deadline)?;
    transport.begin()?;
    require_supervisor_parent(&transport.peer)?;
    let (wire, references) = transport.receive_frame::<16>(2)?;
    validate_endpoint_start_frame(&wire, role)?;
    let references: [OwnedFd; 2] = references.try_into().map_err(|_| invalid())?;
    validate_process_reference(&references[0])?;
    require_cloexec(references[1].as_raw_fd())?;
    require_supervisor_parent(&transport.peer)?;
    transport.check()?;
    Ok(references)
}

/// Receives a child's startup confirmation while preserving the keeper's write side.
pub fn await_endpoint_ready(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    target: u64,
    deadline: Instant,
) -> io::Result<()> {
    let peer = ExpectedPeer::owned_child(owner, target)?;
    let mut transport = Transport::new(socket.try_clone()?, peer, deadline)?;
    transport.begin()?;
    let (wire, _) = transport.receive_frame::<{ SUPERVISOR_READY.len() }>(0)?;
    if &wire != SUPERVISOR_READY {
        return Err(invalid());
    }
    require_same_running_image(&transport.peer)?;
    transport.check()
}

/// Finishes one child's authenticated startup after both children have confirmed.
pub fn finish_endpoint_start(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    target: u64,
    deadline: Instant,
) -> io::Result<()> {
    let peer = ExpectedPeer::owned_child(owner, target)?;
    let mut transport = Transport::new(socket.try_clone()?, peer, deadline)?;
    transport.begin()?;
    require_same_running_image(&transport.peer)?;
    transport.close_write()
}

/// Waits for the retained child to confirm startup, then closes the request stream.
pub fn await_supervisor_ready(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    deadline: Instant,
) -> io::Result<()> {
    await_endpoint_ready(socket, owner, token, deadline)?;
    finish_endpoint_start(socket, owner, token, deadline)
}

/// Confirms authenticated startup and waits for the live keeper to close its request stream.
pub fn acknowledge_supervisor_ready(socket: &OwnedFd, deadline: Instant) -> io::Result<()> {
    let peer = ExpectedPeer::connected_identity(socket)?;
    let mut transport = Transport::new(socket.try_clone()?, peer, deadline)?;
    transport.begin()?;
    require_supervisor_parent(&transport.peer)?;
    transport.send_frame(SUPERVISOR_READY, &[])?;
    transport.wait_write_closed()?;
    require_supervisor_parent(&transport.peer)?;
    transport.check()
}

fn session_parent_transport(socket: &OwnedFd, deadline: Instant) -> io::Result<Transport> {
    let peer = ExpectedPeer::connected_identity(socket)?;
    let mut transport =
        Transport::with_budget(socket.try_clone()?, peer, deadline, Duration::from_secs(60))?;
    transport.begin()?;
    require_supervisor_parent(&transport.peer)?;
    Ok(transport)
}

fn session_owned_transport(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    deadline: Instant,
) -> io::Result<Transport> {
    let peer = ExpectedPeer::owned_child(owner, token)?;
    let mut transport =
        Transport::with_budget(socket.try_clone()?, peer, deadline, Duration::from_secs(60))?;
    transport.begin()?;
    require_same_running_image(&transport.peer)?;
    Ok(transport)
}

pub fn send_session_child_step(
    socket: &OwnedFd,
    step: EndpointSessionStep,
    deadline: Instant,
) -> io::Result<()> {
    let wire = endpoint_session_frame(step, false)?;
    let transport = session_parent_transport(socket, deadline)?;
    transport.send_frame(&wire, &[])?;
    require_supervisor_parent(&transport.peer)?;
    transport.check()
}

pub fn receive_session_child_step(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    step: EndpointSessionStep,
    deadline: Instant,
) -> io::Result<()> {
    endpoint_session_frame(step, false)?;
    let transport = session_owned_transport(socket, owner, token, deadline)?;
    let (wire, _) = transport.receive_frame::<16>(0)?;
    validate_endpoint_session_frame(&wire, step, false)?;
    require_same_running_image(&transport.peer)?;
    transport.check()
}

/// Receives the next child step, whichever it is, for a keeper that accepts several.
pub fn receive_any_session_child_step(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    deadline: Instant,
) -> io::Result<EndpointSessionStep> {
    let transport = session_owned_transport(socket, owner, token, deadline)?;
    let (wire, _) = transport.receive_frame::<16>(0)?;
    let step = decode_session_child_step(&wire)?;
    require_same_running_image(&transport.peer)?;
    transport.check()?;
    Ok(step)
}

/// Passes the unlock secret to the supervisor for its loader.
pub fn send_session_secret(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    secret: &[u8; SESSION_SECRET_LENGTH],
    deadline: Instant,
) -> io::Result<()> {
    let wire = encode_session_secret(secret);
    let transport = session_owned_transport(socket, owner, token, deadline)?;
    transport.send_frame(&wire[..], &[])?;
    require_same_running_image(&transport.peer)?;
    transport.check()
}

pub fn receive_session_secret(
    socket: &OwnedFd,
    deadline: Instant,
) -> io::Result<Zeroizing<Vec<u8>>> {
    let transport = session_parent_transport(socket, deadline)?;
    let secret = transport.receive_session_secret()?;
    require_supervisor_parent(&transport.peer)?;
    transport.check()?;
    Ok(secret)
}

/// Receives `Start` or `Reopen`, which select the authority the supervisor opens with.
#[allow(
    dead_code,
    reason = "The native probes never start a session supervisor."
)]
pub(crate) fn receive_session_keeper_opening(
    socket: &OwnedFd,
    deadline: Instant,
) -> io::Result<OpeningAuthority> {
    let wire = receive_session_keeper_frame(socket, deadline)?;
    decode_session_keeper_opening(&wire)
}

pub fn send_session_keeper_step(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    step: EndpointSessionStep,
    deadline: Instant,
) -> io::Result<()> {
    let wire = endpoint_session_frame(step, true)?;
    let transport = session_owned_transport(socket, owner, token, deadline)?;
    transport.send_frame(&wire, &[])?;
    require_same_running_image(&transport.peer)?;
    transport.check()
}

pub fn receive_session_keeper_step(
    socket: &OwnedFd,
    step: EndpointSessionStep,
    deadline: Instant,
) -> io::Result<()> {
    endpoint_session_frame(step, true)?;
    let wire = receive_session_keeper_frame(socket, deadline)?;
    validate_endpoint_session_frame(&wire, step, true)
}

fn receive_session_keeper_frame(socket: &OwnedFd, deadline: Instant) -> io::Result<[u8; 16]> {
    let transport = session_parent_transport(socket, deadline)?;
    let (wire, _) = transport.receive_frame::<16>(0)?;
    require_supervisor_parent(&transport.peer)?;
    transport.check()?;
    Ok(wire)
}

fn decode_session_keeper_action(wire: &[u8]) -> io::Result<EndpointSessionStep> {
    for step in [EndpointSessionStep::Activate, EndpointSessionStep::Close] {
        if wire == endpoint_session_frame(step, true)? {
            return Ok(step);
        }
    }
    Err(invalid())
}

pub fn await_session_keeper_action(socket: &OwnedFd) -> io::Result<EndpointSessionStep> {
    loop {
        let mut descriptor = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN | libc::POLLRDHUP,
            revents: 0,
        };
        // SAFETY: descriptor is initialized and remains writable while poll waits.
        let count = unsafe { libc::poll(&mut descriptor, 1, -1) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL | libc::POLLRDHUP)
            != 0
        {
            return Err(denied());
        }
        if descriptor.revents & libc::POLLIN != 0 {
            let wire =
                receive_session_keeper_frame(socket, Instant::now() + Duration::from_secs(3))?;
            return decode_session_keeper_action(&wire);
        }
    }
}

pub fn await_session_keeper_close(socket: &OwnedFd) -> io::Result<()> {
    if await_session_keeper_action(socket)? != EndpointSessionStep::Close {
        return Err(denied());
    }
    Ok(())
}

pub fn wait_session_keeper_finish(socket: &OwnedFd, deadline: Instant) -> io::Result<()> {
    let transport = session_parent_transport(socket, deadline)?;
    transport.wait_write_closed()?;
    require_supervisor_parent(&transport.peer)?;
    transport.check()
}

impl KeeperChallengeSender {
    pub fn for_owned_child(
        socket: OwnedFd,
        owner: &SpawnOwner,
        token: u64,
        deadline: Instant,
    ) -> io::Result<Self> {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(3));
        deadline_check(deadline)?;
        Ok(Self {
            transport: Transport::new(socket, ExpectedPeer::owned_child(owner, token)?, deadline)?,
        })
    }

    pub fn send(&mut self) -> io::Result<ChallengeFrame> {
        self.transport.begin()?;
        let challenge = ChallengeFrame::new(fresh_nonce(self.transport.deadline)?)?;
        self.transport.send_frame(&challenge.encode(), &[])?;
        Ok(challenge)
    }

    pub fn into_offer_receiver(mut self) -> io::Result<KeeperOfferReceiver> {
        let challenge = self.send()?;
        self.transport.attempted = false;
        Ok(KeeperOfferReceiver {
            transport: self.transport,
            challenge,
        })
    }

    pub fn receive_launch_context(
        mut self,
        admission: CurrentAdmission,
    ) -> io::Result<KeeperBootstrapReceiver> {
        self.transport.begin()?;
        let (wire, references) = self.transport.receive_frame::<KEEPER_BOOTSTRAP_SIZE>(2)?;
        let expectation = KeeperLaunchExpectation::from_authenticated_bootstrap(
            admission,
            &wire,
            references.try_into().map_err(|_| invalid())?,
        )?;
        let receiver = KeeperBootstrapReceiver {
            transport: self.transport,
            expectation,
        };
        receiver.transport.check()?;
        Ok(receiver)
    }
}

impl KeeperChallengeReceiver {
    pub fn for_parent(socket: OwnedFd, deadline: Instant) -> io::Result<Self> {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(3));
        deadline_check(deadline)?;
        Ok(Self {
            transport: Transport::new(socket, ExpectedPeer::parent()?, deadline)?,
        })
    }

    pub fn receive(&mut self) -> io::Result<ChallengeFrame> {
        self.transport.begin()?;
        let (wire, _) = self.transport.receive_frame::<48>(0)?;
        ChallengeFrame::decode(&wire)
    }

    pub fn into_offer_sender(mut self) -> io::Result<KeeperOfferSender> {
        let challenge = self.receive()?;
        self.transport.attempted = false;
        Ok(KeeperOfferSender {
            transport: self.transport,
            challenge,
        })
    }

    pub fn send_launch_context<'registration, 'preparation>(
        mut self,
        registration: &'registration AdmittedChildRegistration<'preparation>,
    ) -> io::Result<KeeperBootstrapSender<'registration, 'preparation>> {
        self.transport.begin()?;
        registration.with_keeper_bootstrap(|wire, references| {
            self.transport.send_frame(wire, references)
        })?;
        let sender = KeeperBootstrapSender {
            transport: self.transport,
            registration,
        };
        sender.transport.check()?;
        Ok(sender)
    }
}

impl<'registration, 'preparation> KeeperBootstrapSender<'registration, 'preparation> {
    pub fn validate(&self) -> io::Result<()> {
        self.registration.validate()?;
        self.transport.check()
    }

    pub fn send_offer(self) -> io::Result<KeeperOfferSent<'registration, 'preparation>> {
        self.validate()?;
        let (wire, _) = self.transport.receive_frame::<48>(0)?;
        let challenge = ChallengeFrame::decode(&wire)?;
        let offer = self.registration.with_keeper_offer(
            challenge.challenge(),
            |commitment, references| {
                let offer = OfferFrame::new(*challenge.challenge(), commitment)?;
                self.transport.send_frame(&offer.encode(), references)?;
                Ok(offer)
            },
        )?;
        let sent = KeeperOfferSent {
            transport: self.transport,
            registration: self.registration,
            offer,
        };
        sent.transport.check()?;
        Ok(sent)
    }
}

impl<'registration, 'preparation> KeeperOfferSent<'registration, 'preparation> {
    pub fn validate(&self) -> io::Result<()> {
        self.registration.validate()?;
        self.transport.check()
    }

    pub fn receive_confirmation(
        self,
    ) -> io::Result<KeeperConfirmedOffer<'registration, 'preparation>> {
        self.validate()?;
        let (wire, _) = self.transport.receive_frame::<80>(0)?;
        ConfirmationFrame::decode(&wire)?.validate_offer(&self.offer)?;
        self.validate()?;
        Ok(KeeperConfirmedOffer(self))
    }
}

impl KeeperConfirmedOffer<'_, '_> {
    pub fn validate(&self) -> io::Result<()> {
        self.0.validate()
    }

    pub fn into_active_closure_sender(self) -> io::Result<KeeperActiveClosureSender> {
        let binding = self.0.registration.active_closure_binding()?;
        self.0.transport.check_live()?;
        Ok(KeeperActiveClosureSender {
            transport: self.0.transport,
            binding,
            offer: self.0.offer,
        })
    }
}

impl KeeperActiveClosureSender {
    pub fn validate_registration(
        &self,
        registration: &AdmittedChildRegistration<'_>,
    ) -> io::Result<()> {
        self.transport.check_live()?;
        let current = registration.active_closure_binding()?;
        if current != self.binding {
            return Err(invalid());
        }
        self.transport.check_live()
    }

    pub fn send(
        self,
        session: &mut NativeSession,
        deadline: Instant,
    ) -> io::Result<KeeperActiveClosureSent> {
        self.send_with_fault(session, deadline, None)
    }

    pub fn send_fixture_fault(
        self,
        session: &mut NativeSession,
        deadline: Instant,
        fault: ActiveClosureFault,
    ) -> io::Result<KeeperActiveClosureSent> {
        self.send_with_fault(session, deadline, Some(fault))
    }

    fn send_with_fault(
        self,
        session: &mut NativeSession,
        deadline: Instant,
        fault: Option<ActiveClosureFault>,
    ) -> io::Result<KeeperActiveClosureSent> {
        let Self {
            mut transport,
            binding,
            offer,
        } = self;
        transport.check_live()?;
        let generation = binding.consume_closed(session)?;
        let frame = ActiveClosureFrame::new(&offer, generation)?;
        let mut wire = frame.encode();
        let length = match fault {
            Some(ActiveClosureFault::Incomplete) => wire.len() - 1,
            Some(ActiveClosureFault::DifferentGeneration) => {
                let generation = &mut wire[80..];
                if let Some((_, byte)) = generation
                    .iter_mut()
                    .enumerate()
                    .find(|(_, byte)| **byte != u8::MAX)
                {
                    let bit = (!*byte).trailing_zeros();
                    *byte |= 1 << bit;
                } else {
                    generation[0] ^= 1;
                }
                wire.len()
            }
            None => wire.len(),
        };
        transport.renew(deadline)?;
        transport.begin()?;
        transport.send_frame(&wire[..length], &[])?;
        Ok(KeeperActiveClosureSent {
            transport,
            offer,
            generation,
        })
    }
}

impl KeeperActiveClosureSent {
    pub fn receive_confirmation(mut self, deadline: Instant) -> io::Result<()> {
        self.transport.renew(deadline)?;
        self.transport.begin()?;
        let (wire, _) = self.transport.receive_frame::<112>(0)?;
        let confirmation = ActiveClosureConfirmationFrame::decode(&wire)?;
        confirmation.validate_offer(&self.offer)?;
        confirmation.validate_generation(&self.generation)?;
        self.transport.wait_write_closed()
    }
}

impl KeeperBootstrapReceiver {
    pub fn validate(&self) -> io::Result<()> {
        self.transport.check()?;
        self.expectation.validate()?;
        self.transport.check()
    }

    pub fn expected_commitment(&self, challenge: &[u8; 32]) -> io::Result<[u8; 32]> {
        self.validate()?;
        let commitment = self.expectation.commitment(challenge)?;
        self.validate()?;
        Ok(commitment)
    }

    pub fn receive_offer(self) -> io::Result<KeeperValidatedOffer> {
        self.validate()?;
        let challenge = ChallengeFrame::new(fresh_nonce(self.transport.deadline)?)?;
        let commitment = self.expectation.commitment(challenge.challenge())?;
        self.transport.send_frame(&challenge.encode(), &[])?;
        let (wire, references) = self.transport.receive_frame::<80>(3)?;
        let offer = OfferFrame::decode(&wire)?;
        offer.validate_expected(&challenge, &commitment)?;
        let [directory, first, second]: [OwnedFd; 3] =
            references.try_into().map_err(|_| invalid())?;
        let references = [first, second];
        let restricted_generation = self.expectation.restricted_generation();
        let active_generation = self.expectation.active_generation();
        let opening_authority = self.expectation.opening_authority();
        let directory = self.expectation.retain_offer(directory, &references)?;
        let validated = KeeperValidatedOffer {
            transport: self.transport,
            resources: KeeperOfferResources {
                directory,
                references,
                restricted_generation,
                active_generation,
                opening_authority,
            },
            offer,
        };
        validated.validate()?;
        Ok(validated)
    }
}

impl KeeperValidatedOffer {
    pub fn validate(&self) -> io::Result<()> {
        self.transport.check()?;
        self.resources.validate()?;
        self.transport.check()
    }

    pub fn adopt(self) -> io::Result<KeeperAdoptedOffer> {
        let supervisor = self.transport.peer.pidfd.try_clone()?;
        for reference in std::iter::once(&supervisor).chain(&self.resources.references) {
            if retained_process_exited(reference)? {
                return Err(denied());
            }
        }
        self.validate()?;
        KEEPER_ADOPTED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| denied())?;
        // No release without complete closure evidence. This experimental keeper
        // has no such transition and can retain only one set per process.
        Ok(KeeperAdoptedOffer {
            transport: self.transport,
            resources: ManuallyDrop::new(KeeperAdoptedResources {
                offer: self.resources,
                supervisor,
            }),
            offer: self.offer,
            confirmation: ConfirmationState::Pending,
            exit_observation_failed: false,
            provider_collection: ProviderCollection::Pending,
            retirement: GenerationRetirement::Pending,
            active_closure: ActiveClosureState::Pending,
            active_closure_confirmation: ConfirmationState::Pending,
        })
    }
}

impl KeeperOfferResources {
    fn validate(&self) -> io::Result<()> {
        self.directory.validate()?;
        verify_live_process_pair(&self.references, &self.references)?;
        self.directory.validate()
    }
}

impl KeeperAdoptedOffer {
    /// The authority the supervisor opened with, once the keeper has confirmed
    /// the offer and before it hands over the unlock secret.
    pub(crate) fn confirmed_opening_authority(&self) -> Option<OpeningAuthority> {
        if self.confirmation != ConfirmationState::Sent {
            return None;
        }
        self.resources.offer.opening_authority
    }

    pub fn active_agent(&self) -> io::Result<BorrowedFd<'_>> {
        if self.confirmed_opening_authority().is_none()
            || self.active_closure != ActiveClosureState::Pending
            || self.active_closure_confirmation != ConfirmationState::Pending
            || self.provider_collection != ProviderCollection::Pending
            || self.exit_observation_failed
            || self.resources.offer.active_generation.is_none()
            || self.resources.offer.restricted_generation.is_some()
            || !matches!(self.retirement, GenerationRetirement::Pending)
        {
            return Err(denied());
        }
        self.transport.check_live()?;
        self.resources.offer.directory.validate()?;
        // Registration authenticated this exact order as agent, then loader.
        let [agent, loader] = &self.resources.offer.references;
        verify_live_process(agent, agent)?;
        if !retained_process_exited(loader)? {
            return Err(denied());
        }
        self.resources.offer.directory.validate()?;
        self.transport.check_live()?;
        verify_live_process(agent, agent)?;
        Ok(agent.as_fd())
    }

    pub fn confirm(&mut self) -> io::Result<()> {
        if self.confirmation != ConfirmationState::Pending {
            return Err(invalid());
        }
        self.confirmation = ConfirmationState::Failed;
        self.transport.check()?;
        self.resources.offer.validate()?;
        let confirmation = ConfirmationFrame::new(&self.offer)?;
        self.transport.send_frame(&confirmation.encode(), &[])?;
        self.resources.offer.validate()?;
        self.transport.check()?;
        self.confirmation = ConfirmationState::Sent;
        Ok(())
    }

    /// Observes the retained references, independent of the confirmation channel.
    pub fn observe_registered_exits(&mut self) -> io::Result<bool> {
        if self.exit_observation_failed {
            return Err(invalid());
        }
        let result = (|| {
            self.transport.origin.validate()?;
            let mut all_exited = true;
            for reference in
                std::iter::once(&self.resources.supervisor).chain(&self.resources.offer.references)
            {
                all_exited &= retained_process_exited(reference)?;
            }
            self.transport.origin.validate()?;
            Ok(all_exited)
        })();
        self.exit_observation_failed = result.is_err();
        result
    }

    pub fn probe_receive_active_closure(&mut self, deadline: Instant) -> io::Result<()> {
        if self.active_closure == ActiveClosureState::Received {
            return Err(invalid());
        }
        if self.active_closure != ActiveClosureState::Pending {
            return Err(invalid());
        }
        self.active_closure = ActiveClosureState::Failed;
        if self.confirmation != ConfirmationState::Sent
            || self.resources.offer.active_generation.is_none()
            || self.resources.offer.restricted_generation.is_some()
            || !matches!(self.retirement, GenerationRetirement::Pending)
        {
            return Err(denied());
        }
        let generation = self.resources.offer.active_generation.ok_or_else(denied)?;
        self.resources.offer.directory.validate()?;
        self.transport.origin.validate()?;
        self.transport.renew(deadline)?;
        self.transport.begin()?;
        let (wire, references) = self.transport.receive_frame::<112>(0)?;
        if !references.is_empty() {
            return Err(invalid());
        }
        let frame = ActiveClosureFrame::decode(&wire)?;
        frame.validate_offer(&self.offer)?;
        frame.validate_generation(&generation)?;
        for reference in &self.resources.offer.references {
            if !retained_process_exited(reference)? {
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
        }
        self.resources.offer.directory.validate()?;
        self.transport.check()?;
        self.active_closure = ActiveClosureState::Received;
        Ok(())
    }

    pub fn confirm_active_closure(&mut self, deadline: Instant) -> io::Result<()> {
        if self.active_closure_confirmation != ConfirmationState::Pending {
            return Err(denied());
        }
        self.active_closure_confirmation = ConfirmationState::Failed;
        if self.active_closure != ActiveClosureState::Received
            || self.confirmation != ConfirmationState::Sent
            || self.resources.offer.restricted_generation.is_some()
            || !matches!(self.retirement, GenerationRetirement::Pending)
            || self.exit_observation_failed
        {
            return Err(denied());
        }
        let generation = self.resources.offer.active_generation.ok_or_else(denied)?;
        self.resources.offer.directory.validate()?;
        self.transport.origin.validate()?;
        self.transport.renew(deadline)?;
        self.transport.begin()?;
        let confirmation = ActiveClosureConfirmationFrame::new(&self.offer, generation)?;
        self.transport.send_frame(&confirmation.encode(), &[])?;
        self.resources.offer.directory.validate()?;
        self.transport.origin.validate()?;
        self.transport.close_write()?;
        self.active_closure_confirmation = ConfirmationState::Sent;
        Ok(())
    }

    pub fn probe_advance_active_generation(&mut self) -> io::Result<KeeperPreUnlockSuccessor<'_>> {
        let previous = self.consume_active_retirement()?;
        let current = new_session_generation()?;
        if current == previous {
            return Err(invalid());
        }
        self.resources.offer.directory.validate()?;
        self.transport.origin.validate()?;
        self.retirement = GenerationRetirement::Advanced(current);
        Ok(KeeperPreUnlockSuccessor {
            keeper: self,
            previous,
            current,
            authority: SuccessorAuthority::ActiveClosure,
        })
    }

    pub fn publish_clean_close(
        &mut self,
        authorize_rename: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), PublishError> {
        if self.resources.offer.opening_authority.is_none() {
            return Err(PublishError::Unpublished(denied()));
        }
        self.consume_active_retirement()
            .map_err(PublishError::Unpublished)?;
        self.resources
            .offer
            .directory
            .publish_clean_opening_permit(authorize_rename)?;
        self.resources
            .offer
            .directory
            .validate()
            .map_err(PublishError::PublishedDurabilityUnknown)?;
        self.transport
            .origin
            .validate()
            .map_err(PublishError::PublishedDurabilityUnknown)
    }

    pub fn probe_publish_clean_close(&mut self) -> io::Result<()> {
        self.publish_clean_close(|| Ok(()))
            .map_err(|error| match error {
                PublishError::Unpublished(error)
                | PublishError::PublishedDurabilityUnknown(error) => error,
            })
    }

    fn consume_active_retirement(&mut self) -> io::Result<[u8; 32]> {
        if self.active_closure != ActiveClosureState::Received
            || self.confirmation != ConfirmationState::Sent
            || self.resources.offer.active_generation.is_none()
            || self.resources.offer.restricted_generation.is_some()
            || !matches!(self.retirement, GenerationRetirement::Pending)
            || self.exit_observation_failed
            || self.active_closure_confirmation == ConfirmationState::Failed
        {
            return Err(denied());
        }
        let previous = self.resources.offer.active_generation.ok_or_else(denied)?;
        self.transport.origin.validate()?;
        self.resources.offer.directory.validate()?;
        if !self.observe_registered_exits()? {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        if self.active_closure_confirmation != ConfirmationState::Sent {
            return Err(denied());
        }
        self.retirement = GenerationRetirement::Consumed;
        self.resources.offer.directory.validate()?;
        self.transport.origin.validate()?;
        Ok(previous)
    }

    pub(crate) fn probe_advance_pre_unlock_generation(
        &mut self,
    ) -> io::Result<KeeperPreUnlockSuccessor<'_>> {
        self.transport.origin.validate()?;
        let previous = self
            .resources
            .offer
            .restricted_generation
            .ok_or_else(denied)?;
        if self.confirmation != ConfirmationState::Sent {
            return Err(denied());
        }
        if !matches!(self.retirement, GenerationRetirement::Pending) {
            return Err(invalid());
        }
        self.resources.offer.directory.validate()?;
        if !self.observe_registered_exits()? {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        self.retirement = GenerationRetirement::Consumed;
        let current = new_session_generation()?;
        if current == previous {
            return Err(invalid());
        }
        self.resources.offer.directory.validate()?;
        self.transport.origin.validate()?;
        self.retirement = GenerationRetirement::Advanced(current);
        Ok(KeeperPreUnlockSuccessor {
            keeper: self,
            previous,
            current,
            authority: SuccessorAuthority::PreUnlock,
        })
    }

    pub(crate) fn probe_collect_provider_sigkill(&mut self) -> io::Result<()> {
        if self.provider_collection != ProviderCollection::Pending {
            return Err(invalid());
        }
        self.provider_collection = ProviderCollection::Failed;
        if self.confirmation != ConfirmationState::Sent
            || self.resources.offer.restricted_generation.is_none()
            || !self.observe_registered_exits()?
        {
            return Err(invalid());
        }
        self.resources.offer.directory.validate()?;
        self.transport.origin.validate()?;
        for reference in &self.resources.offer.references {
            if !retained_process_exited(reference)? {
                return Err(invalid());
            }
            // SAFETY: the owned pidfd identifies the adopted descendant and
            // status is writable for one waitid result. Only a subreaper
            // entitled to collect this exact child can obtain its exit status.
            let mut status: libc::siginfo_t = unsafe { zeroed() };
            if unsafe {
                libc::waitid(
                    libc::P_PIDFD,
                    reference.as_raw_fd() as libc::id_t,
                    &mut status,
                    libc::WEXITED | libc::WNOHANG,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: a CLD_KILLED result initializes these SIGCHLD fields.
            if status.si_signo != libc::SIGCHLD
                || status.si_code != libc::CLD_KILLED
                || unsafe { status.si_pid() <= 0 || status.si_status() != libc::SIGKILL }
            {
                return Err(invalid());
            }
        }
        self.resources.offer.directory.validate()?;
        self.transport.origin.validate()?;
        self.provider_collection = ProviderCollection::Collected;
        Ok(())
    }
}

// A separate disposable channel retains the PTY master through the crash.
// The keeper offer and confirmation keep their existing wire formats.
pub(crate) fn probe_send_keeper_terminal(
    socket: &OwnedFd,
    master: &OwnedFd,
    deadline: Instant,
) -> io::Result<()> {
    verify_pty_master(master)?;
    let mut transport = Transport::new(socket.try_clone()?, ExpectedPeer::parent()?, deadline)?;
    transport.begin()?;
    transport.send_frame(PROBE_PTY_FRAME, std::slice::from_ref(master))
}

pub(crate) fn probe_receive_keeper_terminal(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    deadline: Instant,
) -> io::Result<OwnedFd> {
    let mut transport = Transport::new(
        socket.try_clone()?,
        ExpectedPeer::owned_child(owner, token)?,
        deadline,
    )?;
    transport.begin()?;
    let (wire, references) = transport.receive_frame::<1>(1)?;
    let [master]: [OwnedFd; 1] = references.try_into().map_err(|_| invalid())?;
    if &wire != PROBE_PTY_FRAME {
        return Err(invalid());
    }
    verify_pty_master(&master)?;
    Ok(master)
}

fn verify_pty_master(master: &OwnedFd) -> io::Result<()> {
    require_cloexec(master.as_raw_fd())?;
    let mut index: libc::c_uint = 0;
    // SAFETY: TIOCGPTN reads the master identity into one writable integer.
    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCGPTN, &mut index) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl KeeperOfferSender {
    pub fn challenge(&self) -> &ChallengeFrame {
        &self.challenge
    }

    /// Sends candidate descriptors in directory, first process, second process order.
    pub fn send(&mut self, commitment: [u8; 32], references: &[OwnedFd; 3]) -> io::Result<()> {
        self.transport.begin()?;
        let offer = OfferFrame::new(*self.challenge.challenge(), commitment)?;
        self.transport.send_frame(&offer.encode(), references)
    }
}

impl KeeperOfferReceiver {
    pub fn receive(&mut self) -> io::Result<ReceivedKeeperOffer> {
        self.transport.begin()?;
        let (wire, references) = self.transport.receive_frame::<80>(3)?;
        let offer = OfferFrame::decode(&wire)?;
        offer.validate_challenge(&self.challenge)?;
        Ok(ReceivedKeeperOffer {
            _offer: offer,
            _references: references.try_into().map_err(|_| invalid())?,
            _origin: Origin::current(),
        })
    }
}

pub struct RegistrationReceiver {
    transport: Transport,
    nonce: [u8; 32],
}

pub struct RegistrationSender {
    transport: Transport,
}

/// Transferred candidates only; this type grants no exit or session authority.
pub struct ReceivedProcessReferences {
    _references: [OwnedFd; 2],
    _origin: Origin,
}

impl RegistrationReceiver {
    pub fn for_owned_child(
        socket: OwnedFd,
        owner: &SpawnOwner,
        token: u64,
        deadline: Instant,
    ) -> io::Result<Self> {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(3));
        deadline_check(deadline)?;
        Self::new(socket, ExpectedPeer::owned_child(owner, token)?, deadline)
    }

    fn new(socket: OwnedFd, peer: ExpectedPeer, deadline: Instant) -> io::Result<Self> {
        let transport = Transport::new(socket, peer, deadline)?;
        let nonce = fresh_nonce(transport.deadline)?;
        transport.check()?;
        Ok(Self { transport, nonce })
    }

    pub fn receive(&mut self) -> io::Result<ReceivedProcessReferences> {
        self.transport.begin()?;
        self.transport.send(CHALLENGE, &self.nonce, &[])?;
        let (_, references) = self.transport.receive(RESPONSE, Some(&self.nonce), 2)?;
        for reference in &references {
            validate_reference(reference)?;
        }
        self.transport.check()?;
        self.transport.send(ACK, &self.nonce, &[])?;
        for reference in &references {
            validate_reference(reference)?;
        }
        self.transport.check()?;
        let references: [OwnedFd; 2] = references.try_into().map_err(|_| invalid())?;
        Ok(ReceivedProcessReferences {
            _references: references,
            _origin: Origin::current(),
        })
    }
}

impl RegistrationSender {
    pub fn new(socket: OwnedFd, deadline: Instant) -> io::Result<Self> {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(3));
        deadline_check(deadline)?;
        Ok(Self {
            transport: Transport::new(socket, ExpectedPeer::parent()?, deadline)?,
        })
    }

    pub fn send_registered(&mut self, registration: &OwnedChildRegistration<'_>) -> io::Result<()> {
        self.transport.begin()?;
        registration.with_references(|references| {
            let (nonce, _) = self.transport.receive(CHALLENGE, None, 0)?;
            registration.validate()?;
            self.transport.send(RESPONSE, &nonce, references)?;
            self.transport.receive(ACK, Some(&nonce), 0)?;
            self.transport.check()
        })
    }

    pub fn send(&mut self, owner: &SpawnOwner, tokens: [u64; 2]) -> io::Result<()> {
        self.transport.begin()?;
        if tokens[0] == tokens[1] {
            return Err(invalid());
        }
        let (nonce, _) = self.transport.receive(CHALLENGE, None, 0)?;
        let references = [
            owner.duplicate_pidfd(tokens[0])?,
            owner.duplicate_pidfd(tokens[1])?,
        ];
        for reference in &references {
            validate_reference(reference)?;
        }
        self.transport.send(RESPONSE, &nonce, &references)?;
        self.transport.receive(ACK, Some(&nonce), 0)?;
        for reference in &references {
            validate_reference(reference)?;
        }
        self.transport.check()
    }
}

fn encode(kind: u8, nonce: &[u8; 32]) -> [u8; WIRE_SIZE] {
    let mut wire = [0; WIRE_SIZE];
    wire[..8].copy_from_slice(TAG);
    wire[8] = 1;
    wire[9] = kind;
    wire[16..].copy_from_slice(nonce);
    wire
}

fn decode(wire: &[u8; WIRE_SIZE], kind: u8, nonce: Option<&[u8; 32]>) -> io::Result<[u8; 32]> {
    if &wire[..8] != TAG
        || wire[8] != 1
        || wire[9] != kind
        || ![CHALLENGE, RESPONSE, ACK].contains(&kind)
        || wire[10..16] != [0; 6]
        || wire[16..] == [0; 32]
        || nonce.is_some_and(|expected| wire[16..] != *expected)
    {
        return Err(invalid());
    }
    wire[16..].try_into().map_err(|_| invalid())
}

fn fresh_nonce(deadline: Instant) -> io::Result<[u8; 32]> {
    let mut nonce = [0; 32];
    let mut used = 0;
    while used < nonce.len() {
        deadline_check(deadline)?;
        // SAFETY: the remaining nonce storage is writable for the supplied length.
        let count = unsafe {
            libc::getrandom(
                nonce[used..].as_mut_ptr().cast(),
                nonce.len() - used,
                libc::GRND_NONBLOCK,
            )
        };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if count == 0 || count as usize > nonce.len() - used {
            return Err(invalid());
        }
        used += count as usize;
    }
    deadline_check(deadline)?;
    if nonce == [0; 32] {
        return Err(invalid());
    }
    Ok(nonce)
}

fn validate_reference(reference: &OwnedFd) -> io::Result<()> {
    require_cloexec(reference.as_raw_fd())?;
    pending(reference)?;
    // SAFETY: signal zero queries permission through the retained descriptor only.
    if unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            reference.as_raw_fd(),
            0,
            std::ptr::null::<libc::siginfo_t>(),
            0u32,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    pending(reference)?;
    require_cloexec(reference.as_raw_fd())
}

fn pending(reference: &OwnedFd) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: reference.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor is one initialized poll descriptor.
    let count = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    if count != 0 || descriptor.revents != 0 {
        return Err(invalid());
    }
    Ok(())
}

fn parse_control(
    control: &[usize; 32],
    reported: usize,
    peer: &ExpectedPeer,
    expected: usize,
) -> io::Result<Vec<OwnedFd>> {
    let credential = libc::ucred {
        pid: peer.pid,
        uid: peer.uid,
        gid: peer.gid,
    };
    parse_control_credentials(control, reported, &credential, expected)
}

fn parse_control_credentials(
    control: &[usize; 32],
    reported: usize,
    peer: &libc::ucred,
    expected: usize,
) -> io::Result<Vec<OwnedFd>> {
    let capacity = size_of_val(control);
    let length = reported.min(capacity);
    let base = control.as_ptr().cast::<u8>();
    let alignment = size_of::<usize>();
    let header_size = size_of::<libc::cmsghdr>();
    let data_offset = (header_size + alignment - 1) & !(alignment - 1);
    let mut offset = 0;
    let mut credentials = 0;
    let mut rights = 0;
    let mut references = Vec::new();
    let mut rejected = reported > capacity;
    while length.saturating_sub(offset) >= header_size {
        // SAFETY: a complete header lies within initialized control storage.
        let header = unsafe { base.add(offset).cast::<libc::cmsghdr>().read_unaligned() };
        let size = header.cmsg_len;
        if size < data_offset {
            rejected = true;
            break;
        }
        let visible = size.min(length - offset);
        if header.cmsg_level == libc::SOL_SOCKET
            && (header.cmsg_type == libc::SCM_RIGHTS || header.cmsg_type == SCM_PIDFD)
        {
            rights += usize::from(header.cmsg_type == libc::SCM_RIGHTS);
            if header.cmsg_type == SCM_PIDFD
                || !(size - data_offset).is_multiple_of(size_of::<i32>())
            {
                rejected = true;
            }
            let mut position = data_offset;
            while position + size_of::<i32>() <= visible {
                // SAFETY: a complete transferred descriptor word lies inside visible data.
                let raw = unsafe { base.add(offset + position).cast::<i32>().read_unaligned() };
                if raw < 0 {
                    rejected = true;
                } else {
                    // SAFETY: the received ancillary record transfers this descriptor.
                    references.push(unsafe { OwnedFd::from_raw_fd(raw) });
                }
                position += size_of::<i32>();
            }
        } else if header.cmsg_level == libc::SOL_SOCKET
            && header.cmsg_type == libc::SCM_CREDENTIALS
            && size == data_offset + size_of::<libc::ucred>()
            && size <= length - offset
        {
            credentials += 1;
            // SAFETY: the full credential record lies inside initialized visible data.
            let credential = unsafe {
                base.add(offset + data_offset)
                    .cast::<libc::ucred>()
                    .read_unaligned()
            };
            if credential.pid != peer.pid
                || credential.uid != peer.uid
                || credential.gid != peer.gid
            {
                rejected = true;
            }
        } else {
            rejected = true;
        }
        if size > length - offset {
            rejected = true;
            break;
        }
        let Some(next) = size
            .checked_add(alignment - 1)
            .map(|value| value & !(alignment - 1))
            .and_then(|value| offset.checked_add(value))
        else {
            rejected = true;
            break;
        };
        offset = next.min(length);
    }
    if offset < length {
        rejected = true;
    }
    if rejected
        || credentials != 1
        || references.len() != expected
        || rights != usize::from(expected != 0)
    {
        return Err(denied());
    }
    Ok(references)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
    use std::os::unix::fs::MetadataExt;
    use std::time::Duration;

    #[test]
    fn endpoint_session_steps_require_the_exact_direction_and_frame() {
        let steps = [
            EndpointSessionStep::Prepared,
            EndpointSessionStep::Start,
            EndpointSessionStep::Register,
            EndpointSessionStep::Unlocked,
            EndpointSessionStep::Prove,
            EndpointSessionStep::Proved,
            EndpointSessionStep::Close,
            EndpointSessionStep::Closed,
            EndpointSessionStep::Activate,
            EndpointSessionStep::Active,
            EndpointSessionStep::Served,
            EndpointSessionStep::Reopen,
        ];
        for step in steps {
            let keeper = matches!(
                step,
                EndpointSessionStep::Start
                    | EndpointSessionStep::Prove
                    | EndpointSessionStep::Close
                    | EndpointSessionStep::Activate
                    | EndpointSessionStep::Reopen
            );
            let wire = endpoint_session_frame(step, keeper).unwrap();
            assert!(endpoint_session_frame(step, !keeper).is_err());
            assert!(validate_endpoint_session_frame(&wire, step, keeper).is_ok());
            assert!(validate_endpoint_session_frame(&wire, step, !keeper).is_err());
            assert!(validate_endpoint_session_frame(&wire[..15], step, keeper).is_err());
            assert!(validate_endpoint_session_frame(SUPERVISOR_READY, step, keeper).is_err());
            for other in steps {
                if other != step {
                    assert!(validate_endpoint_session_frame(&wire, other, keeper).is_err());
                }
            }
            for index in 0..wire.len() {
                let mut changed = wire;
                changed[index] ^= 0x80;
                assert!(validate_endpoint_session_frame(&changed, step, keeper).is_err());
            }
        }
    }

    #[test]
    fn endpoint_session_frames_bind_direction_for_served_and_reopen() {
        for (step, keeper, id) in [
            (EndpointSessionStep::Served, false, 11),
            (EndpointSessionStep::Reopen, true, 12),
        ] {
            let wire = endpoint_session_frame(step, keeper).unwrap();
            assert_eq!(&wire[..9], b"VVKEPSS\0\x01");
            assert_eq!(wire[9], if keeper { 1 } else { 2 });
            assert_eq!(wire[10], id);
            assert!(wire[11..].iter().all(|byte| *byte == 0));
            assert!(endpoint_session_frame(step, !keeper).is_err());
            assert!(validate_endpoint_session_frame(&wire, step, !keeper).is_err());
            assert!(decode_session_keeper_action(&wire).is_err());
        }
        let served = endpoint_session_frame(EndpointSessionStep::Served, false).unwrap();
        assert_eq!(
            decode_session_child_step(&served).unwrap(),
            EndpointSessionStep::Served
        );
        assert!(decode_session_child_step(&served[..15]).is_err());
        assert!(decode_session_keeper_opening(&served).is_err());
        for (step, authority) in [
            (EndpointSessionStep::Start, OpeningAuthority::Permit),
            (EndpointSessionStep::Reopen, OpeningAuthority::Reopened),
        ] {
            let wire = endpoint_session_frame(step, true).unwrap();
            assert_eq!(decode_session_keeper_opening(&wire).unwrap(), authority);
            assert!(decode_session_child_step(&wire).is_err());
            for index in 0..wire.len() {
                let mut changed = wire;
                changed[index] ^= 0x80;
                assert!(decode_session_keeper_opening(&changed).is_err());
            }
        }
        for step in [EndpointSessionStep::Prove, EndpointSessionStep::Close] {
            let wire = endpoint_session_frame(step, true).unwrap();
            assert!(decode_session_keeper_opening(&wire).is_err());
        }
    }

    fn session_secret_pair() -> (Transport, Transport) {
        let (receiver, sender) = super::super::packet_pair().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        (
            Transport::new(receiver, peer(), deadline).unwrap(),
            Transport::new(sender, peer(), deadline).unwrap(),
        )
    }

    #[test]
    fn session_secret_frame_refuses_other_lengths() {
        let secret = [b'a'; 64];
        let wire = encode_session_secret(&secret);
        assert_eq!(wire.len(), 80);
        assert_eq!(&wire[..16], b"VVKEPSU\0\x01\0\0\0\0\0\0\0");
        assert_eq!(&wire[16..], &secret);
        assert_eq!(&decode_session_secret(&wire[..]).unwrap()[..], &secret);
        for length in [16, 79, 81, 96] {
            let mut other = vec![0; length];
            let shared = length.min(wire.len());
            other[..shared].copy_from_slice(&wire[..shared]);
            assert!(decode_session_secret(&other).is_err(), "{length}");
            let (receiver, sender) = session_secret_pair();
            sender.send_frame(&other, &[]).unwrap();
            assert_eq!(
                receiver.receive_session_secret().unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{length}"
            );
        }
        for index in 0..16 {
            let mut changed = *wire;
            changed[index] ^= 0x80;
            assert!(decode_session_secret(&changed).is_err());
        }
        let (receiver, sender) = session_secret_pair();
        sender.send_frame(&wire[..], &[]).unwrap();
        assert_eq!(&receiver.receive_session_secret().unwrap()[..], &secret);
    }

    #[test]
    fn keeper_actions_accept_only_canonical_activation_and_close() {
        for (step, id) in [
            (EndpointSessionStep::Activate, 9),
            (EndpointSessionStep::Close, 7),
        ] {
            let wire = endpoint_session_frame(step, true).unwrap();
            assert_eq!(wire[10], id);
            assert_eq!(decode_session_keeper_action(&wire).unwrap(), step);
            for index in 0..wire.len() {
                let mut changed = wire;
                changed[index] ^= 0x80;
                assert!(decode_session_keeper_action(&changed).is_err());
            }
            assert!(decode_session_keeper_action(&wire[..15]).is_err());
        }
        let active = endpoint_session_frame(EndpointSessionStep::Active, false).unwrap();
        assert_eq!(active[10], 10);
        for (step, keeper) in [
            (EndpointSessionStep::Start, true),
            (EndpointSessionStep::Prove, true),
            (EndpointSessionStep::Active, false),
            (EndpointSessionStep::Closed, false),
        ] {
            assert!(
                decode_session_keeper_action(&endpoint_session_frame(step, keeper).unwrap())
                    .is_err()
            );
        }
    }

    #[test]
    fn endpoint_start_requires_the_exact_role_and_frame() {
        for role in [
            EndpointStartupRole::Supervisor,
            EndpointStartupRole::Verifier,
        ] {
            let wire = endpoint_start_frame(role);
            assert!(validate_endpoint_start_frame(&wire, role).is_ok());
            let other = match role {
                EndpointStartupRole::Supervisor => EndpointStartupRole::Verifier,
                EndpointStartupRole::Verifier => EndpointStartupRole::Supervisor,
            };
            assert!(validate_endpoint_start_frame(&wire, other).is_err());
            assert!(validate_endpoint_start_frame(&wire[..15], role).is_err());
            assert!(validate_endpoint_start_frame(SUPERVISOR_START, role).is_err());
            for index in 0..wire.len() {
                let mut changed = wire;
                changed[index] ^= 0x80;
                assert!(validate_endpoint_start_frame(&changed, role).is_err());
            }
        }
    }

    #[test]
    fn child_pidfd_handoff_requires_current_credentials() {
        let pid = std::process::id();
        let channel = ChildPidfdChannel::new().unwrap();
        send_own_pidfd(channel.child_socket()).unwrap();
        validate_reference(&channel.receive(pid).unwrap()).unwrap();

        let channel = ChildPidfdChannel::new().unwrap();
        send_own_pidfd(channel.child_socket()).unwrap();
        assert_eq!(
            channel.receive(pid - 1).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    fn peer() -> ExpectedPeer {
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0) };
        assert!(raw >= 0);
        ExpectedPeer {
            pidfd: unsafe { OwnedFd::from_raw_fd(raw as i32) },
            pid: unsafe { libc::getpid() },
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        }
    }

    #[test]
    fn keeper_challenge_receiver_rejects_expired_deadline() {
        let (socket, _other) = super::super::packet_pair().unwrap();
        assert_eq!(
            KeeperChallengeReceiver::for_parent(socket, Instant::now())
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    fn keeper_offer_pair() -> (KeeperOfferReceiver, KeeperOfferSender) {
        let (keeper, supervisor) = super::super::packet_pair().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let keeper = KeeperChallengeSender {
            transport: Transport::new(keeper, peer(), deadline).unwrap(),
        };
        let supervisor = KeeperChallengeReceiver {
            transport: Transport::new(supervisor, peer(), deadline).unwrap(),
        };
        let receiver = keeper.into_offer_receiver().unwrap();
        let sender = supervisor.into_offer_sender().unwrap();
        assert_eq!(receiver.transport.deadline, deadline);
        assert_eq!(sender.transport.deadline, deadline);
        (receiver, sender)
    }

    fn candidate_descriptors() -> [OwnedFd; 3] {
        ["/proc/self", "/dev/null", "/dev/zero"].map(|path| File::open(path).unwrap().into())
    }

    fn endpoint_connection_pair() -> (EndpointProofConnection, Transport) {
        let (socket, peer_socket) = super::super::packet_pair().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        (
            EndpointProofConnection {
                transport: Transport::new(socket, peer(), deadline).unwrap(),
                phase: EndpointPhase::Ready([7; 32]),
            },
            Transport::new(peer_socket, peer(), deadline).unwrap(),
        )
    }

    fn endpoint_proof(echo: [u8; 32]) -> [u8; 176] {
        super::super::super::linux_endpoint_codec::ProofFrame {
            echo,
            generation: [8; 32],
            connection_nonce: [9; 32],
            signature: [0; 64],
        }
        .encode()
        .unwrap()
    }

    #[test]
    fn endpoint_connection_sends_only_one_proof_for_the_received_challenge() {
        let (mut connection, receiver) = endpoint_connection_pair();
        let wire = endpoint_proof([7; 32]);
        connection.send_proof(&wire).unwrap();
        let (received, descriptors) = receiver.receive_frame::<176>(0).unwrap();
        assert_eq!(received, wire);
        assert!(descriptors.is_empty());
        assert_eq!(
            connection.send_proof(&wire).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn endpoint_connection_rejects_wrong_echo_and_consumes_the_send() {
        let (mut connection, receiver) = endpoint_connection_pair();
        assert_eq!(
            connection
                .send_proof(&endpoint_proof([6; 32]))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(receiver.check().is_err());
        assert_eq!(
            connection
                .send_proof(&endpoint_proof([7; 32]))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn endpoint_connection_consumes_a_malformed_challenge_before_parsing() {
        let (mut connection, sender) = endpoint_connection_pair();
        connection.phase = EndpointPhase::Waiting;
        sender.send_frame(&[0; 48], &[]).unwrap();
        assert_eq!(
            connection.receive_challenge().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            connection.receive_challenge().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(sender.check().is_err());
    }

    #[test]
    fn endpoint_request_refuses_reuse_before_sending_a_challenge() {
        let (mut connection, receiver) = endpoint_connection_pair();
        assert_eq!(
            connection.request_proof().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(receiver.check().is_err());
        assert_eq!(
            connection.request_proof().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn keeper_offer_transfers_ordered_cloexec_descriptors_once() {
        let (mut receiver, mut sender) = keeper_offer_pair();
        let candidates = candidate_descriptors();
        let commitment = [7; 32];
        sender.send(commitment, &candidates).unwrap();
        let ReceivedKeeperOffer {
            _offer: offer,
            _references: received,
            ..
        } = receiver.receive().unwrap();
        offer
            .validate_expected(sender.challenge(), &commitment)
            .unwrap();
        for (offered, received) in candidates.into_iter().zip(received) {
            require_cloexec(received.as_raw_fd()).unwrap();
            let offered = File::from(offered).metadata().unwrap();
            let received = File::from(received).metadata().unwrap();
            assert_eq!(
                (offered.dev(), offered.ino()),
                (received.dev(), received.ino())
            );
        }
        assert_eq!(
            sender
                .send(commitment, &candidate_descriptors())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            receiver.receive().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn keeper_offer_rejects_wrong_challenge_or_descriptor_count_once() {
        let candidates = candidate_descriptors();
        for count in [0, 2, 3] {
            let (mut receiver, sender) = keeper_offer_pair();
            let challenge = *sender.challenge().challenge();
            let wrong_challenge = if challenge == [1; 32] {
                [2; 32]
            } else {
                [1; 32]
            };
            let sent_challenge = if count == 3 {
                wrong_challenge
            } else {
                challenge
            };
            let offer = OfferFrame::new(sent_challenge, [7; 32]).unwrap();
            sender
                .transport
                .send_frame(&offer.encode(), &candidates[..count])
                .unwrap();
            let expected = if count == 3 {
                io::ErrorKind::InvalidData
            } else {
                io::ErrorKind::PermissionDenied
            };
            assert_eq!(receiver.receive().err().unwrap().kind(), expected);
            assert_eq!(
                receiver.receive().err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn receiver_rejects_non_socket() {
        let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        assert!(
            RegistrationReceiver::new(fd, peer(), Instant::now() + Duration::from_secs(1)).is_err()
        );
    }

    #[test]
    fn receiver_rejects_expired_deadline() {
        let (socket, _other) = super::super::packet_pair().unwrap();
        assert!(RegistrationReceiver::new(socket, peer(), Instant::now()).is_err());
    }

    #[test]
    fn receiver_failure_is_sticky() {
        let (socket, other) = super::super::packet_pair().unwrap();
        let mut receiver =
            RegistrationReceiver::new(socket, peer(), Instant::now() + Duration::from_secs(1))
                .unwrap();
        drop(other);
        assert!(receiver.receive().is_err());
        assert_eq!(
            receiver.receive().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn sender_rejects_non_socket() {
        let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        assert!(RegistrationSender::new(fd, Instant::now() + Duration::from_secs(1)).is_err());
    }

    #[test]
    fn sender_rejects_expired_deadline() {
        let (socket, _other) = super::super::packet_pair().unwrap();
        assert!(RegistrationSender::new(socket, Instant::now()).is_err());
    }

    #[test]
    fn receiver_takes_socket_ownership_on_rejection() {
        let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let raw = fd.as_raw_fd();
        assert!(
            RegistrationReceiver::new(fd, peer(), Instant::now() + Duration::from_secs(1)).is_err()
        );
        assert_eq!(unsafe { libc::fcntl(raw, libc::F_GETFD) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
    }

    fn append(control: &mut [usize; 32], offset: usize, kind: i32, data: &[u8]) -> usize {
        // SAFETY: test payloads fit the fixed aligned ancillary storage.
        unsafe {
            let base = control.as_mut_ptr().cast::<u8>().add(offset);
            let header = base.cast::<libc::cmsghdr>();
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = kind;
            (*header).cmsg_len = libc::CMSG_LEN(data.len() as _) as usize;
            std::ptr::copy_nonoverlapping(data.as_ptr(), libc::CMSG_DATA(header), data.len());
            offset + libc::CMSG_SPACE(data.len() as _) as usize
        }
    }

    fn credentials(control: &mut [usize; 32], peer: &ExpectedPeer) -> usize {
        let credential = libc::ucred {
            pid: peer.pid,
            uid: peer.uid,
            gid: peer.gid,
        };
        // SAFETY: ucred is initialized and has no uninitialized padding on Linux.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&credential as *const libc::ucred).cast(),
                size_of_val(&credential),
            )
        };
        append(control, 0, libc::SCM_CREDENTIALS, bytes)
    }

    fn assert_closed(raw: i32) {
        // SAFETY: F_GETFD queries the descriptor without consuming any ownership.
        assert_eq!(unsafe { libc::fcntl(raw, libc::F_GETFD) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
    }

    #[test]
    fn wire_rejects_header_and_reserved_changes() {
        let wire = encode(RESPONSE, &[7; 32]);
        assert_eq!(decode(&wire, RESPONSE, Some(&[7; 32])).unwrap(), [7; 32]);
        for offset in 0..16 {
            let mut changed = wire;
            changed[offset] ^= 0xff;
            assert!(decode(&changed, RESPONSE, Some(&[7; 32])).is_err());
        }
    }

    #[test]
    fn wire_rejects_replay_zero_nonce_and_wrong_kind() {
        assert!(decode(&encode(RESPONSE, &[7; 32]), RESPONSE, Some(&[8; 32])).is_err());
        assert!(decode(&encode(RESPONSE, &[0; 32]), RESPONSE, None).is_err());
        assert!(decode(&encode(9, &[7; 32]), 9, None).is_err());
        assert!(decode(&encode(ACK, &[7; 32]), RESPONSE, None).is_err());
    }

    #[test]
    fn missing_credentials_closes_rights() {
        let peer = peer();
        let raw = peer.pidfd.try_clone().unwrap().into_raw_fd();
        let mut control = [0; 32];
        let length = append(&mut control, 0, libc::SCM_RIGHTS, &raw.to_ne_bytes());
        assert!(parse_control(&control, length, &peer, 1).is_err());
        assert_closed(raw);
    }

    #[test]
    fn wrong_credentials_still_closes_later_rights() {
        let mut peer = peer();
        let mut control = [0; 32];
        let offset = credentials(&mut control, &peer);
        peer.pid += 1;
        let raw = peer.pidfd.try_clone().unwrap().into_raw_fd();
        let length = append(&mut control, offset, libc::SCM_RIGHTS, &raw.to_ne_bytes());
        assert!(parse_control(&control, length, &peer, 1).is_err());
        assert_closed(raw);
    }

    #[test]
    fn truncated_control_closes_every_visible_reference() {
        let peer = peer();
        let mut control = [0; 32];
        let offset = credentials(&mut control, &peer);
        let raw = peer.pidfd.try_clone().unwrap().into_raw_fd();
        let length = append(&mut control, offset, libc::SCM_RIGHTS, &raw.to_ne_bytes());
        // SAFETY: offset points to the initialized rights header.
        unsafe {
            (*control
                .as_mut_ptr()
                .cast::<u8>()
                .add(offset)
                .cast::<libc::cmsghdr>())
            .cmsg_len += 100;
        }
        let visible = length - (size_of::<usize>() - size_of::<i32>());
        assert!(parse_control(&control, visible, &peer, 1).is_err());
        assert_closed(raw);
    }

    #[test]
    fn unknown_control_does_not_skip_later_rights() {
        let peer = peer();
        let mut control = [0; 32];
        let offset = credentials(&mut control, &peer);
        let offset = append(&mut control, offset, 999, &[1, 2, 3, 4]);
        let raw = peer.pidfd.try_clone().unwrap().into_raw_fd();
        let length = append(&mut control, offset, libc::SCM_RIGHTS, &raw.to_ne_bytes());
        assert!(parse_control(&control, length, &peer, 1).is_err());
        assert_closed(raw);
    }

    #[test]
    fn ancillary_pidfd_is_rejected_and_closed() {
        let peer = peer();
        let mut control = [0; 32];
        let offset = credentials(&mut control, &peer);
        let raw = peer.pidfd.try_clone().unwrap().into_raw_fd();
        let length = append(&mut control, offset, SCM_PIDFD, &raw.to_ne_bytes());
        assert!(parse_control(&control, length, &peer, 1).is_err());
        assert_closed(raw);
    }

    #[test]
    fn regular_file_is_not_a_process_reference() {
        let reference: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        assert!(validate_reference(&reference).is_err());
        validate_reference(&peer().pidfd).unwrap();
    }

    #[test]
    fn kernel_truncated_rights_are_closed_before_rejection() {
        let peer = peer();
        let (sender, receiver) = super::super::packet_pair().unwrap();
        let mut control = [0usize; 64];
        let mut byte = 1u8;
        let mut iov = libc::iovec {
            iov_base: (&mut byte as *mut u8).cast(),
            iov_len: 1,
        };
        // SAFETY: the complete message and aligned control storage are initialized below.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        // SAFETY: the control storage fits eighty descriptor words plus the header.
        unsafe {
            message.msg_controllen = libc::CMSG_SPACE(80 * size_of::<i32>() as u32) as usize;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(80 * size_of::<i32>() as u32) as usize;
            for index in 0..80 {
                libc::CMSG_DATA(header)
                    .cast::<i32>()
                    .add(index)
                    .write_unaligned(sender.as_raw_fd());
            }
            assert_eq!(
                libc::sendmsg(sender.as_raw_fd(), &message, libc::MSG_NOSIGNAL),
                1
            );
        }
        let mut received = [0usize; 32];
        message.msg_control = received.as_mut_ptr().cast();
        message.msg_controllen = size_of_val(&received);
        // SAFETY: the message points to the writable byte and bounded received ancillary storage.
        assert_eq!(
            unsafe { libc::recvmsg(receiver.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) },
            1
        );
        assert_ne!(message.msg_flags & libc::MSG_CTRUNC, 0);
        let mut raw = Vec::new();
        // SAFETY: kernel returned valid ancillary headers inside initialized storage.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                if (*header).cmsg_type == libc::SCM_RIGHTS {
                    let count =
                        ((*header).cmsg_len - libc::CMSG_LEN(0) as usize) / size_of::<i32>();
                    for index in 0..count {
                        raw.push(
                            libc::CMSG_DATA(header)
                                .cast::<i32>()
                                .add(index)
                                .read_unaligned(),
                        );
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        assert!(raw.len() > 2 && raw.len() < 80);
        assert!(parse_control(&received, message.msg_controllen, &peer, 2).is_err());
        for descriptor in raw {
            assert_closed(descriptor);
        }
    }
}
