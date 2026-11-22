use crate::linux_spawn::{ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::thread;
use std::time::{Duration, Instant};

fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<[RawFd; 3]> {
    let mut arguments = arguments.into_iter();
    baseline::require(
        arguments.next().as_deref() == Some(std::ffi::OsStr::new("--isolation-case"))
            && arguments.next().as_deref() == Some(std::ffi::OsStr::new("process-boundary")),
    )?;
    let mut descriptors = [None; 3];
    while let Some(option) = arguments.next() {
        let value = arguments.next().ok_or_else(baseline::refused)?;
        let index = ["--observer-fd", "--child-fd", "--marker-fd"]
            .iter()
            .position(|name| option == *name)
            .ok_or_else(baseline::refused)?;
        baseline::require(descriptors[index].is_none())?;
        let fd = value
            .into_string()
            .map_err(|_| baseline::refused())?
            .parse::<RawFd>()
            .map_err(|_| baseline::refused())?;
        baseline::require(fd >= 3 && !descriptors.contains(&Some(fd)))?;
        descriptors[index] = Some(fd);
    }
    descriptors
        .map(|value| value.ok_or_else(baseline::refused))
        .into_iter()
        .collect::<io::Result<Vec<_>>>()?
        .try_into()
        .map_err(|_| baseline::refused())
}

struct Channel(OwnedFd);
impl Channel {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        let timeout = libc::timeval {
            tv_sec: 5,
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: setsockopt reads one initialized timeval on the owned socket.
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
        // SAFETY: socketpair writes exactly two descriptors into the initialized array.
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
        // SAFETY: successful socketpair transferred both fresh descriptors.
        let [local, peer] = unsafe { fds.map(|fd| OwnedFd::from_raw_fd(fd)) };
        Ok((Self::new(local)?, peer))
    }
    fn bytes(&self, bytes: &[u8]) -> io::Result<()> {
        // SAFETY: the initialized packet remains readable during send.
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
        baseline::require(!bytes.is_empty() && bytes.len() <= 8192)?;
        self.bytes(&bytes)
    }
    fn receive(&self) -> io::Result<Value> {
        let mut bytes = [0u8; 8193];
        // SAFETY: recv writes at most the fixed buffer; MSG_TRUNC reports oversized packets.
        let size = unsafe {
            libc::recv(
                self.0.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_TRUNC,
            )
        };
        baseline::require(size > 0 && size <= 8192)?;
        serde_json::from_slice(&bytes[..size as usize]).map_err(|_| baseline::refused())
    }
    fn command(&self, expected: u8) -> io::Result<()> {
        let mut bytes = [0u8; 2];
        // SAFETY: recv writes at most the fixed buffer and preserves the packet boundary.
        let size = unsafe {
            libc::recv(
                self.0.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_TRUNC,
            )
        };
        baseline::require(size == 1 && bytes[0] == expected)
    }
}

fn null() -> io::Result<OwnedFd> {
    Ok(File::options()
        .read(true)
        .write(true)
        .open("/dev/null")?
        .into())
}

fn launch(
    owner: &mut SpawnOwner,
    case: &str,
    child: OwnedFd,
    marker: OwnedFd,
) -> io::Result<(Receipt, Channel)> {
    let (channel, observer) = Channel::pair()?;
    let arguments = vec![
        "--abandon-case".into(),
        case.into(),
        "--observer-fd".into(),
        observer.as_raw_fd().to_string().into(),
        "--child-fd".into(),
        child.as_raw_fd().to_string().into(),
        "--marker-fd".into(),
        marker.as_raw_fd().to_string().into(),
    ];
    let receipt = owner.spawn(SpawnSpec {
        executable: File::open("/proc/self/exe")?.into(),
        arg0: "rust-launcher-probe".into(),
        arguments,
        environment: vec![("VIVAC_OPENSSH_DISPOSABLE".into(), "1".into())],
        stdio: [null()?, null()?, null()?],
        inherited: vec![observer, child, marker],
        retained: Vec::new(),
        new_session: true,
        controlling_tty: None,
        probe: None,
    })?;
    Ok((receipt, channel))
}

fn worker_packet(channel: &Channel, case: &str, stage: &str) -> io::Result<Value> {
    let value = channel.receive()?;
    baseline::require(value.is_object() && value["case"] == case && value["stage"] == stage)?;
    Ok(value)
}
fn relay(observer: &Channel, receipt: Receipt, payload: &Value) -> io::Result<()> {
    observer.send(&json!({"stage":"worker-a","worker_pid":receipt.pid,"payload":payload}))
}
fn normal_exit(exit: &ExitReceipt, expected: Receipt) -> bool {
    exit.token == expected.token
        && exit.pid == expected.pid
        && exit.code == Some(0)
        && exit.signal.is_none()
}

fn validate_shutdown(exits: &[ExitReceipt], expected: [Receipt; 2]) -> io::Result<()> {
    baseline::require(
        exits.len() == 2
            && expected[0].token != expected[1].token
            && expected[0].pid != expected[1].pid,
    )?;
    for receipt in expected {
        baseline::require(
            exits
                .iter()
                .filter(|exit| normal_exit(exit, receipt))
                .count()
                == 1,
        )?;
    }
    Ok(())
}

fn wait_normal(owner: &mut SpawnOwner, expected: Receipt) -> io::Result<ExitReceipt> {
    let pidfd = owner.duplicate_pidfd(expected.token)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        baseline::require(Instant::now() < deadline)?;
        if let Some(exit) = owner.try_wait(expected.token)? {
            baseline::require(normal_exit(&exit, expected))?;
            let mut readiness = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll receives one initialized writable pollfd for the retained descriptor.
            let ready = unsafe { libc::poll(&mut readiness, 1, 0) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            baseline::require(readiness.revents & (libc::POLLERR | libc::POLLNVAL) == 0)?;
            if ready == 1 && readiness.revents & libc::POLLIN != 0 {
                return Ok(exit);
            }
        }
        baseline::require(Instant::now() < deadline)?;
        thread::sleep(Duration::from_millis(5));
    }
}
fn health(
    observer: &Channel,
    owner: &mut SpawnOwner,
    worker: Receipt,
    responses: u32,
    stage: &str,
) -> io::Result<()> {
    baseline::require(!SpawnOwner::admission_uncertain())?;
    drop(SpawnOwner::new()?);
    // SAFETY: getpid only queries the current disposable host process.
    observer.send(
        &json!({"stage":stage,"host_pid":unsafe {libc::getpid()},"creator_tid":worker.creator_tid,
        "worker_a_pid":worker.pid,"host_admission_uncertain":false,"host_empty_owner_admitted":true,
        "pressure_threads":4,"health_responses":responses}),
    )?;
    baseline::require(owner.receipt(worker.token)? == worker)
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    let [observer, child, marker] = parse(std::env::args_os().skip(1))?;
    // SAFETY: the alarm applies only to this disposable host process.
    unsafe { libc::alarm(90) };
    let observer = Channel::new(baseline::take_inherited(observer)?)?;
    let child = baseline::take_inherited(child)?;
    let marker = baseline::take_inherited(marker)?;
    let pressure = Pressure::start()?;
    let mut owner = SpawnOwner::new()?;
    let (worker_a, a) = launch(&mut owner, "pidfd-failure", child, marker)?;
    let before = worker_packet(&a, "pidfd-failure", "before-drop")?;
    baseline::require(before["shutdown_errno"] == libc::ETIMEDOUT)?;
    let native_pid = before["pid"].as_u64().ok_or_else(baseline::refused)?;
    baseline::require(native_pid > 0 && native_pid <= i32::MAX as u64)?;
    relay(&observer, worker_a, &before)?;
    observer.command(b'G')?;
    a.bytes(b"G")?;
    let dropped = worker_packet(&a, "pidfd-failure", "drop-returned")?;
    baseline::require(
        dropped["child_pid"] == native_pid
            && dropped["admission_poisoned"] == true
            && dropped["marker_retained"] == true
            && dropped["poison_refusals"] == 3
            && dropped["close_ack"] == false
            && dropped["spawn_errno"] == libc::EMFILE
            && dropped["shutdown_errno"] == libc::ETIMEDOUT,
    )?;
    relay(&observer, worker_a, &dropped)?;
    observer.command(b'H')?;
    baseline::require(owner.try_wait(worker_a.token)?.is_none())?;
    health(&observer, &mut owner, worker_a, 1, "host-healthy")?;
    let (worker_b, b) = launch(&mut owner, "admission-limit", null()?, null()?)?;
    baseline::require(worker_b.creator_tid == worker_a.creator_tid)?;
    worker_packet(&b, "admission-limit", "admission-ready")?;
    b.bytes(b"G")?;
    let b_report = worker_packet(&b, "admission-limit", "responsive")?;
    baseline::require(
        b_report["owners_reserved"] == 4
            && b_report["fifth_owner_errno"] == libc::EBUSY
            && b_report["readmission_after_release"] == true
            && b_report["admission_poisoned"] == false
            && b_report["close_ack"] == false,
    )?;
    b.bytes(b"X")?;
    wait_normal(&mut owner, worker_b)?;
    baseline::require(
        owner.try_wait(worker_a.token)?.is_none() && !SpawnOwner::admission_uncertain(),
    )?;
    observer.send(&json!({"stage":"worker-b-completed","worker_pid":worker_b.pid,"payload":b_report,"exit_code":0}))?;
    observer.command(b'G')?;
    baseline::require(owner.try_wait(worker_a.token)?.is_none())?;
    a.bytes(b"G")?;
    let responsive = worker_packet(&a, "pidfd-failure", "responsive")?;
    let mut previous = dropped.clone();
    previous["stage"] = "responsive".into();
    baseline::require(responsive == previous)?;
    relay(&observer, worker_a, &responsive)?;
    observer.command(b'G')?;
    a.bytes(b"X")?;
    wait_normal(&mut owner, worker_a)?;
    observer.send(
        &json!({"stage":"workers-exited","worker_a_pid":worker_a.pid,"worker_b_pid":worker_b.pid,
        "worker_a_exit_code":0,"worker_b_exit_code":0}),
    )?;
    observer.command(b'H')?;
    health(&observer, &mut owner, worker_a, 2, "host-health")?;
    observer.command(b'C')?;
    let exits = owner.shutdown(Duration::from_secs(5))?;
    validate_shutdown(&exits, [worker_a, worker_b])?;
    drop(a);
    drop(b);
    let iterations = pressure.finish()?;
    baseline::require(iterations > 0 && !SpawnOwner::admission_uncertain())?;
    // SAFETY: getpid only queries the current disposable host process.
    let report = json!({"version":1,"case":"process-boundary","status":"observed","host_pid":unsafe {libc::getpid()},
        "creator_tid":worker_a.creator_tid,"worker_a_pid":worker_a.pid,"worker_b_pid":worker_b.pid,"native_pid":native_pid,
        "host_admission_uncertain":false,"host_empty_owner_admitted":true,"host_health_responses":2,
        "worker_a_admission_poisoned":true,"worker_a_poison_refusals":3,"worker_a_drop_elapsed_ns":dropped["drop_elapsed_ns"],
        "worker_b_reserved":4,"worker_b_fifth_errno":libc::EBUSY,"worker_b_readmitted":true,
        "worker_a_exit_code":0,"worker_b_exit_code":0,"worker_exits_observed":true,"pressure_threads":4,
        "pressure_iterations":iterations,"resources_closed_after_worker_exit":true,"custody_accredited":false,
        "product_recovery":false,"same_store_recovery":false,"mcp_integration":false,"server_integration":false,"close_ack":false});
    observer.send(&report)?;
    println!("{report}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_exit_is_bound_to_both_owner_token_and_native_pid() {
        let expected = Receipt {
            token: 1,
            pid: 101,
            creator_tid: 10,
        };
        let observed = ExitReceipt {
            token: 1,
            pid: 101,
            code: Some(0),
            signal: None,
        };
        assert!(normal_exit(&observed, expected));
        for changed in [
            ExitReceipt {
                token: 2,
                ..observed
            },
            ExitReceipt {
                pid: 102,
                ..observed
            },
            ExitReceipt {
                code: Some(1),
                ..observed
            },
            ExitReceipt {
                code: None,
                signal: Some(libc::SIGKILL),
                ..observed
            },
        ] {
            assert!(!normal_exit(&changed, expected));
        }
    }

    #[test]
    fn shutdown_requires_each_worker_once_without_token_or_pid_substitution() {
        let a = Receipt {
            token: 1,
            pid: 101,
            creator_tid: 10,
        };
        let b = Receipt {
            token: 2,
            pid: 102,
            creator_tid: 10,
        };
        let exited_a = ExitReceipt {
            token: 1,
            pid: 101,
            code: Some(0),
            signal: None,
        };
        let exited_b = ExitReceipt {
            token: 2,
            pid: 102,
            code: Some(0),
            signal: None,
        };
        assert!(validate_shutdown(&[exited_a, exited_b], [a, b]).is_ok());
        assert!(validate_shutdown(&[exited_b, exited_a], [a, b]).is_ok());
        for exits in [
            vec![exited_a],
            vec![exited_a, exited_a],
            vec![exited_b, exited_b],
            vec![exited_a, exited_b, exited_b],
            vec![
                ExitReceipt {
                    pid: 102,
                    ..exited_a
                },
                ExitReceipt {
                    pid: 101,
                    ..exited_b
                },
            ],
            vec![
                ExitReceipt {
                    token: 2,
                    ..exited_a
                },
                ExitReceipt {
                    token: 1,
                    ..exited_b
                },
            ],
        ] {
            assert!(validate_shutdown(&exits, [a, b]).is_err());
        }
    }

    #[test]
    fn isolation_cli_accepts_only_its_single_case_and_distinct_capabilities() {
        let good = [
            "--isolation-case",
            "process-boundary",
            "--observer-fd",
            "3",
            "--child-fd",
            "4",
            "--marker-fd",
            "5",
        ];
        assert_eq!(
            parse(good.map(OsString::from)).expect("valid public arguments"),
            [3, 4, 5]
        );
        for (index, value) in [(1, "other"), (3, "2"), (5, "3"), (7, "invalid")] {
            let mut args = good;
            args[index] = value;
            assert!(parse(args.map(OsString::from)).is_err());
        }
        assert!(parse(good[..6].iter().map(OsString::from)).is_err());
    }
}
