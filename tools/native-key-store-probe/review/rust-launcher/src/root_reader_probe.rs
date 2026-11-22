use crate::linux_custody_root::CandidateRoot;
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

const CASES: [&str; 19] = [
    "valid",
    "wrong-root-owner",
    "writable-ancestor",
    "wrong-prefix-mode",
    "wrong-slot-owner",
    "binding-owner",
    "binding-mode",
    "binding-malformed",
    "binding-oversized",
    "binding-identity",
    "ancestor-symlink",
    "binding-symlink",
    "missing-binding",
    "state-mode",
    "state-acl",
    "snapshot-private-replacement",
    "unrelated-entry",
    "binding-hardlink",
    "binding-fifo",
];

struct Channel(OwnedFd);
impl Channel {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        let timeout = libc::timeval {
            tv_sec: 10,
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: timeout is initialized option storage for this owned socket.
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
    fn send(&self, value: &Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|_| baseline::refused())?;
        baseline::require(bytes.len() <= 8192)?;
        // SAFETY: send borrows the complete initialized slice.
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
    fn go(&self) -> io::Result<()> {
        let mut bytes = [0u8; 2];
        // SAFETY: recv writes only into this fixed initialized packet buffer.
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

fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<(String, [RawFd; 2])> {
    let mut args = args.into_iter();
    baseline::require(args.next().as_deref() == Some(std::ffi::OsStr::new("--root-reader-case")))?;
    let case = args
        .next()
        .and_then(|v| v.into_string().ok())
        .ok_or_else(baseline::refused)?;
    baseline::require(CASES.contains(&case.as_str()))?;
    let mut fds = [None; 2];
    while let Some(option) = args.next() {
        let index = ["--observer-fd", "--root-fd"]
            .iter()
            .position(|v| option == *v)
            .ok_or_else(baseline::refused)?;
        baseline::require(fds[index].is_none())?;
        fds[index] = Some(
            args.next()
                .and_then(|v| v.into_string().ok())
                .ok_or_else(baseline::refused)?
                .parse::<RawFd>()
                .map_err(|_| baseline::refused())?,
        );
    }
    let fds = [
        fds[0].ok_or_else(baseline::refused)?,
        fds[1].ok_or_else(baseline::refused)?,
    ];
    baseline::require(fds.iter().all(|fd| *fd >= 3) && fds[0] != fds[1])?;
    Ok((case, fds))
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: alarm bounds this explicitly disposable fixture only.
    unsafe { libc::alarm(15) };
    let (case, [observer_fd, root_fd]) = parse(std::env::args_os().skip(1))?;
    let observer = Channel::new(baseline::take_inherited(observer_fd)?)?;
    let root = baseline::take_inherited(root_fd)?;
    let directory = PathBuf::from(format!("/proc/self/fd/{}", root.as_raw_fd()));
    let slot = directory.join("var/lib/vivac/custody-v1/users/65534");
    let pressure = Pressure::start()?;
    // SAFETY: these read this process and its main thread identities.
    let host_pid = unsafe { libc::getpid() };
    let creator_tid = unsafe { libc::syscall(libc::SYS_gettid) };
    baseline::require(i64::from(host_pid) == creator_tid)?;
    let identity = json!({"host_pid":host_pid,"creator_tid":creator_tid,"children":[]});
    observer.send(&json!({"stage":"before","case":case,"identity":identity}))?;
    observer.go()?;
    let mut result = json!({"version":1,"case":case,"status":"observed","identity":identity,
        "reader_opened":false,"snapshot_invalidated":false,"access_policy_checks":0,
        "resources_closed_after_exit":false,"pressure_threads":4,"pressure_iterations":0,
        "custody_accredited":false,"creation_admission":false,"producer_origin_checked":false,
        "root_bootstrap_accredited":false,"provisioning_accredited":false,"endpoint_authority_accredited":false,
        "product_recovery":false,"same_store_recovery":false,"mcp_integration":false,"server_integration":false,"lock_ack":false});
    if case == "state-acl" {
        let state = File::open(slot.join("state"))?;
        baseline::require(state.metadata()?.mode() & 0o7777 == 0o700)?;
        // SAFETY: query the retained state descriptor and static terminated xattr name.
        let length = unsafe {
            libc::fgetxattr(
                state.as_raw_fd(),
                c"system.posix_acl_access".as_ptr(),
                std::ptr::null_mut(),
                0,
            )
        };
        baseline::require(length > 0)?;
        result["access_policy_checks"] = 1.into();
    }
    let opened = CandidateRoot::from_fixture_filesystem(baseline::duplicate(root.as_raw_fd())?);
    if matches!(
        case.as_str(),
        "valid" | "snapshot-private-replacement" | "unrelated-entry"
    ) {
        let candidate = match opened {
            Ok(candidate) => candidate,
            Err(error) => {
                let diagnostic = error.to_string();
                baseline::require(diagnostic.len() <= 512 && diagnostic.is_ascii())?;
                observer.send(&json!({"stage":"reader-refusal","diagnostic":diagnostic}))?;
                return Err(error);
            }
        };
        candidate.validate()?;
        let bytes = candidate.binding().canonical_bytes()?;
        baseline::require(bytes == fs::read(slot.join("binding.json"))?)?;
        let packages = candidate.packages_descriptor()?;
        let admissions = candidate.admissions_descriptor()?;
        baseline::require(
            File::from(packages).metadata()?.mode() & 0o7777 == 0o700
                && File::from(admissions).metadata()?.mode() & 0o7777 == 0o700,
        )?;
        result["reader_opened"] = true.into();
        if case == "snapshot-private-replacement" {
            let path = slot.join("state/packages");
            fs::rename(&path, slot.join("state/packages-old"))?;
            fs::create_dir(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
            baseline::require(candidate.validate().is_err())?;
            result["snapshot_invalidated"] = true.into();
        } else if case == "unrelated-entry" {
            let path = slot.join("state/admissions/00000000000000000000000002");
            fs::create_dir(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
            candidate.validate()?;
            baseline::require(candidate.binding().canonical_bytes()? == bytes)?;
            fs::remove_dir(&path)?;
            candidate.validate()?;
            baseline::require(fs::read(slot.join("binding.json"))? == bytes)?;
        }
    } else {
        baseline::require(opened.is_err())?;
    }
    observer.send(&json!({"stage":"observation","payload":result}))?;
    observer.go()?;
    let iterations = pressure.finish()?;
    baseline::require(iterations > 0)?;
    drop(root);
    result["resources_closed_after_exit"] = true.into();
    result["pressure_iterations"] = iterations.into();
    observer.send(&result)?;
    let mut bytes = serde_json::to_vec(&result).map_err(|_| baseline::refused())?;
    bytes.push(b'\n');
    use std::io::Write;
    io::stdout().write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_reader_cli_refuses_aliases_unknown_cases_and_duplicate_descriptors() {
        let args = |case: &str, descriptor: &str| {
            [
                "--root-reader-case",
                case,
                "--observer-fd",
                "3",
                "--root-fd",
                descriptor,
            ]
            .map(OsString::from)
        };
        assert!(parse(args("valid", "4")).is_ok());
        assert!(parse(args("unknown", "4")).is_err());
        assert!(parse(args("valid", "3")).is_err());
        assert!(parse(args("valid", "2")).is_err());
    }
}
