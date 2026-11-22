use crate::linux_custody_provision::Provisioner;
use crate::linux_custody_root::CandidateRoot;
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

const CASES: [&str; 9] = [
    "fixed-valid",
    "environment-decoy",
    "cwd-decoy",
    "missing-fixed-root",
    "binding-in-place",
    "binding-replacement",
    "binding-mode",
    "mount-users",
    "mount-state",
];

fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<(String, String, RawFd)> {
    let mut args = args.into_iter();
    let mode = args
        .next()
        .and_then(|v| v.into_string().ok())
        .ok_or_else(baseline::refused)?;
    baseline::require(matches!(
        mode.as_str(),
        "--fixed-root-setup" | "--fixed-root-case"
    ))?;
    let case = args
        .next()
        .and_then(|v| v.into_string().ok())
        .ok_or_else(baseline::refused)?;
    baseline::require(CASES.contains(&case.as_str()))?;
    baseline::require(args.next().as_deref() == Some(std::ffi::OsStr::new("--observer-fd")))?;
    let descriptor = args
        .next()
        .and_then(|v| v.into_string().ok())
        .ok_or_else(baseline::refused)?;
    baseline::require(!descriptor.is_empty() && descriptor.bytes().all(|b| b.is_ascii_digit()))?;
    let fd = descriptor
        .parse::<RawFd>()
        .map_err(|_| baseline::refused())?;
    baseline::require(fd >= 3 && args.next().is_none())?;
    Ok((mode, case, fd))
}

pub(super) struct Channel(OwnedFd);

impl Channel {
    pub(super) fn new(fd: OwnedFd) -> io::Result<Self> {
        let timeout = libc::timeval {
            tv_sec: 45,
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: this initialized option storage belongs to the owned observer socket.
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

    pub(super) fn send(&self, value: &Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|_| baseline::refused())?;
        baseline::require(bytes.len() <= 8192)?;
        // SAFETY: send borrows the complete initialized packet buffer.
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

    pub(super) fn go(&self) -> io::Result<()> {
        let mut bytes = [0u8; 2];
        // SAFETY: recv writes only this fixed initialized buffer and reports full packet size.
        baseline::require(
            unsafe {
                libc::recv(
                    self.0.as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    libc::MSG_TRUNC,
                )
            } == 1
                && bytes[0] == b'G',
        )
    }
}

pub(super) fn scoped(mut value: Value) -> Value {
    for field in [
        "custody_accredited",
        "creation_admission",
        "producer_origin_checked",
        "root_bootstrap_accredited",
        "provisioning_accredited",
        "endpoint_authority_accredited",
        "product_recovery",
        "same_store_recovery",
        "mcp_integration",
        "server_integration",
        "lock_ack",
    ] {
        value[field] = false.into();
    }
    value
}

pub(super) fn identity() -> io::Result<Value> {
    // SAFETY: these calls query only this disposable process and its current thread.
    let host_pid = unsafe { libc::getpid() };
    // SAFETY: gettid queries only the calling thread.
    let creator_tid = unsafe { libc::syscall(libc::SYS_gettid) };
    baseline::require(i64::from(host_pid) == creator_tid)?;
    Ok(json!({"host_pid":host_pid,"creator_tid":creator_tid,"children":[]}))
}

pub(super) fn finish(observer: &Channel, mut result: Value, pressure: Pressure) -> io::Result<()> {
    let iterations = pressure.finish()?;
    baseline::require(iterations > 0)?;
    result["resources_closed_after_exit"] = true.into();
    result["pressure_iterations"] = iterations.into();
    observer.send(&result)?;
    let mut bytes = serde_json::to_vec(&result).map_err(|_| baseline::refused())?;
    bytes.push(b'\n');
    use std::io::Write;
    io::stdout().write_all(&bytes)
}

fn publish(writer: Provisioner) -> io::Result<String> {
    let publication = writer.provision(65534, 65534).map_err(io::Error::other)?;
    Ok(publication.binding.installation_id)
}

fn setup(case: &str, observer: &Channel) -> io::Result<()> {
    let pressure = Pressure::start()?;
    let identity = identity()?;
    observer.send(&json!({"stage":"before","case":case,"identity":identity}))?;
    observer.go()?;
    let real_id = if case == "missing-fixed-root" {
        None
    } else {
        Some(publish(Provisioner::open_fixed_namespace()?)?)
    };
    let decoy = File::open("/decoy")?;
    let decoy_id = publish(Provisioner::from_fixture_filesystem(decoy.into())?)?;
    baseline::require(real_id.as_ref().is_none_or(|id| id != &decoy_id))?;
    let result = scoped(
        json!({"version":1,"case":case,"status":"observed","identity":identity,
        "real_publications":u32::from(real_id.is_some()),"decoy_publications":1,
        "real_installation_id":real_id,"decoy_installation_id":decoy_id,
        "resources_closed_after_exit":false,"pressure_threads":4,"pressure_iterations":0}),
    );
    observer.send(&json!({"stage":"observation","payload":result}))?;
    observer.go()?;
    finish(observer, result, pressure)
}

fn reader(case: &str, observer: &Channel) -> io::Result<()> {
    let pressure = Pressure::start()?;
    let identity = identity()?;
    observer.send(&json!({"stage":"before","case":case,"identity":identity}))?;
    observer.go()?;
    let mut result = scoped(
        json!({"version":1,"case":case,"status":"observed","identity":identity,
        "initial_opened":false,"initial_validated":false,"selected_installation_id":null,
        "initial_error":"none","snapshot_invalidated":false,"validation_error":"none",
        "resources_closed_after_exit":false,"pressure_threads":4,"pressure_iterations":0}),
    );
    let expected_error = match case {
        "missing-fixed-root" => Some((io::ErrorKind::NotFound, "not-found")),
        "mount-users" | "mount-state" => Some((
            io::Error::from_raw_os_error(libc::EXDEV).kind(),
            "crosses-devices",
        )),
        _ => None,
    };
    let candidate = match CandidateRoot::open_fixed_namespace() {
        Ok(candidate) => {
            baseline::require(expected_error.is_none())?;
            candidate.validate()?;
            result["initial_opened"] = true.into();
            result["initial_validated"] = true.into();
            result["selected_installation_id"] = candidate.binding().installation_id.clone().into();
            Some(candidate)
        }
        Err(error) => {
            let Some((kind, label)) = expected_error else {
                return Err(error);
            };
            if error.kind() != kind {
                return Err(error);
            }
            result["initial_error"] = label.into();
            None
        }
    };
    observer.send(&json!({"stage":"snapshot-held","payload":result}))?;
    observer.go()?;
    if let Some(candidate) = &candidate {
        if matches!(
            case,
            "binding-in-place" | "binding-replacement" | "binding-mode"
        ) {
            let error = candidate.validate().err().ok_or_else(baseline::refused)?;
            result["validation_error"] = match error.kind() {
                io::ErrorKind::InvalidInput => "invalid-input",
                io::ErrorKind::PermissionDenied => "permission-denied",
                _ => return Err(error),
            }
            .into();
            result["snapshot_invalidated"] = true.into();
        } else {
            candidate.validate()?;
        }
        baseline::require(
            result["selected_installation_id"] == candidate.binding().installation_id,
        )?;
    }
    observer.send(&json!({"stage":"observation","payload":result}))?;
    observer.go()?;
    drop(candidate);
    finish(observer, result, pressure)
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: this alarm bounds only the explicitly disposable experiment process.
    unsafe { libc::alarm(120) };
    let (mode, case, observer_fd) = parse(std::env::args_os().skip(1))?;
    let observer = Channel::new(baseline::take_inherited(observer_fd)?)?;
    let outcome = if mode == "--fixed-root-setup" {
        setup(&case, &observer)
    } else {
        reader(&case, &observer)
    };
    if let Err(error) = &outcome {
        let diagnostic = error.to_string();
        baseline::require(diagnostic.len() <= 512 && diagnostic.is_ascii())?;
        observer.send(&json!({"stage":"reader-refusal","diagnostic":diagnostic}))?;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_namespace_cli_accepts_only_two_modes_nine_cases_and_one_observer() {
        for mode in ["--fixed-root-setup", "--fixed-root-case"] {
            for case in CASES {
                let parsed = parse([mode, case, "--observer-fd", "3"].map(OsString::from))
                    .expect("explicit fixed namespace observer");
                assert_eq!(parsed, (mode.into(), case.into(), 3));
            }
        }
        for invalid in [
            vec![],
            vec!["--fixed-root-setup", "unknown", "--observer-fd", "3"],
            vec!["--fixed-root-case", "fixed-valid"],
            vec!["--fixed-root-case", "fixed-valid", "--observer-fd", "2"],
            vec!["--fixed-root-case", "fixed-valid", "--observer-fd", "-1"],
            vec![
                "--fixed-root-case",
                "fixed-valid",
                "--observer-fd",
                "2147483648",
            ],
            vec![
                "--fixed-root-case",
                "fixed-valid",
                "--observer-fd",
                "3",
                "--observer-fd",
                "4",
            ],
            vec![
                "--fixed-root-case",
                "fixed-valid",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
            ],
            vec![
                "--fixed-root-case",
                "fixed-valid",
                "--observer-fd",
                "3",
                "--root-path",
                "/decoy",
            ],
            vec![
                "--fixed-root-case",
                "fixed-valid",
                "--observer-fd",
                "3",
                "--expected-installation-id",
                "00000000000000000000000001",
            ],
            vec!["--fixed-root-reader", "fixed-valid", "--observer-fd", "3"],
        ] {
            assert!(parse(invalid.into_iter().map(OsString::from)).is_err());
        }
    }
}
