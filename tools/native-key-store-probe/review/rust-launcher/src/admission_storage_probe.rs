use crate::linux_admission_storage::{Metadata, PublishError, PublishStage, Storage};
use crate::linux_spawn::{ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

const CASES: [&str; 12] = [
    "roundtrip",
    "existing-destination",
    "concurrent-publishers",
    "snapshot-in-place",
    "snapshot-replacement",
    "symlink-refused",
    "hardlink-refused",
    "pre-rename-error",
    "post-rename-sync-error",
    "crash-before-rename",
    "crash-after-rename",
    "acl-refused",
];
const CREDENTIAL: &str = "00000000000000000000000030";

fn set_acl(fd: &OwnedFd, name: &[u8], permissions: u16) -> io::Result<()> {
    let file = File::from(baseline::duplicate(fd.as_raw_fd())?);
    let before = file.metadata()?;
    // SAFETY: F_GETFD reads flags of this retained descriptor.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    baseline::require(before.uid() == 65534 && before.gid() == 65534 && flags == libc::FD_CLOEXEC)?;
    let mut payload = 2u32.to_le_bytes().to_vec();
    for (tag, permission, id) in [
        (1u16, permissions, u32::MAX),
        (2, 0, 0),
        (4, 0, u32::MAX),
        (16, 0, u32::MAX),
        (32, 0, u32::MAX),
    ] {
        payload.extend_from_slice(&tag.to_le_bytes());
        payload.extend_from_slice(&permission.to_le_bytes());
        payload.extend_from_slice(&id.to_le_bytes());
    }
    // SAFETY: name is a static terminated xattr name; payload is initialized UAPI bytes.
    if unsafe {
        libc::fsetxattr(
            fd.as_raw_fd(),
            name.as_ptr().cast(),
            payload.as_ptr().cast(),
            payload.len(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a null buffer and zero size query only this descriptor's xattr length.
    let length = unsafe {
        libc::fgetxattr(
            fd.as_raw_fd(),
            name.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
        )
    };
    let after = file.metadata()?;
    // SAFETY: F_GETFD reads flags of the same retained descriptor.
    let after_flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    baseline::require(
        length > 0
            && before.mode() == after.mode()
            && before.uid() == after.uid()
            && before.gid() == after.gid()
            && flags == after_flags,
    )
}

fn remove_acl(fd: &OwnedFd, name: &[u8]) -> io::Result<()> {
    // SAFETY: name is a static terminated xattr name; fd remains owned by the fixture.
    if unsafe { libc::fremovexattr(fd.as_raw_fd(), name.as_ptr().cast()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

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
    fn pair() -> io::Result<(Self, OwnedFd)> {
        let mut fds = [-1; 2];
        // SAFETY: socketpair initializes both descriptor slots on success.
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
        // SAFETY: both newly returned descriptors transfer exclusive ownership.
        let [left, right] = unsafe { fds.map(|fd| OwnedFd::from_raw_fd(fd)) };
        Ok((Self::new(left)?, right))
    }
    fn bytes(&self, bytes: &[u8]) -> io::Result<()> {
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
    fn send(&self, value: &Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|_| baseline::refused())?;
        baseline::require(bytes.len() <= 8192)?;
        self.bytes(&bytes)
    }
    fn receive(&self) -> io::Result<Vec<u8>> {
        let mut bytes = [0u8; 8193];
        // SAFETY: recv writes only into this fixed initialized packet buffer.
        let count = unsafe {
            libc::recv(
                self.0.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_TRUNC,
            )
        };
        baseline::require(count > 0 && count <= 8192)?;
        Ok(bytes[..count as usize].to_vec())
    }
    fn packet(&self) -> io::Result<Value> {
        serde_json::from_slice(&self.receive()?).map_err(|_| baseline::refused())
    }
    fn go(&self) -> io::Result<()> {
        baseline::require(self.receive()? == b"G")
    }
}

fn metadata(ordinal: u32) -> Metadata {
    Metadata {
        version: 1,
        installation_id: "00000000000000000000000010".into(),
        creation_id: format!("{ordinal:026}"),
        device_id: "00000000000000000000000020".into(),
        credential_id: CREDENTIAL.into(),
        public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
        ciphertext_sha256: "11".repeat(32),
        manifest_sha256: "22".repeat(32),
        ciphertext_len: 64,
        manifest_len: 100,
        creation_profile_revision: 1,
        secret_policy_revision: 1,
    }
}
fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<(bool, String, u32, [RawFd; 2])> {
    let mut args = args.into_iter();
    let prefix = args.next().ok_or_else(baseline::refused)?;
    let child = prefix == "--admission-storage-child";
    baseline::require(child || prefix == "--admission-storage-case")?;
    let case = args
        .next()
        .and_then(|v| v.into_string().ok())
        .ok_or_else(baseline::refused)?;
    baseline::require(CASES.contains(&case.as_str()))?;
    let mut fds = [None; 2];
    let mut ordinal = None;
    while let Some(option) = args.next() {
        let value = args
            .next()
            .and_then(|v| v.into_string().ok())
            .ok_or_else(baseline::refused)?;
        if option == "--ordinal" {
            baseline::require(child && ordinal.is_none())?;
            ordinal = Some(value.parse::<u32>().map_err(|_| baseline::refused())?);
        } else {
            let index = ["--observer-fd", "--root-fd"]
                .iter()
                .position(|v| option == *v)
                .ok_or_else(baseline::refused)?;
            baseline::require(fds[index].is_none())?;
            fds[index] = Some(value.parse::<RawFd>().map_err(|_| baseline::refused())?);
        }
    }
    let [left, right] = fds;
    let fds = [
        left.ok_or_else(baseline::refused)?,
        right.ok_or_else(baseline::refused)?,
    ];
    baseline::require(fds.iter().all(|fd| *fd >= 3) && fds[0] != fds[1])?;
    let ordinal = if child {
        ordinal.ok_or_else(baseline::refused)?
    } else {
        0
    };
    baseline::require(!child || (1..=2).contains(&ordinal))?;
    Ok((child, case, ordinal, fds))
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
                exit.pid == receipt.pid
                    && exit.token == receipt.token
                    && if killed {
                        exit.code.is_none() && exit.signal == Some(libc::SIGKILL)
                    } else {
                        exit.code == Some(0) && exit.signal.is_none()
                    },
            )?;
            let mut descriptor = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll borrows one initialized descriptor for this exact child.
            baseline::require(
                unsafe { libc::poll(&mut descriptor, 1, 0) } == 1
                    && descriptor.revents & libc::POLLIN != 0,
            )?;
            return Ok(exit);
        }
        baseline::require(Instant::now() < deadline)?;
        thread::sleep(Duration::from_millis(5));
    }
}
fn receipt_value(receipt: Receipt, ordinal: u32) -> Value {
    json!({"pid":receipt.pid,"token":receipt.token,"ordinal":ordinal})
}
fn exit_value(exit: ExitReceipt) -> Value {
    json!({"pid":exit.pid,"token":exit.token,"code":exit.code,"signal":exit.signal})
}
fn path(fd: &OwnedFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()))
}
fn publish_error(result: Result<(), PublishError>, published: bool, errno: i32) -> io::Result<()> {
    match result {
        Err(PublishError::PublishedDurabilityUnknown(error)) if published => {
            baseline::require(error.raw_os_error() == Some(errno))
        }
        Err(PublishError::Unpublished(error)) if !published => {
            baseline::require(error.raw_os_error() == Some(errno))
        }
        _ => Err(baseline::refused()),
    }
}
fn child(case: &str, ordinal: u32, observer: Channel, root: OwnedFd) -> io::Result<()> {
    let storage = Storage::from_anchored(root)?;
    // SAFETY: getpid only queries this disposable publisher.
    let pid = unsafe { libc::getpid() };
    if case == "concurrent-publishers" {
        observer.send(&json!({"stage":"child-ready","pid":pid,"ordinal":ordinal}))?;
        observer.go()?;
        let result = storage.publish(&metadata(ordinal));
        let durable = result.is_ok();
        if !durable {
            publish_error(result, false, libc::EEXIST)?;
        }
        observer
            .send(&json!({"stage":"child-result","pid":pid,"ordinal":ordinal,"durable":durable}))?;
        observer.go()?;
        return Ok(());
    }
    let selected = if case == "crash-before-rename" {
        PublishStage::BeforeRename
    } else {
        PublishStage::Renamed
    };
    baseline::require(case == "crash-before-rename" || case == "crash-after-rename")?;
    let _result = storage.publish_with(&metadata(ordinal), &mut |stage| {
        if stage == selected {
            observer.send(&json!({"stage":"child-ready","pid":pid,"ordinal":ordinal}))?;
            observer.go()?;
            return Err(baseline::refused());
        }
        Ok(())
    });
    Err(baseline::refused())
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: the alarm bounds only this explicitly disposable metadata experiment.
    unsafe { libc::alarm(45) };
    let (is_child, case, ordinal, [observer_fd, root_fd]) = parse(std::env::args_os().skip(1))?;
    let observer = Channel::new(baseline::take_inherited(observer_fd)?)?;
    let root = baseline::take_inherited(root_fd)?;
    if is_child {
        return child(&case, ordinal, observer, root);
    }
    let storage = Storage::from_anchored(baseline::duplicate(root.as_raw_fd())?)?;
    let pressure = Pressure::start()?;
    let mut owner = SpawnOwner::new()?;
    let mut children = Vec::new();
    if case == "concurrent-publishers" || case.starts_with("crash-") {
        let count = if case == "concurrent-publishers" {
            2
        } else {
            1
        };
        for ordinal in 1..=count {
            let (control, child_fd) = Channel::pair()?;
            let copy = baseline::duplicate(root.as_raw_fd())?;
            let args = vec![
                "--admission-storage-child".into(),
                case.clone().into(),
                "--ordinal".into(),
                ordinal.to_string().into(),
                "--observer-fd".into(),
                child_fd.as_raw_fd().to_string().into(),
                "--root-fd".into(),
                copy.as_raw_fd().to_string().into(),
            ];
            let receipt = owner.spawn(spec(args, vec![child_fd, copy])?)?;
            let ready = control.packet()?;
            baseline::require(
                ready == json!({"stage":"child-ready","pid":receipt.pid,"ordinal":ordinal}),
            )?;
            children.push((receipt, ordinal, control));
        }
    }
    // SAFETY: these calls query the host main thread, which owns the launcher.
    let host_pid = unsafe { libc::getpid() };
    let creator_tid = unsafe { libc::syscall(libc::SYS_gettid) };
    baseline::require(i64::from(host_pid) == creator_tid)?;
    let identities: Vec<Value> = children
        .iter()
        .map(|(r, n, _)| receipt_value(*r, *n))
        .collect();
    let identity = json!({"host_pid":host_pid,"creator_tid":creator_tid,"children":identities});
    observer.send(&json!({"stage":"before","case":case,"identity":identity}))?;
    observer.go()?;
    let mut result = json!({"version":1,"case":case,"status":"observed","identity":identity,
        "successful_publications":0,"unpublished_refusals":0,"metadata_visible":true,"snapshot_invalidated":false,
        "alias_checks":0,"access_policy_checks":0,"reader_refused":false,"existing_unchanged":false,"durability_indeterminate":false,
        "injected_failure":"none","child_exits":[],"owned_exits_observed":false,"resources_closed_after_exit":false,
        "pressure_threads":4,"pressure_iterations":0,"custody_accredited":false,"creation_admission":false,
        "producer_origin_checked":false,"root_bootstrap_accredited":false,"endpoint_authority_accredited":false,
        "product_recovery":false,"same_store_recovery":false,"mcp_integration":false,"server_integration":false,"lock_ack":false});
    let mut exits = Vec::new();
    let base = metadata(31);
    let directory = path(&root);
    let entry = directory.join(CREDENTIAL);
    match case.as_str() {
        "roundtrip" => {
            storage.publish(&base).map_err(|_| baseline::refused())?;
            let snapshot = storage.read(CREDENTIAL)?;
            baseline::require(snapshot.metadata().canonical_bytes()? == base.canonical_bytes()?)?;
            snapshot.validate()?;
            let reopened = Storage::from_anchored(baseline::duplicate(root.as_raw_fd())?)?;
            baseline::require(
                reopened.read(CREDENTIAL)?.metadata().canonical_bytes()?
                    == base.canonical_bytes()?,
            )?;
            result["successful_publications"] = 1.into();
        }
        "existing-destination" => {
            storage.publish(&base).map_err(|_| baseline::refused())?;
            let snapshot = storage.read(CREDENTIAL)?;
            publish_error(storage.publish(&metadata(32)), false, libc::EEXIST)?;
            snapshot.validate()?;
            baseline::require(
                storage.read(CREDENTIAL)?.metadata().canonical_bytes()?
                    == base.canonical_bytes()?,
            )?;
            result["successful_publications"] = 1.into();
            result["unpublished_refusals"] = 1.into();
            result["existing_unchanged"] = true.into();
        }
        "concurrent-publishers" => {
            for (_, _, control) in &children {
                control.bytes(b"G")?;
            }
            let mut winners = Vec::new();
            for (receipt, ordinal, control) in &children {
                let packet = control.packet()?;
                baseline::require(
                    packet["stage"] == "child-result"
                        && packet["pid"] == receipt.pid
                        && packet["ordinal"] == *ordinal,
                )?;
                if packet["durable"] == true {
                    winners.push(*ordinal);
                } else {
                    baseline::require(packet["durable"] == false)?;
                }
            }
            baseline::require(winners.len() == 1)?;
            baseline::require(
                storage.read(CREDENTIAL)?.metadata().canonical_bytes()?
                    == metadata(winners[0]).canonical_bytes()?,
            )?;
            result["successful_publications"] = 1.into();
            result["unpublished_refusals"] = 1.into();
        }
        "snapshot-in-place" | "snapshot-replacement" => {
            storage.publish(&base).map_err(|_| baseline::refused())?;
            let snapshot = storage.read(CREDENTIAL)?;
            if case == "snapshot-in-place" {
                let before = fs::metadata(entry.join("record.json"))?;
                fs::write(entry.join("record.json"), metadata(32).canonical_bytes()?)?;
                let after = fs::metadata(entry.join("record.json"))?;
                baseline::require(before.dev() == after.dev() && before.ino() == after.ino())?;
            } else {
                fs::rename(&entry, directory.join("old"))?;
                fs::create_dir(&entry)?;
                fs::set_permissions(&entry, fs::Permissions::from_mode(0o700))?;
                fs::write(entry.join("record.json"), base.canonical_bytes()?)?;
                fs::set_permissions(entry.join("record.json"), fs::Permissions::from_mode(0o600))?;
            }
            baseline::require(snapshot.validate().is_err())?;
            result["successful_publications"] = 1.into();
            result["snapshot_invalidated"] = true.into();
        }
        "symlink-refused" | "hardlink-refused" => {
            storage.publish(&base).map_err(|_| baseline::refused())?;
            if case == "symlink-refused" {
                fs::rename(entry.join("record.json"), directory.join("original-record"))?;
                std::os::unix::fs::symlink("../original-record", entry.join("record.json"))?;
                baseline::require(storage.read(CREDENTIAL).is_err())?;
                fs::remove_file(entry.join("record.json"))?;
                fs::rename(directory.join("original-record"), entry.join("record.json"))?;
                fs::rename(&entry, directory.join("old"))?;
                std::os::unix::fs::symlink("old", &entry)?;
                baseline::require(storage.read(CREDENTIAL).is_err())?;
                result["alias_checks"] = 2.into();
            } else {
                fs::hard_link(entry.join("record.json"), directory.join("linked-record"))?;
                baseline::require(
                    fs::metadata(entry.join("record.json"))?.nlink() == 2
                        && storage.read(CREDENTIAL).is_err(),
                )?;
                result["alias_checks"] = 1.into();
            }
            result["successful_publications"] = 1.into();
            result["reader_refused"] = true.into();
        }
        "acl-refused" => {
            for name in [
                b"system.posix_acl_access\0".as_slice(),
                b"system.posix_acl_default\0".as_slice(),
            ] {
                baseline::require(fs::metadata(&directory)?.mode() & 0o7777 == 0o700)?;
                set_acl(&root, name, 7)?;
                baseline::require(
                    Storage::from_anchored(baseline::duplicate(root.as_raw_fd())?).is_err(),
                )?;
                remove_acl(&root, name)?;
            }
            storage.publish(&base).map_err(|_| baseline::refused())?;
            let record = entry.join("record.json");
            let bytes = fs::read(&record)?;
            baseline::require(
                bytes == base.canonical_bytes()? && fs::metadata(&record)?.mode() & 0o7777 == 0o600,
            )?;
            let descriptor: OwnedFd = File::open(&record)?.into();
            set_acl(&descriptor, b"system.posix_acl_access\0", 6)?;
            baseline::require(fs::read(&record)? == bytes && storage.read(CREDENTIAL).is_err())?;
            result["successful_publications"] = 1.into();
            result["reader_refused"] = true.into();
            result["access_policy_checks"] = 3.into();
        }
        "pre-rename-error" | "post-rename-sync-error" => {
            let published = case == "post-rename-sync-error";
            let selected = if published {
                PublishStage::BeforeParentSync
            } else {
                PublishStage::BeforeRename
            };
            publish_error(
                storage.publish_with(&base, &mut |stage| {
                    if stage == selected {
                        Err(io::Error::from_raw_os_error(libc::EIO))
                    } else {
                        Ok(())
                    }
                }),
                published,
                libc::EIO,
            )?;
            result["injected_failure"] = if published {
                "before-parent-sync"
            } else {
                "before-rename"
            }
            .into();
            result["durability_indeterminate"] = published.into();
            if published {
                let snapshot = storage.read(CREDENTIAL)?;
                publish_error(storage.publish(&metadata(32)), false, libc::EEXIST)?;
                snapshot.validate()?;
                result["existing_unchanged"] = true.into();
                result["unpublished_refusals"] = 1.into();
            } else {
                baseline::require(!entry.exists() && storage.read(CREDENTIAL).is_err())?;
                result["metadata_visible"] = false.into();
                result["reader_refused"] = true.into();
                result["unpublished_refusals"] = 1.into();
            }
        }
        "crash-before-rename" | "crash-after-rename" => {
            let receipt = children[0].0;
            owner.signal(receipt.token, libc::SIGKILL)?;
            exits.push(wait(&mut owner, receipt, true)?);
            let published = case == "crash-after-rename";
            result["durability_indeterminate"] = published.into();
            if published {
                let snapshot = storage.read(CREDENTIAL)?;
                publish_error(storage.publish(&metadata(32)), false, libc::EEXIST)?;
                snapshot.validate()?;
                result["existing_unchanged"] = true.into();
                result["unpublished_refusals"] = 1.into();
            } else {
                baseline::require(!entry.exists() && storage.read(CREDENTIAL).is_err())?;
                result["metadata_visible"] = false.into();
                result["reader_refused"] = true.into();
            }
            result["child_exits"] = exits
                .iter()
                .copied()
                .map(exit_value)
                .collect::<Vec<_>>()
                .into();
            result["owned_exits_observed"] = true.into();
        }
        _ => return Err(baseline::refused()),
    }
    observer.send(&json!({"stage":"observation","payload":result}))?;
    observer.go()?;
    if case == "concurrent-publishers" {
        for (receipt, _, control) in &children {
            control.bytes(b"G")?;
            exits.push(wait(&mut owner, *receipt, false)?);
        }
    }
    let all_exits = owner.shutdown(Duration::from_secs(3))?;
    baseline::require(
        all_exits.len() == exits.len()
            && all_exits.iter().zip(&exits).all(|(a, b)| {
                a.pid == b.pid && a.token == b.token && a.code == b.code && a.signal == b.signal
            }),
    )?;
    let iterations = pressure.finish()?;
    baseline::require(iterations > 0)?;
    result["child_exits"] = exits.into_iter().map(exit_value).collect::<Vec<_>>().into();
    result["owned_exits_observed"] = true.into();
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
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn storage_cli_rejects_unknown_cases_aliases_and_duplicate_descriptors() {
        assert!(parse(args(&[
            "--admission-storage-case",
            "roundtrip",
            "--observer-fd",
            "3",
            "--root-fd",
            "4"
        ]))
        .is_ok());
        for values in [
            vec![
                "--admission-storage-case",
                "unknown",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
            ],
            vec![
                "--admission-storage-case",
                "roundtrip",
                "--observer-fd",
                "3",
                "--root-fd",
                "3",
            ],
            vec![
                "--admission-storage-child",
                "roundtrip",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
            ],
        ] {
            assert!(parse(args(&values)).is_err());
        }
    }
}
