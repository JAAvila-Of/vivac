use crate::linux_ipc::registration::EndpointProofConnection;
use crate::linux_ipc::{
    Channel, ConnectedPeerContext, ExpectedPeer, Generation, Role, StoreIdentity,
};
use crate::linux_session::PackageLease;
use crate::linux_spawn::{ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}
fn options(
    arguments: impl IntoIterator<Item = OsString>,
    names: &[&str],
) -> io::Result<BTreeMap<String, OsString>> {
    let mut arguments = arguments.into_iter();
    let mut result = BTreeMap::new();
    while let Some(name) = arguments.next() {
        let name = name.into_string().map_err(|_| baseline::refused())?;
        let value = arguments.next().ok_or_else(baseline::refused)?;
        baseline::require(names.contains(&name.as_str()) && !result.contains_key(&name))?;
        result.insert(name, value);
    }
    baseline::require(result.len() == names.len())?;
    Ok(result)
}
fn descriptors(options: &BTreeMap<String, OsString>, names: &[&str]) -> io::Result<Vec<RawFd>> {
    let mut result = Vec::new();
    for name in names {
        let fd = options
            .get(*name)
            .and_then(|value| value.to_str())
            .ok_or_else(baseline::refused)?
            .parse::<RawFd>()
            .map_err(|_| baseline::refused())?;
        baseline::require(fd >= 3 && !result.contains(&fd))?;
        result.push(fd);
    }
    Ok(result)
}
fn unhex(value: &OsString, size: usize) -> io::Result<Vec<u8>> {
    let text = value.to_str().ok_or_else(baseline::refused)?;
    baseline::require(text.len() == size * 2)?;
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |value| match value {
                b'0'..=b'9' => Ok(value - b'0'),
                b'a'..=b'f' => Ok(value - b'a' + 10),
                _ => Err(baseline::refused()),
            };
            Ok(digit(pair[0])? * 16 + digit(pair[1])?)
        })
        .collect()
}
fn frame(store: [u64; 6], generation: [u8; 32], sequence: u64, value: u64) -> [u8; 112] {
    let mut wire = [0u8; 112];
    wire[..8].copy_from_slice(b"VVIPC01\0");
    wire[8] = 1;
    wire[9] = 1;
    wire[10] = 2;
    for (index, word) in store.into_iter().enumerate() {
        wire[16 + index * 8..24 + index * 8].copy_from_slice(&word.to_be_bytes());
    }
    wire[64..96].copy_from_slice(&generation);
    wire[96..104].copy_from_slice(&sequence.to_be_bytes());
    wire[104..112].copy_from_slice(&value.to_be_bytes());
    wire
}
struct Control(OwnedFd);
impl Control {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        let timeout = libc::timeval {
            tv_sec: 5,
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: setsockopt reads one initialized timeval on the owned control socket.
            if unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    option,
                    (&timeout as *const libc::timeval).cast(),
                    size_of::<libc::timeval>() as libc::socklen_t,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self(fd))
    }
    fn pair() -> io::Result<(Self, OwnedFd)> {
        let mut fds = [-1; 2];
        // SAFETY: socketpair writes exactly two fresh descriptors into the initialized array.
        if unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful socketpair transfers both fresh descriptors.
        let [left, right] = unsafe { fds.map(|fd| OwnedFd::from_raw_fd(fd)) };
        Ok((Self::new(left)?, right))
    }
    fn send(&self, value: &Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|_| baseline::refused())?;
        baseline::require(bytes.len() <= 8192)?;
        raw_send(&self.0, &bytes, None)
    }
    fn receive(&self, stage: &str, pid: u32) -> io::Result<Value> {
        let mut bytes = [0u8; 8193];
        // SAFETY: recv writes only the fixed writable buffer; MSG_TRUNC reports oversized packets.
        let size = unsafe {
            libc::recv(
                self.0.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_TRUNC,
            )
        };
        baseline::require(size > 0 && size <= 8192)?;
        let value: Value =
            serde_json::from_slice(&bytes[..size as usize]).map_err(|_| baseline::refused())?;
        baseline::require(value.is_object() && value["stage"] == stage && value["pid"] == pid)?;
        Ok(value)
    }
    fn go(&self) -> io::Result<()> {
        raw_send(&self.0, b"G", None)
    }
    fn wait_go(&self) -> io::Result<()> {
        let mut bytes = [0u8; 2];
        // SAFETY: recv writes only the fixed command buffer and preserves packet length.
        let size = unsafe {
            libc::recv(
                self.0.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_TRUNC,
            )
        };
        baseline::require(size == 1 && bytes[0] == b'G')
    }
}
fn raw_send(fd: &OwnedFd, bytes: &[u8], right: Option<&OwnedFd>) -> io::Result<()> {
    let count = if let Some(right) = right {
        let mut vector = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let mut ancillary = [0usize; 4];
        // SAFETY: zero is a valid empty msghdr before its fields are initialized.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = ancillary.as_mut_ptr().cast();
        // SAFETY: CMSG_SPACE computes storage for one RawFd.
        message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as usize;
        baseline::require(message.msg_controllen <= size_of::<[usize; 4]>())?;
        // SAFETY: ancillary is aligned and large enough for the header and one descriptor.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            baseline::require(!header.is_null())?;
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as usize;
            std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<RawFd>(), right.as_raw_fd());
            libc::sendmsg(fd.as_raw_fd(), &message, libc::MSG_NOSIGNAL)
        }
    } else {
        // SAFETY: the initialized byte packet stays readable during send.
        unsafe {
            libc::send(
                fd.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_NOSIGNAL,
            )
        }
    };
    baseline::require(count == bytes.len() as isize)
}
fn null() -> io::Result<OwnedFd> {
    Ok(File::options()
        .read(true)
        .write(true)
        .open("/dev/null")?
        .into())
}
fn spawn(
    owner: &mut SpawnOwner,
    arguments: Vec<OsString>,
    inherited: Vec<OwnedFd>,
) -> io::Result<Receipt> {
    owner.spawn(SpawnSpec {
        executable: File::open("/proc/self/exe")?.into(),
        arg0: "rust-launcher-probe".into(),
        arguments,
        environment: vec![("VIVAC_OPENSSH_DISPOSABLE".into(), "1".into())],
        stdio: [null()?, null()?, null()?],
        inherited,
        retained: Vec::new(),
        new_session: true,
        controlling_tty: None,
        probe: None,
    })
}
fn wait_normal(owner: &mut SpawnOwner, receipt: Receipt) -> io::Result<ExitReceipt> {
    let pidfd = owner.duplicate_pidfd(receipt.token)?;
    let until = deadline();
    loop {
        baseline::require(Instant::now() < until)?;
        if let Some(exit) = owner.try_wait(receipt.token)? {
            baseline::require(
                exit.token == receipt.token
                    && exit.pid == receipt.pid
                    && exit.code == Some(0)
                    && exit.signal.is_none(),
            )?;
            let mut poll = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll receives one initialized writable descriptor for the retained pidfd.
            let ready = unsafe { libc::poll(&mut poll, 1, 0) };
            if ready < 0 {
                return Err(io::Error::last_os_error());
            }
            baseline::require(poll.revents & (libc::POLLERR | libc::POLLNVAL) == 0)?;
            if ready == 1 && poll.revents & libc::POLLIN != 0 {
                return Ok(exit);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}
fn sticky_root(channel: &mut Channel) -> io::Result<bool> {
    let error = channel
        .probe(17, deadline())
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::PermissionDenied && channel.poisoned())?;
    Ok(true)
}
fn sticky_server(channel: &mut Channel) -> io::Result<bool> {
    let error = channel
        .respond_probe(deadline())
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::PermissionDenied && channel.poisoned())?;
    Ok(true)
}

const CASES: [&str; 7] = [
    "roundtrip",
    "wrong-generation",
    "wrong-store",
    "wrong-role",
    "forwarded-sender",
    "peer-exit",
    "historical-listener",
];

fn socket() -> io::Result<OwnedFd> {
    // SAFETY: socket returns a fresh descriptor without borrowing memory.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful socket transferred exclusive ownership.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    passcred(&fd)?;
    Ok(fd)
}
fn passcred(fd: &OwnedFd) -> io::Result<()> {
    let enabled: libc::c_int = 1;
    // SAFETY: setsockopt borrows one initialized socket option.
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
    Ok(())
}
fn address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: zero initializes this C socket address, including its terminator.
    let mut address: libc::sockaddr_un = unsafe { zeroed() };
    baseline::require(
        path.is_absolute()
            && !bytes.is_empty()
            && bytes.len() < address.sun_path.len()
            && !bytes.contains(&0),
    )?;
    address.sun_family = libc::AF_UNIX as _;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as _;
    }
    let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as _;
    Ok((address, length))
}
pub(super) fn listen(path: &Path) -> io::Result<OwnedFd> {
    let fd = socket()?;
    let (address, length) = address(path)?;
    // SAFETY: bind borrows the live address with its validated initialized length.
    if unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: listen acts only on the owned fixture socket.
    if unsafe { libc::listen(fd.as_raw_fd(), 1) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}
pub(super) fn accept(fd: &OwnedFd) -> io::Result<OwnedFd> {
    let mut item = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll borrows one initialized writable descriptor.
    baseline::require(
        unsafe { libc::poll(&mut item, 1, 5000) } == 1 && item.revents == libc::POLLIN,
    )?;
    // SAFETY: null address outputs request no peer address; CLOEXEC is atomic.
    let raw = unsafe {
        libc::accept4(
            fd.as_raw_fd(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            libc::SOCK_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful accept4 returned a fresh owned socket.
    let connected = unsafe { OwnedFd::from_raw_fd(raw) };
    passcred(&connected)?;
    Ok(connected)
}
pub(super) fn connect(path: &Path) -> io::Result<OwnedFd> {
    let fd = socket()?;
    let (address, length) = address(path)?;
    // SAFETY: connect borrows one initialized validated sockaddr_un.
    if unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}
fn peer_identity(fd: &OwnedFd) -> io::Result<[u32; 3]> {
    // SAFETY: zero initializes the entire output credential structure.
    let mut value: libc::ucred = unsafe { zeroed() };
    let mut length = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes only the supplied credential and length storage.
    if unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut value as *mut libc::ucred).cast(),
            &mut length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    baseline::require(length as usize == size_of::<libc::ucred>() && value.pid > 0)?;
    Ok([value.pid as u32, value.uid, value.gid])
}
fn verify_process(pid: u32, parent: u32, pidfd: &OwnedFd) -> io::Result<u64> {
    let alive = || -> io::Result<()> {
        let mut item = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll only queries this retained pidfd.
        baseline::require(unsafe { libc::poll(&mut item, 1, 0) } == 0 && item.revents == 0)
    };
    alive()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let field = |name: &str| -> io::Result<Vec<u64>> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .ok_or_else(baseline::refused)?
            .split_whitespace()
            .map(|word| word.parse().map_err(|_| baseline::refused()))
            .collect()
    };
    // SAFETY: these calls query only this process's effective credentials.
    let (uid, gid) = unsafe { (libc::geteuid() as u64, libc::getegid() as u64) };
    baseline::require(
        field("PPid:")? == [u64::from(parent)]
            && field("Uid:")? == [uid; 4]
            && field("Gid:")? == [gid; 4]
            && field("NoNewPrivs:")? == [1],
    )?;
    let own = std::fs::metadata("/proc/self/exe")?;
    let peer = std::fs::metadata(format!("/proc/{pid}/exe"))?;
    baseline::require((own.dev(), own.ino()) == (peer.dev(), peer.ino()))?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (_, after) = stat.rsplit_once(')').ok_or_else(baseline::refused)?;
    let started = after
        .split_whitespace()
        .nth(19)
        .ok_or_else(baseline::refused)?
        .parse::<u64>()
        .map_err(|_| baseline::refused())?;
    baseline::require(started > 0)?;
    alive()?;
    Ok(started)
}
fn failure(error: &io::Error) -> io::Result<&'static str> {
    match error.kind() {
        io::ErrorKind::InvalidData => Ok("invalid-data"),
        io::ErrorKind::PermissionDenied => Ok("permission-denied"),
        _ => Err(baseline::refused()),
    }
}
fn injector(values: BTreeMap<String, OsString>) -> io::Result<()> {
    let fds = descriptors(&values, &["--transport-fd", "--control-fd"])?;
    let transport = baseline::take_inherited(fds[0])?;
    let control = Control::new(baseline::take_inherited(fds[1])?)?;
    let wire = unhex(&values["--wire"], 112)?;
    // SAFETY: getpid queries this disposable public-frame sender.
    let pid = unsafe { libc::getpid() } as u32;
    control.send(&json!({"stage":"injector-ready","pid":pid}))?;
    control.wait_go()?;
    raw_send(&transport, &wire, None)?;
    control.send(&json!({"stage":"injector-sent","pid":pid}))?;
    control.wait_go()
}
fn worker(case: &str, server: bool, values: BTreeMap<String, OsString>) -> io::Result<()> {
    let mut names = vec!["--directory-fd", "--cipher-fd", "--control-fd"];
    if server && case == "historical-listener" {
        names.push("--listener-fd");
    }
    let fds = descriptors(&values, &names)?;
    let directory = baseline::take_inherited(fds[0])?;
    let cipher = baseline::take_inherited(fds[1])?;
    let control = Control::new(baseline::take_inherited(fds[2])?)?;
    let path = Path::new(&values["--socket"]);
    // SAFETY: getpid queries this worker's identity.
    let pid = unsafe { libc::getpid() } as u32;
    let transport = if server {
        let listener = if case == "historical-listener" {
            baseline::take_inherited(fds[3])?
        } else {
            listen(path)?
        };
        control.send(&json!({"stage":"listening","pid":pid}))?;
        let connected = accept(&listener)?;
        drop(listener);
        connected
    } else {
        connect(path)?
    };
    let mut context = ConnectedPeerContext::capture(&transport)?;
    let (local_identity, connected_identity) = context.identities()?;
    // SAFETY: these calls query the current worker's real process credentials.
    let actual_local = unsafe { [libc::getpid() as u32, libc::getuid(), libc::getgid()] };
    baseline::require(
        local_identity == actual_local && connected_identity == peer_identity(&transport)?,
    )?;
    let connected_peer_pid = connected_identity[0];
    let expected = ExpectedPeer::connected_identity(&transport)?;
    let raw = baseline::duplicate(transport.as_raw_fd())?;
    let lease = PackageLease::capture(directory, &cipher)?;
    let store = StoreIdentity::from_lease(&lease).map_err(|_| baseline::refused())?;
    let generation = Generation::from_bootstrap(
        unhex(&values["--bootstrap"], 32)?
            .try_into()
            .map_err(|_| baseline::refused())?,
    )?;
    let role = if server || case == "wrong-role" {
        Role::Responder
    } else {
        Role::Initiator
    };
    let mut channel = Channel::new(transport, expected, store, generation, role)?;
    control
        .send(&json!({"stage":"connected","pid":pid,"connected_peer_pid":connected_peer_pid}))?;
    control.wait_go()?;
    let hello = channel.handshake(deadline());
    let hello_ok = hello.is_ok();
    let hello_error = match hello {
        Ok(()) => None,
        Err(error) => Some(failure(&error)?),
    };
    let mut refused = u64::from(!hello_ok);
    let mut sticky = if hello_ok {
        false
    } else if server {
        sticky_server(&mut channel)?
    } else {
        sticky_root(&mut channel)?
    };
    context.validate()?;
    control.send(
        &json!({"stage":"hello","pid":pid,"completed":hello_ok,"error":hello_error,
        "context_valid_after_exchange":true}),
    )?;
    control.wait_go()?;
    if server && case == "peer-exit" {
        return Ok(());
    }
    let mut accepted = 0u64;
    let mut injector_owned = None;
    let mut injector_exit = None;
    if hello_ok {
        if case == "roundtrip" {
            for value in [17, 23] {
                let actual = if server {
                    channel.respond_probe(deadline())?
                } else {
                    channel.probe(value, deadline())?
                };
                baseline::require(actual == value)?;
                accepted += 1;
            }
            crate::linux_ipc::registration::probe_endpoint_transport(
                baseline::duplicate(raw.as_raw_fd())?,
                server,
                deadline(),
            )?;
        } else if case == "forwarded-sender" {
            if server {
                let error = channel
                    .respond_probe(deadline())
                    .err()
                    .ok_or_else(baseline::refused)?;
                baseline::require(failure(&error)? == "permission-denied")?;
                refused += 1;
                sticky = sticky_server(&mut channel)?;
            } else {
                let mut owner = SpawnOwner::new()?;
                let (child_control, child_report) = Control::pair()?;
                let copied = baseline::duplicate(raw.as_raw_fd())?;
                let wire = frame(store.words(), generation.bytes(), 1, 17);
                let receipt = spawn(
                    &mut owner,
                    vec![
                        "--connected-injector".into(),
                        "--transport-fd".into(),
                        copied.as_raw_fd().to_string().into(),
                        "--control-fd".into(),
                        child_report.as_raw_fd().to_string().into(),
                        "--wire".into(),
                        baseline::hex_encode(&wire).into(),
                    ],
                    vec![copied, child_report],
                )?;
                child_control.receive("injector-ready", receipt.pid)?;
                let pidfd = owner.duplicate_pidfd(receipt.token)?;
                verify_process(receipt.pid, pid, &pidfd)?;
                control.send(&json!({"stage":"injector-ready","pid":pid,"injector_pid":receipt.pid,"injector_token":receipt.token}))?;
                control.wait_go()?;
                child_control.go()?;
                child_control.receive("injector-sent", receipt.pid)?;
                injector_owned = Some((owner, receipt, child_control));
            }
        } else if !server && case == "peer-exit" {
            let error = channel
                .probe(17, deadline())
                .err()
                .ok_or_else(baseline::refused)?;
            baseline::require(failure(&error)? == "permission-denied")?;
            refused += 1;
            sticky = sticky_root(&mut channel)?;
        }
    }
    let context_peer_exit_refused = !server && case == "peer-exit";
    let context_sticky_refusal = if context_peer_exit_refused {
        let error = context.validate().err().ok_or_else(baseline::refused)?;
        baseline::require(error.kind() == io::ErrorKind::PermissionDenied)?;
        baseline::require(
            context.validate().err().map(|error| error.kind())
                == Some(io::ErrorKind::PermissionDenied)
                && context.identities().err().map(|error| error.kind())
                    == Some(io::ErrorKind::PermissionDenied),
        )?;
        true
    } else {
        context.validate()?;
        baseline::require(context.identities()? == (local_identity, connected_identity))?;
        false
    };
    control.send(&json!({"stage":"result","pid":pid,"handshake_completed":hello_ok,"hello_error":hello_error,
        "accepted_probes":accepted,"refusals":refused,"poisoned":channel.poisoned(),"sticky_refusal":sticky,
        "context_captured":true,"context_identities_verified":true,
        "context_valid_after_exchange":!context_peer_exit_refused,
        "context_peer_exit_refused":context_peer_exit_refused,"context_sticky_refusal":context_sticky_refusal}))?;
    control.wait_go()?;
    if let Some((mut owner, receipt, child_control)) = injector_owned {
        child_control.go()?;
        let exit = wait_normal(&mut owner, receipt)?;
        let exits = owner.shutdown(Duration::from_secs(3))?;
        baseline::require(
            exits.len() == 1
                && exits[0].pid == exit.pid
                && exits[0].token == exit.token
                && exits[0].code == Some(0)
                && exits[0].signal.is_none(),
        )?;
        injector_exit = Some(0);
    }
    control.send(&json!({"stage":"closing","pid":pid,"injector_exit_code":injector_exit}))?;
    Ok(())
}
fn root(case: &str, values: BTreeMap<String, OsString>) -> io::Result<()> {
    let fds = descriptors(
        &values,
        &[
            "--observer-fd",
            "--directory-fd",
            "--other-directory-fd",
            "--cipher-fd",
        ],
    )?;
    let observer = Control::new(baseline::take_inherited(fds[0])?)?;
    let directory = baseline::take_inherited(fds[1])?;
    let other = baseline::take_inherited(fds[2])?;
    let cipher = baseline::take_inherited(fds[3])?;
    let path = Path::new(&values["--socket"]);
    baseline::require(!path.exists())?;
    let generation = Generation::fresh(deadline())?;
    let alternate = if case == "wrong-generation" {
        Generation::fresh(deadline())?
    } else {
        generation
    };
    baseline::require(case != "wrong-generation" || generation.bytes() != alternate.bytes())?;
    let historical = case == "historical-listener";
    let listener = if historical {
        Some(listen(path)?)
    } else {
        None
    };
    let pressure = Pressure::start()?;
    let mut owner = SpawnOwner::new()?;
    let (server, server_report) = Control::pair()?;
    let mut server_inherited = vec![
        baseline::duplicate(directory.as_raw_fd())?,
        baseline::duplicate(cipher.as_raw_fd())?,
        server_report,
    ];
    let mut server_args = vec![
        "--connected-server".into(),
        case.into(),
        "--socket".into(),
        values["--socket"].clone(),
        "--bootstrap".into(),
        baseline::hex_encode(&alternate.bytes()).into(),
    ];
    for (option, fd) in ["--directory-fd", "--cipher-fd", "--control-fd"]
        .into_iter()
        .zip(&server_inherited)
    {
        server_args.extend([option.into(), fd.as_raw_fd().to_string().into()]);
    }
    if let Some(fd) = listener {
        server_args.extend(["--listener-fd".into(), fd.as_raw_fd().to_string().into()]);
        server_inherited.push(fd);
    }
    let server_receipt = spawn(&mut owner, server_args, server_inherited)?;
    server.receive("listening", server_receipt.pid)?;
    let (client, client_report) = Control::pair()?;
    let client_inherited = vec![
        baseline::duplicate(if case == "wrong-store" {
            other.as_raw_fd()
        } else {
            directory.as_raw_fd()
        })?,
        baseline::duplicate(cipher.as_raw_fd())?,
        client_report,
    ];
    let mut client_args = vec![
        "--connected-client".into(),
        case.into(),
        "--socket".into(),
        values["--socket"].clone(),
        "--bootstrap".into(),
        baseline::hex_encode(&generation.bytes()).into(),
    ];
    for (option, fd) in ["--directory-fd", "--cipher-fd", "--control-fd"]
        .into_iter()
        .zip(&client_inherited)
    {
        client_args.extend([option.into(), fd.as_raw_fd().to_string().into()]);
    }
    let client_receipt = spawn(&mut owner, client_args, client_inherited)?;
    let server_ready = server.receive("connected", server_receipt.pid)?;
    let client_ready = client.receive("connected", client_receipt.pid)?;
    // SAFETY: getpid queries the root process owning both sibling receipts.
    let host_pid = unsafe { libc::getpid() } as u32;
    let server_fd = owner.duplicate_pidfd(server_receipt.token)?;
    let client_fd = owner.duplicate_pidfd(client_receipt.token)?;
    let server_start = verify_process(server_receipt.pid, host_pid, &server_fd)?;
    let client_start = verify_process(client_receipt.pid, host_pid, &client_fd)?;
    baseline::require(
        server_ready["connected_peer_pid"] == client_receipt.pid
            && client_ready["connected_peer_pid"]
                == if historical {
                    host_pid
                } else {
                    server_receipt.pid
                },
    )?;
    let mut identity = json!({"stage":"ready","case":case,"host_pid":host_pid,"creator_tid":server_receipt.creator_tid,
        "server_pid":server_receipt.pid,"server_token":server_receipt.token,"client_pid":client_receipt.pid,"client_token":client_receipt.token,
        "injector_pid":null,"injector_token":null,"server_connected_peer_pid":server_ready["connected_peer_pid"],"client_connected_peer_pid":client_ready["connected_peer_pid"]});
    observer.send(&identity)?;
    observer.wait_go()?;
    server.go()?;
    client.go()?;
    let server_hello = server.receive("hello", server_receipt.pid)?;
    let client_hello = client.receive("hello", client_receipt.pid)?;
    let mut server_exited = false;
    if case == "peer-exit" {
        baseline::require(server_hello["completed"] == true && client_hello["completed"] == true)?;
        server.go()?;
        wait_normal(&mut owner, server_receipt)?;
        server_exited = true;
        identity["stage"] = "peer-exited".into();
        observer.send(&identity)?;
        observer.wait_go()?;
    } else {
        server.go()?;
    }
    client.go()?;
    let mut injector_fd = None;
    if case == "forwarded-sender" {
        let injected = client.receive("injector-ready", client_receipt.pid)?;
        let pid = injected["injector_pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(baseline::refused)?;
        // SAFETY: pidfd_open binds the reported fixture child before proc identity validation.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) } as i32;
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful pidfd_open returned one fresh descriptor.
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw) };
        verify_process(pid, client_receipt.pid, &pidfd)?;
        injector_fd = Some(pidfd);
        identity["stage"] = "injector-ready".into();
        identity["injector_pid"] = injected["injector_pid"].clone();
        identity["injector_token"] = injected["injector_token"].clone();
        observer.send(&identity)?;
        observer.wait_go()?;
        client.go()?;
    }
    let server_result = if server_exited {
        baseline::require(server_hello["context_valid_after_exchange"] == true)?;
        json!({"handshake_completed":true,"hello_error":null,"accepted_probes":0,"refusals":0,"poisoned":false,"sticky_refusal":false,
            "context_captured":true,"context_identities_verified":true,"context_valid_after_exchange":true,
            "context_peer_exit_refused":false,"context_sticky_refusal":false})
    } else {
        server.receive("result", server_receipt.pid)?
    };
    let client_result = client.receive("result", client_receipt.pid)?;
    baseline::require(
        client_result["handshake_completed"] == client_hello["completed"]
            && server_result["handshake_completed"] == server_hello["completed"],
    )?;
    baseline::require(verify_process(client_receipt.pid, host_pid, &client_fd)? == client_start)?;
    if !server_exited {
        baseline::require(
            verify_process(server_receipt.pid, host_pid, &server_fd)? == server_start,
        )?;
    }
    let mut report = identity;
    let object = report.as_object_mut().ok_or_else(baseline::refused)?;
    object.insert("stage".into(), "observation".into());
    for (prefix, result) in [("server", server_result), ("client", client_result)] {
        for field in [
            "handshake_completed",
            "hello_error",
            "accepted_probes",
            "refusals",
            "poisoned",
            "sticky_refusal",
            "context_captured",
            "context_identities_verified",
            "context_valid_after_exchange",
            "context_peer_exit_refused",
            "context_sticky_refusal",
        ] {
            object.insert(format!("{prefix}_{field}"), result[field].clone());
        }
    }
    for field in [
        "custody_accredited",
        "product_recovery",
        "same_store_recovery",
        "mcp_integration",
        "server_integration",
        "lock_ack",
        "endpoint_authority_accredited",
    ] {
        object.insert(field.into(), false.into());
    }
    for (field, value) in [
        ("version", json!(1)),
        ("status", json!("observed")),
        ("listener_origin_parent", json!(historical)),
        (
            "server_exit_code",
            if server_exited { json!(0) } else { Value::Null },
        ),
        ("client_exit_code", Value::Null),
        ("injector_exit_code", Value::Null),
        ("server_exit_observed", json!(server_exited)),
        ("owned_exits_observed", json!(false)),
        ("resources_closed_after_exit", json!(false)),
        ("pressure_threads", json!(4)),
        ("pressure_iterations", json!(0)),
    ] {
        object.insert(field.into(), value);
    }
    observer.send(&report)?;
    observer.wait_go()?;
    client.go()?;
    let closing = client.receive("closing", client_receipt.pid)?;
    wait_normal(&mut owner, client_receipt)?;
    if !server_exited {
        server.go()?;
        server.receive("closing", server_receipt.pid)?;
    }
    wait_normal(&mut owner, server_receipt)?;
    let exits = owner.shutdown(Duration::from_secs(3))?;
    baseline::require(
        exits.len() == 2
            && [server_receipt, client_receipt].into_iter().all(|receipt| {
                exits
                    .iter()
                    .filter(|exit| {
                        exit.pid == receipt.pid
                            && exit.token == receipt.token
                            && exit.code == Some(0)
                            && exit.signal.is_none()
                    })
                    .count()
                    == 1
            }),
    )?;
    if let Some(fd) = injector_fd {
        let mut item = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll checks the retained verified fixture injector pidfd.
        baseline::require(
            unsafe { libc::poll(&mut item, 1, 0) } == 1
                && item.revents & libc::POLLIN != 0
                && closing["injector_exit_code"] == 0,
        )?;
    }
    report
        .as_object_mut()
        .ok_or_else(baseline::refused)?
        .remove("stage");
    report["server_exit_code"] = 0.into();
    report["client_exit_code"] = 0.into();
    report["injector_exit_code"] = closing["injector_exit_code"].clone();
    report["server_exit_observed"] = true.into();
    report["owned_exits_observed"] = true.into();
    report["resources_closed_after_exit"] = true.into();
    report["pressure_iterations"] = pressure.finish()?.into();
    std::fs::remove_file(path)?;
    observer.send(&report)?;
    println!("{report}");
    Ok(())
}
fn expected_endpoint_peer(path: &Path) -> io::Result<()> {
    let listener = listen(path)?;
    let identity = std::fs::symlink_metadata(path)?;
    let mut owner = SpawnOwner::new_dedicated()?;
    let mut launch = || {
        let (control, child_control) = Control::pair()?;
        let receipt = spawn(
            &mut owner,
            vec![
                "--connected-expected-child".into(),
                "--socket".into(),
                path.as_os_str().into(),
                "--control-fd".into(),
                child_control.as_raw_fd().to_string().into(),
            ],
            vec![child_control],
        )?;
        Ok::<_, io::Error>((control, receipt, accept(&listener)?))
    };
    let (first_control, first, first_socket) = launch()?;
    let (second_control, second, second_socket) = launch()?;
    let first_reference = owner.duplicate_pidfd(first.token)?;
    let second_reference = owner.duplicate_pidfd(second.token)?;
    let first_bound = EndpointProofConnection::for_expected_peer(
        first_socket.try_clone()?,
        &first_reference,
        deadline(),
    )?;
    let second_bound = EndpointProofConnection::for_expected_peer(
        second_socket.try_clone()?,
        &second_reference,
        deadline(),
    )?;
    for (socket, other) in [
        (&first_socket, &second_reference),
        (&second_socket, &first_reference),
    ] {
        baseline::require(
            EndpointProofConnection::for_expected_peer(socket.try_clone()?, other, deadline())
                .is_err(),
        )?;
    }
    baseline::require(
        EndpointProofConnection::for_expected_peer(
            first_socket.try_clone()?,
            &second_socket,
            deadline(),
        )
        .is_err(),
    )?;
    second_control.go()?;
    wait_normal(&mut owner, second)?;
    baseline::require(
        EndpointProofConnection::for_expected_peer(
            first_socket.try_clone()?,
            &second_reference,
            deadline(),
        )
        .is_err(),
    )?;
    first_control.go()?;
    wait_normal(&mut owner, first)?;
    drop((first_bound, second_bound));
    let current = std::fs::symlink_metadata(path)?;
    baseline::require(
        current.file_type().is_socket()
            && (current.dev(), current.ino()) == (identity.dev(), identity.ino()),
    )?;
    std::fs::remove_file(path)?;
    println!(
        "{}",
        json!({
            "expected_children_bound": 2,
            "swapped_live_processes_refused": true,
            "dead_reference_refused": true,
            "nonpidfd_refused": true,
            "no_challenge_observed": true,
            "owned_exits_observed": true
        })
    );
    Ok(())
}

fn expected_endpoint_child(values: BTreeMap<String, OsString>) -> io::Result<()> {
    let fds = descriptors(&values, &["--control-fd"])?;
    let control = Control::new(baseline::take_inherited(fds[0])?)?;
    let socket = connect(Path::new(&values["--socket"]))?;
    control.wait_go()?;
    let mut byte = 0u8;
    // SAFETY: the socket is retained and byte is a writable receive buffer.
    let result = unsafe {
        libc::recv(
            socket.as_raw_fd(),
            (&mut byte as *mut u8).cast(),
            1,
            libc::MSG_DONTWAIT,
        )
    };
    baseline::require(result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock)
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: the alarm bounds only this explicitly disposable fixture.
    unsafe { libc::alarm(60) };
    let mut args = std::env::args_os().skip(1);
    let mode = args.next().ok_or_else(baseline::refused)?;
    if mode == "--connected-injector" {
        return injector(options(
            args,
            &["--transport-fd", "--control-fd", "--wire"],
        )?);
    }
    if mode == "--connected-expected-peer" {
        let values = options(args, &["--socket"])?;
        return expected_endpoint_peer(Path::new(&values["--socket"]));
    }
    if mode == "--connected-expected-child" {
        return expected_endpoint_child(options(args, &["--socket", "--control-fd"])?);
    }
    baseline::require(
        mode == "--connected-case" || mode == "--connected-server" || mode == "--connected-client",
    )?;
    let case = args
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    baseline::require(CASES.contains(&case.as_str()))?;
    if mode == "--connected-case" {
        root(
            &case,
            options(
                args,
                &[
                    "--observer-fd",
                    "--directory-fd",
                    "--other-directory-fd",
                    "--cipher-fd",
                    "--socket",
                ],
            )?,
        )
    } else {
        let server = mode == "--connected-server";
        let mut names = vec![
            "--directory-fd",
            "--cipher-fd",
            "--control-fd",
            "--socket",
            "--bootstrap",
        ];
        if server && case == "historical-listener" {
            names.push("--listener-fd");
        }
        worker(&case, server, options(args, &names)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connected_fixture_has_only_seven_cases_and_exact_options() {
        assert_eq!(CASES.len(), 7);
        assert!(options(
            ["--socket", "/tmp/public"].map(OsString::from),
            &["--socket"]
        )
        .is_ok());
        assert!(options(
            ["--socket", "/tmp/public", "--socket", "/tmp/other"].map(OsString::from),
            &["--socket"]
        )
        .is_err());
        assert!(options(
            ["--other", "/tmp/public"].map(OsString::from),
            &["--socket"]
        )
        .is_err());
        assert!(address(Path::new("relative.sock")).is_err());
    }
}
