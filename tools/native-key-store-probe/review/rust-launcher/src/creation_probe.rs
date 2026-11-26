use crate::linux_creation_binding::LocalCreationChallenge;
use crate::linux_creation_record::Record;
use crate::linux_gate::{Error, Gate, RequestOutcome, State as GateState};
use crate::linux_loader::PtyLoader;
use crate::linux_reservation::StoreReservation;
use crate::linux_session::{CreationIdentityError, NativeSession, NativeSigner, PackageLease};
use crate::linux_spawn::SpawnOwner;
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{FileExt, PermissionsExt};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const CASES: [&str; 3] = ["complete", "wrong-key", "wrong-comment"];
const TIMEOUT: Duration = Duration::from_secs(15);

fn parse(
    arguments: impl IntoIterator<Item = OsString>,
) -> io::Result<(String, baseline::Args, RawFd)> {
    let mut arguments = arguments.into_iter();
    baseline::require(
        arguments.next().as_deref() == Some(std::ffi::OsStr::new("--creation-binding-case")),
    )?;
    let case = arguments
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    baseline::require(CASES.contains(&case.as_str()))?;
    let mut forwarded = vec!["--loader-case".into(), "normal".into()];
    let mut lease = None;
    while let Some(option) = arguments.next() {
        let value = arguments.next().ok_or_else(baseline::refused)?;
        if option == "--lease-dir-fd" {
            baseline::require(lease.is_none())?;
            let fd = value
                .into_string()
                .map_err(|_| baseline::refused())?
                .parse::<RawFd>()
                .map_err(|_| baseline::refused())?;
            baseline::require(fd >= 3)?;
            lease = Some(fd);
        } else {
            baseline::require(option != "--ssh-lifetime")?;
            forwarded.extend([option, value]);
        }
    }
    let args = baseline::parse(forwarded)?;
    baseline::require(!args.ssh_lifetime)?;
    let lease = lease.ok_or_else(baseline::refused)?;
    baseline::require(!args.descriptors.contains(&lease))?;
    Ok((case, args, lease))
}

fn read_record(descriptor: &OwnedFd) -> io::Result<Record> {
    // SAFETY: F_GET_SEALS only queries the retained public fixture descriptor.
    let seals = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GET_SEALS) };
    let required = libc::F_SEAL_SEAL | libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK;
    baseline::require(seals >= 0 && seals & required == required)?;
    let file = File::from(baseline::duplicate(descriptor.as_raw_fd())?);
    let metadata = file.metadata()?;
    baseline::require(metadata.is_file() && metadata.len() > 0 && metadata.len() <= 2048)?;
    let mut bytes = vec![0; metadata.len() as usize];
    file.read_exact_at(&mut bytes, 0)?;
    Record::parse(&bytes)
}

fn gate_io<T>(value: Result<T, Error>) -> io::Result<T> {
    value.map_err(|_| baseline::refused())
}

struct NativeIdentity {
    agent_pid: u32,
    loader_pid: u32,
    creator_tid: i32,
}

fn coordinate(
    case: String,
    signer: Result<NativeSigner, CreationIdentityError>,
    challenge: LocalCreationChallenge,
    record: Record,
    control: baseline::Control,
    gate: Arc<Gate>,
    identity: NativeIdentity,
) -> io::Result<(baseline::Control, Value)> {
    let operation = (|| -> io::Result<Value> {
        let record_sha256 = baseline::hex_encode(&Sha256::digest(record.canonical_bytes()?));
        let (identity_mismatch, native_binding_verified, challenge_bytes, signature_bytes) =
            match (case.as_str(), signer) {
                ("complete", Ok(signer)) => {
                    let encoded_challenge = baseline::hex_encode(challenge.bytes());
                    let id = gate_io(gate.start(move |request| signer.sign(request, |_| Ok(()))))?;
                    baseline::require(
                        gate_io(gate.wait_request(id, TIMEOUT))? == RequestOutcome::Published,
                    )?;
                    let signature = gate_io(gate.take_proof(id))?.ok_or_else(baseline::refused)?;
                    let verified = challenge.verify(&signature)?;
                    baseline::require(verified.matches_record(&record)?)?;
                    (
                        false,
                        true,
                        Some(encoded_challenge),
                        Some(baseline::hex_encode(&signature)),
                    )
                }
                ("wrong-key" | "wrong-comment", Err(CreationIdentityError::IdentityMismatch)) => {
                    (true, false, None, None)
                }
                _ => return Err(baseline::refused()),
            };
        let report = json!({
            "stage":"creation-result","case":case,
            "agent_pid":identity.agent_pid,"loader_pid":identity.loader_pid,
            "creator_tid":identity.creator_tid,"record_sha256":record_sha256,
            "identity_mismatch":identity_mismatch,"native_binding_verified":native_binding_verified,
            "challenge":challenge_bytes,"signature":signature_bytes,
            "ready_published":false,"producer_origin_checked":false,
            "creation_admission":false,"custody_accredited":false
        });
        control.send(&report)?;
        baseline::require(control.command()? == b'V')?;
        Ok(report)
    })();
    // Even a failed observation requests owned native shutdown; only the
    // successful observation and verified closure may produce a final receipt.
    let closed = gate_io(gate.lock(TIMEOUT));
    let mut report = operation?;
    closed?;
    let snapshot = gate_io(gate.snapshot())?;
    let proofs = u64::from(case == "complete");
    baseline::require(
        snapshot.state == GateState::Closed
            && !snapshot.active
            && snapshot.native_requests_sent == proofs
            && snapshot.native_replies_received == proofs
            && snapshot.proofs_verified == proofs
            && snapshot.proofs_published == proofs
            && snapshot.workers_joined == proofs
            && snapshot.lock_callers == 1
            && snapshot.lock_acknowledgments == 1
            && snapshot.close_acknowledged
            && snapshot.resources_closed_after_drain,
    )?;
    report["stage"] = "creation-final".into();
    report["version"] = 1.into();
    report["status"] = "observed".into();
    report["state"] = "CLOSED".into();
    report["active"] = snapshot.active.into();
    report["native_requests_sent"] = snapshot.native_requests_sent.into();
    report["native_replies_received"] = snapshot.native_replies_received.into();
    report["proofs_verified"] = snapshot.proofs_verified.into();
    report["proofs_published"] = snapshot.proofs_published.into();
    report["lock_callers"] = snapshot.lock_callers.into();
    report["lock_acknowledgments"] = snapshot.lock_acknowledgments.into();
    report["workers_joined"] = snapshot.workers_joined.into();
    report["close_acknowledged"] = snapshot.close_acknowledged.into();
    report["resources_closed_after_drain"] = snapshot.resources_closed_after_drain.into();
    report["busy_refused"] = snapshot.busy_refused.into();
    report["closed_refused"] = snapshot.closed_refused.into();
    Ok((control, report))
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    let (case, args, lease_fd) = parse(std::env::args_os().skip(1))?;
    // SAFETY: this deadline applies only to the disposable native fixture.
    unsafe { libc::alarm(90) };
    baseline::require(!args.socket.exists())?;
    let parent = args.socket.parent().ok_or_else(baseline::refused)?;
    let home = parent.join("home");
    for path in [parent, home.as_path()] {
        let metadata = path.metadata()?;
        baseline::require(metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0)?;
    }
    let [agent, loader, cipher, secret, fixture, observer] = args.descriptors;
    let mut inputs = baseline::Inputs {
        agent: baseline::take_inherited(agent)?,
        loader: baseline::take_inherited(loader)?,
        cipher: baseline::take_inherited(cipher)?,
        secret: Some(baseline::take_inherited(secret)?),
        fixture: baseline::take_inherited(fixture)?,
    };
    let control = baseline::Control::new(baseline::take_inherited(observer)?, false)?;
    let record = read_record(&inputs.fixture)?;
    let lease = Arc::new(PackageLease::capture(
        baseline::take_inherited(lease_fd)?,
        &inputs.cipher,
    )?);
    let reservation = StoreReservation::acquire(&lease)?;
    let pressure = Pressure::start()?;
    let mut owner = SpawnOwner::new_reserved(reservation)?;
    let agent = baseline::launch_agent(&mut owner, &inputs.agent, &args.socket, &home)?;
    baseline::empty_agent(&args.socket, &mut owner, agent.token)?;
    baseline::native_credentials(agent.pid)?;
    let mut loader = PtyLoader::start(
        &mut owner,
        baseline::duplicate(inputs.loader.as_raw_fd())?,
        baseline::duplicate(inputs.cipher.as_raw_fd())?,
        &args.socket,
        &home,
    )?;
    let master = loader.duplicate_master()?;
    control.send_master(&master)?;
    loader.wait_prompt(&mut owner, Duration::from_secs(10))?;
    let mut state = baseline::State {
        agent,
        loader: loader.receipt(),
        loader_exit: None,
        agent_exit: None,
        secret_writes: 0,
        ready_published: false,
        stopped: false,
        identity_bound: false,
        proof_verified: false,
        loader_failed: false,
    };
    let mut prompt = state.stage("prompt", false);
    for field in [
        "producer_origin_checked",
        "creation_admission",
        "custody_accredited",
    ] {
        prompt[field] = false.into();
    }
    control.send(&prompt)?;
    baseline::require(control.command()? == b'H')?;
    gate_io(lease.validate())?;
    {
        let secret = inputs.read_secret()?;
        loader.handoff_stopped(&mut owner, &secret, Duration::from_secs(10))?;
    }
    baseline::require(loader.handed_off())?;
    state.secret_writes = 1;
    state.stopped = true;
    let mut handoff = state.stage("handoff", false);
    for field in [
        "producer_origin_checked",
        "creation_admission",
        "custody_accredited",
    ] {
        handoff[field] = false.into();
    }
    control.send(&handoff)?;
    baseline::require(control.command()? == b'L')?;
    gate_io(lease.validate())?;
    let exit = loader.complete(&mut owner, Duration::from_secs(10))?;
    baseline::require(exit.code == Some(0) && exit.signal.is_none())?;
    let loader_receipt = state.loader;
    let baseline::Inputs {
        agent: agent_fd,
        loader: loader_fd,
        cipher,
        secret,
        fixture: fixture_fd,
    } = inputs;
    baseline::require(secret.is_none())?;
    let retained = vec![master, agent_fd, loader_fd, cipher, fixture_fd];
    let mut session = NativeSession::new(owner, agent.token, loader, lease, retained, args.socket)?;
    let challenge = LocalCreationChallenge::issue(&record)?;
    let gate = session.gate();
    let signer = session.creation_identity(&challenge).and_then(|identity| {
        session
            .signer(identity)
            .map_err(|_| CreationIdentityError::Unavailable)
    });
    let identity = NativeIdentity {
        agent_pid: agent.pid,
        loader_pid: loader_receipt.pid,
        creator_tid: agent.creator_tid,
    };
    let coordinator =
        thread::spawn(move || coordinate(case, signer, challenge, record, control, gate, identity));
    let deadline = Instant::now() + Duration::from_secs(60);
    while !coordinator.is_finished() || !session.released() {
        baseline::require(Instant::now() < deadline)?;
        gate_io(session.service(Duration::from_millis(50)))?;
    }
    let (control, mut report) = coordinator.join().map_err(|_| baseline::refused())??;
    let exits = session.exits().ok_or_else(baseline::refused)?;
    baseline::require(exits.len() == 2)?;
    let observed_agent = exits
        .iter()
        .find(|exit| exit.token == agent.token)
        .ok_or_else(baseline::refused)?;
    let observed_loader = exits
        .iter()
        .find(|exit| exit.token == loader_receipt.token)
        .ok_or_else(baseline::refused)?;
    baseline::require(
        observed_agent.pid == agent.pid
            && observed_agent.code.is_none()
            && observed_agent.signal == Some(libc::SIGKILL)
            && observed_loader.pid == loader_receipt.pid
            && observed_loader.code == Some(0)
            && observed_loader.signal.is_none(),
    )?;
    report["agent_exit_code"] = observed_agent.code.into();
    report["agent_exit_signal"] = observed_agent.signal.into();
    report["loader_exit_code"] = observed_loader.code.into();
    report["loader_exit_signal"] = observed_loader.signal.into();
    report["pressure_iterations"] = pressure.finish()?.into();
    control.send(&report)?;
    println!("{report}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(case: &str) -> Vec<OsString> {
        [
            "--creation-binding-case",
            case,
            "--agent-fd",
            "3",
            "--loader-fd",
            "4",
            "--cipher-fd",
            "5",
            "--secret-fd",
            "6",
            "--fixture-fd",
            "7",
            "--observer-fd",
            "8",
            "--socket",
            "/private/creation.sock",
            "--lease-dir-fd",
            "9",
        ]
        .map(OsString::from)
        .to_vec()
    }

    #[test]
    fn creation_cases_require_a_separate_private_lease_descriptor() {
        for case in CASES {
            let (actual, args, lease) = parse(arguments(case)).unwrap();
            assert_eq!(actual, case);
            assert_eq!(lease, 9);
            assert!(!args.ssh_lifetime);
        }
        for value in ["3", "2", "invalid"] {
            let mut args = arguments("complete");
            args[17] = value.into();
            assert!(parse(args).is_err());
        }
        let mut duplicate = arguments("complete");
        duplicate.extend(["--lease-dir-fd".into(), "10".into()]);
        assert!(parse(duplicate).is_err());
    }

    #[test]
    fn creation_cli_refuses_unknown_cases_challenges_and_ssh_lifetimes() {
        assert!(parse(arguments("unknown")).is_err());
        for option in ["--challenge", "--ssh-lifetime"] {
            let mut args = arguments("complete");
            args.extend([option.into(), "stdin".into()]);
            assert!(parse(args).is_err());
        }
    }
}
