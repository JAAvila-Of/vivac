use super::creation_producer::{
    AttendedCreation, ClosedCreationDraft, PackageCheckpoints, ProofEvent, ProofResult,
    PublicationCheckpoint, StartupEvent,
};
use super::RunError;
use crate::linux_gate::Snapshot;
use crate::linux_spawn::SpawnOwner;
use serde_json::{json, Value};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PACKET: usize = 8192;
const PUBLICATION_CASES: &[&str] = &[
    "publish-contended-credential",
    "publish-complete",
    "publish-package-failure",
    "publish-record-before-rename-failure",
    "publish-record-after-rename-failure",
    "publish-invalidate-before-package",
    "publish-invalidate-package",
    "publish-invalidate-retained-record",
    "publish-invalidate-retained-package",
    "death-package-before-rename",
    "death-package-renamed",
    "death-package-parent-synced",
    "death-record-file-synced",
    "death-record-staging-synced",
    "death-record-before-rename",
    "death-record-renamed",
    "death-record-before-parent-sync",
    "death-record-parent-synced",
    "death-before-durable-reply",
];
type ProbeResult<T> = Result<T, RunError>;

struct Arguments {
    case: String,
    device: String,
    credential: String,
    observer: RawFd,
    runtime: PathBuf,
    initial_opening_permit: bool,
}

fn require(value: bool) -> ProbeResult<()> {
    if value {
        Ok(())
    } else {
        Err(RunError::Refused)
    }
}
fn refused<T>(_: T) -> RunError {
    RunError::Refused
}

fn parse_arguments(mut arguments: impl Iterator<Item = OsString>) -> ProbeResult<Arguments> {
    let case = arguments.next().ok_or(())?.into_string().map_err(refused)?;
    require(
        ["complete", "invalidate-before-proof", "abandon-after-load"].contains(&case.as_str())
            || PUBLICATION_CASES.contains(&case.as_str()),
    )?;
    let device = arguments.next().ok_or(())?.into_string().map_err(refused)?;
    let credential = arguments.next().ok_or(())?.into_string().map_err(refused)?;
    require(super::valid_identifier(&device) && super::valid_identifier(&credential))?;
    require(arguments.next().as_deref() == Some(OsStr::new("--observer-fd")))?;
    let value = arguments.next().ok_or(())?.into_string().map_err(refused)?;
    require(!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))?;
    let observer = value.parse::<RawFd>().map_err(refused)?;
    require(observer >= 3 && observer.to_string() == value)?;
    require(arguments.next().as_deref() == Some(OsStr::new("--runtime-dir")))?;
    let runtime = PathBuf::from(arguments.next().ok_or(())?);
    let initial_opening_permit = match arguments.next() {
        None => false,
        Some(option) if option == "--initial-opening-permit" => {
            require(matches!(
                case.as_str(),
                "publish-complete" | "publish-record-before-rename-failure"
            ))?;
            true
        }
        Some(_) => return Err(RunError::Refused),
    };
    require(runtime.is_absolute() && arguments.next().is_none())?;
    Ok(Arguments {
        case,
        device,
        credential,
        observer,
        runtime,
        initial_opening_permit,
    })
}

fn take_inherited(descriptor: RawFd) -> ProbeResult<OwnedFd> {
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    require(descriptor >= 3 && flags >= 0)?;
    // SAFETY: the observer CLI descriptor transfers ownership exactly once.
    let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
    require(unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == 0)?;
    require(unsafe { libc::fcntl(descriptor, libc::F_GETFD) } & libc::FD_CLOEXEC != 0)?;
    Ok(owned)
}
struct Control(OwnedFd);
impl Control {
    fn new(descriptor: OwnedFd) -> ProbeResult<Self> {
        let mut kind: libc::c_int = 0;
        let mut length = size_of::<libc::c_int>() as libc::socklen_t;
        require(
            unsafe {
                libc::getsockopt(
                    descriptor.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_TYPE,
                    std::ptr::addr_of_mut!(kind).cast(),
                    &mut length,
                )
            } == 0
                && length as usize == size_of::<libc::c_int>()
                && kind == libc::SOCK_SEQPACKET,
        )?;
        let mut address: libc::sockaddr_storage = unsafe { zeroed() };
        let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        require(
            unsafe {
                libc::getsockname(
                    descriptor.as_raw_fd(),
                    std::ptr::addr_of_mut!(address).cast(),
                    &mut length,
                )
            } == 0
                && length >= size_of::<libc::sa_family_t>() as libc::socklen_t
                && address.ss_family as libc::c_int == libc::AF_UNIX,
        )?;
        let timeout = libc::timeval {
            tv_sec: 15,
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            require(
                unsafe {
                    libc::setsockopt(
                        descriptor.as_raw_fd(),
                        libc::SOL_SOCKET,
                        option,
                        std::ptr::addr_of!(timeout).cast(),
                        size_of::<libc::timeval>() as libc::socklen_t,
                    )
                } == 0,
            )?;
        }
        Ok(Self(descriptor))
    }
    fn send(&self, value: &Value) -> ProbeResult<Vec<u8>> {
        let packet = serde_json::to_vec(value).map_err(refused)?;
        require(!packet.is_empty() && packet.len() <= MAX_PACKET)?;
        require(
            unsafe {
                libc::send(
                    self.0.as_raw_fd(),
                    packet.as_ptr().cast(),
                    packet.len(),
                    libc::MSG_NOSIGNAL,
                )
            } == packet.len() as isize,
        )?;
        Ok(packet)
    }
    fn acknowledge(&self, expected: u8) -> ProbeResult<()> {
        let mut received = [0u8; 1];
        require(
            unsafe {
                libc::recv(
                    self.0.as_raw_fd(),
                    received.as_mut_ptr().cast(),
                    1,
                    libc::MSG_TRUNC,
                )
            } == 1
                && received[0] == expected,
        )
    }
    fn send_master(&self, descriptor: &OwnedFd) -> ProbeResult<()> {
        let mut marker = [b'F'];
        let mut vector = libc::iovec {
            iov_base: marker.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let mut ancillary = [0usize; 4];
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = ancillary.as_mut_ptr().cast();
        message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as usize;
        require(message.msg_controllen <= size_of::<[usize; 4]>())?;
        // SAFETY: aligned ancillary storage is sized for one descriptor.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            require(!header.is_null())?;
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as usize;
            std::ptr::write_unaligned(
                libc::CMSG_DATA(header).cast::<RawFd>(),
                descriptor.as_raw_fd(),
            );
            require(libc::sendmsg(self.0.as_raw_fd(), &message, libc::MSG_NOSIGNAL) == 1)?;
        }
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    result
}

fn death_point(case: &str) -> Option<&str> {
    if PUBLICATION_CASES.contains(&case) {
        case.strip_prefix("death-")
    } else {
        None
    }
}

fn cut_packet(report: &Value, point: &str) -> ProbeResult<Value> {
    let case = report["case"].as_str().ok_or(())?;
    require(death_point(case) == Some(point))?;
    let mut packet = report.clone();
    let fields = packet.as_object_mut().ok_or(())?;
    require(
        [
            "publication_outcome",
            "package_published",
            "record_published",
            "experimental_durable_publication_ack",
        ]
        .iter()
        .all(|field| !fields.contains_key(*field)),
    )?;
    // A cut witnesses native closure and a publication boundary, not destruction.
    fields.remove("preparation_dropped");
    fields.insert("stage".into(), json!("publication-cut"));
    fields.insert("cut_point".into(), json!(point));
    fields.insert("durable_reply_sent".into(), json!(false));
    Ok(packet)
}

fn publication_cut(control: &Control, report: &Value, point: &str) -> ProbeResult<()> {
    let packet = cut_packet(report, point)?;
    control.send(&packet)?;
    control.acknowledge(b'K')?;
    // Only an externally observed death satisfies this experiment. Crossing the
    // barrier must never manufacture a final result or a durable reply.
    Err(RunError::Refused)
}

fn publish(
    closed: ClosedCreationDraft,
    case: &str,
    initial_opening_permit: bool,
    control: &Control,
    report: &mut Value,
    snapshot: Snapshot,
) -> ProbeResult<()> {
    use crate::package::PackagePublishStage;
    use rust_launcher_probe::linux_admission_storage::PublishStage;

    require(PUBLICATION_CASES.contains(&case))?;
    report["closure_acknowledged"] = json!(true);
    report["resources_closed_after_drain"] = json!(true);
    report["preparation_dropped"] = json!(false);
    report["proofs_published"] = json!(snapshot.proofs_published);
    report["proofs_verified"] = json!(snapshot.proofs_verified);
    report["native_requests_sent"] = json!(snapshot.native_requests_sent);
    report["native_replies_received"] = json!(snapshot.native_replies_received);
    report["workers_joined"] = json!(snapshot.workers_joined);
    report["lock_callers"] = json!(snapshot.lock_callers);
    report["lock_acknowledgments"] = json!(snapshot.lock_acknowledgments);
    report["stage"] = json!("closed-before-publication");
    control.send(report)?;
    control.acknowledge(b'C')?;
    let package_checkpoints = if case == "publish-package-failure"
        || death_point(case).is_some_and(|point| point.starts_with("package-"))
    {
        PackageCheckpoints::Enabled
    } else {
        PackageCheckpoints::Disabled
    };
    let interrupted = |_: RunError| std::io::Error::from(std::io::ErrorKind::Interrupted);
    let result = closed.publish_with(package_checkpoints, initial_opening_permit, |checkpoint| {
        match checkpoint {
            PublicationCheckpoint::Package(stage) => {
                if case == "publish-package-failure" && stage == PackagePublishStage::Renamed {
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
                }
                let point = match stage {
                    PackagePublishStage::BeforeRename => "package-before-rename",
                    PackagePublishStage::Renamed => "package-renamed",
                    PackagePublishStage::ParentSynced => "package-parent-synced",
                };
                if death_point(case) == Some(point) {
                    publication_cut(control, report, point).map_err(interrupted)?;
                }
            }
            PublicationCheckpoint::PackageDurable => {
                report["stage"] = json!("package-durable");
                control.send(report).map_err(interrupted)?;
                control.acknowledge(b'D').map_err(interrupted)?;
            }
            PublicationCheckpoint::OpeningPermitDurable => {
                report["stage"] = json!("opening-permit-durable");
                control.send(report).map_err(interrupted)?;
                control.acknowledge(b'O').map_err(interrupted)?;
            }
            PublicationCheckpoint::Record(stage) => {
                let point = match stage {
                    PublishStage::FileSynced => "record-file-synced",
                    PublishStage::StagingSynced => "record-staging-synced",
                    PublishStage::BeforeRename => "record-before-rename",
                    PublishStage::Renamed => "record-renamed",
                    PublishStage::BeforeParentSync => "record-before-parent-sync",
                    PublishStage::ParentSynced => "record-parent-synced",
                };
                if death_point(case) == Some(point) {
                    publication_cut(control, report, point).map_err(interrupted)?;
                }
                if (case == "publish-record-before-rename-failure"
                    && stage == PublishStage::BeforeRename)
                    || (case == "publish-record-after-rename-failure"
                        && stage == PublishStage::Renamed)
                {
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
                }
            }
        }
        Ok(())
    });
    let retained = matches!(
        case,
        "publish-invalidate-retained-record" | "publish-invalidate-retained-package"
    );
    if result.outcome() == super::creation_producer::PublicationOutcome::Durable {
        let record = result.record()?;
        require(
            record.canonical_bytes().map_err(refused)?
                == report["record_canonical"].as_str().ok_or(())?.as_bytes(),
        )?;
    }
    if retained {
        require(result.outcome() == super::creation_producer::PublicationOutcome::Durable)?;
        report["stage"] = json!("publication-retained");
        control.send(report)?;
        control.acknowledge(b'R')?;
        require(result.record().is_err())?;
    }
    let outcome = result.outcome().as_str();
    if let Some(point) = death_point(case) {
        if point == "before-durable-reply" {
            require(outcome == "durable")?;
            return publication_cut(control, report, point);
        }
        return Err(RunError::Refused);
    }
    let expected = match case {
        "publish-complete" => "durable",
        "publish-package-failure" => "package-durability-unknown",
        "publish-record-before-rename-failure" | "publish-invalidate-package" => "package-only",
        "publish-record-after-rename-failure"
        | "publish-invalidate-retained-record"
        | "publish-invalidate-retained-package" => "record-durability-unknown",
        "publish-invalidate-before-package" | "publish-contended-credential" => "unpublished",
        _ => return Err(RunError::Refused),
    };
    require(outcome == expected)?;
    report["publication_outcome"] = json!(outcome);
    report["package_published"] = json!(outcome != "unpublished");
    report["record_published"] = json!(matches!(outcome, "durable" | "record-durability-unknown"));
    report["experimental_durable_publication_ack"] = json!(outcome == "durable");
    drop(result);
    Ok(())
}

pub(super) fn run(arguments: impl Iterator<Item = OsString>) -> ProbeResult<()> {
    let arguments = parse_arguments(arguments)?;
    let control = Control::new(take_inherited(arguments.observer)?)?;
    let mut report = Value::Null;
    let mut roles = None;
    let (creation, session) = AttendedCreation::start(
        &arguments.runtime,
        &arguments.device,
        &arguments.credential,
        TIMEOUT,
        |event| {
            let observation = (|| -> ProbeResult<()> {
                match event {
                    StartupEvent::LoaderReady {
                        master,
                        record,
                        agent,
                        loader,
                    } => {
                        let record_canonical =
                            String::from_utf8(record.canonical_bytes().map_err(refused)?)
                                .map_err(refused)?;
                        control.send_master(master)?;
                        report = json!({"version": 1, "case": arguments.case, "stage": "loader-ready",
                            "helper_pid": unsafe { libc::getpid() }, "agent_pid": agent.pid, "loader_pid": loader.pid,
                            "creator_tid": agent.creator_tid, "record_canonical": record_canonical,
                            "ready_published": false, "producer_origin_checked": false, "creation_admission": false,
                            "custody_accredited": false, "native_binding_verified": false});
                        roles = Some((agent, loader));
                        control.send(&report)?;
                        control.acknowledge(b'H')?;
                    }
                    StartupEvent::LoaderHandoff => {
                        report["stage"] = json!("loader-handoff");
                        control.send(&report)?;
                        control.acknowledge(b'L')?;
                    }
                }
                Ok(())
            })();
            observation.map_err(|_| crate::package::CreateError::Refused)
        },
    )?;
    let (agent, loader_receipt) = roles.ok_or(())?;
    let endpoint_witness = session.endpoint_witness().map_err(refused)?;
    session
        .validate_endpoint_witness(&endpoint_witness)
        .map_err(refused)?;
    require(endpoint_witness.generation().map_err(refused)? != [0; 32])?;
    let witnessed_store = endpoint_witness.store_identity().map_err(refused)?;
    require(endpoint_witness.store_identity().map_err(refused)? == witnessed_store)?;
    require(
        session
            .endpoint_attempt(Instant::now())
            .err()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::TimedOut),
    )?;
    let mut discarded_attempt = session
        .endpoint_attempt(Instant::now() + TIMEOUT)
        .map_err(refused)?;
    require(
        session
            .endpoint_attempt(Instant::now() + TIMEOUT)
            .err()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock),
    )?;
    session
        .consume_endpoint_attempt(&mut discarded_attempt)
        .map_err(refused)?;
    discarded_attempt.check_live().map_err(refused)?;
    require(
        session
            .consume_endpoint_attempt(&mut discarded_attempt)
            .is_err(),
    )?;
    require(discarded_attempt.check_live().is_err())?;
    require(
        session
            .endpoint_attempt(Instant::now() + TIMEOUT)
            .err()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock),
    )?;
    drop(discarded_attempt);
    let mut endpoint_attempt = session
        .endpoint_attempt(Instant::now() + TIMEOUT)
        .map_err(refused)?;
    session
        .consume_endpoint_attempt(&mut endpoint_attempt)
        .map_err(refused)?;
    session
        .validate_endpoint_attempt(&endpoint_attempt)
        .map_err(refused)?;
    if arguments.case == "abandon-after-load" {
        report["stage"] = json!("loaded");
        control.send(&report)?;
        control.acknowledge(b'A')?;
        drop(session);
        require(endpoint_witness.check_live().is_err())?;
        require(endpoint_attempt.check_live().is_err())?;
        drop(creation);
        report["stage"] = json!("abandoned");
        report["producer_context_dropped"] = json!(true);
        report["closure_acknowledged"] = json!(false);
        report["admission_stopped"] = json!(SpawnOwner::admission_uncertain());
        control.send(&report)?;
        control.acknowledge(b'X')?;
        return Err(RunError::Refused);
    }
    let complete =
        arguments.case == "complete" || PUBLICATION_CASES.contains(&arguments.case.as_str());
    let control = Arc::new(control);
    let report = Arc::new(Mutex::new(report));
    let proof_control = control.clone();
    let proof_report = report.clone();
    let closed_report = report.clone();
    let proof_result = creation.prove_and_close(
        session,
        TIMEOUT,
        move |event| {
            let observation = (|| -> ProbeResult<()> {
                let mut report = proof_report.try_lock().map_err(refused)?;
                match event {
                    ProofEvent::BeforePublication => {
                        if !complete {
                            let mut value = report.clone();
                            value["stage"] = json!("before-proof");
                            proof_control.send(&value)?;
                            proof_control.acknowledge(b'P')?;
                        }
                        return Ok(());
                    }
                    ProofEvent::Verified {
                        challenge,
                        signature,
                    } => {
                        require(complete)?;
                        report["challenge"] = json!(hex(challenge));
                        report["signature"] = json!(hex(signature));
                        report["stage"] = json!("bound");
                        report["native_binding_verified"] = json!(true);
                    }
                    ProofEvent::LeaseInvalidated => {
                        require(!complete)?;
                        report["stage"] = json!("binding-refused");
                        report["challenge"] = Value::Null;
                        report["signature"] = Value::Null;
                    }
                }
                proof_control.send(&report)?;
                proof_control.acknowledge(b'V')?;
                Ok(())
            })();
            observation.map_err(|_| crate::package::CreateError::Refused)
        },
        move |session, _| {
            let observation = (|| -> ProbeResult<()> {
                require(
                    session
                        .validate_endpoint_attempt(&endpoint_attempt)
                        .is_err(),
                )?;
                require(endpoint_attempt.check_live().is_err())?;
                drop(endpoint_attempt);
                require(
                    session
                        .validate_endpoint_witness(&endpoint_witness)
                        .is_err(),
                )?;
                require(
                    endpoint_witness.check_live().is_err()
                        && endpoint_witness.generation().is_err()
                        && endpoint_witness.store_identity().is_err(),
                )?;
                drop(endpoint_witness);
                let exits = session.exits().ok_or(())?;
                require(exits.len() == 2)?;
                let agent_exit = exits
                    .iter()
                    .find(|exit| exit.token == agent.token && exit.pid == agent.pid)
                    .ok_or(())?;
                let loader_exit = exits
                    .iter()
                    .find(|exit| {
                        exit.token == loader_receipt.token && exit.pid == loader_receipt.pid
                    })
                    .ok_or(())?;
                require(
                    agent_exit.code.is_none()
                        && agent_exit.signal == Some(libc::SIGKILL)
                        && loader_exit.code == Some(0)
                        && loader_exit.signal.is_none(),
                )?;
                let mut report = closed_report.try_lock().map_err(refused)?;
                report["agent_exit_code"] = json!(agent_exit.code);
                report["agent_exit_signal"] = json!(agent_exit.signal);
                report["loader_exit_code"] = json!(loader_exit.code);
                report["loader_exit_signal"] = json!(loader_exit.signal);
                Ok(())
            })();
            observation.map_err(|_| crate::package::CreateError::Refused)
        },
    )?;
    let mut report = Arc::try_unwrap(report)
        .map_err(refused)?
        .into_inner()
        .map_err(refused)?;
    let ProofResult { draft, snapshot } = proof_result;
    match draft {
        Ok(closed) => {
            require(complete)?;
            if PUBLICATION_CASES.contains(&arguments.case.as_str()) {
                publish(
                    closed,
                    &arguments.case,
                    arguments.initial_opening_permit,
                    &control,
                    &mut report,
                    snapshot,
                )?;
            } else {
                drop(closed);
            }
        }
        Err(_) => {
            require(!complete)?;
        }
    }
    report["stage"] = json!("final");
    report["closure_acknowledged"] = json!(true);
    report["resources_closed_after_drain"] = json!(true);
    report["preparation_dropped"] = json!(true);
    report["proofs_published"] = json!(snapshot.proofs_published);
    report["proofs_verified"] = json!(snapshot.proofs_verified);
    report["native_requests_sent"] = json!(snapshot.native_requests_sent);
    report["native_replies_received"] = json!(snapshot.native_replies_received);
    report["workers_joined"] = json!(snapshot.workers_joined);
    report["lock_callers"] = json!(snapshot.lock_callers);
    report["lock_acknowledgments"] = json!(snapshot.lock_acknowledgments);
    let packet = control.send(&report)?;
    let mut output = std::io::stdout().lock();
    output.write_all(&packet).map_err(refused)?;
    output.write_all(b"\n").map_err(refused)?;
    output.flush().map_err(refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn death_cut_points_accept_only_explicit_publication_cases() {
        for (case, point) in [
            ("death-package-before-rename", "package-before-rename"),
            ("death-package-renamed", "package-renamed"),
            ("death-package-parent-synced", "package-parent-synced"),
            ("death-record-file-synced", "record-file-synced"),
            ("death-record-staging-synced", "record-staging-synced"),
            ("death-record-before-rename", "record-before-rename"),
            ("death-record-renamed", "record-renamed"),
            (
                "death-record-before-parent-sync",
                "record-before-parent-sync",
            ),
            ("death-record-parent-synced", "record-parent-synced"),
            ("death-before-durable-reply", "before-durable-reply"),
        ] {
            assert_eq!(death_point(case), Some(point));
        }
        for case in [
            "publish-complete",
            "death-",
            "death-record-before-rename-extra",
            "death-record-unknown",
            "death-package-before-rename/path",
        ] {
            assert_eq!(death_point(case), None);
        }
    }

    #[test]
    fn publication_cut_preserves_evidence_without_a_reply_or_destruction_claim() {
        let report = json!({
            "case": "death-before-durable-reply", "stage": "package-durable",
            "record_canonical": "public record", "challenge": "public challenge",
            "signature": "public signature", "native_binding_verified": true,
            "closure_acknowledged": true, "resources_closed_after_drain": true,
            "preparation_dropped": false, "agent_exit_signal": 9, "loader_exit_code": 0,
        });
        let packet = cut_packet(&report, "before-durable-reply").unwrap();
        assert_eq!(packet["stage"], "publication-cut");
        assert_eq!(packet["cut_point"], "before-durable-reply");
        assert_eq!(packet["durable_reply_sent"], false);
        assert!(packet.get("preparation_dropped").is_none());
        for field in [
            "record_canonical",
            "challenge",
            "signature",
            "native_binding_verified",
            "closure_acknowledged",
            "resources_closed_after_drain",
            "agent_exit_signal",
            "loader_exit_code",
        ] {
            assert_eq!(packet[field], report[field]);
        }
        assert!(cut_packet(&report, "record-renamed").is_err());
    }

    #[test]
    fn publication_cut_refuses_to_erase_any_existing_publication_result() {
        let report = json!({"case": "death-record-renamed", "preparation_dropped": false});
        for field in [
            "publication_outcome",
            "package_published",
            "record_published",
            "experimental_durable_publication_ack",
        ] {
            let mut changed = report.clone();
            changed[field] = json!(false);
            assert!(cut_packet(&changed, "record-renamed").is_err(), "{field}");
        }
        assert!(cut_packet(&json!({"case": "publish-complete"}), "record-renamed").is_err());
    }

    fn arguments() -> Vec<OsString> {
        [
            "complete",
            "00000000000000000000000001",
            "00000000000000000000000002",
            "--observer-fd",
            "5",
            "--runtime-dir",
            "/private/runtime",
        ]
        .into_iter()
        .map(Into::into)
        .collect()
    }

    #[test]
    fn retained_publication_cases_require_explicit_identifiers_and_observer() {
        for case in [
            "publish-invalidate-retained-record",
            "publish-invalidate-retained-package",
        ] {
            let mut values = arguments();
            values[0] = case.into();
            let parsed = parse_arguments(values.clone().into_iter()).unwrap();
            assert_eq!(parsed.case, case);
            assert_eq!(parsed.observer, 5);
            for (index, value) in [
                (1, "invalid"),
                (2, "invalid"),
                (3, "--agent-fd"),
                (4, "03"),
                (5, "--record"),
                (6, "relative"),
            ] {
                let mut changed = values.clone();
                changed[index] = value.into();
                assert!(
                    parse_arguments(changed.into_iter()).is_err(),
                    "{case}: {index}"
                );
            }
            let mut duplicate = values.clone();
            duplicate.extend(["--observer-fd", "6"].map(Into::into));
            assert!(parse_arguments(duplicate.into_iter()).is_err());
            values.remove(3);
            assert!(parse_arguments(values.into_iter()).is_err());
        }
    }
    #[test]
    fn accepts_only_complete_fixed_root_capability_arguments() {
        let parsed = parse_arguments(arguments().into_iter()).unwrap();
        assert_eq!(parsed.observer, 5);
        for case in ["complete", "invalidate-before-proof", "abandon-after-load"]
            .into_iter()
            .chain(PUBLICATION_CASES.iter().copied())
        {
            let mut values = arguments();
            values[0] = case.into();
            assert!(parse_arguments(values.into_iter()).is_ok());
        }
    }

    #[test]
    fn refuses_caller_executable_descriptors_even_when_valid_and_distinct() {
        let mut injected = arguments();
        injected.splice(
            3..3,
            ["--agent-fd", "3", "--loader-fd", "4"].map(Into::into),
        );
        assert!(parse_arguments(injected.into_iter()).is_err());
        for flag in ["--agent-fd", "--loader-fd"] {
            let mut injected = arguments();
            injected.extend([flag, "6"].map(Into::into));
            assert!(parse_arguments(injected.into_iter()).is_err());
        }
    }

    #[test]
    fn refuses_noncanonical_observer_numbers_and_custody_selectors() {
        for (index, value) in [
            (0, "normal"),
            (1, "invalid"),
            (2, "invalid"),
            (4, "0"),
            (4, "1"),
            (4, "2"),
            (4, "03"),
            (4, "+3"),
            (4, "-3"),
            (4, "2147483648"),
            (3, "--agent-fd"),
            (3, "--loader-fd"),
            (3, "--package"),
            (5, "--record"),
            (5, "--key"),
            (5, "--digest"),
            (6, "relative"),
        ] {
            let mut values = arguments();
            values[index] = value.into();
            assert!(
                parse_arguments(values.into_iter()).is_err(),
                "{index}: {value}"
            );
        }
        let mut extra = arguments();
        extra.push("--package".into());
        assert!(parse_arguments(extra.into_iter()).is_err());
        let mut missing = arguments();
        missing.pop();
        assert!(parse_arguments(missing.into_iter()).is_err());
    }
}
