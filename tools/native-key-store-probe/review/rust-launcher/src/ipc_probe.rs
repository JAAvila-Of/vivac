use crate::linux_ipc::{self, Channel, ExpectedPeer, Generation, Role, StoreIdentity};
use crate::linux_session::PackageLease;
use crate::linux_spawn::{ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::thread;
use std::time::{Duration, Instant};

const CASES: [&str; 10] = [
    "roundtrip",
    "wrong-generation",
    "wrong-store",
    "replay",
    "forwarded-sender",
    "ancillary-rights",
    "peer-exit",
    "receive-timeout",
    "truncated-frame",
    "oversized-frame",
];
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
        .chunks_exact(2)
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
fn fd_count() -> io::Result<usize> {
    Ok(std::fs::read_dir("/proc/self/fd")?.count())
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
fn server(case: &str, values: BTreeMap<String, OsString>) -> io::Result<()> {
    let fds = descriptors(
        &values,
        &[
            "--channel-fd",
            "--directory-fd",
            "--cipher-fd",
            "--report-fd",
        ],
    )?;
    let transport = baseline::take_inherited(fds[0])?;
    let directory = baseline::take_inherited(fds[1])?;
    let cipher = baseline::take_inherited(fds[2])?;
    let report = Control::new(baseline::take_inherited(fds[3])?)?;
    let generation = Generation::from_bootstrap(
        unhex(&values["--generation"], 32)?
            .try_into()
            .map_err(|_| baseline::refused())?,
    )?;
    let lease = PackageLease::capture(directory, &cipher)?;
    let store = StoreIdentity::from_lease(&lease).map_err(|_| baseline::refused())?;
    let mut channel = Channel::new(
        transport,
        ExpectedPeer::parent()?,
        store,
        generation,
        Role::Responder,
    )?;
    // SAFETY: getpid only queries this explicitly disposable server process.
    let pid = unsafe { libc::getpid() } as u32;
    let before = fd_count()?;
    report.send(&json!({"stage":"server-ready","pid":pid}))?;
    report.wait_go()?;
    let mut accepted = 0u64;
    let mut refusals = 0u64;
    let mut sticky = false;
    let handshake = if case == "receive-timeout" {
        false
    } else {
        match channel.handshake(deadline()) {
            Ok(()) => true,
            Err(_) => {
                refusals = 1;
                sticky = sticky_server(&mut channel)?;
                false
            }
        }
    };
    if handshake {
        report.send(&json!({"stage":"handshake-ready","pid":pid,"fd_count":before}))?;
        report.wait_go()?;
        if case == "peer-exit" {
            return Ok(());
        }
        let requests = if matches!(case, "roundtrip" | "replay") {
            2
        } else {
            1
        };
        for index in 0..requests {
            match channel.respond_probe(deadline()) {
                Ok(value) => {
                    baseline::require(value == if index == 0 { 17 } else { 23 })?;
                    accepted += 1;
                }
                Err(_) => {
                    refusals = 1;
                    sticky = sticky_server(&mut channel)?;
                    break;
                }
            }
        }
    }
    let after = fd_count()?;
    report.send(&json!({"stage":"result","pid":pid,"handshake_completed":handshake,"accepted_probes":accepted,
        "poisoned":channel.poisoned(),"sticky_refusal":sticky,"refusals":refusals,"fd_count_before":before,"fd_count_after":after}))?;
    report.wait_go()
}
fn injector(values: BTreeMap<String, OsString>) -> io::Result<()> {
    let fds = descriptors(&values, &["--channel-fd", "--report-fd"])?;
    let channel = baseline::take_inherited(fds[0])?;
    let report = Control::new(baseline::take_inherited(fds[1])?)?;
    let wire = unhex(&values["--frame"], 112)?;
    // SAFETY: getpid only queries this explicitly disposable injector.
    let pid = unsafe { libc::getpid() } as u32;
    report.send(&json!({"stage":"injector-ready","pid":pid}))?;
    report.wait_go()?;
    raw_send(&channel, &wire, None)?;
    report.send(&json!({"stage":"injector-sent","pid":pid}))?;
    report.wait_go()
}

fn root(case: &str, values: BTreeMap<String, OsString>) -> io::Result<()> {
    let fds = descriptors(
        &values,
        &[
            "--observer-fd",
            "--directory-fd",
            "--cipher-fd",
            "--other-directory-fd",
        ],
    )?;
    let observer = Control::new(baseline::take_inherited(fds[0])?)?;
    let directory = baseline::take_inherited(fds[1])?;
    let cipher = baseline::take_inherited(fds[2])?;
    let other = baseline::take_inherited(fds[3])?;
    let lease = PackageLease::capture(
        baseline::duplicate(if case == "wrong-store" {
            other.as_raw_fd()
        } else {
            directory.as_raw_fd()
        })?,
        &cipher,
    )?;
    let store = StoreIdentity::from_lease(&lease).map_err(|_| baseline::refused())?;
    let generation = Generation::fresh(deadline())?;
    let server_generation = if case == "wrong-generation" {
        Generation::fresh(deadline())?
    } else {
        generation
    };
    baseline::require(
        case != "wrong-generation" || generation.bytes() != server_generation.bytes(),
    )?;
    let (client, peer) = linux_ipc::packet_pair()?;
    let raw = baseline::duplicate(client.as_raw_fd())?;
    let (server_control, server_report) = Control::pair()?;
    let pressure = Pressure::start()?;
    let mut owner = SpawnOwner::new()?;
    let server_receipt = spawn(
        &mut owner,
        vec![
            "--ipc-server".into(),
            case.into(),
            "--channel-fd".into(),
            peer.as_raw_fd().to_string().into(),
            "--directory-fd".into(),
            directory.as_raw_fd().to_string().into(),
            "--cipher-fd".into(),
            cipher.as_raw_fd().to_string().into(),
            "--report-fd".into(),
            server_report.as_raw_fd().to_string().into(),
            "--generation".into(),
            baseline::hex_encode(&server_generation.bytes()).into(),
        ],
        vec![peer, directory, cipher, server_report],
    )?;
    let mut channel = Channel::new(
        client,
        ExpectedPeer::owned_child(&owner, server_receipt.token)?,
        store,
        generation,
        Role::Initiator,
    )?;
    server_control.receive("server-ready", server_receipt.pid)?;
    // SAFETY: getpid only queries this explicitly disposable root process.
    let host_pid = unsafe { libc::getpid() } as u32;
    let announce = |stage: &str| json!({"stage":stage,"case":case,"host_pid":host_pid,"server_pid":server_receipt.pid,"creator_tid":server_receipt.creator_tid});
    observer.send(&announce("ready"))?;
    observer.wait_go()?;
    if case != "receive-timeout" {
        server_control.go()?;
    }
    let handshake_result = channel.handshake(if case == "receive-timeout" {
        Instant::now() + Duration::from_millis(200)
    } else {
        deadline()
    });
    let handshake = handshake_result.is_ok();
    let timed_out = handshake_result
        .as_ref()
        .err()
        .is_some_and(|error| error.kind() == io::ErrorKind::TimedOut);
    let mut root_refusals = u64::from(handshake_result.is_err());
    let mut root_sticky = if handshake_result.is_err() {
        sticky_root(&mut channel)?
    } else {
        false
    };
    let mut accepted = 0u64;
    let mut injector_owned = None;
    let mut receipts = vec![server_receipt];
    let server_report;
    if handshake {
        let ready = server_control.receive("handshake-ready", server_receipt.pid)?;
        observer.send(&announce("handshake-ready"))?;
        observer.wait_go()?;
        server_control.go()?;
        if case == "peer-exit" {
            wait_normal(&mut owner, server_receipt)?;
            baseline::require(channel.probe(17, deadline()).is_err())?;
            root_refusals += 1;
            root_sticky = sticky_root(&mut channel)?;
            server_report = json!({"handshake_completed":true,"accepted_probes":0,"poisoned":false,"sticky_refusal":false,
                "refusals":0,"fd_count_before":ready["fd_count"],"fd_count_after":null});
        } else {
            if matches!(case, "roundtrip" | "replay") {
                baseline::require(channel.probe(17, deadline())? == 17)?;
                accepted += 1;
            }
            if case == "roundtrip" {
                baseline::require(channel.probe(23, deadline())? == 23)?;
                accepted += 1;
            } else {
                let mut wire = frame(store.words(), generation.bytes(), 1, 17).to_vec();
                match case {
                    "truncated-frame" => {
                        wire.pop();
                    }
                    "oversized-frame" => wire.push(0),
                    _ => {}
                }
                if case == "forwarded-sender" {
                    let (control, report) = Control::pair()?;
                    let transport = baseline::duplicate(raw.as_raw_fd())?;
                    let receipt = spawn(
                        &mut owner,
                        vec![
                            "--ipc-injector".into(),
                            "--channel-fd".into(),
                            transport.as_raw_fd().to_string().into(),
                            "--report-fd".into(),
                            report.as_raw_fd().to_string().into(),
                            "--frame".into(),
                            baseline::hex_encode(&wire).into(),
                        ],
                        vec![transport, report],
                    )?;
                    control.receive("injector-ready", receipt.pid)?;
                    let mut ready = announce("injector-ready");
                    ready["injector_pid"] = receipt.pid.into();
                    observer.send(&ready)?;
                    observer.wait_go()?;
                    control.go()?;
                    control.receive("injector-sent", receipt.pid)?;
                    receipts.push(receipt);
                    injector_owned = Some((receipt, control));
                } else if case == "ancillary-rights" {
                    // SAFETY: the constant name is NUL terminated; memfd_create returns a fresh descriptor.
                    let fd = unsafe {
                        libc::memfd_create(c"vivac-public-ipc-right".as_ptr(), libc::MFD_CLOEXEC)
                    };
                    if fd < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    // SAFETY: successful memfd_create transferred this fresh descriptor.
                    let mut marker = unsafe { File::from_raw_fd(fd) };
                    marker.write_all(b"public IPC ancillary marker")?;
                    let marker: OwnedFd = marker.into();
                    raw_send(&raw, &wire, Some(&marker))?;
                } else {
                    raw_send(&raw, &wire, None)?;
                }
            }
            server_report = server_control.receive("result", server_receipt.pid)?;
        }
    } else {
        baseline::require(matches!(
            case,
            "wrong-generation" | "wrong-store" | "receive-timeout"
        ))?;
        if case == "receive-timeout" {
            baseline::require(timed_out)?;
            server_control.go()?;
        }
        server_report = server_control.receive("result", server_receipt.pid)?;
    }
    baseline::require(
        server_report["handshake_completed"] == handshake
            && server_report["accepted_probes"] == accepted,
    )?;
    let before = server_report["fd_count_before"]
        .as_u64()
        .ok_or_else(baseline::refused)?;
    let after = server_report["fd_count_after"].as_u64();
    let rights_closed = case == "ancillary-rights" && after.is_some_and(|value| value <= before);
    baseline::require(case != "ancillary-rights" || rights_closed)?;
    let mut report = json!({"version":1,"case":case,"status":"observed","host_pid":host_pid,"server_pid":server_receipt.pid,
        "injector_pid":injector_owned.as_ref().map(|(receipt,_)|receipt.pid),"creator_tid":server_receipt.creator_tid,
        "handshake_completed":handshake,"root_poisoned":channel.poisoned(),"server_poisoned":server_report["poisoned"],
        "root_sticky_refusal":root_sticky,"server_sticky_refusal":server_report["sticky_refusal"],
        "accepted_probes":accepted,"server_accepted_probes":server_report["accepted_probes"],"root_refusals":root_refusals,
        "server_refusals":server_report["refusals"],"deadline_timed_out":timed_out,"server_fd_count_before":before,
        "server_fd_count_after":server_report["fd_count_after"],"ancillary_fds_closed":rights_closed,
        "server_exit_code":if case=="peer-exit" {Some(0)} else {None::<i32>},"injector_exit_code":null,
        "owned_exits_observed":case=="peer-exit","resources_closed_after_exit":false,"pressure_threads":4,"pressure_iterations":0,
        "custody_accredited":false,"product_recovery":false,"mcp_integration":false,"server_integration":false,
        "same_store_recovery":false,"lock_ack":false,"stage":"observation"});
    observer.send(&report)?;
    observer.wait_go()?;
    if let Some((receipt, control)) = &injector_owned {
        control.go()?;
        wait_normal(&mut owner, *receipt)?;
    }
    if case != "peer-exit" {
        server_control.go()?;
    }
    wait_normal(&mut owner, server_receipt)?;
    let exits = owner.shutdown(Duration::from_secs(5))?;
    baseline::require(
        exits.len() == receipts.len()
            && receipts.iter().all(|expected| {
                exits
                    .iter()
                    .filter(|exit| {
                        exit.token == expected.token
                            && exit.pid == expected.pid
                            && exit.code == Some(0)
                            && exit.signal.is_none()
                    })
                    .count()
                    == 1
            }),
    )?;
    drop(injector_owned);
    drop(server_control);
    drop(channel);
    drop(raw);
    drop(lease);
    drop(other);
    let iterations = pressure.finish()?;
    report
        .as_object_mut()
        .ok_or_else(baseline::refused)?
        .remove("stage");
    report["server_exit_code"] = 0.into();
    report["injector_exit_code"] = if case == "forwarded-sender" {
        json!(0)
    } else {
        Value::Null
    };
    report["owned_exits_observed"] = true.into();
    report["resources_closed_after_exit"] = true.into();
    report["pressure_iterations"] = iterations.into();
    observer.send(&report)?;
    println!("{report}");
    Ok(())
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: this deadline applies only to an explicitly disposable probe process.
    unsafe { libc::alarm(90) };
    let mut arguments = std::env::args_os().skip(1);
    let mode = arguments.next().ok_or_else(baseline::refused)?;
    if mode == "--ipc-injector" {
        return injector(options(
            arguments,
            &["--channel-fd", "--report-fd", "--frame"],
        )?);
    }
    baseline::require(mode == "--ipc-case" || mode == "--ipc-server")?;
    let case = arguments
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    baseline::require(CASES.contains(&case.as_str()))?;
    if mode == "--ipc-server" {
        server(
            &case,
            options(
                arguments,
                &[
                    "--channel-fd",
                    "--directory-fd",
                    "--cipher-fd",
                    "--report-fd",
                    "--generation",
                ],
            )?,
        )
    } else {
        root(
            &case,
            options(
                arguments,
                &[
                    "--observer-fd",
                    "--directory-fd",
                    "--cipher-fd",
                    "--other-directory-fd",
                ],
            )?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_probe_fixture_has_the_exact_public_wire_layout() {
        let wire = frame([1, 2, 3, 4, 5, 6], [7; 32], 1, 17);
        assert_eq!(&wire[..16], b"VVIPC01\0\x01\x01\x02\0\0\0\0\0");
        assert_eq!(&wire[16..24], &1u64.to_be_bytes());
        assert_eq!(&wire[56..64], &6u64.to_be_bytes());
        assert_eq!(&wire[64..96], &[7; 32]);
        assert_eq!(&wire[96..104], &1u64.to_be_bytes());
        assert_eq!(&wire[104..112], &17u64.to_be_bytes());
    }
    #[test]
    fn ipc_capabilities_must_be_complete_distinct_and_non_stdio() {
        let names = [
            "--observer-fd",
            "--directory-fd",
            "--cipher-fd",
            "--other-directory-fd",
        ];
        let args = [
            "--observer-fd",
            "3",
            "--directory-fd",
            "4",
            "--cipher-fd",
            "5",
            "--other-directory-fd",
            "6",
        ];
        let good = options(args.map(OsString::from), &names).expect("public capability arguments");
        assert_eq!(
            descriptors(&good, &names).expect("distinct descriptors"),
            vec![3, 4, 5, 6]
        );
        for value in ["2", "3", "invalid"] {
            let mut changed = good.clone();
            changed.insert("--directory-fd".into(), value.into());
            assert!(descriptors(&changed, &names).is_err());
        }
        assert!(options(args[..6].iter().map(OsString::from), &names).is_err());
        let mut duplicate = args.to_vec();
        duplicate.extend(["--observer-fd", "7"]);
        assert!(options(duplicate.into_iter().map(OsString::from), &names).is_err());
    }
}
