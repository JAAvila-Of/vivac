use crate::linux_gate::{Error, Gate, RequestOutcome, State as GateState};
use crate::linux_loader::PtyLoader;
use crate::linux_reservation::StoreReservation;
use crate::linux_session::{NativeSession, NativeSigner, PackageLease, SigningIdentity};
use crate::linux_spawn::SpawnOwner;
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CASES: [&str; 8] = [
    "completed-before-lock",
    "lock-before-native-send",
    "lock-native-response-pending",
    "lock-after-native-reply",
    "lock-before-publication",
    "parallel-lock-callers",
    "lease-change-before-publication",
    "session-abandon",
];
const TIMEOUT: Duration = Duration::from_secs(15);
const SSH_CASES: [&str; 6] = [
    "normal-close",
    "disconnect-before-native-send",
    "disconnect-native-response-pending",
    "disconnect-after-native-reply",
    "disconnect-before-publication",
    "disconnect-published",
];

fn parse(
    arguments: impl IntoIterator<Item = OsString>,
) -> io::Result<(String, baseline::Args, i32)> {
    let mut arguments = arguments.into_iter();
    let prefix = arguments.next().ok_or_else(baseline::refused)?;
    let ssh = prefix == "--concurrent-ssh-case";
    baseline::require(ssh || prefix == "--concurrent-case")?;
    let case = arguments
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    baseline::require(if ssh {
        SSH_CASES.contains(&case.as_str())
    } else {
        CASES.contains(&case.as_str())
    })?;
    let mut forwarded = vec!["--loader-case".into(), "normal".into()];
    let mut lease = None;
    while let Some(option) = arguments.next() {
        let value = arguments.next().ok_or_else(baseline::refused)?;
        if option == "--lease-dir-fd" {
            baseline::require(lease.is_none())?;
            let fd = value
                .into_string()
                .map_err(|_| baseline::refused())?
                .parse::<i32>()
                .map_err(|_| baseline::refused())?;
            baseline::require(fd >= 3)?;
            lease = Some(fd);
        } else {
            baseline::require(ssh || option != "--ssh-lifetime")?;
            forwarded.extend([option, value]);
        }
    }
    let args = baseline::parse(forwarded)?;
    baseline::require(args.ssh_lifetime == ssh)?;
    let lease = lease.ok_or_else(baseline::refused)?;
    baseline::require(!args.descriptors.contains(&lease))?;
    Ok((case, args, lease))
}

struct Pause {
    reached: Mutex<bool>,
    reached_cv: Condvar,
    released: Mutex<bool>,
    released_cv: Condvar,
}
impl Pause {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            reached: Mutex::new(false),
            reached_cv: Condvar::new(),
            released: Mutex::new(false),
            released_cv: Condvar::new(),
        })
    }
    fn hit(&self) -> Result<(), Error> {
        *self.reached.lock().map_err(|_| Error::Poisoned)? = true;
        self.reached_cv.notify_all();
        let released = self.released.lock().map_err(|_| Error::Poisoned)?;
        let (released, _) = self
            .released_cv
            .wait_timeout_while(released, TIMEOUT, |value| !*value)
            .map_err(|_| Error::Poisoned)?;
        if !*released {
            return Err(Error::Timeout);
        }
        Ok(())
    }
    fn wait(&self) -> io::Result<()> {
        let reached = self.reached.lock().map_err(|_| baseline::refused())?;
        let (reached, _) = self
            .reached_cv
            .wait_timeout_while(reached, TIMEOUT, |value| !*value)
            .map_err(|_| baseline::refused())?;
        baseline::require(*reached)
    }
    fn release(&self) -> io::Result<()> {
        *self.released.lock().map_err(|_| baseline::refused())? = true;
        self.released_cv.notify_all();
        Ok(())
    }
}
fn seam(case: &str) -> &'static str {
    match case {
        "lock-native-response-pending" | "disconnect-native-response-pending" => {
            "native-response-pending"
        }
        "lock-after-native-reply" | "disconnect-after-native-reply" => "after-native-reply",
        "lock-before-publication"
        | "lease-change-before-publication"
        | "disconnect-before-publication" => "before-publication",
        _ => "before-native-send",
    }
}
fn gate_io<T>(value: Result<T, Error>) -> io::Result<T> {
    value.map_err(|_| baseline::refused())
}
fn closed_request(gate: &Arc<Gate>) -> io::Result<()> {
    baseline::require(matches!(gate.start(|_| Ok([0; 64])), Err(Error::Closed)))
}

struct NativeIdentity {
    agent_pid: u32,
    loader_pid: u32,
    creator_tid: i32,
}

type DescriptorIdentity = (RawFd, u64, u64, u32);

fn descriptor_identity(fd: RawFd) -> io::Result<DescriptorIdentity> {
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes this correctly sized storage on success.
    if unsafe { libc::fstat(fd, status.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful fstat above initialized all fields read here.
    let status = unsafe { status.assume_init() };
    Ok((fd, status.st_dev, status.st_ino, status.st_mode))
}

fn retained_descriptors(identities: &[DescriptorIdentity]) -> io::Result<()> {
    for identity in identities {
        baseline::require(descriptor_identity(identity.0)? == *identity)?;
    }
    Ok(())
}

fn wait_pidfd_exit(pidfd: &OwnedFd) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut item = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one live initialized pollfd and never owns it.
        let result = unsafe { libc::poll(&mut item, 1, 20) };
        if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(io::Error::last_os_error());
        }
        if result > 0 {
            return baseline::require(item.revents & libc::POLLIN != 0);
        }
        baseline::require(Instant::now() < deadline)?;
    }
}

fn abandon_session(
    session: NativeSession,
    signer: NativeSigner,
    control: baseline::Control,
    identity: NativeIdentity,
    descriptors: Vec<DescriptorIdentity>,
    agent_pidfd: OwnedFd,
    pressure: Pressure,
) -> io::Result<()> {
    baseline::require(control.command()? == b'S')?;
    let gate = session.gate();
    let pause = Pause::new();
    let worker_pause = Arc::clone(&pause);
    let id = gate_io(gate.start(move |request| {
        signer.sign(request, |stage| {
            if stage.as_str() == "before-native-send" {
                worker_pause.hit()
            } else {
                Ok(())
            }
        })
    }))?;
    pause.wait()?;
    let NativeIdentity {
        agent_pid,
        loader_pid,
        creator_tid,
    } = identity;
    control.send(&json!({"stage":"abandon-paused","case":"session-abandon",
        "agent_pid":agent_pid,"loader_pid":loader_pid,"creator_tid":creator_tid,
        "active":true,"close_ack":false,"descriptors":descriptors}))?;
    baseline::require(control.command()? == b'G')?;
    let started = Instant::now();
    drop(session);
    let elapsed: u64 = started
        .elapsed()
        .as_nanos()
        .try_into()
        .map_err(|_| baseline::refused())?;
    baseline::require(elapsed > 0 && SpawnOwner::admission_uncertain())?;
    for _ in 0..3 {
        match SpawnOwner::new() {
            Err(error) => baseline::require(error.raw_os_error() == Some(libc::EOWNERDEAD))?,
            Ok(_) => return Err(baseline::refused()),
        }
    }
    retained_descriptors(&descriptors)?;
    let snapshot = gate_io(gate.snapshot())?;
    baseline::require(
        snapshot.active && !snapshot.close_acknowledged && snapshot.proofs_published == 0,
    )?;
    control.send(&json!({"stage":"abandon-returned","case":"session-abandon",
        "drop_elapsed_ns":elapsed,"active":true,"close_ack":false,
        "admission_uncertain":true,"readmission_refused":3,"descriptors":descriptors}))?;
    baseline::require(control.command()? == b'G')?;
    // This fixture waits on an independently duplicated process descriptor, not
    // on the abandoned owner's state. It makes the before-send failure causal.
    wait_pidfd_exit(&agent_pidfd)?;
    pause.release()?;
    baseline::require(gate_io(gate.wait_request(id, TIMEOUT))? == RequestOutcome::Failed)?;
    baseline::require(gate.lock(Duration::from_secs(1)) == Err(Error::Uncertain))?;
    retained_descriptors(&descriptors)?;
    let snapshot = gate_io(gate.snapshot())?;
    baseline::require(
        snapshot.state == GateState::Closing
            && !snapshot.active
            && snapshot.workers_joined == 1
            && snapshot.native_requests_sent == 0
            && snapshot.native_replies_received == 0
            && snapshot.proofs_verified == 0
            && snapshot.proofs_published == 0
            && snapshot.lock_callers == 1
            && snapshot.lock_acknowledgments == 0
            && !snapshot.close_acknowledged
            && !snapshot.resources_closed_after_drain,
    )?;
    let report = json!({"case":"session-abandon","status":"observed","version":1,
        "selected_seam":"before-native-send","request_outcome":"failed_after_abandon",
        "state":"CLOSING","active":false,"native_requests_sent":0,"native_replies_received":0,
        "proofs_verified":0,"proofs_published":0,"workers_joined":1,"lock_callers":1,
        "lock_acknowledgments":0,"close_acknowledged":false,"resources_closed_after_drain":false,
        "admission_uncertain":true,"readmission_refused":3,"descriptors_retained":true,
        "retained_descriptor_count":descriptors.len(),"drop_elapsed_ns":elapsed,
        "creator_tid":creator_tid,"agent_pid":agent_pid,"loader_pid":loader_pid,
        "loader_exit_code":0,"loader_exit_signal":null,"pressure_iterations":pressure.finish()?,
        "custody_accredited":false,"full_session_accredited":false,"concurrent_lock_tested":false,
        "callback_uncertain":true});
    control.send(&report)?;
    baseline::require(control.command()? == b'X')?;
    println!("{report}");
    Ok(())
}

fn coordinate(
    case: String,
    signer: NativeSigner,
    control: baseline::Control,
    gate: Arc<Gate>,
    identity: NativeIdentity,
) -> io::Result<(baseline::Control, Value)> {
    let NativeIdentity {
        agent_pid,
        loader_pid,
        creator_tid,
    } = identity;
    baseline::require(control.command()? == b'S')?;
    let pause = Pause::new();
    let selected = seam(&case);
    let worker_pause = Arc::clone(&pause);
    let epoch = gate_io(gate.snapshot())?.epoch;
    let id = gate_io(gate.start(move |request| {
        signer.sign(request, |stage| {
            if stage.as_str() == selected {
                worker_pause.hit()
            } else {
                Ok(())
            }
        })
    }))?;
    pause.wait()?;
    control.send(&json!({"stage":"race-paused", "case":case, "seam":selected, "agent_pid":agent_pid, "loader_pid":loader_pid, "creator_tid":creator_tid, "active":true}))?;
    baseline::require(control.command()? == b'G')?;
    let completed = case == "completed-before-lock";
    let invalidated = case == "lease-change-before-publication";
    let cancellation = !completed && !invalidated;
    let callers = if case == "parallel-lock-callers" {
        2
    } else {
        1
    };
    let mut locks = Vec::new();
    let outcome;
    if completed || invalidated {
        if completed {
            baseline::require(matches!(gate.start(|_| Ok([0; 64])), Err(Error::Busy)))?;
        }
        pause.release()?;
        outcome = gate_io(gate.wait_request(id, TIMEOUT))?;
        if completed {
            baseline::require(outcome == RequestOutcome::Published)?;
            let signature = gate_io(rust_launcher_probe::probe_take_ready_proof(
                &gate, id, epoch,
            ))?
            .ok_or_else(baseline::refused)?;
            control.send(
                &json!({"stage":"published-proof", "signature":baseline::hex_encode(&signature)}),
            )?;
            baseline::require(control.command()? == b'V')?;
        } else {
            baseline::require(
                outcome == RequestOutcome::LeaseInvalidated
                    && gate_io(gate.snapshot())?.state == GateState::Closing,
            )?;
        }
        gate_io(gate.lock(TIMEOUT))?;
    } else {
        for _ in 0..callers {
            let gate = Arc::clone(&gate);
            locks.push(thread::spawn(move || gate.lock(TIMEOUT)));
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let snapshot = gate_io(gate.snapshot())?;
            if snapshot.state == GateState::Closing
                && snapshot.drain_waiting
                && snapshot.lock_callers == callers
            {
                baseline::require(
                    snapshot.active
                        && !snapshot.close_acknowledged
                        && snapshot.lock_acknowledgments == 0,
                )?;
                break;
            }
            baseline::require(Instant::now() < deadline)?;
            thread::sleep(Duration::from_millis(5));
        }
        closed_request(&gate)?;
        control.send(&json!({"stage":"closing-paused", "case":case, "seam":selected, "active":true, "close_ack":false, "lock_callers":callers}))?;
        baseline::require(control.command()? == b'G')?;
        pause.release()?;
        outcome = gate_io(gate.wait_request(id, TIMEOUT))?;
        baseline::require(outcome == RequestOutcome::Cancelled)?;
        for handle in locks {
            gate_io(handle.join().map_err(|_| baseline::refused())?)?;
        }
    }
    baseline::require(matches!(
        rust_launcher_probe::probe_take_ready_proof(&gate, id, epoch),
        Err(Error::Cancelled)
    ))?;
    closed_request(&gate)?;
    let snapshot = gate_io(gate.snapshot())?;
    baseline::require(
        snapshot.state == GateState::Closed
            && !snapshot.active
            && snapshot.close_acknowledged
            && snapshot.resources_closed_after_drain
            && snapshot.workers_joined == 1
            && snapshot.lock_acknowledgments == callers,
    )?;
    let outcome = match outcome {
        RequestOutcome::Published => "published",
        RequestOutcome::Cancelled => "cancelled_by_lock",
        RequestOutcome::LeaseInvalidated => "lease_invalidated",
        _ => return Err(baseline::refused()),
    };
    let report = json!({"case":case,"status":"observed","selected_seam":selected,"request_outcome":outcome,
        "state":"CLOSED","active":false,"native_requests_sent":snapshot.native_requests_sent,
        "native_replies_received":snapshot.native_replies_received,"proofs_verified":snapshot.proofs_verified,
        "proofs_published":snapshot.proofs_published,"lock_callers":snapshot.lock_callers,
        "lock_acknowledgments":snapshot.lock_acknowledgments,"workers_joined":snapshot.workers_joined,
        "close_acknowledged":true,"resources_closed_after_drain":true,"closing_observed_while_paused":cancellation,
        "request_late_refused":cancellation,"drain_wait_observed":cancellation,"busy_refused":snapshot.busy_refused,
        "closed_refused":snapshot.closed_refused,"creator_tid":creator_tid,"agent_pid":agent_pid,"loader_pid":loader_pid,
        "version":1,"custody_accredited":false,"full_session_accredited":false,"concurrent_lock_tested":true});
    Ok((control, report))
}

fn published_paused(case: &str) -> Value {
    json!({"stage":"published-paused","case":case,"seam":"published","active":false,
        "workers_joined":1,"proofs_published":1})
}

fn coordinate_ssh(
    case: String,
    signer: NativeSigner,
    control: baseline::Control,
    gate: Arc<Gate>,
    identity: NativeIdentity,
) -> io::Result<(baseline::Control, Value)> {
    let NativeIdentity {
        agent_pid,
        loader_pid,
        creator_tid,
    } = identity;
    baseline::require(control.command()? == b'S')?;
    let pause = Pause::new();
    let selected = seam(&case);
    let worker_pause = Arc::clone(&pause);
    let id = gate_io(gate.start(move |request| {
        signer.sign(request, |stage| {
            if stage.as_str() == selected {
                worker_pause.hit()
            } else {
                Ok(())
            }
        })
    }))?;
    pause.wait()?;
    control.send(
        &json!({"stage":"race-paused","case":case,"seam":selected,"agent_pid":agent_pid,
        "loader_pid":loader_pid,"creator_tid":creator_tid,"active":true}),
    )?;
    let published = matches!(case.as_str(), "normal-close" | "disconnect-published");
    let final_seam;
    let outcome;
    if published {
        baseline::require(control.command()? == b'G')?;
        baseline::require(matches!(gate.start(|_| Ok([0; 64])), Err(Error::Busy)))?;
        pause.release()?;
        baseline::require(gate_io(gate.wait_request(id, TIMEOUT))? == RequestOutcome::Published)?;
        let signature = gate_io(gate.take_proof(id))?.ok_or_else(baseline::refused)?;
        control.send(
            &json!({"stage":"published-proof","signature":baseline::hex_encode(&signature)}),
        )?;
        baseline::require(control.command()? == b'V')?;
        let snapshot = gate_io(gate.snapshot())?;
        baseline::require(
            !snapshot.active && snapshot.workers_joined == 1 && snapshot.proofs_published == 1,
        )?;
        control.send(&published_paused(&case))?;
        if case == "normal-close" {
            baseline::require(control.command()? == b'C' && control.disconnect_cause().is_none())?;
        } else {
            baseline::require(control.command()? == b'D' && control.disconnect_cause().is_some())?;
        }
        gate_io(gate.lock(TIMEOUT))?;
        final_seam = "published";
        outcome = "published";
    } else {
        baseline::require(control.command()? == b'D')?;
        let cause = control.disconnect_cause().ok_or_else(baseline::refused)?;
        let lock_gate = Arc::clone(&gate);
        let locker = thread::spawn(move || lock_gate.lock(TIMEOUT));
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let snapshot = gate_io(gate.snapshot())?;
            if snapshot.state == GateState::Closing
                && snapshot.drain_waiting
                && snapshot.lock_callers == 1
            {
                baseline::require(
                    snapshot.active
                        && !snapshot.close_acknowledged
                        && snapshot.lock_acknowledgments == 0,
                )?;
                break;
            }
            baseline::require(Instant::now() < deadline)?;
            thread::sleep(Duration::from_millis(5));
        }
        closed_request(&gate)?;
        control.send(&json!({"stage":"closing-paused","case":case,"seam":selected,"active":true,
            "close_ack":false,"lock_callers":1,"ssh_disconnect_observed":true,"ssh_disconnect_cause":cause.as_str()}))?;
        baseline::require(control.observer_command()? == b'G')?;
        pause.release()?;
        baseline::require(gate_io(gate.wait_request(id, TIMEOUT))? == RequestOutcome::Cancelled)?;
        gate_io(locker.join().map_err(|_| baseline::refused())?)?;
        final_seam = selected;
        outcome = "cancelled_by_ssh";
    }
    closed_request(&gate)?;
    let snapshot = gate_io(gate.snapshot())?;
    baseline::require(
        snapshot.state == GateState::Closed
            && !snapshot.active
            && snapshot.close_acknowledged
            && snapshot.resources_closed_after_drain
            && snapshot.workers_joined == 1
            && snapshot.lock_callers == 1
            && snapshot.lock_acknowledgments == 1,
    )?;
    let cause = control.disconnect_cause();
    baseline::require(cause.is_some() == (case != "normal-close"))?;
    let cause = cause.map_or(Value::Null, |value| value.as_str().into());
    let report = json!({"case":case,"status":"observed","selected_seam":final_seam,"request_outcome":outcome,
        "state":"CLOSED","active":false,"native_requests_sent":snapshot.native_requests_sent,
        "native_replies_received":snapshot.native_replies_received,"proofs_verified":snapshot.proofs_verified,
        "proofs_published":snapshot.proofs_published,"lock_callers":snapshot.lock_callers,
        "lock_acknowledgments":snapshot.lock_acknowledgments,"workers_joined":snapshot.workers_joined,
        "close_acknowledged":true,"resources_closed_after_drain":true,"closing_observed_while_paused":!published,
        "request_late_refused":!published,"drain_wait_observed":!published,"busy_refused":snapshot.busy_refused,
        "closed_refused":snapshot.closed_refused,"creator_tid":creator_tid,"agent_pid":agent_pid,"loader_pid":loader_pid,
        "version":1,"custody_accredited":false,"full_session_accredited":false,"concurrent_lock_tested":true,
        "ssh_disconnect_observed":case != "normal-close","ssh_disconnect_cause":cause});
    Ok((control, report))
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    let (case, args, lease_fd) = parse(std::env::args_os().skip(1))?;
    let ssh = args.ssh_lifetime;
    // SAFETY: the deadline applies only to this explicitly disposable harness.
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
    let control = baseline::Control::new(baseline::take_inherited(observer)?, ssh)?;
    let lease = Arc::new(PackageLease::capture(
        baseline::take_inherited(lease_fd)?,
        &inputs.cipher,
    )?);
    let reservation = StoreReservation::acquire(&lease)?;
    let fixture = baseline::Fixture::read(&inputs.fixture)?;
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
    baseline::require(loader.prompt_seen() && agent.creator_tid == loader.receipt().creator_tid)?;
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
    control.send(&state.stage("prompt", false))?;
    baseline::require(control.command()? == b'H')?;
    gate_io(lease.validate())?;
    {
        let secret = inputs.read_secret()?;
        loader.handoff_stopped(&mut owner, &secret, Duration::from_secs(10))?;
    }
    baseline::require(loader.handed_off())?;
    state.secret_writes = 1;
    state.stopped = true;
    control.send(&state.stage("handoff", false))?;
    baseline::require(control.command()? == b'L')?;
    gate_io(lease.validate())?;
    let exit = loader.complete(&mut owner, Duration::from_secs(10))?;
    baseline::require(exit.code == Some(0) && exit.signal.is_none())?;
    state.loader_exit = Some(exit);
    state.stopped = false;
    baseline::native_credentials(agent.pid)?;
    let signature = baseline::sign_and_verify(&args.socket, &fixture)?;
    state.identity_bound = true;
    state.proof_verified = true;
    let mut stage = state.stage("before-ready", false);
    stage["signature"] = baseline::hex_encode(&signature).into();
    control.send(&stage)?;
    baseline::require(control.command()? == b'R')?;
    gate_io(lease.validate())?;
    state.ready_published = true;
    control.send(&state.stage("ready", false))?;
    control.send(&json!({"stage":"concurrent-ready","agent_pid":agent.pid,"loader_pid":state.loader.pid,"creator_tid":agent.creator_tid}))?;
    let identity = SigningIdentity::new(
        fixture.blob,
        fixture.public_key,
        fixture.challenge,
        fixture.comment,
    )?;
    let baseline::Inputs {
        agent: agent_fd,
        loader: loader_fd,
        cipher,
        secret,
        fixture: fixture_fd,
    } = inputs;
    baseline::require(secret.is_none())?;
    let retained = vec![master, agent_fd, loader_fd, cipher, fixture_fd];
    let abandonment = case == "session-abandon";
    let descriptor_identities = if abandonment {
        retained
            .iter()
            .map(|fd| descriptor_identity(fd.as_raw_fd()))
            .collect::<io::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let agent_pidfd = if abandonment {
        Some(owner.duplicate_pidfd(agent.token)?)
    } else {
        None
    };
    let mut session = NativeSession::new(owner, agent.token, loader, lease, retained, args.socket)?;
    let gate = session.gate();
    let signer = session.signer(identity)?;
    let loader_receipt = state.loader;
    if abandonment {
        return abandon_session(
            session,
            signer,
            control,
            NativeIdentity {
                agent_pid: agent.pid,
                loader_pid: loader_receipt.pid,
                creator_tid: agent.creator_tid,
            },
            descriptor_identities,
            agent_pidfd.ok_or_else(baseline::refused)?,
            pressure,
        );
    }
    let coordinator = thread::spawn(move || {
        let identity = NativeIdentity {
            agent_pid: agent.pid,
            loader_pid: loader_receipt.pid,
            creator_tid: agent.creator_tid,
        };
        if ssh {
            coordinate_ssh(case, signer, control, gate, identity)
        } else {
            coordinate(case, signer, control, gate, identity)
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    while !coordinator.is_finished() {
        baseline::require(Instant::now() < deadline)?;
        gate_io(session.service(Duration::from_millis(50)))?;
    }
    let (control, mut report) = coordinator.join().map_err(|_| baseline::refused())??;
    baseline::require(session.released())?;
    let exits = session.exits().ok_or_else(baseline::refused)?;
    for exit in exits {
        if exit.token == agent.token {
            report["agent_exit_code"] = exit.code.into();
            report["agent_exit_signal"] = exit.signal.into();
        } else if exit.token == state.loader.token {
            report["loader_exit_code"] = exit.code.into();
            report["loader_exit_signal"] = exit.signal.into();
        } else {
            return Err(baseline::refused());
        }
    }
    report["pressure_iterations"] = pressure.finish()?.into();
    control.send(&report)?;
    if !ssh {
        println!("{report}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn arguments() -> Vec<OsString> {
        [
            "--concurrent-case",
            "lock-before-native-send",
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
            "/private/a.sock",
            "--lease-dir-fd",
            "9",
        ]
        .map(OsString::from)
        .to_vec()
    }
    #[test]
    fn concurrent_cli_requires_an_independent_directory_capability() {
        assert!(parse(arguments()).is_ok());
        for value in ["3", "2", "invalid"] {
            let mut args = arguments();
            args[17] = value.into();
            assert!(parse(args).is_err());
        }
        let mut args = arguments();
        args.extend([OsString::from("--lease-dir-fd"), OsString::from("10")]);
        assert!(parse(args).is_err());
        let mut args = arguments();
        args.extend([OsString::from("--ssh-lifetime"), OsString::from("stdin")]);
        assert!(parse(args).is_err());
    }
    #[test]
    fn concurrent_cases_select_only_the_reviewed_boundaries() {
        assert_eq!(seam("completed-before-lock"), "before-native-send");
        assert_eq!(seam("parallel-lock-callers"), "before-native-send");
        assert_eq!(
            seam("lock-native-response-pending"),
            "native-response-pending"
        );
        assert_eq!(seam("lock-after-native-reply"), "after-native-reply");
        assert_eq!(
            seam("lease-change-before-publication"),
            "before-publication"
        );
        let mut args = arguments();
        args[1] = "unknown-case".into();
        assert!(parse(args).is_err());
    }

    fn ssh_arguments(case: &str) -> Vec<OsString> {
        let mut args = arguments();
        args[0] = "--concurrent-ssh-case".into();
        args[1] = case.into();
        args.extend(["--ssh-lifetime".into(), "stdin".into()]);
        args
    }

    #[test]
    fn ssh_cli_requires_stdin_lifetime_and_its_own_case_matrix() {
        for case in SSH_CASES {
            let (parsed_case, args, lease) = parse(ssh_arguments(case)).expect("SSH arguments");
            assert_eq!(parsed_case, case);
            assert!(args.ssh_lifetime);
            assert_eq!(lease, 9);
        }
        let mut missing = ssh_arguments("normal-close");
        missing.truncate(missing.len() - 2);
        assert!(parse(missing).is_err());
        let mut duplicate = ssh_arguments("normal-close");
        duplicate.extend(["--ssh-lifetime".into(), "stdin".into()]);
        assert!(parse(duplicate).is_err());
        let mut wrong_value = ssh_arguments("normal-close");
        *wrong_value.last_mut().expect("lifetime value") = "other".into();
        assert!(parse(wrong_value).is_err());
        assert!(parse(ssh_arguments("lock-before-native-send")).is_err());
        let mut alias = ssh_arguments("normal-close");
        alias[17] = "8".into();
        assert!(parse(alias).is_err());
        let mut local_case = arguments();
        local_case[1] = "normal-close".into();
        assert!(parse(local_case).is_err());
    }

    #[test]
    fn ssh_pauses_at_the_reviewed_work_boundaries() {
        for (case, expected) in [
            ("normal-close", "before-native-send"),
            ("disconnect-published", "before-native-send"),
            ("disconnect-before-native-send", "before-native-send"),
            (
                "disconnect-native-response-pending",
                "native-response-pending",
            ),
            ("disconnect-after-native-reply", "after-native-reply"),
            ("disconnect-before-publication", "before-publication"),
        ] {
            assert_eq!(seam(case), expected);
        }
    }

    #[test]
    fn published_pause_reports_joined_publication_without_proof_content() {
        let packet = published_paused("disconnect-published");
        let fields = packet
            .as_object()
            .expect("published packet")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            fields,
            [
                "stage",
                "case",
                "seam",
                "active",
                "workers_joined",
                "proofs_published"
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(packet["stage"], "published-paused");
        assert_eq!(packet["case"], "disconnect-published");
        assert_eq!(packet["seam"], "published");
        assert_eq!(packet["active"], false);
        assert_eq!(packet["workers_joined"], 1);
        assert_eq!(packet["proofs_published"], 1);
    }
}
