use crate::linux_reservation::StoreReservation;
use crate::linux_session::PackageLease;
use crate::linux_spawn::{ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CASES: [&str; 6] = [
    "contention",
    "normal-release",
    "abandonment-retains",
    "death-is-not-exit",
    "replacement-binding",
    "invalidation-retains",
];

struct Channel(OwnedFd);
impl Channel {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        let timeout = libc::timeval {
            tv_sec: 10,
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: setsockopt borrows the initialized timeout for this owned socket.
            if unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    option,
                    (&timeout as *const libc::timeval).cast(),
                    std::mem::size_of_val(&timeout) as _,
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
        // SAFETY: socketpair initializes the two-element descriptor array on success.
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
        // SAFETY: both descriptors are fresh and exclusively owned.
        let [local, peer] = unsafe { fds.map(|fd| OwnedFd::from_raw_fd(fd)) };
        Ok((Self::new(local)?, peer))
    }
    fn bytes(&self, bytes: &[u8]) -> io::Result<()> {
        // SAFETY: send borrows the complete initialized packet.
        baseline::require(
            unsafe {
                libc::send(
                    self.0.as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    libc::MSG_NOSIGNAL,
                )
            } == bytes.len() as isize,
        )
    }
    fn send(&self, value: &Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|_| baseline::refused())?;
        baseline::require(bytes.len() <= 8192)?;
        self.bytes(&bytes)
    }
    fn receive(&self) -> io::Result<Vec<u8>> {
        let mut bytes = [0; 8193];
        // SAFETY: recv writes only this fixed buffer; MSG_TRUNC detects oversized packets.
        let size = unsafe {
            libc::recv(
                self.0.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_TRUNC,
            )
        };
        baseline::require(size > 0 && size <= 8192)?;
        Ok(bytes[..size as usize].to_vec())
    }
    fn packet(&self) -> io::Result<Value> {
        serde_json::from_slice(&self.receive()?).map_err(|_| baseline::refused())
    }
    fn go(&self) -> io::Result<()> {
        baseline::require(self.receive()? == b"G")
    }
}

fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<(bool, String, [RawFd; 4])> {
    let mut args = args.into_iter();
    let prefix = args.next().ok_or_else(baseline::refused)?;
    let worker = prefix == "--reservation-worker";
    baseline::require(worker || prefix == "--reservation-case")?;
    let case = args
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    baseline::require(CASES.contains(&case.as_str()))?;
    let mut fds = [None; 4];
    while let Some(option) = args.next() {
        let index = [
            "--observer-fd",
            "--child-fd",
            "--store-a-fd",
            "--store-b-fd",
        ]
        .iter()
        .position(|name| option == *name)
        .ok_or_else(baseline::refused)?;
        let fd = args
            .next()
            .ok_or_else(baseline::refused)?
            .to_str()
            .ok_or_else(baseline::refused)?
            .parse::<RawFd>()
            .map_err(|_| baseline::refused())?;
        baseline::require(fd >= 3 && fds[index].is_none() && !fds.contains(&Some(fd)))?;
        fds[index] = Some(fd);
    }
    Ok((
        worker,
        case,
        fds.map(|fd| fd.ok_or_else(baseline::refused))
            .into_iter()
            .collect::<io::Result<Vec<_>>>()?
            .try_into()
            .map_err(|_| baseline::refused())?,
    ))
}

fn lease(directory: &OwnedFd) -> io::Result<PackageLease> {
    // SAFETY: openat borrows the directory and fixed NUL-terminated public marker name.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c"key.openssh".as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned one newly owned descriptor.
    let cipher = unsafe { OwnedFd::from_raw_fd(fd) };
    PackageLease::capture(baseline::duplicate(directory.as_raw_fd())?, &cipher)
}
fn busy(lease: &PackageLease) -> io::Result<()> {
    match StoreReservation::acquire(lease) {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
        _ => Err(baseline::refused()),
    }
}
fn null() -> io::Result<OwnedFd> {
    Ok(File::options()
        .read(true)
        .write(true)
        .open("/dev/null")?
        .into())
}

fn spec(arguments: Vec<OsString>, inherited: Vec<OwnedFd>) -> io::Result<SpawnSpec> {
    Ok(SpawnSpec {
        executable: File::open("/proc/self/exe")?.into(),
        arg0: "rust-launcher-probe".into(),
        arguments,
        environment: vec![("VIVAC_OPENSSH_DISPOSABLE".into(), "1".into())],
        stdio: [null()?, null()?, null()?],
        inherited,
        retained: Vec::new(),
        new_session: false,
        controlling_tty: None,
        probe: None,
    })
}
fn wait(owner: &mut SpawnOwner, receipt: Receipt, killed: bool) -> io::Result<ExitReceipt> {
    let pidfd = owner.duplicate_pidfd(receipt.token)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(exit) = owner.try_wait(receipt.token)? {
            baseline::require(
                exit.token == receipt.token
                    && exit.pid == receipt.pid
                    && if killed {
                        exit.code.is_none() && exit.signal == Some(libc::SIGKILL)
                    } else {
                        exit.code == Some(0) && exit.signal.is_none()
                    },
            )?;
            let mut poll = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll receives one initialized descriptor for the same observed process.
            baseline::require(
                unsafe { libc::poll(&mut poll, 1, 0) } == 1 && poll.revents & libc::POLLIN != 0,
            )?;
            return Ok(exit);
        }
        baseline::require(Instant::now() < deadline)?;
        thread::sleep(Duration::from_millis(5));
    }
}

fn child() -> io::Result<()> {
    baseline::require(std::env::args_os().count() == 2)?;
    // Duplicate stdin before using the normal non-stdio socket responder.
    let control = Channel::new(baseline::duplicate(0)?)?;
    // SAFETY: these calls query only the current disposable responder.
    let pid = unsafe { libc::getpid() };
    let mut parent_signal: libc::c_int = 0;
    // SAFETY: prctl writes one initialized c_int.
    baseline::require(
        unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, &mut parent_signal, 0, 0, 0) } == 0
            && parent_signal == 0,
    )?;
    control.send(&json!({"stage":"child-ready","pid":pid}))?;
    loop {
        baseline::require(control.receive()? == b"P")?;
        control.send(&json!({"stage":"child-alive","pid":pid}))?;
    }
}

fn worker(
    case: &str,
    observer: Channel,
    child_fd: OwnedFd,
    directory_a: OwnedFd,
    directory_b: OwnedFd,
) -> io::Result<()> {
    let a = lease(&directory_a)?;
    let b = lease(&directory_b)?;
    let guard = StoreReservation::acquire(&a)?;
    let mut owner = None;
    let mut held = Some(guard);
    let mut unmanaged = None;
    let mut native_pid = None;
    let mut native_token = None;
    if case == "abandonment-retains" {
        let mut reserved = SpawnOwner::new_reserved(held.take().ok_or_else(baseline::refused)?)?;
        let child_number = child_fd.as_raw_fd();
        let result = reserved.spawn_with_pidfd_probe_failure(spec(
            vec![
                "--abandon-child".into(),
                "--control-fd".into(),
                child_number.to_string().into(),
            ],
            vec![child_fd],
        )?);
        baseline::require(result.is_err_and(|error| error.raw_os_error() == Some(libc::EMFILE)))?;
        let receipt = reserved.receipt(1)?;
        native_pid = Some(receipt.pid);
        native_token = Some(receipt.token);
        owner = Some(reserved);
    } else if case == "death-is-not-exit" {
        // Negative control: deliberately no pre_exec hook or parent-death signal.
        let process = Command::new("/proc/self/exe")
            .arg("--reservation-child")
            .env_clear()
            .env("VIVAC_OPENSSH_DISPOSABLE", "1")
            .stdin(Stdio::from(child_fd))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        native_pid = Some(process.id());
        unmanaged = Some(process);
    } else {
        drop(child_fd);
    }
    observer
        .send(&json!({"stage":"reserved","native_pid":native_pid,"native_token":native_token}))?;
    observer.go()?;
    let mut invalidated = false;
    let mut replacement_refused = false;
    let mut original_valid = false;
    let mut poisoned = false;
    let mut drop_elapsed_ns = 0u64;
    match case {
        "normal-release" => {
            drop(held.take());
        }
        "replacement-binding" => {
            let guard = held.as_ref().ok_or_else(baseline::refused)?;
            original_valid = guard.validate(&a).is_ok();
            replacement_refused = guard.validate(&b).is_err();
            baseline::require(original_valid && replacement_refused)?;
        }
        "invalidation-retains" => {
            invalidated = held
                .as_ref()
                .ok_or_else(baseline::refused)?
                .validate(&a)
                .is_err();
            baseline::require(invalidated)?;
        }
        "abandonment-retains" => {
            let started = Instant::now();
            drop(owner.take());
            drop_elapsed_ns = started
                .elapsed()
                .as_nanos()
                .try_into()
                .map_err(|_| baseline::refused())?;
            poisoned = SpawnOwner::admission_uncertain();
            baseline::require(poisoned && drop_elapsed_ns > 0)?;
            baseline::require(
                SpawnOwner::new()
                    .is_err_and(|error| error.raw_os_error() == Some(libc::EOWNERDEAD)),
            )?;
        }
        "contention" | "death-is-not-exit" => {}
        _ => return Err(baseline::refused()),
    }
    observer.send(&json!({"stage":"action","lease_invalidated":invalidated,"replacement_refused":replacement_refused,
        "original_valid":original_valid,"admission_poisoned":poisoned,"drop_elapsed_ns":drop_elapsed_ns}))?;
    observer.go()?;
    drop(held);
    drop(unmanaged.take());
    Ok(())
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: this alarm bounds only the explicitly disposable public-marker experiment.
    unsafe { libc::alarm(45) };
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--reservation-child")) {
        return child();
    }
    let (is_worker, case, [observer, child_fd, a_fd, b_fd]) = parse(std::env::args_os().skip(1))?;
    let observer = Channel::new(baseline::take_inherited(observer)?)?;
    let child_fd = baseline::take_inherited(child_fd)?;
    let directory_a = baseline::take_inherited(a_fd)?;
    let directory_b = baseline::take_inherited(b_fd)?;
    if is_worker {
        return worker(&case, observer, child_fd, directory_a, directory_b);
    }
    let a = lease(&directory_a)?;
    let b = lease(&directory_b)?;
    let pressure = Pressure::start()?;
    let mut owner = SpawnOwner::new()?;
    let (worker, worker_fd) = Channel::pair()?;
    let mut arguments = vec!["--reservation-worker".into(), case.clone().into()];
    let inherited = vec![
        worker_fd,
        child_fd,
        baseline::duplicate(directory_a.as_raw_fd())?,
        baseline::duplicate(directory_b.as_raw_fd())?,
    ];
    for (option, fd) in [
        "--observer-fd",
        "--child-fd",
        "--store-a-fd",
        "--store-b-fd",
    ]
    .into_iter()
    .zip(&inherited)
    {
        arguments.extend([option.into(), fd.as_raw_fd().to_string().into()]);
    }
    let receipt = owner.spawn(spec(arguments, inherited)?)?;
    let reserved = worker.packet()?;
    baseline::require(reserved["stage"] == "reserved")?;
    busy(&a)?;
    drop(StoreReservation::acquire(&b)?);
    // SAFETY: getpid queries this host, whose main thread owns the launcher.
    let host_pid = unsafe { libc::getpid() };
    let identity = json!({"stage":"before","case":case,"host_pid":host_pid,"creator_tid":receipt.creator_tid,
        "worker_pid":receipt.pid,"worker_token":receipt.token,"native_pid":reserved["native_pid"],"native_token":reserved["native_token"]});
    observer.send(&identity)?;
    observer.go()?;
    let death = case == "death-is-not-exit";
    let mut worker_exit = None;
    let action = if death {
        owner.signal(receipt.token, libc::SIGKILL)?;
        worker_exit = Some(wait(&mut owner, receipt, true)?);
        json!({"lease_invalidated":false,"replacement_refused":false,"original_valid":false,"admission_poisoned":false,"drop_elapsed_ns":0})
    } else {
        worker.bytes(b"G")?;
        let action = worker.packet()?;
        baseline::require(action["stage"] == "action")?;
        action
    };
    let readmitted = death || case == "normal-release";
    if readmitted {
        drop(StoreReservation::acquire(&a)?);
    } else if case == "invalidation-retains" {
        busy(&lease(&directory_a)?)?;
    } else {
        busy(&a)?;
    }
    let mut report = json!({"version":1,"case":case,"status":"observed","host_pid":host_pid,"creator_tid":receipt.creator_tid,
        "worker_pid":receipt.pid,"worker_token":receipt.token,"native_pid":reserved["native_pid"],"native_token":reserved["native_token"],
        "same_ofd_contention_refused":true,"different_store_available":true,"readmitted_before_worker_exit":case == "normal-release",
        "readmitted_after_worker_death":death,"reservation_retained_after_action":!readmitted,
        "lease_invalidated":action["lease_invalidated"],"replacement_refused":action["replacement_refused"],"original_valid":action["original_valid"],
        "admission_poisoned":action["admission_poisoned"],"drop_elapsed_ns":action["drop_elapsed_ns"],
        "worker_exit_code":null,"worker_exit_signal":if death { json!(9) } else { Value::Null },
        "worker_exit_observed":death,"pressure_threads":4,"pressure_iterations":0,"resources_closed_after_worker_exit":false,
        "custody_accredited":false,"product_recovery":false,"same_store_recovery":false,"mcp_integration":false,"server_integration":false,"lock_ack":false});
    observer.send(&json!({"stage":"observation","payload":report}))?;
    observer.go()?;
    if !death {
        worker.bytes(b"G")?;
        worker_exit = Some(wait(&mut owner, receipt, false)?);
    }
    let expected_exit = worker_exit.ok_or_else(baseline::refused)?;
    report["worker_exit_code"] = json!(expected_exit.code);
    report["worker_exit_signal"] = json!(expected_exit.signal);
    report["worker_exit_observed"] = true.into();
    observer.send(
        &json!({"stage":"worker-exited","worker_pid":receipt.pid,"worker_token":receipt.token,
        "exit_code":expected_exit.code,"exit_signal":expected_exit.signal}),
    )?;
    observer.go()?;
    let exits = owner.shutdown(Duration::from_secs(2))?;
    baseline::require(
        exits.len() == 1
            && exits[0].token == expected_exit.token
            && exits[0].pid == expected_exit.pid
            && exits[0].code == expected_exit.code
            && exits[0].signal == expected_exit.signal,
    )?;
    let iterations = pressure.finish()?;
    baseline::require(iterations > 0)?;
    report["pressure_iterations"] = iterations.into();
    report["resources_closed_after_worker_exit"] = true.into();
    observer.send(&report)?;
    println!("{report}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservation_cli_requires_exact_cases_and_distinct_capabilities() {
        let good = [
            "--reservation-case",
            "contention",
            "--observer-fd",
            "3",
            "--child-fd",
            "4",
            "--store-a-fd",
            "5",
            "--store-b-fd",
            "6",
        ];
        assert!(parse(good.map(OsString::from)).is_ok());
        for (index, value) in [
            (1, "other"),
            (3, "2"),
            (5, "3"),
            (7, "bad"),
            (8, "--observer-fd"),
        ] {
            let mut args = good;
            args[index] = value;
            assert!(parse(args.map(OsString::from)).is_err());
        }
        assert!(parse(good[..8].iter().map(OsString::from)).is_err());
    }
}
