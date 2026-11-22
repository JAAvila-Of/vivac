use crate::linux_spawn::{Receipt, SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use serde_json::json;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::mem::{size_of, MaybeUninit};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

const CASES: [&str; 5] = [
    "alive-pending",
    "stopped-pending",
    "exit-before-reap",
    "exit-after-reap",
    "unknown-token-refused",
];

enum Invocation {
    Case(String),
    Child(RawFd),
}

fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<Invocation> {
    let mut arguments = arguments.into_iter();
    let first = arguments.next().ok_or_else(baseline::refused)?;
    if first == "--exit-watch-case" {
        let case = arguments
            .next()
            .ok_or_else(baseline::refused)?
            .into_string()
            .map_err(|_| baseline::refused())?;
        baseline::require(CASES.contains(&case.as_str()) && arguments.next().is_none())?;
        return Ok(Invocation::Case(case));
    }
    baseline::require(
        first == "--exit-watch-child"
            && arguments.next().as_deref() == Some(std::ffi::OsStr::new("--control-fd")),
    )?;
    let text = arguments
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    let fd: RawFd = text.parse().map_err(|_| baseline::refused())?;
    baseline::require(fd >= 3 && fd.to_string() == text && arguments.next().is_none())?;
    Ok(Invocation::Child(fd))
}

struct Control(OwnedFd);

impl Control {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        for (option, expected) in [
            (libc::SO_TYPE, libc::SOCK_SEQPACKET),
            (libc::SO_DOMAIN, libc::AF_UNIX),
        ] {
            let mut value: libc::c_int = 0;
            let mut length = size_of::<libc::c_int>() as libc::socklen_t;
            // SAFETY: the owned descriptor and complete scalar storage remain live.
            baseline::require(
                unsafe {
                    libc::getsockopt(
                        fd.as_raw_fd(),
                        libc::SOL_SOCKET,
                        option,
                        std::ptr::addr_of_mut!(value).cast(),
                        &mut length,
                    )
                } == 0
                    && length as usize == size_of::<libc::c_int>()
                    && value == expected,
            )?;
        }
        let timeout = libc::timeval {
            tv_sec: 5,
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: setsockopt reads this complete initialized timeout.
            if unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    option,
                    (&timeout as *const libc::timeval).cast(),
                    size_of::<libc::timeval>() as _,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self(fd))
    }

    fn pair() -> io::Result<(Self, OwnedFd)> {
        let mut values = [-1; 2];
        // SAFETY: socketpair writes exactly two freshly owned descriptors.
        if unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                values.as_mut_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful socketpair transferred two independent descriptor owners.
        let [local, child] = unsafe { values.map(|fd| OwnedFd::from_raw_fd(fd)) };
        Ok((Self::new(local)?, child))
    }

    fn send(&self, value: u8) -> io::Result<()> {
        // SAFETY: send borrows one initialized byte from the owned socket.
        baseline::require(
            unsafe {
                libc::send(
                    self.0.as_raw_fd(),
                    std::ptr::addr_of!(value).cast(),
                    1,
                    libc::MSG_NOSIGNAL,
                )
            } == 1,
        )
    }

    fn expect(&self, expected: u8) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut bytes = [0; 2];
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
            let microseconds = remaining.as_micros().max(1);
            let timeout = libc::timeval {
                tv_sec: (microseconds / 1_000_000) as libc::time_t,
                tv_usec: (microseconds % 1_000_000) as libc::suseconds_t,
            };
            // SAFETY: setsockopt reads this complete initialized remaining timeout.
            if unsafe {
                libc::setsockopt(
                    self.0.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    (&timeout as *const libc::timeval).cast(),
                    size_of::<libc::timeval>() as _,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: recv writes within the fixed initialized packet buffer.
            let count = unsafe {
                libc::recv(
                    self.0.as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    libc::MSG_TRUNC,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return baseline::require(
                Instant::now() < deadline && count == 1 && bytes[0] == expected,
            );
        }
    }
}

fn null() -> io::Result<OwnedFd> {
    Ok(File::options()
        .read(true)
        .write(true)
        .open("/dev/null")?
        .into())
}

fn spec(control: OwnedFd) -> io::Result<SpawnSpec> {
    let descriptor = control.as_raw_fd();
    Ok(SpawnSpec {
        executable: File::open("/proc/self/exe")?.into(),
        arg0: "rust-launcher-probe".into(),
        arguments: vec![
            "--exit-watch-child".into(),
            "--control-fd".into(),
            descriptor.to_string().into(),
        ],
        environment: vec![("VIVAC_OPENSSH_DISPOSABLE".into(), "1".into())],
        stdio: [null()?, null()?, null()?],
        inherited: vec![control],
        retained: Vec::new(),
        new_session: false,
        controlling_tty: None,
        probe: None,
    })
}

fn observe_status(
    pidfd: &OwnedFd,
    receipt: Receipt,
    stopped: bool,
    deadline: Instant,
) -> io::Result<()> {
    loop {
        baseline::require(Instant::now() < deadline)?;
        let mut information = MaybeUninit::<libc::siginfo_t>::zeroed();
        let events = if stopped {
            libc::WSTOPPED | libc::WEXITED
        } else {
            libc::WEXITED
        };
        // SAFETY: this owned-child pidfd and complete siginfo storage remain live;
        // WNOWAIT preserves Child's exclusive eventual exit collection.
        if unsafe {
            libc::waitid(
                libc::P_PIDFD,
                pidfd.as_raw_fd() as libc::id_t,
                information.as_mut_ptr(),
                events | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: waitid initialized the result, or left its zero PID unchanged.
        let information = unsafe { information.assume_init() };
        // SAFETY: the accessors read the initialized SIGCHLD representation.
        let pid = unsafe { information.si_pid() };
        if pid != 0 {
            baseline::require(
                pid == receipt.pid as libc::pid_t
                    && information.si_signo == libc::SIGCHLD
                    && information.si_code
                        == if stopped {
                            libc::CLD_STOPPED
                        } else {
                            libc::CLD_EXITED
                        }
                    && unsafe { information.si_status() }
                        == if stopped { libc::SIGSTOP } else { 0 },
            )?;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn collect_exit(owner: &mut SpawnOwner, receipt: Receipt, deadline: Instant) -> io::Result<()> {
    loop {
        baseline::require(Instant::now() < deadline)?;
        if let Some(exit) = owner.try_wait(receipt.token)? {
            return baseline::require(
                exit.token == receipt.token
                    && exit.pid == receipt.pid
                    && exit.code == Some(0)
                    && exit.signal.is_none(),
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn run_case(case: &str) -> io::Result<()> {
    let (control, child) = Control::pair()?;
    let mut owner = SpawnOwner::new()?;
    let receipt = owner.spawn(spec(child)?)?;
    control.expect(b'R')?;
    if case == "unknown-token-refused" {
        let error = owner
            .watch_child_exit(receipt.token + 1)
            .err()
            .ok_or_else(baseline::refused)?;
        baseline::require(error.raw_os_error() == Some(libc::ENOENT))?;
    }
    let mut watch = owner.watch_child_exit(receipt.token)?;
    baseline::require(watch.try_observe()?.is_none())?;
    let retained = owner.duplicate_pidfd(receipt.token)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    if case == "stopped-pending" {
        owner.signal(receipt.token, libc::SIGSTOP)?;
        observe_status(&retained, receipt, true, deadline)?;
        baseline::require(watch.try_observe()?.is_none())?;
        owner.signal(receipt.token, libc::SIGCONT)?;
    }
    control.send(b'G')?;
    if case == "exit-after-reap" {
        collect_exit(&mut owner, receipt, deadline)?;
    } else {
        observe_status(&retained, receipt, false, deadline)?;
    }
    let proof = watch.try_observe()?.ok_or_else(baseline::refused)?;
    proof.validate()?;
    let error = watch.try_observe().err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
    if case != "exit-after-reap" {
        collect_exit(&mut owner, receipt, deadline)?;
    }
    proof.validate()?;
    let exits = owner.shutdown(Duration::from_secs(2))?;
    baseline::require(exits.len() == 1 && exits[0].pid == receipt.pid && exits[0].code == Some(0))?;
    proof.validate()?;
    let result = json!({"version":1,"case":case,"pending_observed":true,
        "stopped_observed":case == "stopped-pending","stopped_pending_observed":case == "stopped-pending",
        "exit_observed":true,"reaped_before_observation":case == "exit-after-reap",
        "consumed_rejected":true,"proof_revalidated":true,"unknown_token_refused":case == "unknown-token-refused",
        "native_exit_observed":true,"closure_authority":false,"session_activated":false});
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
    // SAFETY: alarm bounds only this explicitly disposable probe process.
    unsafe { libc::alarm(15) };
    match parse(std::env::args_os().skip(1))? {
        Invocation::Child(fd) => {
            let control = Control::new(baseline::take_inherited(fd)?)?;
            control.send(b'R')?;
            control.expect(b'G')
        }
        Invocation::Case(case) => run_case(&case),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_watch_cli_accepts_only_fixed_cases_and_the_private_child_slot() {
        for case in CASES {
            assert!(matches!(
                parse(["--exit-watch-case", case].map(Into::into)),
                Ok(Invocation::Case(_))
            ));
        }
        assert!(matches!(
            parse(["--exit-watch-child", "--control-fd", "3"].map(Into::into)),
            Ok(Invocation::Child(3))
        ));
    }

    #[test]
    fn exit_watch_cli_rejects_claims_paths_and_noncanonical_descriptors() {
        for arguments in [
            vec!["--exit-watch-case"],
            vec!["--exit-watch-case", "unknown"],
            vec!["--exit-watch-case", "alive-pending", "--pid", "123"],
            vec!["--exit-watch-case", "/tmp/claimed-exit"],
            vec!["--exit-watch-child", "--control-fd", "03"],
            vec!["--exit-watch-child", "--control-fd", "2"],
            vec!["--exit-watch-child", "--control-fd", "+3"],
            vec!["--exit-watch-child", "--control-fd", "3", "extra"],
        ] {
            assert!(parse(arguments.into_iter().map(Into::into)).is_err());
        }
    }
}
