use crate::creation_reader_probe::{
    launch_preparation, verify_admitted_exit_observation, TemporaryOpenSshHome,
};
use crate::linux_current_admission::CurrentAdmission;
use crate::linux_ipc::packet_pair;
use crate::linux_ipc::registration::{
    authorize_supervisor_start, receive_supervisor_start, verify_live_process_pair,
    ActiveClosureFault, KeeperChallengeReceiver, KeeperChallengeSender, KeeperPreUnlockSuccessor,
    RegistrationReceiver, RegistrationSender,
};
use crate::linux_spawn::{current_executable, Receipt, SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use rust_launcher_probe::probe_launch_admitted_pair;
use serde_json::json;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Write};
use std::mem::zeroed;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const CASES: [&str; 15] = [
    "valid",
    "duplicate-token-rejected",
    "nonpidfd-rejected",
    "missing-reference-rejected",
    "extra-reference-rejected",
    "wrong-challenge-rejected",
    "forwarded-sender-rejected",
    "timeout",
    "consumed-rejected",
    "truncated-rights-rejected",
    "owned-pair",
    "owned-extra-child",
    "owned-spawn-after-drop",
    "owned-dead-child",
    "owned-repeated-capture",
];

enum Invocation {
    KeeperBootstrap,
    KeeperSupervisor(RawFd, RawFd, RawFd),
    AdmittedKeeper(String, ConfirmationCase),
    AdmittedBootstrapSupervisor(String, ConfirmationCase, RawFd, RawFd),
    OpenSshKeeper(String),
    OpenSshSupervisor(String, PathBuf, RawFd, RawFd, RawFd),
    ActiveSupervisor(String, PathBuf, RawFd, RawFd, bool, bool),
    Case(String),
    Supervisor(String, RawFd, RawFd),
    Marker(RawFd),
    TaskMarker(String, RawFd),
    Forwarder(RawFd, RawFd, [RawFd; 2]),
}

#[derive(Clone, Copy)]
enum ConfirmationCase {
    Delivered,
    SupervisorExited,
}

impl ConfirmationCase {
    fn parse(argument: Option<OsString>) -> io::Result<Self> {
        match argument.as_deref().and_then(std::ffi::OsStr::to_str) {
            Some("delivered") => Ok(Self::Delivered),
            Some("supervisor-exited") => Ok(Self::SupervisorExited),
            _ => Err(baseline::refused()),
        }
    }

    fn argument(self) -> OsString {
        match self {
            Self::Delivered => "delivered".into(),
            Self::SupervisorExited => "supervisor-exited".into(),
        }
    }
}

fn descriptor(arguments: &mut impl Iterator<Item = OsString>) -> io::Result<RawFd> {
    let text = arguments
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    let value: RawFd = text.parse().map_err(|_| baseline::refused())?;
    baseline::require(value >= 3 && value.to_string() == text)?;
    Ok(value)
}

fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<Invocation> {
    let mut arguments = arguments.into_iter();
    let first = arguments.next().ok_or_else(baseline::refused)?;
    let invocation = if first == "--registration-keeper-bootstrap" {
        Invocation::KeeperBootstrap
    } else if first == "--registration-keeper-supervisor" {
        let challenge = descriptor(&mut arguments)?;
        let offer = descriptor(&mut arguments)?;
        let control = descriptor(&mut arguments)?;
        baseline::require(challenge != offer && challenge != control && offer != control)?;
        Invocation::KeeperSupervisor(challenge, offer, control)
    } else if first == "--registration-admitted-keeper"
        || first == "--registration-admitted-bootstrap-supervisor"
    {
        let credential = arguments
            .next()
            .ok_or_else(baseline::refused)?
            .into_string()
            .map_err(|_| baseline::refused())?;
        let confirmation = ConfirmationCase::parse(arguments.next())?;
        if first == "--registration-admitted-keeper" {
            Invocation::AdmittedKeeper(credential, confirmation)
        } else {
            let transport = descriptor(&mut arguments)?;
            let control = descriptor(&mut arguments)?;
            baseline::require(transport != control)?;
            Invocation::AdmittedBootstrapSupervisor(credential, confirmation, transport, control)
        }
    } else if first == "--registration-active-supervisor" {
        let credential = arguments
            .next()
            .ok_or_else(baseline::refused)?
            .into_string()
            .map_err(|_| baseline::refused())?;
        let home = PathBuf::from(arguments.next().ok_or_else(baseline::refused)?);
        let transport = descriptor(&mut arguments)?;
        let control = descriptor(&mut arguments)?;
        baseline::require(home.is_absolute() && transport != control)?;
        let (keeper_death_cut, opening_permit) = match arguments.next().as_deref() {
            None => (false, false),
            Some(mode) if mode == "keeper-lost" => (true, false),
            Some(mode) if mode == "opening-permit" => (false, true),
            Some(mode) if mode == "opening-permit-keeper-lost" => (true, true),
            Some(_) => return Err(baseline::refused()),
        };
        Invocation::ActiveSupervisor(
            credential,
            home,
            transport,
            control,
            keeper_death_cut,
            opening_permit,
        )
    } else if first == "--registration-openssh-keeper"
        || first == "--registration-openssh-supervisor"
    {
        let credential = arguments
            .next()
            .ok_or_else(baseline::refused)?
            .into_string()
            .map_err(|_| baseline::refused())?;
        if first == "--registration-openssh-keeper" {
            Invocation::OpenSshKeeper(credential)
        } else {
            let home = PathBuf::from(arguments.next().ok_or_else(baseline::refused)?);
            baseline::require(home.is_absolute())?;
            let transport = descriptor(&mut arguments)?;
            let terminal = descriptor(&mut arguments)?;
            let control = descriptor(&mut arguments)?;
            baseline::require(
                transport != terminal && transport != control && terminal != control,
            )?;
            Invocation::OpenSshSupervisor(credential, home, transport, terminal, control)
        }
    } else if first == "--registration-case" || first == "--registration-supervisor" {
        let case = arguments
            .next()
            .ok_or_else(baseline::refused)?
            .into_string()
            .map_err(|_| baseline::refused())?;
        baseline::require(CASES.contains(&case.as_str()))?;
        if first == "--registration-case" {
            Invocation::Case(case)
        } else {
            let transport = descriptor(&mut arguments)?;
            let control = descriptor(&mut arguments)?;
            baseline::require(transport != control)?;
            Invocation::Supervisor(case, transport, control)
        }
    } else if first == "--registration-marker" {
        Invocation::Marker(descriptor(&mut arguments)?)
    } else if first == "--registration-task-marker" {
        let attempt = arguments
            .next()
            .ok_or_else(baseline::refused)?
            .into_string()
            .map_err(|_| baseline::refused())?;
        baseline::require(TASK_ATTEMPTS.contains(&attempt.as_str()))?;
        Invocation::TaskMarker(attempt, descriptor(&mut arguments)?)
    } else {
        baseline::require(first == "--registration-forwarder")?;
        let descriptors = [
            descriptor(&mut arguments)?,
            descriptor(&mut arguments)?,
            descriptor(&mut arguments)?,
            descriptor(&mut arguments)?,
        ];
        for (index, value) in descriptors.iter().enumerate() {
            baseline::require(!descriptors[..index].contains(value))?;
        }
        Invocation::Forwarder(
            descriptors[0],
            descriptors[1],
            [descriptors[2], descriptors[3]],
        )
    };
    baseline::require(arguments.next().is_none())?;
    Ok(invocation)
}

pub(super) struct Control(OwnedFd);

impl Control {
    fn pair() -> io::Result<(Self, OwnedFd)> {
        let (parent, child) = packet_pair()?;
        Ok((Self(parent), child))
    }

    pub(super) fn send(&self, value: u8) -> io::Result<()> {
        self.send_packet(&[value])
    }

    fn send_packet(&self, bytes: &[u8]) -> io::Result<()> {
        // SAFETY: send borrows the initialized byte slice and the retained socket.
        baseline::require(
            unsafe {
                libc::send(
                    self.0.as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
                )
            } == bytes.len() as isize,
        )
    }

    pub(super) fn expect(&self, expected: u8) -> io::Result<()> {
        let mut bytes = [0; 2];
        let count = receive(&self.0, &mut bytes)?;
        baseline::require(count == 1 && bytes[0] == expected)
    }
}

fn receive(socket: &OwnedFd, bytes: &mut [u8]) -> io::Result<usize> {
    receive_before(socket, bytes, Instant::now() + Duration::from_secs(5))
}

fn receive_before(socket: &OwnedFd, bytes: &mut [u8], deadline: Instant) -> io::Result<usize> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        // SAFETY: recv writes only within the borrowed initialized buffer.
        let count = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT | libc::MSG_TRUNC,
            )
        };
        if count >= 0 {
            return Ok(count as usize);
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
        ) {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(2));
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
        executable: current_executable()?,
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

fn collect(owner: &mut SpawnOwner, receipts: &[Receipt]) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    for receipt in receipts {
        loop {
            baseline::require(Instant::now() < deadline)?;
            if let Some(exit) = owner.try_wait(receipt.token)? {
                baseline::require(
                    exit.token == receipt.token
                        && exit.pid == receipt.pid
                        && exit.code == Some(0)
                        && exit.signal.is_none(),
                )?;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let exits = owner.shutdown(Duration::from_secs(2))?;
    baseline::require(
        exits.len() == receipts.len()
            && exits
                .iter()
                .all(|exit| exit.code == Some(0) && exit.signal.is_none()),
    )
}

pub(super) fn marker_spec() -> io::Result<(Control, SpawnSpec)> {
    let (control, child) = Control::pair()?;
    let marker = spec(
        vec![
            "--registration-marker".into(),
            child.as_raw_fd().to_string().into(),
        ],
        vec![child],
    )?;
    Ok((control, marker))
}

pub(super) fn sleep_spec() -> io::Result<SpawnSpec> {
    let mut child = spec(vec!["2".into()], Vec::new())?;
    child.executable = File::open("/usr/bin/sleep")?.into();
    child.arg0 = "sleep".into();
    Ok(child)
}

const TASK_ATTEMPTS: [&str; 7] = [
    "fork", "vfork", "clone", "clone3", "io-uring", "x32", "legacy",
];

#[cfg(target_arch = "x86_64")]
fn attempt_task_creation(attempt: &str) -> io::Result<()> {
    let (number, first) = match attempt {
        "fork" => (libc::SYS_fork, 0),
        "vfork" => (libc::SYS_vfork, 0),
        "clone" => (libc::SYS_clone, libc::SIGCHLD as libc::c_long),
        // Invalid arguments still reach seccomp before syscall validation.
        "clone3" => (libc::SYS_clone3, 0),
        "io-uring" => (libc::SYS_io_uring_setup, 0),
        "x32" => (libc::SYS_getpid | 0x4000_0000, 0),
        "legacy" => (512, 0),
        _ => return Err(baseline::refused()),
    };
    let result: libc::c_long;
    // SAFETY: the only valid child-producing calls use a null child stack and
    // SIGCHLD semantics. A child exits inside this assembly before returning to
    // Rust, including under vfork. Every optional syscall pointer is null.
    unsafe {
        std::arch::asm!(
            "syscall",
            "test rax, rax",
            "jnz 2f",
            "mov eax, 60",
            "xor edi, edi",
            "syscall",
            "ud2",
            "2:",
            inlateout("rax") number => result,
            inlateout("rdi") first => _,
            in("rsi") 0usize,
            in("rdx") 0usize,
            in("r10") 0usize,
            in("r8") 0usize,
            in("r9") 0usize,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    baseline::require(matches!(attempt, "fork" | "vfork" | "clone") && result > 0)?;
    let child = libc::pid_t::try_from(result).map_err(|_| baseline::refused())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        baseline::require(Instant::now() < deadline)?;
        let mut status = 0;
        // SAFETY: waitpid observes only the fixture's freshly created child.
        let observed = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if observed == child {
            return baseline::require(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        }
        if observed < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn attempt_task_creation(_: &str) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(libc::ENOTSUP))
}

fn verify_task_attempt(attempt: &str, restricted: bool) -> io::Result<()> {
    let (control, child) = Control::pair()?;
    let child_spec = spec(
        vec![
            "--registration-task-marker".into(),
            attempt.into(),
            child.as_raw_fd().to_string().into(),
        ],
        vec![child],
    )?;
    let mut owner = SpawnOwner::new()?;
    let receipt = if restricted {
        owner.spawn_task_restricted(child_spec)?
    } else {
        owner.spawn(child_spec)?
    };
    // The ready byte comes from the executed helper before its syscall gate.
    control.expect(b'R')?;
    control.send(b'G')?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let exit = loop {
        baseline::require(Instant::now() < deadline)?;
        if let Some(exit) = owner.try_wait(receipt.token)? {
            break exit;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    baseline::require(exit.pid == receipt.pid && exit.token == receipt.token)?;
    if restricted {
        baseline::require(exit.code.is_none() && exit.signal == Some(libc::SIGSYS))?;
    } else {
        baseline::require(exit.code == Some(0) && exit.signal.is_none())?;
        control.expect(b'D')?;
    }
    baseline::require(owner.shutdown(Duration::from_secs(2))? == [exit])
}

pub(super) fn verify_task_creation_restriction() -> io::Result<()> {
    verify_task_attempt("fork", false)?;
    for attempt in TASK_ATTEMPTS {
        verify_task_attempt(attempt, true)?;
    }
    Ok(())
}

extern "C" fn fixture_sigchld_handler(_: libc::c_int) {}

pub(super) fn verify_sigchld_admission() -> io::Result<()> {
    for (handler, flags) in [
        (libc::SIG_IGN, 0),
        (libc::SIG_DFL, libc::SA_NOCLDWAIT),
        (
            fixture_sigchld_handler as *const () as libc::sighandler_t,
            0,
        ),
    ] {
        let mut owner = SpawnOwner::new()?;
        let (_control, marker) = marker_spec()?;
        // SAFETY: both objects are initialized storage for sigaction.
        let mut action: libc::sigaction = unsafe { zeroed() };
        let mut original: libc::sigaction = unsafe { zeroed() };
        action.sa_sigaction = handler;
        action.sa_flags = flags;
        // SAFETY: the mask is writable, and the action changes only SIGCHLD
        // in the disposable reader. Save the previous disposition atomically.
        if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0
            || unsafe { libc::sigaction(libc::SIGCHLD, &action, &mut original) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let attempted = owner.spawn(marker);
        // SAFETY: restore the saved disposition before inspecting either result.
        if unsafe { libc::sigaction(libc::SIGCHLD, &original, std::ptr::null_mut()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let error = attempted.err().ok_or_else(baseline::refused)?;
        baseline::require(error.kind() == io::ErrorKind::PermissionDenied)?;
        let missing = owner.receipt(1).err().ok_or_else(baseline::refused)?;
        baseline::require(missing.raw_os_error() == Some(libc::ENOENT))?;
    }
    Ok(())
}

pub(super) fn verify_dedicated_host_admission() -> io::Result<()> {
    let mut owner = SpawnOwner::new_dedicated()?;
    let (_control, marker) = marker_spec()?;
    let (ready, started) = mpsc::channel();
    let (release, held) = mpsc::channel::<()>();
    let contender = std::thread::Builder::new().spawn(move || {
        let _ = ready.send(());
        let _ = held.recv();
    })?;
    let attempted = started
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| baseline::refused())
        .and_then(|()| owner.spawn(marker));
    drop(release);
    contender.join().map_err(|_| baseline::refused())?;
    let error = attempted.err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::PermissionDenied)?;

    // The kernel can still list the exiting thread briefly after join.
    let deadline = Instant::now() + Duration::from_secs(3);
    while std::fs::read_dir("/proc/self/task")?.nth(1).is_some() {
        baseline::require(Instant::now() < deadline)?;
        std::thread::sleep(Duration::from_millis(2));
    }
    let (_control, marker) = marker_spec()?;
    let error = owner.spawn(marker).err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::PermissionDenied)
}

fn marker(owner: &mut SpawnOwner) -> io::Result<(Control, Receipt)> {
    let (control, spec) = marker_spec()?;
    let receipt = owner.spawn(spec)?;
    control.expect(b'R')?;
    Ok((control, receipt))
}

fn open_fixture_pidfd(pid: u32) -> io::Result<OwnedFd> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| baseline::refused())?;
    // SAFETY: pidfd_open takes scalar arguments and returns a fresh descriptor.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful pidfd_open transfers an owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw as RawFd) })
}

fn exercise_process_pair(
    owner: &SpawnOwner,
    first: Receipt,
    second: Receipt,
) -> io::Result<([OwnedFd; 2], [OwnedFd; 2])> {
    let expected = [
        owner.duplicate_pidfd(first.token)?,
        owner.duplicate_pidfd(second.token)?,
    ];
    let mut offered = [
        open_fixture_pidfd(first.pid)?,
        open_fixture_pidfd(second.pid)?,
    ];
    verify_live_process_pair(&expected, &offered)?;

    offered.swap(0, 1);
    baseline::require(verify_live_process_pair(&expected, &offered).is_err())?;
    offered.swap(0, 1);

    let duplicate = [offered[0].try_clone()?, offered[0].try_clone()?];
    baseline::require(verify_live_process_pair(&expected, &duplicate).is_err())?;
    baseline::require(verify_live_process_pair(&duplicate, &duplicate).is_err())?;

    let substitute = [
        offered[0].try_clone()?,
        open_fixture_pidfd(std::process::id())?,
    ];
    baseline::require(verify_live_process_pair(&expected, &substitute).is_err())?;
    let non_pidfd = [offered[0].try_clone()?, null()?];
    baseline::require(verify_live_process_pair(&expected, &non_pidfd).is_err())?;
    Ok((expected, offered))
}

fn raw_response(socket: &OwnedFd, descriptors: &[RawFd], wrong_challenge: bool) -> io::Result<()> {
    let mut frame = [0u8; 48];
    baseline::require(receive(socket, &mut frame)? == frame.len())?;
    baseline::require(
        &frame[..8] == b"vv-proc\0" && frame[8] == 1 && frame[9] == 1 && frame[10..16] == [0; 6],
    )?;
    frame[9] = 2;
    if wrong_challenge {
        frame[16] ^= 1;
    }
    let mut vector = libc::iovec {
        iov_base: frame.as_mut_ptr().cast(),
        iov_len: frame.len(),
    };
    // SAFETY: zeroed msghdr is filled with the live vector and aligned control storage.
    let mut message: libc::msghdr = unsafe { zeroed() };
    let mut control = [0usize; 64];
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    // SAFETY: CMSG_SPACE only computes the complete initialized payload size.
    message.msg_controllen =
        unsafe { libc::CMSG_SPACE(std::mem::size_of_val(descriptors) as libc::c_uint) as usize };
    baseline::require(message.msg_controllen <= std::mem::size_of_val(&control))?;
    // SAFETY: the first header and payload fit within the aligned control storage.
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        baseline::require(!header.is_null())?;
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(descriptors) as _) as usize;
        std::ptr::copy_nonoverlapping(
            descriptors.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(header),
            std::mem::size_of_val(descriptors),
        );
        baseline::require(
            libc::sendmsg(
                socket.as_raw_fd(),
                &message,
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            ) == frame.len() as isize,
        )
    }
}

fn owned_supervisor(case: &str, transport: OwnedFd, control: Control) -> io::Result<()> {
    let mut owner = SpawnOwner::new()?;
    let (first_gate, first) = marker(&mut owner)?;
    let (second_gate, second) = marker(&mut owner)?;
    let extra = if case == "owned-extra-child" {
        Some(marker(&mut owner)?)
    } else {
        None
    };
    if case == "owned-dead-child" {
        first_gate.send(b'G')?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            baseline::require(Instant::now() < deadline)?;
            if let Some(exit) = owner.try_wait(first.token)? {
                baseline::require(
                    exit.token == first.token
                        && exit.pid == first.pid
                        && exit.code == Some(0)
                        && exit.signal.is_none(),
                )?;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    control.send(b'R')?;
    let process_pair = if case == "owned-pair" {
        Some(exercise_process_pair(&owner, first, second)?)
    } else {
        None
    };
    match case {
        "owned-pair" => {
            let registration = owner.prepare_child_registration()?;
            registration.validate()?;
            let mut sender =
                RegistrationSender::new(transport, Instant::now() + Duration::from_secs(3))?;
            sender.send_registered(&registration)?;
        }
        "owned-extra-child" | "owned-dead-child" => {
            baseline::require(owner.prepare_child_registration().is_err())?;
            drop(transport);
        }
        "owned-spawn-after-drop" | "owned-repeated-capture" => {
            {
                let registration = owner.prepare_child_registration()?;
                registration.validate()?;
            }
            if case == "owned-spawn-after-drop" {
                baseline::require(marker(&mut owner).is_err())?;
            } else {
                baseline::require(owner.prepare_child_registration().is_err())?;
            }
            drop(transport);
        }
        _ => return Err(baseline::refused()),
    }
    if case != "owned-dead-child" {
        baseline::require(owner.try_wait(first.token)?.is_none())?;
    }
    baseline::require(owner.try_wait(second.token)?.is_none())?;
    if let Some((_, receipt)) = &extra {
        baseline::require(owner.try_wait(receipt.token)?.is_none())?;
    }
    control.send(b'D')?;
    control.expect(b'G')?;
    if case != "owned-dead-child" {
        first_gate.send(b'G')?;
    }
    second_gate.send(b'G')?;
    let mut receipts = vec![first, second];
    if let Some((gate, receipt)) = extra {
        gate.send(b'G')?;
        receipts.push(receipt);
    }
    collect(&mut owner, &receipts)?;
    if let Some((expected, offered)) = process_pair {
        baseline::require(verify_live_process_pair(&expected, &offered).is_err())?;
    }
    control.send(b'C')
}

fn supervisor(case: &str, transport: OwnedFd, control: Control) -> io::Result<()> {
    if case.starts_with("owned-") {
        return owned_supervisor(case, transport, control);
    }
    let mut owner = SpawnOwner::new()?;
    let (first_gate, first) = marker(&mut owner)?;
    let (second_gate, second) = marker(&mut owner)?;
    let receipts = [first, second];
    control.send(b'R')?;
    let mut forwarder = None;
    if matches!(
        case,
        "valid" | "consumed-rejected" | "duplicate-token-rejected"
    ) {
        let mut sender =
            RegistrationSender::new(transport, Instant::now() + Duration::from_secs(3))?;
        let tokens = if case == "duplicate-token-rejected" {
            [first.token, first.token]
        } else {
            [first.token, second.token]
        };
        let result = sender.send(&owner, tokens);
        baseline::require(result.is_ok() != (case == "duplicate-token-rejected"))?;
    } else if case == "forwarded-sender-rejected" {
        let first_fd = owner.duplicate_pidfd(first.token)?;
        let second_fd = owner.duplicate_pidfd(second.token)?;
        let (gate, child_gate) = Control::pair()?;
        let forwarded = owner.spawn(spec(
            vec![
                "--registration-forwarder".into(),
                transport.as_raw_fd().to_string().into(),
                child_gate.as_raw_fd().to_string().into(),
                first_fd.as_raw_fd().to_string().into(),
                second_fd.as_raw_fd().to_string().into(),
            ],
            vec![transport, child_gate, first_fd, second_fd],
        )?)?;
        gate.expect(b'R')?;
        forwarder = Some((gate, forwarded));
    } else if case != "timeout" {
        let first_fd = owner.duplicate_pidfd(first.token)?;
        let second_fd = owner.duplicate_pidfd(second.token)?;
        let unrelated = null()?;
        let descriptors = match case {
            "nonpidfd-rejected" => vec![unrelated.as_raw_fd(), second_fd.as_raw_fd()],
            "missing-reference-rejected" => vec![first_fd.as_raw_fd()],
            "extra-reference-rejected" => {
                vec![
                    first_fd.as_raw_fd(),
                    second_fd.as_raw_fd(),
                    unrelated.as_raw_fd(),
                ]
            }
            "wrong-challenge-rejected" => vec![first_fd.as_raw_fd(), second_fd.as_raw_fd()],
            "truncated-rights-rejected" => vec![first_fd.as_raw_fd(); 80],
            _ => return Err(baseline::refused()),
        };
        raw_response(&transport, &descriptors, case == "wrong-challenge-rejected")?;
    }
    control.send(b'D')?;
    control.expect(b'G')?;
    first_gate.send(b'G')?;
    second_gate.send(b'G')?;
    let mut all = receipts.to_vec();
    if let Some((gate, receipt)) = forwarder {
        gate.send(b'G')?;
        all.push(receipt);
    }
    collect(&mut owner, &all)?;
    control.send(b'C')
}

fn keeper_supervisor(challenge: OwnedFd, offer: OwnedFd, control: Control) -> io::Result<()> {
    let mut receiver =
        KeeperChallengeReceiver::for_parent(challenge, Instant::now() + Duration::from_secs(3))?;
    let _challenge = receiver.receive()?;
    let error = receiver.receive().err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
    control.send(b'R')?;
    let offer =
        KeeperChallengeReceiver::for_parent(offer, Instant::now() + Duration::from_secs(3))?;
    let mut sender = offer.into_offer_sender()?;
    let references = [
        File::open("/proc/self")?.into(),
        null()?,
        File::open("/dev/zero")?.into(),
    ];
    let commitment = [0x5a; 32];
    sender.send(commitment, &references)?;
    let error = sender
        .send(commitment, &references)
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
    control.expect(b'G')?;
    drop(sender);
    drop(receiver);
    Ok(())
}

fn admitted_bootstrap_supervisor(
    credential: &str,
    confirmation: ConfirmationCase,
    transport: OwnedFd,
    control: Control,
) -> io::Result<()> {
    let mut launch = launch_preparation(credential)?;
    let commitment = launch.keeper_admission_commitment(&[1; 32])?;
    let (first_gate, first) = marker_spec()?;
    let (second_gate, second) = marker_spec()?;
    let registration = probe_launch_admitted_pair(&mut launch, [first, second])?;
    first_gate.expect(b'R')?;
    second_gate.expect(b'R')?;
    control.send(b'R')?;
    let bootstrap =
        KeeperChallengeReceiver::for_parent(transport, Instant::now() + Duration::from_secs(3))?
            .send_launch_context(&registration)?;
    bootstrap.validate()?;
    let (second_channel, _unused_peer) = packet_pair()?;
    let retry = KeeperChallengeReceiver::for_parent(
        second_channel,
        Instant::now() + Duration::from_secs(3),
    )?;
    let error = retry
        .send_launch_context(&registration)
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
    control.send_packet(&commitment)?;
    let offer = bootstrap.send_offer()?;
    offer.validate()?;
    match confirmation {
        ConfirmationCase::Delivered => {
            let confirmed = offer.receive_confirmation()?;
            confirmed.validate()?;
            control.send(b'C')?;
            control.expect(b'G')?;
        }
        ConfirmationCase::SupervisorExited => {
            control.expect(b'G')?;
            drop(offer);
        }
    }
    drop(registration);

    first_gate.send(b'G')?;
    second_gate.send(b'G')?;
    verify_admitted_exit_observation(credential, &mut launch, 2)?;
    control.send(b'X')?;
    control.expect(b'Y')
}

fn admitted_keeper(credential: &str, confirmation: ConfirmationCase) -> io::Result<()> {
    let admission = CurrentAdmission::open_fixed_namespace(credential)?;
    let (parent, child) = packet_pair()?;
    let (control, child_control) = Control::pair()?;
    let mut owner = SpawnOwner::new_dedicated()?;
    let receipt = owner.spawn(spec(
        vec![
            "--registration-admitted-bootstrap-supervisor".into(),
            credential.into(),
            confirmation.argument(),
            child.as_raw_fd().to_string().into(),
            child_control.as_raw_fd().to_string().into(),
        ],
        vec![child, child_control],
    )?)?;
    control.expect(b'R')?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let bootstrap =
        KeeperChallengeSender::for_owned_child(parent, &owner, receipt.token, deadline)?
            .receive_launch_context(admission)?;
    bootstrap.validate()?;
    let challenge = [1; 32];
    let commitment = bootstrap.expected_commitment(&challenge)?;
    let mut supervisor_commitment = [0; 32];
    let count = receive(&control.0, &mut supervisor_commitment)?;
    baseline::require(count == commitment.len() && supervisor_commitment == commitment)?;
    baseline::require(bootstrap.expected_commitment(&challenge)? == commitment)?;
    baseline::require(bootstrap.expected_commitment(&[2; 32])? != commitment)?;
    baseline::require(bootstrap.expected_commitment(&[0; 32]).is_err())?;
    let offer = bootstrap.receive_offer()?;
    offer.validate()?;
    let mut adopted = offer.adopt()?;
    baseline::require(!adopted.observe_registered_exits()?)?;

    if let ConfirmationCase::Delivered = confirmation {
        adopted.confirm()?;
        control.expect(b'C')?;
    }
    control.send(b'G')?;
    control.expect(b'X')?;
    baseline::require(!adopted.observe_registered_exits()?)?;
    control.send(b'Y')?;
    collect(&mut owner, &[receipt])?;
    baseline::require(Instant::now() < deadline)?;
    baseline::require(adopted.observe_registered_exits()?)?;
    baseline::require(adopted.observe_registered_exits()?)?;
    let error = rust_launcher_probe::probe_advance_pre_unlock_generation(&mut adopted)
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::PermissionDenied)?;
    if let ConfirmationCase::SupervisorExited = confirmation {
        baseline::require(adopted.confirm().is_err())?;
    }
    let error = adopted.confirm().err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)?;

    expect_adopted_reservation(credential)?;
    drop(adopted);
    expect_adopted_reservation(credential)
}

pub(super) fn expect_adopted_reservation(credential: &str) -> io::Result<()> {
    let competing =
        CurrentAdmission::open_fixed_namespace(credential)?.prepare_native_material()?;
    let error = competing.reserve().err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::WouldBlock)
}

pub(super) fn verify_admitted_bootstrap(credential: &str) -> io::Result<()> {
    for confirmation in [
        ConfirmationCase::Delivered,
        ConfirmationCase::SupervisorExited,
    ] {
        let mut owner = SpawnOwner::new()?;
        let receipt = owner.spawn(spec(
            vec![
                "--registration-admitted-keeper".into(),
                credential.into(),
                confirmation.argument(),
            ],
            Vec::new(),
        )?)?;
        collect(&mut owner, &[receipt])?;
        CurrentAdmission::open_fixed_namespace(credential)?
            .prepare_native_material()?
            .reserve()?
            .validate()?;
    }
    Ok(())
}

fn openssh_supervisor(
    credential: &str,
    home: &Path,
    transport: OwnedFd,
    terminal: OwnedFd,
    control: Control,
) -> io::Result<()> {
    let mut phase = b'M';
    let result = (|| {
        let mut launch = launch_preparation(credential)?;
        phase = b'O';
        rust_launcher_probe::probe_with_admitted_openssh_registration(
            &mut launch,
            home,
            |registration, master| {
                phase = b'T';
                let deadline = Instant::now() + Duration::from_secs(3);
                rust_launcher_probe::probe_send_keeper_terminal(&terminal, master, deadline)?;
                control.send(b'R')?;
                phase = b'R';
                let bootstrap = KeeperChallengeReceiver::for_parent(transport, deadline)?
                    .send_launch_context(registration)?;
                let _confirmed = bootstrap.send_offer()?.receive_confirmation()?;
                control.send(b'C')?;
                // The keeper kills this process while the loader remains at its prompt.
                control.expect(b'G')?;
                Err(baseline::refused())
            },
        )
    })();
    if result.is_err() && phase != b'R' {
        let _ = control.send_packet(&[b'E', phase]);
    }
    result
}

fn wait_openssh_predecessor(control: &Control) -> io::Result<()> {
    let mut packet = [0; 2];
    // Separate process readiness from the later three-second authenticated exchange.
    let deadline = Instant::now() + Duration::from_secs(10);
    let count =
        receive_before(&control.0, &mut packet, deadline).map_err(keeper_stage("keeper-ready"))?;
    if count == 1 && packet[0] == b'R' {
        return Ok(());
    }
    let label = match (count, packet) {
        (0, _) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "keeper-ready")),
        (2, [b'E', b'M']) => "keeper-predecessor-material",
        (2, [b'E', b'O']) => "keeper-predecessor-openssh",
        (2, [b'E', b'T']) => "keeper-predecessor-terminal",
        _ => "keeper-ready",
    };
    Err(io::Error::new(io::ErrorKind::InvalidData, label))
}

fn openssh_keeper(credential: &str) -> io::Result<()> {
    let successor_home = TemporaryOpenSshHome::create()?;
    with_restricted_keeper_successor(credential, |successor| {
        let mut contention_observed = false;
        rust_launcher_probe::probe_launch_restricted_successor(
            successor,
            successor_home.path(),
            || {
                expect_adopted_reservation(credential)?;
                contention_observed = true;
                Ok(())
            },
        )?;
        baseline::require(contention_observed)?;
        baseline::require(std::fs::read_dir(successor_home.path())?.next().is_none())?;
        std::fs::remove_dir(successor_home.path())
    })
}

fn active_supervisor(
    credential: &str,
    home: &Path,
    transport: OwnedFd,
    control: Control,
    keeper_death_cut: bool,
    opening_permit: bool,
) -> io::Result<()> {
    if let Err(error) =
        receive_supervisor_start(&transport, Instant::now() + Duration::from_secs(3))
    {
        control.send(b'X')?;
        return Err(keeper_stage("active-supervisor-start")(error));
    }
    let register =
        |registration: &crate::linux_current_admission::AdmittedChildRegistration<'_>| {
            control.send(b'R')?;
            let confirmed = KeeperChallengeReceiver::for_parent(
                transport,
                Instant::now() + Duration::from_secs(3),
            )?
            .send_launch_context(registration)?
            .send_offer()?
            .receive_confirmation()?;
            let sender = confirmed.into_active_closure_sender()?;
            control.send(b'C')?;
            Ok(sender)
        };
    let (mut session, provider, sender) = if opening_permit {
        CurrentAdmission::open_fixed_namespace(credential)?
            .prepare_native_material()?
            .reserve()?
            .prepare_attended_opening()?
            .probe_unlock_registered_active(home, register)?
    } else {
        rust_launcher_probe::probe_unlock_registered_active(
            launch_preparation(credential)?,
            home,
            register,
        )?
    };
    let (_generation, connection) = crate::endpoint_native_probe::exchange_as_custodian_with_cut(
        credential,
        home,
        &mut session,
        &provider,
        None,
        || {
            if !keeper_death_cut {
                return Ok(());
            }
            control.send(b'P')?;
            // Stay at the cut even if the dying keeper closes its control socket.
            std::thread::sleep(Duration::from_secs(15));
            Err(io::ErrorKind::TimedOut.into())
        },
    )?;
    control.send(b'D')?;
    let mut command = [0; 2];
    baseline::require(receive(&control.0, &mut command)? == 1)?;
    if command[0] == b'L' {
        let error = sender
            .send(&mut session, Instant::now() + Duration::from_secs(3))
            .err()
            .ok_or_else(baseline::refused)?;
        baseline::require(error.kind() == io::ErrorKind::PermissionDenied)?;
        control.send(b'X')?;
        control.expect(b'A')?;
    } else if command[0] != b'M' {
        let deadline = Instant::now() + Duration::from_secs(3);
        let sent = match command[0] {
            b'S' | b'U' => sender.send(&mut session, deadline)?,
            b'I' => {
                sender.send_fixture_fault(&mut session, deadline, ActiveClosureFault::Incomplete)?
            }
            b'W' => sender.send_fixture_fault(
                &mut session,
                deadline,
                ActiveClosureFault::DifferentGeneration,
            )?,
            _ => return Err(baseline::refused()),
        };
        if command[0] == b'S' {
            sent.receive_confirmation(Instant::now() + Duration::from_secs(3))
                .map_err(keeper_stage("active-close-confirmed"))?;
        } else {
            control.expect(b'A')?;
        }
    }
    drop(connection);
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActiveRelayCase {
    Normal,
    Missing,
    Incomplete,
    DifferentGeneration,
    ConnectionLost,
    KeeperLost,
    PermitClose,
    PermitUnconfirmed,
    PermitMissing,
    PermitKeeperLost,
}

impl ActiveRelayCase {
    pub(super) fn parse(case: &OsStr) -> io::Result<Self> {
        match case.to_str() {
            Some("normal") => Ok(Self::Normal),
            Some("missing") => Ok(Self::Missing),
            Some("incomplete") => Ok(Self::Incomplete),
            Some("different-generation") => Ok(Self::DifferentGeneration),
            Some("connection-lost") => Ok(Self::ConnectionLost),
            Some("keeper-lost") => Ok(Self::KeeperLost),
            Some("permit-close") => Ok(Self::PermitClose),
            Some("permit-unconfirmed") => Ok(Self::PermitUnconfirmed),
            Some("permit-missing") => Ok(Self::PermitMissing),
            Some("permit-keeper-lost") => Ok(Self::PermitKeeperLost),
            _ => Err(baseline::refused()),
        }
    }

    pub(super) fn refusal(self) -> Option<&'static str> {
        match self {
            Self::Normal
            | Self::Missing
            | Self::KeeperLost
            | Self::PermitClose
            | Self::PermitUnconfirmed
            | Self::PermitMissing
            | Self::PermitKeeperLost => None,
            Self::Incomplete => Some("incomplete"),
            Self::DifferentGeneration => Some("different-generation"),
            Self::ConnectionLost => Some("connection-lost"),
        }
    }

    fn command(self) -> io::Result<u8> {
        match self {
            Self::Normal | Self::PermitClose => Ok(b'S'),
            Self::PermitUnconfirmed => Ok(b'U'),
            Self::Missing | Self::PermitMissing => Ok(b'M'),
            Self::Incomplete => Ok(b'I'),
            Self::DifferentGeneration => Ok(b'W'),
            Self::ConnectionLost => Ok(b'L'),
            Self::KeeperLost | Self::PermitKeeperLost => Err(baseline::refused()),
        }
    }

    fn keeper_lost(self) -> bool {
        matches!(self, Self::KeeperLost | Self::PermitKeeperLost)
    }
}

pub(super) fn with_active_keeper_successor<T>(
    credential: &str,
    home: &Path,
    case: ActiveRelayCase,
    operation: impl FnOnce(KeeperPreUnlockSuccessor<'_>) -> io::Result<T>,
) -> io::Result<Option<T>> {
    let previous = home.join("previous");
    std::fs::DirBuilder::new().mode(0o700).create(&previous)?;
    let admission = CurrentAdmission::open_fixed_namespace(credential)?;
    let (transport, child_transport) = packet_pair()?;
    let disconnect = if case == ActiveRelayCase::ConnectionLost {
        Some(transport.try_clone()?)
    } else {
        None
    };
    let (control, child_control) = Control::pair()?;
    let mut owner = SpawnOwner::new_dedicated()?;
    let mut arguments = vec![
        "--registration-active-supervisor".into(),
        credential.into(),
        previous.as_os_str().to_owned(),
        child_transport.as_raw_fd().to_string().into(),
        child_control.as_raw_fd().to_string().into(),
    ];
    match case {
        ActiveRelayCase::KeeperLost => arguments.push("keeper-lost".into()),
        ActiveRelayCase::PermitClose
        | ActiveRelayCase::PermitUnconfirmed
        | ActiveRelayCase::PermitMissing => {
            arguments.push("opening-permit".into());
        }
        ActiveRelayCase::PermitKeeperLost => arguments.push("opening-permit-keeper-lost".into()),
        _ => {}
    }
    let receipt = owner.spawn(spec(arguments, vec![child_transport, child_control])?)?;
    authorize_supervisor_start(
        &transport,
        &owner,
        receipt.token,
        Instant::now() + Duration::from_secs(3),
    )
    .map_err(keeper_stage("active-keeper-start"))?;
    wait_openssh_predecessor(&control).map_err(keeper_stage("active-old-ready"))?;
    let bootstrap = KeeperChallengeSender::for_owned_child(
        transport,
        &owner,
        receipt.token,
        Instant::now() + Duration::from_secs(3),
    )
    .and_then(|channel| channel.receive_launch_context(admission))
    .map_err(keeper_stage("keeper-bootstrap"))?;
    let mut adopted = bootstrap
        .receive_offer()
        .and_then(|offer| offer.adopt())
        .map_err(keeper_stage("keeper-adopt"))?;
    adopted.confirm().map_err(keeper_stage("keeper-confirm"))?;
    control
        .expect(b'C')
        .map_err(keeper_stage("keeper-confirm-received"))?;
    // The supervisor needs time for the independent verifier and a possible
    // attended input. The authenticated closure exchange has its own deadline.
    let mut closed = [0; 2];
    let count = receive_before(
        &control.0,
        &mut closed,
        Instant::now() + Duration::from_secs(45),
    )
    .map_err(keeper_stage("active-old-closed"))?;
    let expected_phase = if case.keeper_lost() { b'P' } else { b'D' };
    baseline::require(count == 1 && closed[0] == expected_phase)?;
    crate::endpoint_native_probe::wait_release()?;
    baseline::require(adopted.probe_advance_active_generation().is_err())?;
    baseline::require(adopted.probe_publish_clean_close().is_err())?;
    if case.keeper_lost() {
        baseline::require(!adopted.observe_registered_exits()?)?;
        expect_adopted_reservation(credential)?;
        println!(
            "{}",
            json!({"active_keeper_cut": true, "supervisor_pid": receipt.pid})
        );
        io::stdout().flush()?;
        crate::endpoint_native_probe::wait_release()?;
        return Err(baseline::refused());
    }

    let command = case.command()?;
    if let Some(transport) = disconnect {
        // SAFETY: the fixture holds a duplicate of its own connected socket.
        baseline::require(unsafe { libc::shutdown(transport.as_raw_fd(), libc::SHUT_RDWR) } == 0)?;
        control.send(command)?;
        control.expect(b'X')?;
        baseline::require(!adopted.observe_registered_exits()?)?;
        let refusal = adopted
            .probe_receive_active_closure(Instant::now() + Duration::from_secs(3))
            .err()
            .ok_or_else(baseline::refused)?;
        baseline::require(refusal.kind() == io::ErrorKind::PermissionDenied)?;
        expect_adopted_reservation(credential)?;
        control.send(b'A')?;
    } else {
        control.send(command)?;
    }
    if !matches!(
        case,
        ActiveRelayCase::Missing | ActiveRelayCase::PermitMissing | ActiveRelayCase::ConnectionLost
    ) {
        let received =
            adopted.probe_receive_active_closure(Instant::now() + Duration::from_secs(3));
        if matches!(
            case,
            ActiveRelayCase::Normal
                | ActiveRelayCase::PermitClose
                | ActiveRelayCase::PermitUnconfirmed
        ) {
            received.map_err(keeper_stage("active-close-frame"))?;
            let live = adopted
                .probe_advance_active_generation()
                .err()
                .ok_or_else(baseline::refused)?;
            baseline::require(live.kind() == io::ErrorKind::WouldBlock)?;
            if matches!(
                case,
                ActiveRelayCase::PermitClose | ActiveRelayCase::PermitUnconfirmed
            ) {
                let live = adopted
                    .probe_publish_clean_close()
                    .err()
                    .ok_or_else(baseline::refused)?;
                baseline::require(live.kind() == io::ErrorKind::WouldBlock)?;
            }
        } else {
            let error = received.err().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "active-close-refusal")
            })?;
            if error.kind() != io::ErrorKind::InvalidData {
                return Err(keeper_stage("active-close-refusal")(error));
            }
        }
        baseline::require(
            adopted
                .probe_receive_active_closure(Instant::now() + Duration::from_secs(3))
                .is_err(),
        )?;
        if matches!(case, ActiveRelayCase::Normal | ActiveRelayCase::PermitClose) {
            adopted
                .confirm_active_closure(Instant::now() + Duration::from_secs(3))
                .map_err(keeper_stage("active-close-confirm"))?;
            baseline::require(
                adopted
                    .confirm_active_closure(Instant::now() + Duration::from_secs(3))
                    .is_err(),
            )?;
        } else {
            control.send(b'A')?;
        }
    }
    collect(&mut owner, &[receipt]).map_err(keeper_stage("active-old-exit"))?;
    baseline::require(adopted.observe_registered_exits()?)?;
    expect_adopted_reservation(credential)?;
    baseline::require(std::fs::read_dir(&previous)?.next().is_none())?;
    std::fs::remove_dir(&previous)?;
    baseline::require(
        rust_launcher_probe::probe_advance_pre_unlock_generation(&mut adopted).is_err(),
    )?;

    let result = if case == ActiveRelayCase::PermitClose {
        adopted
            .probe_publish_clean_close()
            .map_err(keeper_stage("active-close-publish"))?;
        baseline::require(adopted.probe_publish_clean_close().is_err())?;
        baseline::require(adopted.probe_advance_active_generation().is_err())?;
        None
    } else if case == ActiveRelayCase::PermitUnconfirmed {
        let refusal = adopted
            .probe_publish_clean_close()
            .err()
            .ok_or_else(baseline::refused)?;
        baseline::require(refusal.kind() == io::ErrorKind::PermissionDenied)?;
        let refusal = adopted
            .probe_advance_active_generation()
            .err()
            .ok_or_else(baseline::refused)?;
        baseline::require(refusal.kind() == io::ErrorKind::PermissionDenied)?;
        None
    } else if case != ActiveRelayCase::Normal {
        baseline::require(
            adopted
                .probe_receive_active_closure(Instant::now() + Duration::from_secs(3))
                .is_err(),
        )?;
        baseline::require(adopted.probe_advance_active_generation().is_err())?;
        baseline::require(adopted.probe_publish_clean_close().is_err())?;
        None
    } else {
        // The legacy active launch may advance a live successor, but it cannot
        // mint a persisted permit.
        baseline::require(adopted.probe_publish_clean_close().is_err())?;
        let successor = adopted
            .probe_advance_active_generation()
            .map_err(keeper_stage("active-advance"))?;
        let (old, current) = successor.generations();
        baseline::require(old != current && successor.validate_generation(&old).is_err())?;
        let result = operation(successor)?;
        baseline::require(adopted.probe_advance_active_generation().is_err())?;
        Some(result)
    };
    expect_adopted_reservation(credential)?;
    if case == ActiveRelayCase::ConnectionLost {
        drop(adopted);
        expect_adopted_reservation(credential)?;
    }
    Ok(result)
}

fn keeper_stage(stage: &'static str) -> impl FnOnce(io::Error) -> io::Error {
    move |error| io::Error::new(error.kind(), stage)
}

pub(super) fn with_restricted_keeper_successor<T>(
    credential: &str,
    operation: impl FnOnce(KeeperPreUnlockSuccessor<'_>) -> io::Result<T>,
) -> io::Result<T> {
    // SAFETY: this disposable keeper adopts only its own orphaned descendants.
    baseline::require(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == 0)?;
    let mut subreaper = 0;
    // SAFETY: the output points to one complete initialized integer.
    baseline::require(unsafe {
        libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut subreaper, 0, 0, 0) == 0 && subreaper == 1
    })?;
    let home = TemporaryOpenSshHome::create()?;
    let admission = CurrentAdmission::open_fixed_namespace(credential)?;
    let (transport, child_transport) = packet_pair()?;
    let (terminal, child_terminal) = packet_pair()?;
    let (control, child_control) = Control::pair()?;
    let mut owner = SpawnOwner::new_dedicated().map_err(keeper_stage("keeper-owner"))?;
    let receipt = owner
        .spawn(spec(
            vec![
                "--registration-openssh-supervisor".into(),
                credential.into(),
                home.path().as_os_str().to_owned(),
                child_transport.as_raw_fd().to_string().into(),
                child_terminal.as_raw_fd().to_string().into(),
                child_control.as_raw_fd().to_string().into(),
            ],
            vec![child_transport, child_terminal, child_control],
        )?)
        .map_err(keeper_stage("keeper-launch"))?;
    wait_openssh_predecessor(&control)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let master = rust_launcher_probe::probe_receive_keeper_terminal(
        &terminal,
        &owner,
        receipt.token,
        deadline,
    )
    .map_err(keeper_stage("keeper-terminal"))?;
    let bootstrap =
        KeeperChallengeSender::for_owned_child(transport, &owner, receipt.token, deadline)
            .and_then(|channel| channel.receive_launch_context(admission))
            .map_err(keeper_stage("keeper-bootstrap"))?;
    let offer = bootstrap
        .receive_offer()
        .map_err(keeper_stage("keeper-offer"))?;
    let mut adopted = offer.adopt().map_err(keeper_stage("keeper-adopt"))?;
    baseline::require(!adopted.observe_registered_exits()?)?;
    let error = rust_launcher_probe::probe_advance_pre_unlock_generation(&mut adopted)
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::PermissionDenied)?;
    adopted.confirm().map_err(keeper_stage("keeper-confirm"))?;
    control
        .expect(b'C')
        .map_err(keeper_stage("keeper-confirm-received"))?;
    expect_adopted_reservation(credential)?;
    baseline::require(!adopted.observe_registered_exits()?)?;
    let error = rust_launcher_probe::probe_advance_pre_unlock_generation(&mut adopted)
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::WouldBlock)?;
    owner
        .signal(receipt.token, libc::SIGKILL)
        .map_err(keeper_stage("keeper-stop"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let supervisor_exit = loop {
        baseline::require(Instant::now() < deadline)?;
        if let Some(exit) = owner
            .try_wait(receipt.token)
            .map_err(keeper_stage("keeper-stop"))?
        {
            break exit;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    baseline::require(
        supervisor_exit.token == receipt.token
            && supervisor_exit.pid == receipt.pid
            && supervisor_exit.code.is_none()
            && supervisor_exit.signal == Some(libc::SIGKILL),
    )?;
    while !adopted.observe_registered_exits()? {
        baseline::require(Instant::now() < deadline)?;
        std::thread::sleep(Duration::from_millis(2));
    }
    rust_launcher_probe::probe_collect_keeper_provider_sigkill(&mut adopted)
        .map_err(keeper_stage("keeper-collect"))?;
    baseline::require(adopted.observe_registered_exits()?)?;
    baseline::require(
        owner
            .shutdown(Duration::from_secs(2))
            .map_err(keeper_stage("keeper-stop"))?
            == [supervisor_exit],
    )?;
    expect_adopted_reservation(credential)?;
    let result = {
        let successor = rust_launcher_probe::probe_advance_pre_unlock_generation(&mut adopted)
            .map_err(keeper_stage("keeper-advance"))?;
        let (previous, next) = successor.generations();
        baseline::require(previous != [0; 32] && next != [0; 32] && previous != next)?;
        let previous_error = successor
            .validate_generation(&previous)
            .err()
            .ok_or_else(baseline::refused)?;
        baseline::require(previous_error.kind() == io::ErrorKind::InvalidData)?;
        let mut changed = next;
        changed[31] ^= 1;
        let changed_error = successor
            .validate_generation(&changed)
            .err()
            .ok_or_else(baseline::refused)?;
        baseline::require(changed_error.kind() == io::ErrorKind::InvalidData)?;
        successor.validate_generation(&next)?;
        expect_adopted_reservation(credential)?;
        operation(successor)
    };
    let replay = rust_launcher_probe::probe_advance_pre_unlock_generation(&mut adopted)
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(replay.kind() == io::ErrorKind::InvalidData)?;
    expect_adopted_reservation(credential)?;
    drop(adopted);
    expect_adopted_reservation(credential)?;
    // Keep the independent master alive through every provider exit observation.
    drop(master);
    let socket = home.path().join("agent.sock");
    let metadata = std::fs::symlink_metadata(&socket)?;
    // SAFETY: this call queries the effective identity of this keeper.
    baseline::require(
        metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() },
    )?;
    std::fs::remove_file(socket)?;
    baseline::require(std::fs::read_dir(home.path())?.next().is_none())?;
    std::fs::remove_dir(home.path())?;
    result
}

pub(super) fn verify_openssh_keeper_successor(credential: &str) -> io::Result<()> {
    let mut owner = SpawnOwner::new()?;
    let receipt = owner.spawn(spec(
        vec!["--registration-openssh-keeper".into(), credential.into()],
        Vec::new(),
    )?)?;
    collect(&mut owner, &[receipt])?;
    CurrentAdmission::open_fixed_namespace(credential)?
        .prepare_native_material()?
        .reserve()?
        .validate()
}

fn keeper_bootstrap() -> io::Result<()> {
    let (parent, child) = packet_pair()?;
    let (offer, child_offer) = packet_pair()?;
    let (control, child_control) = Control::pair()?;
    let mut owner = SpawnOwner::new()?;
    let receipt = owner.spawn(spec(
        vec![
            "--registration-keeper-supervisor".into(),
            child.as_raw_fd().to_string().into(),
            child_offer.as_raw_fd().to_string().into(),
            child_control.as_raw_fd().to_string().into(),
        ],
        vec![child, child_offer, child_control],
    )?)?;
    let mut sender = KeeperChallengeSender::for_owned_child(
        parent,
        &owner,
        receipt.token,
        Instant::now() + Duration::from_secs(3),
    )?;
    let _challenge = sender.send()?;
    let error = sender.send().err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
    control.expect(b'R')?;
    let offer = KeeperChallengeSender::for_owned_child(
        offer,
        &owner,
        receipt.token,
        Instant::now() + Duration::from_secs(3),
    )?;
    let mut receiver = offer.into_offer_receiver()?;
    let _offer = receiver.receive()?;
    let error = receiver.receive().err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
    baseline::require(owner.try_wait(receipt.token)?.is_none())?;
    control.send(b'G')?;
    collect(&mut owner, &[receipt])?;
    drop(sender);
    let report = json!({"version":1,"case":"keeper-challenge-bootstrap",
        "challenge_received":true,"sender_single_use_rejected":true,
        "receiver_single_use_rejected":true,"supervisor_exit_observed":true,
        "closure_authority":false,"session_activated":false});
    let mut bytes = serde_json::to_vec(&report).map_err(|_| baseline::refused())?;
    bytes.push(b'\n');
    io::stdout().lock().write_all(&bytes)
}

fn run_case(case: &str) -> io::Result<()> {
    let (parent, child) = packet_pair()?;
    let (control, child_control) = Control::pair()?;
    let mut owner = SpawnOwner::new()?;
    let receipt = owner.spawn(spec(
        vec![
            "--registration-supervisor".into(),
            case.into(),
            child.as_raw_fd().to_string().into(),
            child_control.as_raw_fd().to_string().into(),
        ],
        vec![child, child_control],
    )?)?;
    control.expect(b'R')?;
    let mut receiver = RegistrationReceiver::for_owned_child(
        parent,
        &owner,
        receipt.token,
        Instant::now() + Duration::from_secs(3),
    )?;
    let descriptors_before = std::fs::read_dir("/proc/self/fd")?.count();
    let result = receiver.receive();
    let accepted = matches!(case, "valid" | "consumed-rejected" | "owned-pair");
    baseline::require(result.is_ok() == accepted)?;
    if case == "truncated-rights-rejected" {
        baseline::require(std::fs::read_dir("/proc/self/fd")?.count() == descriptors_before)?;
    }
    if case == "timeout" {
        baseline::require(
            result.as_ref().err().map(io::Error::kind) == Some(io::ErrorKind::TimedOut),
        )?;
    }
    let error = receiver.receive().err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
    control.expect(b'D')?;
    control.send(b'G')?;
    control.expect(b'C')?;
    collect(&mut owner, &[receipt])?;
    drop(result);
    let result = json!({"version":1,"case":case,"references_received":accepted,
        "transport_rejected":!accepted,"single_use_rejected":true,"supervisor_exit_observed":true,
        "child_exits_observed":true,"truncated_rights_closed":case == "truncated-rights-rejected",
        "closure_authority":false,"session_activated":false});
    let mut bytes = serde_json::to_vec(&result).map_err(|_| baseline::refused())?;
    baseline::require(bytes.len() <= 8192)?;
    bytes.push(b'\n');
    io::stdout().lock().write_all(&bytes)
}

pub(super) fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: these calls query only this disposable process's credentials and flag.
    baseline::require(unsafe {
        libc::getuid() == 65534
            && libc::geteuid() == 65534
            && libc::getgid() == 65534
            && libc::getegid() == 65534
            && libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1
    })?;
    let invocation = parse(std::env::args_os().skip(1))?;
    // SAFETY: alarm bounds only this explicitly disposable probe process.
    unsafe {
        libc::alarm(if matches!(invocation, Invocation::ActiveSupervisor(..)) {
            55
        } else {
            15
        })
    };
    match invocation {
        Invocation::KeeperBootstrap => keeper_bootstrap(),
        Invocation::KeeperSupervisor(challenge, offer, control) => keeper_supervisor(
            baseline::take_inherited(challenge)?,
            baseline::take_inherited(offer)?,
            Control(baseline::take_inherited(control)?),
        ),
        Invocation::AdmittedKeeper(credential, confirmation) => {
            admitted_keeper(&credential, confirmation)
        }
        Invocation::AdmittedBootstrapSupervisor(credential, confirmation, transport, control) => {
            admitted_bootstrap_supervisor(
                &credential,
                confirmation,
                baseline::take_inherited(transport)?,
                Control(baseline::take_inherited(control)?),
            )
        }
        Invocation::OpenSshKeeper(credential) => openssh_keeper(&credential),
        Invocation::ActiveSupervisor(
            credential,
            home,
            transport,
            control,
            keeper_death_cut,
            opening_permit,
        ) => active_supervisor(
            &credential,
            &home,
            baseline::take_inherited(transport)?,
            Control(baseline::take_inherited(control)?),
            keeper_death_cut,
            opening_permit,
        ),
        Invocation::OpenSshSupervisor(credential, home, transport, terminal, control) => {
            openssh_supervisor(
                &credential,
                &home,
                baseline::take_inherited(transport)?,
                baseline::take_inherited(terminal)?,
                Control(baseline::take_inherited(control)?),
            )
        }
        Invocation::Case(case) => run_case(&case),
        Invocation::Marker(fd) => {
            let control = Control(baseline::take_inherited(fd)?);
            control.send(b'R')?;
            control.expect(b'G')
        }
        Invocation::TaskMarker(attempt, fd) => {
            let control = Control(baseline::take_inherited(fd)?);
            control.send(b'R')?;
            control.expect(b'G')?;
            attempt_task_creation(&attempt)?;
            control.send(b'D')
        }
        Invocation::Supervisor(case, transport, control) => supervisor(
            &case,
            baseline::take_inherited(transport)?,
            Control(baseline::take_inherited(control)?),
        ),
        Invocation::Forwarder(transport, control, descriptors) => {
            let transport = baseline::take_inherited(transport)?;
            let control = Control(baseline::take_inherited(control)?);
            let descriptors = descriptors.map(baseline::take_inherited);
            let [first, second] = descriptors;
            let (first, second) = (first?, second?);
            raw_response(&transport, &[first.as_raw_fd(), second.as_raw_fd()], false)?;
            control.send(b'R')?;
            control.expect(b'G')
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_cli_accepts_only_fixed_cases_and_private_slots() {
        for case in CASES {
            assert!(matches!(
                parse(["--registration-case", case].map(Into::into)),
                Ok(Invocation::Case(_))
            ));
        }
        assert!(matches!(
            parse(["--registration-supervisor", "valid", "3", "4"].map(Into::into)),
            Ok(Invocation::Supervisor(_, 3, 4))
        ));
        assert!(matches!(
            parse(["--registration-marker", "3"].map(Into::into)),
            Ok(Invocation::Marker(3))
        ));
        for attempt in TASK_ATTEMPTS {
            assert!(matches!(
                parse(["--registration-task-marker", attempt, "3"].map(Into::into)),
                Ok(Invocation::TaskMarker(_, 3))
            ));
        }
        assert!(matches!(
            parse(["--registration-forwarder", "3", "4", "5", "6"].map(Into::into)),
            Ok(Invocation::Forwarder(3, 4, [5, 6]))
        ));
        assert!(matches!(
            parse(["--registration-openssh-keeper", "credential"].map(Into::into)),
            Ok(Invocation::OpenSshKeeper(_))
        ));
        assert!(matches!(
            parse(
                [
                    "--registration-openssh-supervisor",
                    "credential",
                    "/tmp/private",
                    "3",
                    "4",
                    "5",
                ]
                .map(Into::into)
            ),
            Ok(Invocation::OpenSshSupervisor(_, _, 3, 4, 5))
        ));
    }

    #[test]
    fn registration_cli_rejects_claims_paths_and_noncanonical_slots() {
        for arguments in [
            vec!["--registration-case"],
            vec!["--registration-case", "unknown"],
            vec!["--registration-case", "valid", "--pid", "123"],
            vec!["--registration-case", "/tmp/claimed-reference"],
            vec!["--registration-marker", "03"],
            vec!["--registration-marker", "2"],
            vec!["--registration-marker", "+3"],
            vec!["--registration-supervisor", "valid", "3", "3"],
            vec!["--registration-forwarder", "3", "4", "5", "5"],
            vec!["--registration-marker", "3", "extra"],
            vec!["--registration-task-marker", "unknown", "3"],
            vec!["--registration-task-marker", "fork", "03"],
            vec!["--registration-task-marker", "fork", "3", "extra"],
            vec!["--registration-openssh-keeper", "credential", "extra"],
            vec![
                "--registration-openssh-supervisor",
                "credential",
                "relative",
                "3",
                "4",
                "5",
            ],
            vec![
                "--registration-openssh-supervisor",
                "credential",
                "/tmp/private",
                "3",
                "4",
                "3",
            ],
        ] {
            assert!(parse(arguments.into_iter().map(Into::into)).is_err());
        }
    }
}
