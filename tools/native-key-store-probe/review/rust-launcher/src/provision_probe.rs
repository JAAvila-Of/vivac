use crate::linux_custody_provision::{ProvisionError, ProvisionStage, Provisioner, Publication};
use crate::linux_custody_root::CandidateRoot;
use crate::linux_spawn::{ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const CASES: [&str; 33] = [
    "new-slot",
    "existing-destination",
    "existing-malformed",
    "existing-symlink",
    "concurrent-writers",
    "wrong-scaffold-owner",
    "wrong-scaffold-mode",
    "scaffold-symlink",
    "scaffold-acl",
    "missing-capabilities",
    "invalid-selectors",
    "error-scaffold-2",
    "error-scaffold-3",
    "error-scaffold-4",
    "error-scaffold-5",
    "error-stage-created",
    "error-before-handoff",
    "error-after-handoff",
    "error-binding-sync",
    "error-packages-sync",
    "error-admissions-sync",
    "error-openings-sync",
    "error-endpoints-sync",
    "error-state-sync",
    "error-stage-sync",
    "error-before-rename",
    "error-after-rename",
    "error-parent-sync",
    "error-after-parent-sync",
    "crash-before-rename",
    "crash-after-rename",
    "lost-reply",
    "lost-capabilities",
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

struct Arguments {
    mode: String,
    case: String,
    ordinal: u32,
    fds: [RawFd; 2],
}

fn child_case(case: &str) -> bool {
    matches!(
        case,
        "concurrent-writers" | "crash-before-rename" | "crash-after-rename" | "lost-reply"
    )
}

fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<Arguments> {
    let mut args = args.into_iter();
    let mode = args
        .next()
        .and_then(|v| v.into_string().ok())
        .ok_or_else(baseline::refused)?;
    baseline::require(matches!(
        mode.as_str(),
        "--provision-case" | "--provision-child" | "--provision-reader"
    ))?;
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
            baseline::require(mode == "--provision-child" && ordinal.is_none())?;
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
    let fds = [
        fds[0].ok_or_else(baseline::refused)?,
        fds[1].ok_or_else(baseline::refused)?,
    ];
    baseline::require(fds.iter().all(|fd| *fd >= 3) && fds[0] != fds[1])?;
    let ordinal = if mode == "--provision-child" {
        let ordinal = ordinal.ok_or_else(baseline::refused)?;
        baseline::require(
            child_case(&case)
                && (1..=if case == "concurrent-writers" { 2 } else { 1 }).contains(&ordinal),
        )?;
        ordinal
    } else {
        0
    };
    Ok(Arguments {
        mode,
        case,
        ordinal,
        fds,
    })
}

fn failure_stage(case: &str) -> Option<ProvisionStage> {
    Some(match case {
        "error-scaffold-2" => ProvisionStage::BeforeScaffoldSync(2),
        "error-scaffold-3" => ProvisionStage::BeforeScaffoldSync(3),
        "error-scaffold-4" => ProvisionStage::BeforeScaffoldSync(4),
        "error-scaffold-5" => ProvisionStage::BeforeScaffoldSync(5),
        "error-stage-created" => ProvisionStage::StageCreated,
        "error-before-handoff" => ProvisionStage::BeforeOwnershipHandoff,
        "error-after-handoff" => ProvisionStage::OwnershipHandedOff,
        "error-binding-sync" => ProvisionStage::BeforeBindingSync,
        "error-packages-sync" => ProvisionStage::BeforePackagesSync,
        "error-admissions-sync" => ProvisionStage::BeforeAdmissionsSync,
        "error-openings-sync" => ProvisionStage::BeforeOpeningsSync,
        "error-endpoints-sync" => ProvisionStage::BeforeEndpointsSync,
        "error-state-sync" => ProvisionStage::BeforeStateSync,
        "error-stage-sync" => ProvisionStage::BeforeStageSync,
        "error-before-rename" => ProvisionStage::BeforeRename,
        "error-after-rename" => ProvisionStage::Renamed,
        "error-parent-sync" => ProvisionStage::BeforeParentSync,
        "error-after-parent-sync" => ProvisionStage::ParentSynced,
        _ => return None,
    })
}

fn indeterminate(case: &str) -> bool {
    matches!(
        case,
        "error-after-rename"
            | "error-parent-sync"
            | "error-after-parent-sync"
            | "crash-after-rename"
    )
}

fn valid_final(case: &str) -> bool {
    matches!(
        case,
        "new-slot" | "existing-destination" | "concurrent-writers" | "lost-reply"
    ) || indeterminate(case)
}

fn scoped(mut value: Value) -> Value {
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

fn diagnostic(observer: &Channel, error: &impl std::fmt::Display) -> io::Result<()> {
    let diagnostic = error.to_string();
    baseline::require(diagnostic.len() <= 512 && diagnostic.is_ascii())?;
    observer.send(&json!({"stage":"reader-refusal", "diagnostic":diagnostic}))
}

fn provisioner(root: &OwnedFd, observer: &Channel) -> io::Result<Provisioner> {
    Provisioner::from_fixture_filesystem(baseline::duplicate(root.as_raw_fd())?).inspect_err(
        |error| {
            let _ = diagnostic(observer, error);
        },
    )
}

fn publish(writer: &Provisioner, observer: &Channel) -> io::Result<Publication> {
    writer.provision(65534, 65534).map_err(|error| {
        let _ = diagnostic(observer, &error);
        baseline::refused()
    })
}

fn unpublished(result: Result<Publication, ProvisionError>, occupied: bool) -> io::Result<()> {
    match result {
        Err(ProvisionError::Unpublished(error)) => {
            baseline::require(!occupied || error.kind() == io::ErrorKind::AlreadyExists)
        }
        _ => Err(baseline::refused()),
    }
}

// Capture only the explicitly selected disposable hierarchy, without following links.
fn snapshot(root: &Path) -> io::Result<Value> {
    fn visit(path: &Path, remaining: &mut usize, descriptor_root: bool) -> io::Result<Value> {
        baseline::require(*remaining > 0)?;
        *remaining -= 1;
        let metadata = if descriptor_root {
            fs::metadata(path)?
        } else {
            fs::symlink_metadata(path)?
        };
        let mut value = json!({"dev":metadata.dev(),"ino":metadata.ino(),"mode":metadata.mode(),"uid":metadata.uid(),"gid":metadata.gid(),"links":metadata.nlink()});
        if metadata.is_symlink() {
            value["target"] = fs::read_link(path)?
                .to_str()
                .ok_or_else(baseline::refused)?
                .into();
        } else if metadata.is_dir() {
            let mut entries = fs::read_dir(path)?
                .map(|v| v.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()?;
            baseline::require(entries.len() <= *remaining)?;
            entries.sort();
            let mut children = serde_json::Map::new();
            for entry in entries {
                let name = entry
                    .file_name()
                    .and_then(|v| v.to_str())
                    .ok_or_else(baseline::refused)?;
                children.insert(name.into(), visit(&entry, remaining, false)?);
            }
            value["children"] = children.into();
        } else {
            baseline::require(metadata.is_file() && metadata.len() <= 512)?;
            value["bytes"] = json!(fs::read(path)?);
        }
        Ok(value)
    }
    let descriptor_root = root.parent() == Some(Path::new("/proc/self/fd"));
    visit(root, &mut 32, descriptor_root)
}

fn clear_capabilities() -> io::Result<()> {
    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut header = Header {
        version: 0x20080522,
        pid: 0,
    };
    let data = [Data::default(); 2];
    // SAFETY: capset v3 borrows the initialized header and two records, for this thread only.
    if unsafe { libc::syscall(libc::SYS_capset, &mut header, data.as_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn child(case: &str, ordinal: u32, observer: Channel, root: OwnedFd) -> io::Result<()> {
    let writer = provisioner(&root, &observer)?;
    // SAFETY: getpid queries only this disposable writer identity.
    let pid = unsafe { libc::getpid() };
    let selected = if case == "crash-after-rename" {
        ProvisionStage::Renamed
    } else {
        ProvisionStage::BeforeRename
    };
    let mut checkpoint_seen = false;
    let result = writer.provision_with(65534, 65534, &mut |stage| {
        if case != "lost-reply" && stage == selected {
            baseline::require(!checkpoint_seen)?;
            checkpoint_seen = true;
            observer.send(&json!({"stage":"child-ready","pid":pid,"ordinal":ordinal}))?;
            observer.go()?;
            baseline::require(case == "concurrent-writers")?;
        }
        Ok(())
    });
    if !checkpoint_seen && case != "lost-reply" {
        if let Err(error) = &result {
            diagnostic(&observer, error)?;
        }
        return Err(baseline::refused());
    }
    if case == "concurrent-writers" {
        baseline::require(checkpoint_seen)?;
        let durable = result.is_ok();
        if !durable {
            if let Err(error) = &result {
                let expected = matches!(error, ProvisionError::Unpublished(inner) if inner.kind() == io::ErrorKind::AlreadyExists);
                if !expected {
                    diagnostic(&observer, error)?;
                    return Err(baseline::refused());
                }
            }
            unpublished(result, true)?;
        }
        observer
            .send(&json!({"stage":"child-result","pid":pid,"ordinal":ordinal,"durable":durable}))?;
        observer.go()?;
        return Ok(());
    }
    if case == "lost-reply" {
        if let Err(error) = result {
            diagnostic(&observer, &error)?;
            return Err(baseline::refused());
        }
        observer.send(&json!({"stage":"child-ready","pid":pid,"ordinal":ordinal}))?;
        observer.go()?;
    }
    Err(baseline::refused())
}

fn finish(observer: &Channel, result: &Value) -> io::Result<()> {
    observer.send(result)?;
    let mut bytes = serde_json::to_vec(result).map_err(|_| baseline::refused())?;
    bytes.push(b'\n');
    use std::io::Write;
    io::stdout().write_all(&bytes)
}

fn reader(case: &str, observer: Channel, root: OwnedFd) -> io::Result<()> {
    let pressure = Pressure::start()?;
    // SAFETY: these query only this disposable reader and its main thread.
    let identity = json!({"host_pid":unsafe { libc::getpid() },"creator_tid":unsafe { libc::syscall(libc::SYS_gettid) },"children":[]});
    observer.send(&json!({"stage":"before","case":case,"identity":identity}))?;
    observer.go()?;
    let mut result = scoped(
        json!({"version":1,"case":case,"status":"observed","identity":identity,
        "reader_opened":false,"resources_closed_after_exit":false,"pressure_threads":4,"pressure_iterations":0}),
    );
    let opened = CandidateRoot::from_fixture_filesystem(baseline::duplicate(root.as_raw_fd())?);
    if valid_final(case) {
        let candidate = match opened {
            Ok(candidate) => candidate,
            Err(error) => {
                diagnostic(&observer, &error)?;
                return Err(error);
            }
        };
        candidate.validate()?;
        let slot = path(&root).join("var/lib/vivac/custody-v1/users/65534");
        baseline::require(
            candidate.binding().canonical_bytes()? == fs::read(slot.join("binding.json"))?,
        )?;
        for descriptor in [
            candidate.packages_descriptor()?,
            candidate.admissions_descriptor()?,
            candidate
                .with_opening_directory(|directory| baseline::duplicate(directory.as_raw_fd()))?,
        ] {
            let file = File::from(descriptor);
            let metadata = file.metadata()?;
            baseline::require(
                metadata.uid() == 65534
                    && metadata.gid() == 65534
                    && metadata.mode() & 0o7777 == 0o700,
            )?;
            baseline::require(
                fs::read_dir(format!("/proc/self/fd/{}", file.as_raw_fd()))?
                    .next()
                    .is_none(),
            )?;
        }
        candidate.validate()?;
        result["reader_opened"] = true.into();
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
    finish(&observer, &result)
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: alarm bounds only this explicitly disposable fixture process.
    unsafe { libc::alarm(45) };
    let Arguments {
        mode,
        case,
        ordinal,
        fds: [observer_fd, root_fd],
    } = parse(std::env::args_os().skip(1))?;
    let observer = Channel::new(baseline::take_inherited(observer_fd)?)?;
    let root = baseline::take_inherited(root_fd)?;
    if mode == "--provision-child" {
        return child(&case, ordinal, observer, root);
    }
    if mode == "--provision-reader" {
        return reader(&case, observer, root);
    }
    if case == "missing-capabilities" {
        clear_capabilities()?;
    }
    let mut pressure = if case == "lost-capabilities" {
        None
    } else {
        Some(Pressure::start()?)
    };
    let mut owner = SpawnOwner::new()?;
    let mut children = Vec::new();
    if child_case(&case) {
        let count = if case == "concurrent-writers" { 2 } else { 1 };
        for ordinal in 1..=count {
            let (control, child_fd) = Channel::pair()?;
            let copy = baseline::duplicate(root.as_raw_fd())?;
            let args = vec![
                "--provision-child".into(),
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
            if ready["stage"] == "reader-refusal" {
                observer.send(&ready)?;
                return Err(baseline::refused());
            }
            baseline::require(
                ready == json!({"stage":"child-ready","pid":receipt.pid,"ordinal":ordinal}),
            )?;
            children.push((receipt, ordinal, control));
        }
    }
    // SAFETY: these query the host's current process and owner thread identities.
    let host_pid = unsafe { libc::getpid() };
    let creator_tid = unsafe { libc::syscall(libc::SYS_gettid) };
    baseline::require(i64::from(host_pid) == creator_tid)?;
    let identities: Vec<Value> = children
        .iter()
        .map(|(receipt, ordinal, _)| receipt_value(*receipt, *ordinal))
        .collect();
    let identity = json!({"host_pid":host_pid,"creator_tid":creator_tid,"children":identities});
    observer.send(&json!({"stage":"before","case":case,"identity":identity}))?;
    observer.go()?;
    let mut result = scoped(
        json!({"version":1,"case":case,"status":"observed","identity":identity,
        "publication_results":0,"unpublished_refusals":0,"durability_indeterminate":false,"reply_lost":false,
        "final_name_occupied":false,"existing_unchanged":false,"access_policy_checks":0,"injected_failure":"none",
        "child_exits":[],"owned_exits_observed":false,"resources_closed_after_exit":false,
        "pressure_threads":4,"pressure_iterations":0}),
    );
    let directory = path(&root);
    let slot = directory.join("var/lib/vivac/custody-v1/users/65534");
    let mut exits = Vec::new();
    match case.as_str() {
        "concurrent-writers" => {
            // Both children have reported the pre-rename barrier before either GO.
            for (_, _, control) in &children {
                control.bytes(b"G")?;
            }
            let mut publications = 0;
            for (receipt, ordinal, control) in &children {
                let packet = control.packet()?;
                if packet["stage"] == "reader-refusal" {
                    observer.send(&packet)?;
                    return Err(baseline::refused());
                }
                baseline::require(
                    packet["stage"] == "child-result"
                        && packet["pid"] == receipt.pid
                        && packet["ordinal"] == *ordinal,
                )?;
                if packet["durable"] == true {
                    publications += 1;
                } else {
                    baseline::require(packet["durable"] == false)?;
                }
            }
            baseline::require(publications == 1)?;
            result["publication_results"] = 1.into();
            result["unpublished_refusals"] = 1.into();
        }
        "crash-before-rename" | "crash-after-rename" | "lost-reply" => {
            let receipt = children[0].0;
            owner.signal(receipt.token, libc::SIGKILL)?;
            exits.push(wait(&mut owner, receipt, true)?);
            result["durability_indeterminate"] = indeterminate(&case).into();
            result["reply_lost"] = (case == "lost-reply").into();
            result["child_exits"] = exits
                .iter()
                .copied()
                .map(exit_value)
                .collect::<Vec<_>>()
                .into();
            result["owned_exits_observed"] = true.into();
        }
        "missing-capabilities" => {
            let before = snapshot(&directory)?;
            baseline::require(
                Provisioner::from_fixture_filesystem(baseline::duplicate(root.as_raw_fd())?)
                    .is_err(),
            )?;
            baseline::require(snapshot(&directory)? == before)?;
            result["unpublished_refusals"] = 1.into();
        }
        "lost-capabilities" | "invalid-selectors" => {
            let before = snapshot(&directory)?;
            let writer = provisioner(&root, &observer)?;
            if case == "lost-capabilities" {
                clear_capabilities()?;
                pressure = Some(Pressure::start()?);
                unpublished(writer.provision(65534, 65534), false)?;
                result["unpublished_refusals"] = 1.into();
            } else {
                for (uid, gid) in [(0, 65534), (65534, 0), (u32::MAX, 65534), (65534, u32::MAX)] {
                    unpublished(writer.provision(uid, gid), false)?;
                    baseline::require(snapshot(&directory)? == before)?;
                }
                result["unpublished_refusals"] = 4.into();
            }
            baseline::require(snapshot(&directory)? == before)?;
        }
        "new-slot" | "existing-destination" => {
            let writer = provisioner(&root, &observer)?;
            let publication = publish(&writer, &observer)?;
            baseline::require(
                publication.binding.canonical_bytes()? == fs::read(slot.join("binding.json"))?,
            )?;
            result["publication_results"] = 1.into();
            if case == "existing-destination" {
                let before = snapshot(&slot)?;
                unpublished(writer.provision(65534, 65534), true)?;
                baseline::require(snapshot(&slot)? == before)?;
                result["unpublished_refusals"] = 1.into();
                result["existing_unchanged"] = true.into();
            }
        }
        "existing-malformed" | "existing-symlink" => {
            let before = snapshot(&slot)?;
            let writer = provisioner(&root, &observer)?;
            unpublished(writer.provision(65534, 65534), true)?;
            baseline::require(snapshot(&slot)? == before)?;
            result["unpublished_refusals"] = 1.into();
            result["existing_unchanged"] = true.into();
        }
        "wrong-scaffold-owner" | "wrong-scaffold-mode" | "scaffold-symlink" | "scaffold-acl" => {
            if case == "scaffold-acl" {
                let scaffold = File::open(directory.join("var/lib/vivac/custody-v1"))?;
                // SAFETY: the retained descriptor and fixed terminated name query only xattr size.
                baseline::require(
                    unsafe {
                        libc::fgetxattr(
                            scaffold.as_raw_fd(),
                            c"system.posix_acl_access".as_ptr(),
                            std::ptr::null_mut(),
                            0,
                        )
                    } > 0,
                )?;
                result["access_policy_checks"] = 1.into();
            }
            if let Ok(writer) =
                Provisioner::from_fixture_filesystem(baseline::duplicate(root.as_raw_fd())?)
            {
                unpublished(writer.provision(65534, 65534), false)?;
            }
            result["unpublished_refusals"] = 1.into();
        }
        _ => {
            let selected = failure_stage(&case).ok_or_else(baseline::refused)?;
            let writer = provisioner(&root, &observer)?;
            let mut hits = 0;
            let attempt = writer.provision_with(65534, 65534, &mut |stage| {
                if stage == selected {
                    hits += 1;
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            });
            baseline::require(hits == 1)?;
            let error = match attempt {
                Err(ProvisionError::Unpublished(error)) if !indeterminate(&case) => error,
                Err(ProvisionError::PublishedDurabilityUnknown(error)) if indeterminate(&case) => {
                    error
                }
                _ => return Err(baseline::refused()),
            };
            baseline::require(error.kind() == io::Error::from_raw_os_error(libc::EIO).kind())?;
            result["injected_failure"] = case.clone().into();
            result["durability_indeterminate"] = indeterminate(&case).into();
            if !indeterminate(&case) {
                result["unpublished_refusals"] = 1.into();
            }
        }
    }
    let occupied = match fs::symlink_metadata(&slot) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    baseline::require(
        occupied
            == (valid_final(&case)
                || matches!(case.as_str(), "existing-malformed" | "existing-symlink")),
    )?;
    result["final_name_occupied"] = occupied.into();
    observer.send(&json!({"stage":"observation","payload":result}))?;
    observer.go()?;
    if case == "concurrent-writers" {
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
    let iterations = pressure.ok_or_else(baseline::refused)?.finish()?;
    baseline::require(iterations > 0)?;
    drop(children);
    drop(owner);
    drop(root);
    result["child_exits"] = exits.into_iter().map(exit_value).collect::<Vec<_>>().into();
    result["owned_exits_observed"] = true.into();
    result["resources_closed_after_exit"] = true.into();
    result["pressure_iterations"] = iterations.into();
    finish(&observer, &result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provision_cli_accepts_explicit_new_slot() {
        assert!(parse(
            [
                "--provision-case",
                "new-slot",
                "--observer-fd",
                "3",
                "--root-fd",
                "4"
            ]
            .map(OsString::from)
        )
        .is_ok());
    }
    #[test]
    fn provision_cli_requires_exact_cases_modes_and_distinct_explicit_descriptors() {
        for case in CASES {
            for mode in ["--provision-case", "--provision-reader"] {
                assert!(parse(
                    [mode, case, "--observer-fd", "3", "--root-fd", "4"].map(OsString::from)
                )
                .is_ok());
            }
        }
        for values in [
            vec![
                "--provision-case",
                "unknown",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
            ],
            vec![
                "--provision-case",
                "new-slot",
                "--observer-fd",
                "3",
                "--root-fd",
                "3",
            ],
            vec![
                "--provision-case",
                "new-slot",
                "--observer-fd",
                "2",
                "--root-fd",
                "4",
            ],
            vec![
                "--provision-case",
                "new-slot",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
                "--observer-fd",
                "5",
            ],
            vec![
                "--provision-case",
                "new-slot",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
                "--ordinal",
                "1",
            ],
            vec![
                "--provision-child",
                "new-slot",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
                "--ordinal",
                "1",
            ],
            vec![
                "--provision-child",
                "concurrent-writers",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
            ],
            vec![
                "--provision-child",
                "lost-reply",
                "--observer-fd",
                "3",
                "--root-fd",
                "4",
                "--ordinal",
                "2",
            ],
        ] {
            assert!(parse(values.into_iter().map(OsString::from)).is_err());
        }
    }
}
