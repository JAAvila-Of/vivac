use super::linux_session::PackageLease;
use super::linux_spawn::SpawnOwner;
use std::fs::File;
use std::io;
use std::mem::{size_of, size_of_val, zeroed, MaybeUninit};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Instant;

#[path = "linux_ipc/keeper_codec.rs"]
pub mod keeper_codec;
#[path = "linux_ipc/registration.rs"]
pub mod registration;

const WIRE_SIZE: usize = 112;
// Linux UAPI ancillary type; older libc versions do not expose this constant.
const SCM_PIDFD: libc::c_int = 0x04;
const SO_PEERPIDFD: libc::c_int = 77;

pub(super) fn require_procfs(file: &File) -> io::Result<()> {
    let mut filesystem = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs writes a full statfs structure for the owned descriptor.
    if unsafe { libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the preceding fstatfs call succeeded. 0x9fa0 is PROC_SUPER_MAGIC.
    if unsafe { filesystem.assume_init() }.f_type != 0x9fa0 {
        return Err(invalid());
    }
    Ok(())
}
fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}
fn denied() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}
fn deadline_check(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::from(io::ErrorKind::TimedOut))
    } else {
        Ok(())
    }
}
fn stable_ids() -> io::Result<(u32, u32)> {
    // SAFETY: these calls only query the calling process credentials.
    let (uid, euid, gid, egid) = unsafe {
        (
            libc::getuid(),
            libc::geteuid(),
            libc::getgid(),
            libc::getegid(),
        )
    };
    if uid != euid || gid != egid {
        return Err(denied());
    }
    Ok((uid, gid))
}

pub fn packet_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut raw = [-1; 2];
    // SAFETY: raw has room for the two returned descriptors.
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            raw.as_mut_ptr(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful socketpair transfers both fresh descriptors.
    let pair = unsafe { (OwnedFd::from_raw_fd(raw[0]), OwnedFd::from_raw_fd(raw[1])) };
    for fd in [&pair.0, &pair.1] {
        let enabled: libc::c_int = 1;
        // SAFETY: enabled is a correctly sized socket option value.
        if unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                (&enabled as *const libc::c_int).cast(),
                size_of::<libc::c_int>() as _,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(pair)
}

pub struct ExpectedPeer {
    pidfd: OwnedFd,
    pid: i32,
    uid: u32,
    gid: u32,
}
impl ExpectedPeer {
    pub fn connected_identity(socket: &OwnedFd) -> io::Result<Self> {
        let (uid, gid) = stable_ids()?;
        let raw = socket.as_raw_fd();
        require_cloexec(raw)?;
        if socket_option(raw, libc::SO_DOMAIN)? != libc::AF_UNIX
            || socket_option(raw, libc::SO_TYPE)? != libc::SOCK_SEQPACKET
            || socket_option(raw, libc::SO_PASSCRED)? != 1
        {
            return Err(invalid());
        }
        // SAFETY: zeroed sockaddr_storage is writable address storage.
        let mut address: libc::sockaddr_storage = unsafe { zeroed() };
        let mut length = size_of_val(&address) as libc::socklen_t;
        // SAFETY: address and length describe the complete writable storage.
        if unsafe {
            libc::getpeername(
                raw,
                (&mut address as *mut libc::sockaddr_storage).cast(),
                &mut length,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        if (length as usize) < size_of::<libc::sa_family_t>()
            || length as usize > size_of_val(&address)
            || i32::from(address.ss_family) != libc::AF_UNIX
        {
            return Err(invalid());
        }
        let before = connected_credentials(raw)?;
        // SAFETY: getpid only queries the current process identity.
        if before.pid <= 1
            || before.pid == unsafe { libc::getpid() }
            || before.uid != uid
            || before.gid != gid
        {
            return Err(denied());
        }
        let pidfd = connected_pidfd(raw)?;
        let after = connected_credentials(raw)?;
        if (before.pid, before.uid, before.gid) != (after.pid, after.uid, after.gid) {
            return Err(denied());
        }
        let peer = Self {
            pidfd,
            pid: before.pid,
            uid,
            gid,
        };
        peer.alive()?;
        Ok(peer)
    }

    pub fn owned_child(owner: &SpawnOwner, token: u64) -> io::Result<Self> {
        let (uid, gid) = stable_ids()?;
        let receipt = owner.receipt(token)?;
        let peer = Self {
            pidfd: owner.duplicate_pidfd(token)?,
            pid: i32::try_from(receipt.pid).map_err(|_| denied())?,
            uid,
            gid,
        };
        if peer.pid <= 0 {
            return Err(denied());
        }
        peer.alive()?;
        Ok(peer)
    }
    pub fn parent() -> io::Result<Self> {
        let (uid, gid) = stable_ids()?;
        // SAFETY: getppid queries the kernel parent relationship.
        let pid = unsafe { libc::getppid() };
        if pid <= 1 {
            return Err(denied());
        }
        // SAFETY: pidfd_open takes scalar arguments and returns a fresh descriptor.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful pidfd_open transfers a fresh descriptor.
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        // SAFETY: getppid queries the kernel parent relationship again.
        if unsafe { libc::getppid() } != pid {
            return Err(denied());
        }
        let peer = Self {
            pidfd,
            pid,
            uid,
            gid,
        };
        peer.alive()?;
        Ok(peer)
    }
    fn alive(&self) -> io::Result<()> {
        if stable_ids()? != (self.uid, self.gid) {
            return Err(denied());
        }
        let mut pfd = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd is one valid poll descriptor.
        let result = unsafe { libc::poll(&mut pfd, 1, 0) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if result == 0 && pfd.revents == 0 {
            Ok(())
        } else {
            Err(denied())
        }
    }
}

/// Retains the historical identities established by a connected socket.
/// Message senders still require independent SCM_CREDENTIALS validation.
pub struct ConnectedPeerContext {
    socket: OwnedFd,
    peer: ExpectedPeer,
    local: [u32; 3],
    socket_identity: (libc::dev_t, libc::ino_t),
    refused: bool,
}
impl ConnectedPeerContext {
    pub fn capture(socket: &OwnedFd) -> io::Result<Self> {
        require_cloexec(socket.as_raw_fd())?;
        // SAFETY: F_DUPFD_CLOEXEC returns a fresh descriptor at or above three.
        let raw = unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful duplication transfers a fresh owned descriptor.
        let socket = unsafe { OwnedFd::from_raw_fd(raw) };
        let (uid, gid) = stable_ids()?;
        if uid == 0 || gid == 0 {
            return Err(denied());
        }
        // SAFETY: getpid only queries this process's identity.
        let pid = unsafe { libc::getpid() };
        if pid <= 1 {
            return Err(denied());
        }
        let peer = ExpectedPeer::connected_identity(&socket)?;
        let socket_identity = Self::socket_identity(socket.as_raw_fd())?;
        let mut context = Self {
            socket,
            peer,
            local: [pid as u32, uid, gid],
            socket_identity,
            refused: false,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn validate(&mut self) -> io::Result<()> {
        if self.refused {
            return Err(denied());
        }
        let result = self.check();
        if result.is_err() {
            self.refused = true;
        }
        result
    }

    pub fn identities(&mut self) -> io::Result<([u32; 3], [u32; 3])> {
        self.validate()?;
        let identities = (
            self.local,
            [self.peer.pid as u32, self.peer.uid, self.peer.gid],
        );
        self.validate()?;
        Ok(identities)
    }

    fn check_local(&self) -> io::Result<()> {
        let (uid, gid) = stable_ids()?;
        // SAFETY: getpid only queries this process's current identity.
        if [unsafe { libc::getpid() } as u32, uid, gid] != self.local || uid == 0 || gid == 0 {
            return Err(denied());
        }
        Ok(())
    }

    fn check_credentials(&self) -> io::Result<()> {
        let credentials = connected_credentials(self.socket.as_raw_fd())?;
        if (credentials.pid, credentials.uid, credentials.gid)
            != (self.peer.pid, self.peer.uid, self.peer.gid)
        {
            return Err(denied());
        }
        Ok(())
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

    fn check(&self) -> io::Result<()> {
        self.check_local()?;
        self.check_credentials()?;
        let raw = self.socket.as_raw_fd();
        require_cloexec(raw)?;
        if Self::socket_identity(raw)? != self.socket_identity
            || socket_option(raw, libc::SO_DOMAIN)? != libc::AF_UNIX
            || socket_option(raw, libc::SO_TYPE)? != libc::SOCK_SEQPACKET
            || socket_option(raw, libc::SO_PASSCRED)? != 1
        {
            return Err(invalid());
        }
        self.peer.alive()?;
        let mut socket = libc::pollfd {
            fd: raw,
            events: 0,
            revents: 0,
        };
        // SAFETY: socket is one initialized writable poll descriptor.
        if unsafe { libc::poll(&mut socket, 1, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if socket.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(denied());
        }
        self.check_credentials()?;
        self.peer.alive()?;
        self.check_local()
    }
}

fn require_cloexec(fd: i32) -> io::Result<()> {
    if fd < 3 {
        return Err(invalid());
    }
    // SAFETY: F_GETFD only queries descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::FD_CLOEXEC == 0 {
        return Err(invalid());
    }
    Ok(())
}

fn connected_credentials(socket: i32) -> io::Result<libc::ucred> {
    // SAFETY: zero initializes all fields of the credentials storage.
    let mut credentials: libc::ucred = unsafe { zeroed() };
    let mut length = size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: credentials and length provide correctly sized writable storage.
    if unsafe {
        libc::getsockopt(
            socket,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if length as usize != size_of_val(&credentials) {
        return Err(invalid());
    }
    Ok(credentials)
}

fn connected_pidfd(socket: i32) -> io::Result<OwnedFd> {
    let mut raw: i32 = -1;
    let mut length = size_of_val(&raw) as libc::socklen_t;
    // SAFETY: raw and length provide correctly sized writable option storage.
    if unsafe {
        libc::getsockopt(
            socket,
            libc::SOL_SOCKET,
            SO_PEERPIDFD,
            (&mut raw as *mut i32).cast(),
            &mut length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if raw < 0 {
        return Err(invalid());
    }
    // SAFETY: successful SO_PEERPIDFD returned a fresh descriptor. Own it before
    // checking lengths or flags so every subsequent error closes it.
    let pidfd = unsafe { OwnedFd::from_raw_fd(raw) };
    if length as usize != size_of::<i32>() {
        return Err(invalid());
    }
    require_cloexec(pidfd.as_raw_fd())?;
    Ok(pidfd)
}

#[derive(Clone, Copy)]
pub struct StoreIdentity([u64; 6]);
impl StoreIdentity {
    pub fn from_lease(lease: &PackageLease) -> Result<Self, super::linux_gate::Error> {
        lease.ipc_identity().map(Self)
    }
    pub fn words(self) -> [u64; 6] {
        self.0
    }
}
#[derive(Clone, Copy)]
pub struct Generation([u8; 32]);
impl Generation {
    pub fn from_bootstrap(bytes: [u8; 32]) -> io::Result<Self> {
        if bytes == [0; 32] {
            Err(invalid())
        } else {
            Ok(Self(bytes))
        }
    }
    pub fn bytes(self) -> [u8; 32] {
        self.0
    }
    pub fn fresh(deadline: Instant) -> io::Result<Self> {
        let mut bytes = [0; 32];
        let mut offset = 0;
        while offset < bytes.len() {
            deadline_check(deadline)?;
            // SAFETY: the remaining array region is writable for the supplied length.
            let count = unsafe {
                libc::getrandom(
                    bytes[offset..].as_mut_ptr().cast(),
                    bytes.len() - offset,
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
            if count == 0 {
                return Err(invalid());
            }
            offset += count as usize;
        }
        deadline_check(deadline)?;
        Self::from_bootstrap(bytes)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    Initiator = 1,
    Responder = 2,
}
#[derive(Clone, Copy)]
#[repr(u8)]
enum Kind {
    Hello = 1,
    Probe = 2,
    Reply = 3,
}
struct Frame {
    role: Role,
    kind: Kind,
    store: StoreIdentity,
    generation: Generation,
    sequence: u64,
    value: u64,
}
impl Frame {
    fn encode(&self) -> [u8; WIRE_SIZE] {
        let mut wire = [0; WIRE_SIZE];
        wire[..8].copy_from_slice(b"VVIPC01\0");
        wire[8] = 1;
        wire[9] = self.role as u8;
        wire[10] = self.kind as u8;
        for (slot, word) in wire[16..64]
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .zip(self.store.0)
        {
            slot.copy_from_slice(&word.to_be_bytes());
        }
        wire[64..96].copy_from_slice(&self.generation.0);
        wire[96..104].copy_from_slice(&self.sequence.to_be_bytes());
        wire[104..112].copy_from_slice(&self.value.to_be_bytes());
        wire
    }
    fn decode(
        wire: &[u8; WIRE_SIZE],
        role: Role,
        store: &StoreIdentity,
        generation: &Generation,
        kind: Kind,
        sequence: u64,
    ) -> io::Result<u64> {
        let opposite = match role {
            Role::Initiator => Role::Responder,
            Role::Responder => Role::Initiator,
        };
        let expected = Self {
            role: opposite,
            kind,
            store: *store,
            generation: *generation,
            sequence,
            value: 0,
        }
        .encode();
        if wire[..104] != expected[..104] {
            return Err(invalid());
        }
        Ok(u64::from_be_bytes(
            wire[104..112].try_into().map_err(|_| invalid())?,
        ))
    }
}

fn socket_option(fd: i32, option: i32) -> io::Result<i32> {
    let mut value: i32 = 0;
    let mut length = size_of::<i32>() as libc::socklen_t;
    // SAFETY: value and length are correctly sized writable option storage.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            (&mut value as *mut i32).cast(),
            &mut length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if length as usize != size_of::<i32>() {
        return Err(invalid());
    }
    Ok(value)
}

pub struct Channel {
    fd: OwnedFd,
    peer: ExpectedPeer,
    store: StoreIdentity,
    generation: Generation,
    role: Role,
    authenticated: bool,
    poisoned: bool,
    sent: u64,
    received: u64,
}
impl Channel {
    pub fn new(
        fd: OwnedFd,
        peer: ExpectedPeer,
        store: StoreIdentity,
        generation: Generation,
        role: Role,
    ) -> io::Result<Self> {
        let raw = fd.as_raw_fd();
        // SAFETY: F_GETFD queries the owned descriptor.
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if raw < 3
            || flags & libc::FD_CLOEXEC == 0
            || socket_option(raw, libc::SO_DOMAIN)? != libc::AF_UNIX
            || socket_option(raw, libc::SO_TYPE)? != libc::SOCK_SEQPACKET
            || socket_option(raw, libc::SO_PASSCRED)? != 1
        {
            return Err(invalid());
        }
        peer.alive()?;
        Ok(Self {
            fd,
            peer,
            store,
            generation,
            role,
            authenticated: false,
            poisoned: false,
            sent: 1,
            received: 1,
        })
    }
    pub fn poisoned(&self) -> bool {
        self.poisoned
    }
    fn operation<T>(&mut self, action: impl FnOnce(&mut Self) -> io::Result<T>) -> io::Result<T> {
        if self.poisoned {
            return Err(denied());
        }
        let result = action(self);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn handshake(&mut self, deadline: Instant) -> io::Result<()> {
        self.operation(|this| {
            if this.authenticated {
                return Err(denied());
            }
            this.send(Kind::Hello, 0, 0, deadline)?;
            if this.receive(Kind::Hello, 0, deadline)? != 0 {
                return Err(invalid());
            }
            this.authenticated = true;
            Ok(())
        })
    }
    pub fn probe(&mut self, value: u64, deadline: Instant) -> io::Result<u64> {
        self.operation(|this| {
            if !this.authenticated || this.role != Role::Initiator {
                return Err(denied());
            }
            this.send_next(Kind::Probe, value, deadline)?;
            let reply = this.receive_next(Kind::Reply, deadline)?;
            if reply != value {
                return Err(invalid());
            }
            Ok(reply)
        })
    }
    pub fn respond_probe(&mut self, deadline: Instant) -> io::Result<u64> {
        self.operation(|this| {
            if !this.authenticated || this.role != Role::Responder {
                return Err(denied());
            }
            let value = this.receive_next(Kind::Probe, deadline)?;
            this.send_next(Kind::Reply, value, deadline)?;
            Ok(value)
        })
    }
    fn send_next(&mut self, kind: Kind, value: u64, deadline: Instant) -> io::Result<()> {
        let sequence = self.sent;
        self.sent = self.sent.checked_add(1).ok_or_else(invalid)?;
        self.send(kind, sequence, value, deadline)
    }
    fn receive_next(&mut self, kind: Kind, deadline: Instant) -> io::Result<u64> {
        let sequence = self.received;
        self.received = self.received.checked_add(1).ok_or_else(invalid)?;
        self.receive(kind, sequence, deadline)
    }
    fn check_live(&self, deadline: Instant) -> io::Result<()> {
        loop {
            deadline_check(deadline)?;
            self.peer.alive()?;
            let mut socket = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: 0,
                revents: 0,
            };
            // SAFETY: socket is one valid initialized poll descriptor.
            if unsafe { libc::poll(&mut socket, 1, 0) } < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if socket.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                return Err(denied());
            }
            return deadline_check(deadline);
        }
    }
    fn wait(&self, events: i16, deadline: Instant) -> io::Result<()> {
        loop {
            deadline_check(deadline)?;
            self.peer.alive()?;
            let millis = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let mut fds = [
                libc::pollfd {
                    fd: self.fd.as_raw_fd(),
                    events,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.peer.pidfd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: fds contains two initialized poll descriptors.
            let count = unsafe { libc::poll(fds.as_mut_ptr(), 2, millis) };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            deadline_check(deadline)?;
            self.peer.alive()?;
            if fds[1].revents != 0
                || fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
            {
                return Err(denied());
            }
            if fds[0].revents & events != 0 {
                return Ok(());
            }
        }
    }
    fn send(&self, kind: Kind, sequence: u64, value: u64, deadline: Instant) -> io::Result<()> {
        let wire = Frame {
            role: self.role,
            kind,
            store: self.store,
            generation: self.generation,
            sequence,
            value,
        }
        .encode();
        loop {
            self.check_live(deadline)?;
            // SAFETY: wire is a readable fixed frame; MSG_NOSIGNAL suppresses process signals.
            let count = unsafe {
                libc::send(
                    self.fd.as_raw_fd(),
                    wire.as_ptr().cast(),
                    wire.len(),
                    libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if error.kind() == io::ErrorKind::WouldBlock {
                    self.wait(libc::POLLOUT, deadline)?;
                    continue;
                }
                return Err(error);
            }
            self.check_live(deadline)?;
            if count as usize != wire.len() {
                return Err(invalid());
            }
            return Ok(());
        }
    }
    fn receive(&self, kind: Kind, sequence: u64, deadline: Instant) -> io::Result<u64> {
        loop {
            self.check_live(deadline)?;
            let mut wire = [0u8; WIRE_SIZE];
            let mut control = [0usize; 32];
            let mut iov = libc::iovec {
                iov_base: wire.as_mut_ptr().cast(),
                iov_len: wire.len(),
            };
            // SAFETY: zero is a valid initial msghdr; all pointer fields are assigned below.
            let mut message: libc::msghdr = unsafe { zeroed() };
            message.msg_iov = &mut iov;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = size_of_val(&control);
            // SAFETY: all message buffers are valid writable storage for the supplied lengths.
            let count = unsafe {
                libc::recvmsg(
                    self.fd.as_raw_fd(),
                    &mut message,
                    libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC | libc::MSG_TRUNC,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if error.kind() == io::ErrorKind::WouldBlock {
                    self.wait(libc::POLLIN, deadline)?;
                    continue;
                }
                return Err(error);
            }
            let ancillary = parse_control(&control, message.msg_controllen, &self.peer);
            self.check_live(deadline)?;
            ancillary?;
            if message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0
                || count as usize != WIRE_SIZE
            {
                return Err(invalid());
            }
            return Frame::decode(
                &wire,
                self.role,
                &self.store,
                &self.generation,
                kind,
                sequence,
            );
        }
    }
}

fn parse_control(control: &[usize; 32], length: usize, peer: &ExpectedPeer) -> io::Result<()> {
    let capacity = size_of_val(control);
    let length = length.min(capacity);
    let base = control.as_ptr().cast::<u8>();
    let alignment = size_of::<usize>();
    let header_size = size_of::<libc::cmsghdr>();
    let data_offset = (header_size + alignment - 1) & !(alignment - 1);
    let mut offset = 0usize;
    let mut credentials = 0;
    let mut rejected = false;
    while length.saturating_sub(offset) >= header_size {
        // SAFETY: a complete header lies inside initialized aligned control storage.
        let header = unsafe { std::ptr::read_unaligned(base.add(offset).cast::<libc::cmsghdr>()) };
        let size = header.cmsg_len;
        if size < data_offset {
            rejected = true;
            break;
        }
        let visible = size.min(length - offset);
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            rejected = true;
            let mut position = data_offset;
            while position + size_of::<i32>() <= visible {
                // SAFETY: the complete descriptor word lies inside the returned control bytes.
                let fd =
                    unsafe { std::ptr::read_unaligned(base.add(offset + position).cast::<i32>()) };
                if fd >= 0 {
                    // SAFETY: SCM_RIGHTS transferred ownership of each visible descriptor.
                    unsafe {
                        libc::close(fd);
                    }
                }
                position += size_of::<i32>();
            }
        } else if header.cmsg_level == libc::SOL_SOCKET
            && header.cmsg_type == SCM_PIDFD
            && size == data_offset + size_of::<libc::c_int>()
            && size <= length - offset
        {
            rejected = true;
            // SAFETY: the complete descriptor word lies inside the returned control bytes.
            let fd = unsafe {
                std::ptr::read_unaligned(base.add(offset + data_offset).cast::<libc::c_int>())
            };
            if fd >= 0 {
                // SAFETY: SCM_PIDFD transferred ownership of this visible descriptor.
                unsafe {
                    libc::close(fd);
                }
            }
        } else if header.cmsg_level == libc::SOL_SOCKET
            && header.cmsg_type == libc::SCM_CREDENTIALS
            && size == data_offset + size_of::<libc::ucred>()
            && size <= length - offset
        {
            credentials += 1;
            // SAFETY: the complete credential record lies inside the returned control bytes.
            let credential = unsafe {
                std::ptr::read_unaligned(base.add(offset + data_offset).cast::<libc::ucred>())
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
            .map(|n| n & !(alignment - 1))
            .and_then(|n| offset.checked_add(n))
        else {
            rejected = true;
            break;
        };
        offset = next;
    }
    if rejected || credentials != 1 {
        Err(denied())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame {
            role: Role::Initiator,
            kind: Kind::Probe,
            store: StoreIdentity([1, 2, 3, 4, 5, 6]),
            generation: Generation([7; 32]),
            sequence: 1,
            value: 42,
        }
    }
    #[test]
    fn wire_rejects_reserved_role_kind_and_binding_changes() {
        let frame = frame();
        let wire = frame.encode();
        for offset in [0, 8, 9, 10, 11, 16, 64, 96] {
            let mut changed = wire;
            changed[offset] ^= 0xff;
            assert!(Frame::decode(
                &changed,
                Role::Responder,
                &frame.store,
                &frame.generation,
                Kind::Probe,
                1
            )
            .is_err());
        }
        assert_eq!(
            Frame::decode(
                &wire,
                Role::Responder,
                &frame.store,
                &frame.generation,
                Kind::Probe,
                1
            )
            .unwrap(),
            42
        );
        assert!(Frame::decode(
            &wire,
            Role::Initiator,
            &frame.store,
            &frame.generation,
            Kind::Probe,
            1
        )
        .is_err());
        assert!(Frame::decode(
            &wire,
            Role::Responder,
            &frame.store,
            &frame.generation,
            Kind::Probe,
            2
        )
        .is_err());
        assert!(Frame::decode(
            &wire,
            Role::Responder,
            &frame.store,
            &frame.generation,
            Kind::Reply,
            1
        )
        .is_err());
    }
    #[test]
    fn bootstrap_refuses_zero_generation() {
        assert!(Generation::from_bootstrap([0; 32]).is_err());
    }
    fn self_peer() -> ExpectedPeer {
        let (uid, gid) = stable_ids().unwrap();
        // SAFETY: getpid queries the caller; pidfd_open returns a fresh descriptor.
        let pid = unsafe { libc::getpid() };
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
        assert!(raw >= 0);
        // SAFETY: successful pidfd_open transfers descriptor ownership.
        ExpectedPeer {
            pidfd: unsafe { OwnedFd::from_raw_fd(raw as i32) },
            pid,
            uid,
            gid,
        }
    }
    #[test]
    fn connected_identity_rejects_stream_and_non_unix_sockets() {
        let (stream, _other) = std::os::unix::net::UnixStream::pair().unwrap();
        let stream: OwnedFd = stream.into();
        assert!(ExpectedPeer::connected_identity(&stream).is_err());
        // SAFETY: socket takes scalar arguments and returns a fresh descriptor.
        let raw = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
        assert!(raw >= 0);
        // SAFETY: successful socket transfers descriptor ownership.
        let datagram = unsafe { OwnedFd::from_raw_fd(raw) };
        assert!(ExpectedPeer::connected_identity(&datagram).is_err());
    }

    #[test]
    fn connected_identity_requires_a_connected_socket_and_another_process() {
        // SAFETY: socket takes scalar arguments and returns a fresh descriptor.
        let raw =
            unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
        assert!(raw >= 0);
        // SAFETY: successful socket transfers descriptor ownership.
        let unconnected = unsafe { OwnedFd::from_raw_fd(raw) };
        let enabled: i32 = 1;
        // SAFETY: enabled is correctly sized option storage.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    unconnected.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PASSCRED,
                    (&enabled as *const i32).cast(),
                    size_of::<i32>() as _,
                )
            },
            0
        );
        assert_eq!(
            ExpectedPeer::connected_identity(&unconnected)
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::ENOTCONN)
        );
        let (own, _other) = packet_pair().unwrap();
        assert_eq!(
            ExpectedPeer::connected_identity(&own).err().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn unauthenticated_operation_poison_is_irreversible() {
        let (fd, _other) = packet_pair().unwrap();
        let sample = frame();
        let mut channel = Channel::new(
            fd,
            self_peer(),
            sample.store,
            sample.generation,
            Role::Initiator,
        )
        .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        assert_eq!(
            channel.probe(42, deadline).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(channel.poisoned());
        assert_eq!(
            channel.handshake(deadline).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    #[test]
    fn rejected_pidfd_ancillary_closes_the_visible_descriptor() {
        use std::os::fd::IntoRawFd;

        let peer = self_peer();
        let fd = peer.pidfd.try_clone().unwrap().into_raw_fd();
        let mut control = [0usize; 32];
        // SAFETY: zero initializes optional fields; control is aligned ancillary storage.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_control = control.as_mut_ptr().cast();
        // SAFETY: CMSG_SPACE computes the size for one descriptor word.
        message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<i32>() as _) } as usize;
        // SAFETY: the header and one descriptor word fit in aligned control storage.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = 0x04;
            (*header).cmsg_len = libc::CMSG_LEN(size_of::<i32>() as _) as usize;
            libc::CMSG_DATA(header).cast::<i32>().write(fd);
        }
        assert!(parse_control(&control, message.msg_controllen, &peer).is_err());
        // SAFETY: F_GETFD queries whether the transferred descriptor remains open.
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
    }
    #[test]
    fn truncated_rights_are_closed_before_rejection() {
        let (sender, receiver) = packet_pair().unwrap();
        let peer = self_peer();
        let mut outbound = [0usize; 48];
        let mut byte = [1u8];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        // SAFETY: zero initializes all optional message fields.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = outbound.as_mut_ptr().cast();
        // SAFETY: CMSG_SPACE computes the buffer size for 80 descriptor words.
        message.msg_controllen = unsafe { libc::CMSG_SPACE((80 * size_of::<i32>()) as _) } as usize;
        assert!(message.msg_controllen <= size_of_val(&outbound));
        // SAFETY: aligned outbound storage has sufficient room for the header and 80 words.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN((80 * size_of::<i32>()) as _) as usize;
            let data = libc::CMSG_DATA(header).cast::<i32>();
            for index in 0..80 {
                data.add(index).write(sender.as_raw_fd());
            }
            assert_eq!(
                libc::sendmsg(sender.as_raw_fd(), &message, libc::MSG_NOSIGNAL),
                1
            );
        }
        let mut inbound = [0usize; 32];
        message.msg_control = inbound.as_mut_ptr().cast();
        message.msg_controllen = size_of_val(&inbound);
        // SAFETY: all receive buffers are writable and live.
        assert_eq!(
            unsafe { libc::recvmsg(receiver.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) },
            1
        );
        assert_ne!(message.msg_flags & libc::MSG_CTRUNC, 0);
        let mut visible = [-1; 80];
        let mut count = 0;
        // SAFETY: walk the kernel-returned complete ancillary headers and descriptor words.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    let words =
                        ((*header).cmsg_len - libc::CMSG_LEN(0) as usize) / size_of::<i32>();
                    for index in 0..words {
                        visible[count] = libc::CMSG_DATA(header).cast::<i32>().add(index).read();
                        count += 1;
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        assert!(count > 0);
        assert!(parse_control(&inbound, message.msg_controllen, &peer).is_err());
        for fd in &visible[..count] {
            // SAFETY: F_GETFD queries whether the received descriptor remains open.
            assert_eq!(unsafe { libc::fcntl(*fd, libc::F_GETFD) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
        }
    }
}
