use crate::linux_loader::PtyLoader;
use crate::linux_spawn::{ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use crate::linux_ssh_lifetime::{DisconnectCause, SshLifetime};
use crate::probe::Pressure;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const CASES: [&str; 8] = [
    "normal",
    "cancel-prompt",
    "cancel-handoff",
    "cancel-before-ready",
    "owner-kill-prompt",
    "owner-kill-handoff",
    "owner-kill-before-ready",
    "loader-kill-prompt",
];
const LIMIT: usize = 8192;
const DOMAIN: &[u8] = b"vivac.device.enrollment-proof.v1\0";

pub(super) fn refused() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}
pub(super) fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(refused())
    }
}

#[derive(Debug)]
pub(super) struct Args {
    pub(super) case: String,
    pub(super) descriptors: [RawFd; 6],
    pub(super) socket: PathBuf,
    pub(super) ssh_lifetime: bool,
}

pub(super) fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<Args> {
    let mut arguments = arguments.into_iter();
    require(arguments.next().as_deref() == Some(std::ffi::OsStr::new("--loader-case")))?;
    let case = arguments
        .next()
        .ok_or_else(refused)?
        .into_string()
        .map_err(|_| refused())?;
    require(CASES.contains(&case.as_str()))?;
    let mut descriptors = [None; 6];
    let mut socket = None;
    let mut ssh_lifetime = false;
    while let Some(option) = arguments.next() {
        let value = arguments.next().ok_or_else(refused)?;
        if option == "--ssh-lifetime" {
            require(!ssh_lifetime && value == "stdin")?;
            ssh_lifetime = true;
            continue;
        }
        if option == "--socket" {
            require(socket.is_none())?;
            let path = PathBuf::from(value);
            require(path.is_absolute() && !path.as_os_str().is_empty())?;
            socket = Some(path);
            continue;
        }
        let index = [
            "--agent-fd",
            "--loader-fd",
            "--cipher-fd",
            "--secret-fd",
            "--fixture-fd",
            "--observer-fd",
        ]
        .iter()
        .position(|name| option == *name)
        .ok_or_else(refused)?;
        require(descriptors[index].is_none())?;
        let descriptor = value
            .into_string()
            .map_err(|_| refused())?
            .parse::<RawFd>()
            .map_err(|_| refused())?;
        require(descriptor >= 3)?;
        descriptors[index] = Some(descriptor);
    }
    let descriptors = descriptors
        .map(|value| value.ok_or_else(refused))
        .into_iter()
        .collect::<io::Result<Vec<_>>>()?;
    for (index, descriptor) in descriptors.iter().enumerate() {
        require(!descriptors[..index].contains(descriptor))?;
    }
    Ok(Args {
        case,
        descriptors: descriptors.try_into().map_err(|_| refused())?,
        socket: socket.ok_or_else(refused)?,
        ssh_lifetime,
    })
}

pub(super) fn duplicate(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC creates a new owned descriptor from the borrowed input.
    let result = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fcntl returned a fresh descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(result) })
}

pub(super) fn take_inherited(fd: RawFd) -> io::Result<OwnedFd> {
    let copied = duplicate(fd)?;
    // SAFETY: successful duplication proved the input live. Startup is still
    // single-threaded, and the CLI validated distinct transferred descriptors.
    let original = unsafe { OwnedFd::from_raw_fd(fd) };
    drop(original);
    Ok(copied)
}

pub(super) struct Inputs {
    pub(super) agent: OwnedFd,
    pub(super) loader: OwnedFd,
    pub(super) cipher: OwnedFd,
    pub(super) secret: Option<OwnedFd>,
    pub(super) fixture: OwnedFd,
}

impl Inputs {
    pub(super) fn read_secret(&mut self) -> io::Result<Zeroizing<Vec<u8>>> {
        let descriptor = self.secret.take().ok_or_else(refused)?;
        let link = std::fs::read_link(format!("/proc/self/fd/{}", descriptor.as_raw_fd()))?;
        require(link.to_string_lossy().starts_with("pipe:["))?;
        let mut file = File::from(descriptor);
        let mut secret = Zeroizing::new(Vec::new());
        (&mut file).take(513).read_to_end(&mut secret)?;
        validate_secret(&secret)?;
        Ok(secret)
    }
}

fn validate_secret(secret: &[u8]) -> io::Result<()> {
    require(
        (16..=512).contains(&secret.len())
            && std::str::from_utf8(secret).is_ok()
            && !secret.iter().any(|byte| matches!(*byte, 0 | b'\r' | b'\n')),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureWire {
    public_blob: String,
    comment: String,
    challenge: String,
    public_key: String,
}

pub(super) struct Fixture {
    pub(super) blob: Vec<u8>,
    pub(super) comment: Vec<u8>,
    pub(super) challenge: Vec<u8>,
    pub(super) public_key: [u8; 32],
}

fn hex_decode(value: &str) -> io::Result<Vec<u8>> {
    require(value.len() <= LIMIT && value.len() % 2 == 0)?;
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |value| match value {
                b'0'..=b'9' => Ok(value - b'0'),
                b'a'..=b'f' => Ok(value - b'a' + 10),
                _ => Err(refused()),
            };
            Ok((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect()
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for value in bytes {
        result.push(char::from(HEX[usize::from(value >> 4)]));
        result.push(char::from(HEX[usize::from(value & 15)]));
    }
    result
}

pub(super) fn ssh_string(bytes: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
    require(bytes.len() <= LIMIT)?;
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

fn take_u32(input: &mut &[u8]) -> io::Result<u32> {
    require(input.len() >= 4)?;
    let value = u32::from_be_bytes(input[..4].try_into().map_err(|_| refused())?);
    *input = &input[4..];
    Ok(value)
}

fn take_string<'a>(input: &mut &'a [u8]) -> io::Result<&'a [u8]> {
    let size = take_u32(input)? as usize;
    require(size <= LIMIT && size <= input.len())?;
    let value = &input[..size];
    *input = &input[size..];
    Ok(value)
}

impl Fixture {
    fn parse(bytes: &[u8]) -> io::Result<Self> {
        require(!bytes.is_empty() && bytes.len() <= LIMIT)?;
        let value: FixtureWire = serde_json::from_slice(bytes).map_err(|_| refused())?;
        let blob = hex_decode(&value.public_blob)?;
        let challenge = hex_decode(&value.challenge)?;
        let public_key: [u8; 32] = hex_decode(&value.public_key)?
            .try_into()
            .map_err(|_| refused())?;
        require(challenge.starts_with(DOMAIN) && challenge.len() <= 4096)?;
        require(
            value.comment.len() <= 128
                && value.comment.starts_with("vivac.device-key.v1:")
                && value.comment.is_ascii()
                && !value.comment.as_bytes().contains(&0),
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
            comment: value.comment.into_bytes(),
            challenge,
            public_key,
        })
    }

    pub(super) fn read(descriptor: &OwnedFd) -> io::Result<Self> {
        // SAFETY: F_GET_SEALS queries the live fixture descriptor.
        let seals = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GET_SEALS) };
        let required =
            libc::F_SEAL_SEAL | libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK;
        require(seals >= 0 && seals & required == required)?;
        let mut file = File::from(duplicate(descriptor.as_raw_fd())?);
        require(file.metadata()?.is_file() && file.metadata()?.len() <= LIMIT as u64)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        Self::parse(&bytes)
    }
}

pub(super) struct Control {
    descriptor: OwnedFd,
    lifetime: Option<SshLifetime>,
}

impl Control {
    pub(super) fn new(descriptor: OwnedFd, ssh_lifetime: bool) -> io::Result<Self> {
        let timeout = libc::timeval {
            tv_sec: 40,
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: setsockopt reads one initialized timeval on the owned socket.
            if unsafe {
                libc::setsockopt(
                    descriptor.as_raw_fd(),
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
        Ok(Self {
            descriptor,
            lifetime: if ssh_lifetime {
                Some(SshLifetime::stdin()?)
            } else {
                None
            },
        })
    }

    fn ssh_disconnected(&self) -> io::Result<bool> {
        self.lifetime.as_ref().map_or(Ok(false), |lifetime| {
            lifetime.poll().map(|cause| cause.is_some())
        })
    }

    pub(super) fn disconnect_cause(&self) -> Option<DisconnectCause> {
        self.lifetime
            .as_ref()
            .and_then(SshLifetime::disconnect_cause)
    }

    pub(super) fn prioritize(&self, command: u8) -> io::Result<u8> {
        Ok(if self.ssh_disconnected()? {
            b'D'
        } else {
            command
        })
    }

    pub(super) fn command(&self) -> io::Result<u8> {
        self.receive_command(true)
    }

    pub(super) fn observer_command(&self) -> io::Result<u8> {
        self.receive_command(false)
    }

    fn receive_command(&self, observe_ssh: bool) -> io::Result<u8> {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            if observe_ssh && self.ssh_disconnected()? {
                return Ok(b'D');
            }
            require(Instant::now() < deadline)?;
            let mut descriptors = [
                libc::pollfd {
                    fd: self.descriptor.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if observe_ssh {
                        self.lifetime.as_ref().map_or(-1, AsRawFd::as_raw_fd)
                    } else {
                        -1
                    },
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: both pollfds remain initialized and writable during the bounded poll.
            let result = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, 50) };
            if result < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io::Error::last_os_error());
            }
            if observe_ssh && self.ssh_disconnected()? {
                return Ok(b'D');
            }
            require(
                descriptors[0].revents & (libc::POLLNVAL | libc::POLLERR | libc::POLLHUP) == 0,
            )?;
            if descriptors[0].revents & libc::POLLIN == 0 {
                continue;
            }
            let mut bytes = [0; 2];
            // SAFETY: recv writes only the fixed buffer; nonblocking avoids an unbounded race.
            let size = unsafe {
                libc::recv(
                    self.descriptor.as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    libc::MSG_TRUNC | libc::MSG_DONTWAIT,
                )
            };
            if size < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            require(size == 1 && bytes[0] != b'D')?;
            return if observe_ssh {
                self.prioritize(bytes[0])
            } else {
                Ok(bytes[0])
            };
        }
    }

    pub(super) fn send(&self, value: &Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|_| refused())?;
        require(bytes.len() <= LIMIT)?;
        // SAFETY: bytes stays readable for this packet send; MSG_NOSIGNAL avoids SIGPIPE.
        let size = unsafe {
            libc::send(
                self.descriptor.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if size != bytes.len() as isize {
            return Err(refused());
        }
        Ok(())
    }

    pub(super) fn send_master(&self, descriptor: &OwnedFd) -> io::Result<()> {
        let mut marker = [b'F'];
        let mut vector = libc::iovec {
            iov_base: marker.as_mut_ptr().cast(),
            iov_len: marker.len(),
        };
        let mut ancillary = [0usize; 4];
        // SAFETY: zero is a valid empty msghdr before its pointers and lengths are filled.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = ancillary.as_mut_ptr().cast();
        // SAFETY: CMSG_SPACE computes the fixed buffer length for one RawFd.
        message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as usize;
        require(message.msg_controllen <= size_of::<[usize; 4]>())?;
        // SAFETY: message points to aligned storage sufficient for one cmsghdr and RawFd.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            if header.is_null() {
                return Err(refused());
            }
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as usize;
            std::ptr::write_unaligned(
                libc::CMSG_DATA(header).cast::<RawFd>(),
                descriptor.as_raw_fd(),
            );
            if libc::sendmsg(self.descriptor.as_raw_fd(), &message, libc::MSG_NOSIGNAL) != 1 {
                return Err(refused());
            }
        }
        Ok(())
    }
}

fn agent_request(socket: &Path, request: &[u8]) -> io::Result<Vec<u8>> {
    require(!request.is_empty() && request.len() <= LIMIT)?;
    let mut connection = UnixStream::connect(socket)?;
    connection.set_read_timeout(Some(Duration::from_secs(3)))?;
    connection.set_write_timeout(Some(Duration::from_secs(3)))?;
    connection.write_all(&(request.len() as u32).to_be_bytes())?;
    connection.write_all(request)?;
    let mut length = [0; 4];
    connection.read_exact(&mut length)?;
    let size = u32::from_be_bytes(length) as usize;
    require(size > 0 && size <= LIMIT)?;
    let mut reply = vec![0; size];
    connection.read_exact(&mut reply)?;
    Ok(reply)
}

pub(super) fn empty_agent(socket: &Path, owner: &mut SpawnOwner, token: u64) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(response) = agent_request(socket, &[11]) {
            require(response == [12, 0, 0, 0, 0])?;
            return Ok(());
        }
        require(owner.try_wait(token)?.is_none() && Instant::now() < deadline)?;
        thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn parse_identity(response: &[u8], fixture: &Fixture) -> io::Result<()> {
    require(response.first() == Some(&12))?;
    let mut input = &response[1..];
    require(
        take_u32(&mut input)? == 1
            && take_string(&mut input)? == fixture.blob
            && take_string(&mut input)? == fixture.comment
            && input.is_empty(),
    )
}

pub(super) fn parse_signature(response: &[u8]) -> io::Result<[u8; 64]> {
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

pub(super) fn sign_and_verify(socket: &Path, fixture: &Fixture) -> io::Result<[u8; 64]> {
    parse_identity(&agent_request(socket, &[11])?, fixture)?;
    let mut request = vec![13];
    ssh_string(&fixture.blob, &mut request)?;
    ssh_string(&fixture.challenge, &mut request)?;
    request.extend_from_slice(&0u32.to_be_bytes());
    let signature = parse_signature(&agent_request(socket, &request)?)?;
    VerifyingKey::from_bytes(&fixture.public_key)
        .map_err(|_| refused())?
        .verify_strict(&fixture.challenge, &Signature::from_bytes(&signature))
        .map_err(|_| refused())?;
    Ok(signature)
}

pub(super) fn native_credentials(pid: u32) -> io::Result<()> {
    let path = format!("/proc/{pid}/status");
    let bytes = std::fs::read(path)?;
    require(bytes.len() <= 16384)?;
    let status = std::str::from_utf8(&bytes).map_err(|_| refused())?;
    // SAFETY: geteuid/getegid only query this harness process.
    for (key, expected) in [
        ("Uid:", unsafe { libc::geteuid() }),
        ("Gid:", unsafe { libc::getegid() }),
    ] {
        let values = status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .ok_or_else(refused)?
            .split_whitespace()
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| refused())?;
        require(values.len() == 4 && values.iter().all(|value| *value == expected))?;
    }
    require(status.lines().any(|line| {
        line.strip_prefix("NoNewPrivs:")
            .is_some_and(|value| value.trim() == "1")
    }))
}

fn null() -> io::Result<OwnedFd> {
    Ok(File::options()
        .read(true)
        .write(true)
        .open("/dev/null")?
        .into())
}

pub(super) fn launch_agent(
    owner: &mut SpawnOwner,
    descriptor: &OwnedFd,
    socket: &Path,
    home: &Path,
) -> io::Result<Receipt> {
    owner.spawn(SpawnSpec {
        executable: duplicate(descriptor.as_raw_fd())?,
        arg0: "/usr/bin/ssh-agent".into(),
        arguments: vec!["-D".into(), "-a".into(), socket.as_os_str().to_owned()],
        environment: vec![
            ("HOME".into(), home.as_os_str().to_owned()),
            ("LANG".into(), "C".into()),
            ("LC_ALL".into(), "C".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ],
        stdio: [null()?, null()?, null()?],
        inherited: Vec::new(),
        retained: Vec::new(),
        new_session: true,
        controlling_tty: None,
        probe: None,
    })
}

pub(super) struct State {
    pub(super) agent: Receipt,
    pub(super) loader: Receipt,
    pub(super) loader_exit: Option<ExitReceipt>,
    pub(super) agent_exit: Option<ExitReceipt>,
    pub(super) secret_writes: u32,
    pub(super) ready_published: bool,
    pub(super) stopped: bool,
    pub(super) identity_bound: bool,
    pub(super) proof_verified: bool,
    pub(super) loader_failed: bool,
}

impl State {
    pub(super) fn stage(&self, stage: &str, closed: bool) -> Value {
        json!({"stage": stage, "agent_pid": self.agent.pid, "loader_pid": self.loader.pid,
            "creator_tid": self.agent.creator_tid, "secret_writes": self.secret_writes,
            "ready_published": self.ready_published, "echo_disabled": true, "prompt_observed": true,
            "loader_stopped": self.stopped, "loader_exit_code": self.loader_exit.and_then(|value| value.code),
            "loader_exit_signal": self.loader_exit.and_then(|value| value.signal),
            "agent_exit_code": self.agent_exit.and_then(|value| value.code), "agent_exit_signal": self.agent_exit.and_then(|value| value.signal),
            "identity_bound": self.identity_bound, "proof_verified": self.proof_verified,
            "loader_failed": self.loader_failed, "close_ack": closed, "resource_release_after_exit": closed})
    }
}

fn wait_failed_loader(owner: &mut SpawnOwner, token: u64) -> io::Result<ExitReceipt> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(value) = owner.try_wait(token)? {
            require(value.signal == Some(libc::SIGKILL))?;
            return Ok(value);
        }
        require(Instant::now() < deadline)?;
        thread::sleep(Duration::from_millis(5));
    }
}

pub fn run() -> io::Result<()> {
    require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    let args = parse(std::env::args_os().skip(1))?;
    // SAFETY: the deadline affects only this explicitly disposable harness process.
    unsafe { libc::alarm(45) };
    require(!args.socket.exists())?;
    let parent = args.socket.parent().ok_or_else(refused)?;
    let metadata = parent.metadata()?;
    require(metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0)?;
    let home = parent.join("home");
    require(home.is_dir() && home.metadata()?.permissions().mode() & 0o077 == 0)?;
    let [agent, loader, cipher, secret, fixture, observer] = args.descriptors;
    let mut inputs = Inputs {
        agent: take_inherited(agent)?,
        loader: take_inherited(loader)?,
        cipher: take_inherited(cipher)?,
        secret: Some(take_inherited(secret)?),
        fixture: take_inherited(fixture)?,
    };
    let control = Control::new(take_inherited(observer)?, args.ssh_lifetime)?;
    let fixture = Fixture::read(&inputs.fixture)?;
    let pressure = Pressure::start()?;
    let mut owner = SpawnOwner::new()?;
    let agent = launch_agent(&mut owner, &inputs.agent, &args.socket, &home)?;
    empty_agent(&args.socket, &mut owner, agent.token)?;
    native_credentials(agent.pid)?;
    let mut loader = PtyLoader::start(
        &mut owner,
        duplicate(inputs.loader.as_raw_fd())?,
        duplicate(inputs.cipher.as_raw_fd())?,
        &args.socket,
        &home,
    )?;
    let master = loader.duplicate_master()?;
    control.send_master(&master)?;
    loader.wait_prompt(&mut owner, Duration::from_secs(10))?;
    require(loader.prompt_seen())?;
    let mut state = State {
        agent,
        loader: loader.receipt(),
        loader_exit: None,
        agent_exit: None,
        secret_writes: 0,
        ready_published: false,
        stopped: false,
        identity_bound: false,
        proof_verified: false,
        loader_failed: false,
    };
    require(state.agent.creator_tid == state.loader.creator_tid)?;
    control.send(&state.stage("prompt", false))?;
    let mut command = control.command()?;
    command = control.prioritize(command)?;
    if command == b'H' {
        let secret = inputs.read_secret()?;
        command = control.prioritize(command)?;
        if command != b'D' {
            loader.handoff_stopped(&mut owner, &secret, Duration::from_secs(10))?;
            require(loader.handed_off())?;
            state.secret_writes = 1;
            state.stopped = true;
            control.send(&state.stage("handoff", false))?;
            command = control.command()?;
            command = control.prioritize(command)?;
            if command == b'L' {
                let exit = loader.complete(&mut owner, Duration::from_secs(10))?;
                require(exit.code == Some(0) && exit.signal.is_none())?;
                state.loader_exit = Some(exit);
                state.stopped = false;
                native_credentials(agent.pid)?;
                let signature = sign_and_verify(&args.socket, &fixture)?;
                state.identity_bound = true;
                state.proof_verified = true;
                let mut stage = state.stage("before-ready", false);
                stage["signature"] = hex_encode(&signature).into();
                control.send(&stage)?;
                command = control.command()?;
                if command == b'R' {
                    require(args.case == "normal")?;
                    command = control.prioritize(command)?;
                    if command != b'D' {
                        state.ready_published = true;
                        control.send(&state.stage("ready", false))?;
                        command = control.command()?;
                    }
                }
            }
        }
    }
    if command == b'F' {
        require(args.case == "loader-kill-prompt" && state.secret_writes == 0)?;
        state.loader_exit = Some(wait_failed_loader(&mut owner, state.loader.token)?);
        state.loader_failed = true;
        command = b'C';
    }
    require(command == b'C' || (command == b'D' && control.disconnect_cause().is_some()))?;
    let exits = owner.shutdown(Duration::from_secs(3))?;
    require(exits.len() == 2)?;
    for exit in exits {
        if exit.token == state.agent.token {
            state.agent_exit = Some(exit);
        } else if exit.token == state.loader.token {
            state.loader_exit = Some(exit);
        } else {
            return Err(refused());
        }
    }
    require(
        state
            .agent_exit
            .is_some_and(|value| value.signal == Some(libc::SIGKILL))
            && state.loader_exit.is_some(),
    )?;
    if state.secret_writes == 0 || state.stopped {
        require(
            state
                .loader_exit
                .is_some_and(|value| value.signal == Some(libc::SIGKILL)),
        )?;
    }
    state.stopped = false;
    // All local leases follow the owner's observed exits, even on cancellation.
    drop(loader);
    drop(master);
    drop(inputs);
    let iterations = pressure.finish()?;
    let closed = state.stage("closed", true);
    control.send(&closed)?;
    let mut report = closed;
    report["version"] = 1.into();
    report["case"] = if args.ssh_lifetime {
        "normal".into()
    } else {
        args.case.into()
    };
    report["status"] = "observed".into();
    report["pressure_iterations"] = iterations.into();
    report["custody_accredited"] = false.into();
    report["launcher_accredited"] = false.into();
    report["full_session_accredited"] = false.into();
    if args.ssh_lifetime {
        let cause = control.disconnect_cause();
        report["ssh_disconnect_observed"] = cause.is_some().into();
        report["ssh_disconnect_cause"] = cause.map_or(Value::Null, |value| value.as_str().into());
        control.send(&report)?;
    } else {
        println!("{report}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control_pair() -> (Control, OwnedFd) {
        let mut descriptors = [-1; 2];
        // SAFETY: socketpair writes two distinct fresh descriptors into the fixed array.
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    descriptors.as_mut_ptr(),
                )
            },
            0
        );
        // SAFETY: successful socketpair transferred ownership of both descriptors.
        let control = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        // SAFETY: the second descriptor is separately owned by the observer.
        let observer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
        (Control::new(control, false).unwrap(), observer)
    }

    fn send_command(observer: &OwnedFd, packet: &[u8]) {
        // SAFETY: the borrowed packet remains readable for this bounded send.
        assert_eq!(
            unsafe {
                libc::send(
                    observer.as_raw_fd(),
                    packet.as_ptr().cast(),
                    packet.len(),
                    libc::MSG_NOSIGNAL,
                )
            },
            packet.len() as isize
        );
    }

    #[test]
    fn observer_barrier_accepts_only_an_exact_local_packet_and_refuses_injected_disconnect() {
        let (control, observer) = control_pair();
        send_command(&observer, b"G");
        assert_eq!(control.observer_command().unwrap(), b'G');
        assert!(control.disconnect_cause().is_none());
        for packet in [b"D".as_slice(), b"GG".as_slice()] {
            let (control, observer) = control_pair();
            send_command(&observer, packet);
            assert!(control.observer_command().is_err());
        }
    }

    #[test]
    fn legacy_control_still_accepts_one_packet_and_never_accepts_observer_disconnect() {
        let (control, observer) = control_pair();
        send_command(&observer, b"H");
        assert_eq!(control.command().unwrap(), b'H');
        assert_eq!(control.prioritize(b'L').unwrap(), b'L');
        send_command(&observer, b"D");
        assert!(control.command().is_err());
    }

    fn fixture() -> Value {
        let public = hex_decode("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
            .expect("public key fixture");
        let mut blob = Vec::new();
        ssh_string(b"ssh-ed25519", &mut blob).expect("public algorithm");
        ssh_string(&public, &mut blob).expect("public key");
        json!({"public_blob":hex_encode(&blob),"public_key":hex_encode(&public),
            "comment":"vivac.device-key.v1:00000000000000000000000001:00000000000000000000000002",
            "challenge":hex_encode(&[DOMAIN,b"public challenge"].concat())})
    }

    #[test]
    fn fixture_fields_and_duplicate_keys_are_strict() {
        let good = fixture();
        assert!(Fixture::parse(&serde_json::to_vec(&good).expect("public fixture")).is_ok());
        let mut extra = good.clone();
        extra["unexpected"] = true.into();
        assert!(Fixture::parse(&serde_json::to_vec(&extra).expect("public fixture")).is_err());
        let mut duplicate = serde_json::to_string(&good).expect("public fixture");
        duplicate.pop();
        duplicate.push_str(",\"comment\":\"other\"}");
        assert!(Fixture::parse(duplicate.as_bytes()).is_err());
        assert!(Fixture::parse(&vec![b'x'; LIMIT + 1]).is_err());
    }

    #[test]
    fn fixture_binds_blob_to_the_selected_public_key_and_domain() {
        for (field, value) in [
            ("public_key", "00".repeat(32)),
            ("challenge", hex_encode(b"wrong domain")),
            ("public_blob", "00".into()),
        ] {
            let mut changed = fixture();
            changed[field] = value.into();
            assert!(
                Fixture::parse(&serde_json::to_vec(&changed).expect("public fixture")).is_err()
            );
        }
    }

    #[test]
    fn agent_signature_packet_requires_one_exact_ed25519_signature() {
        let mut inner = Vec::new();
        ssh_string(b"ssh-ed25519", &mut inner).expect("algorithm");
        ssh_string(&[7; 64], &mut inner).expect("public signature");
        let mut response = vec![14];
        ssh_string(&inner, &mut response).expect("response");
        assert_eq!(
            parse_signature(&response).expect("public response"),
            [7; 64]
        );
        response.push(0);
        assert!(parse_signature(&response).is_err());
        assert!(parse_signature(&[14, 255, 255, 255, 255]).is_err());
    }

    #[test]
    fn secret_transport_bounds_do_not_claim_entropy() {
        assert!(validate_secret(&[b'a'; 16]).is_ok());
        assert!(validate_secret(&[b'b'; 512]).is_ok());
        for bad in [
            vec![b'a'; 15],
            vec![b'a'; 513],
            b"public\nfixture-secret".to_vec(),
            vec![255; 16],
            vec![0; 16],
        ] {
            assert!(validate_secret(&bad).is_err());
        }
    }

    #[test]
    fn loader_cli_refuses_aliasing_missing_and_extra_descriptors() {
        let good = [
            "--loader-case",
            "normal",
            "--agent-fd",
            "3",
            "--loader-fd",
            "4",
            "--cipher-fd",
            "5",
            "--secret-fd",
            "6",
            "--fixture-fd",
            "7",
            "--observer-fd",
            "8",
            "--socket",
            "/private/a.sock",
        ];
        assert!(
            !parse(good.map(OsString::from))
                .expect("default arguments")
                .ssh_lifetime
        );
        let mut ssh = good.to_vec();
        ssh.extend(["--ssh-lifetime", "stdin"]);
        assert!(
            parse(ssh.iter().map(OsString::from))
                .expect("SSH arguments")
                .ssh_lifetime
        );
        ssh.extend(["--ssh-lifetime", "stdin"]);
        assert!(parse(ssh.iter().map(OsString::from)).is_err());
        ssh.truncate(good.len());
        ssh.extend(["--ssh-lifetime", "other"]);
        assert!(parse(ssh.iter().map(OsString::from)).is_err());
        ssh.pop();
        assert!(parse(ssh.iter().map(OsString::from)).is_err());
        let mut alias = good;
        alias[5] = "3";
        assert!(parse(alias.map(OsString::from)).is_err());
        let mut relative = good;
        relative[15] = "relative";
        assert!(parse(relative.map(OsString::from)).is_err());
        let mut stdio = good;
        stdio[3] = "2";
        assert!(parse(stdio.map(OsString::from)).is_err());
        assert!(parse(good[..good.len() - 2].iter().map(OsString::from)).is_err());
    }
}
