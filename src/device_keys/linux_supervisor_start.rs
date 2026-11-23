use crate::attended_terminal::AttendedUnlockTerminal;
use crate::linux_admission_storage::PublishError;
use crate::linux_creation_producer::CreationRuntime;
use crate::linux_current_admission::CurrentAdmission;
use crate::linux_current_admission::OpeningAuthority;
use crate::linux_endpoint_runtime::{EndpointRuntime, RuntimeRoot};
use crate::linux_ipc::packet_pair;
use crate::linux_ipc::registration::{
    acknowledge_supervisor_ready, authorize_endpoint_start, await_endpoint_ready,
    await_session_keeper_action, finish_endpoint_start, receive_any_session_child_step,
    receive_endpoint_start, receive_session_child_step, receive_session_keeper_opening,
    receive_session_keeper_step, receive_session_secret, send_session_child_step,
    send_session_keeper_step, send_session_secret, wait_session_keeper_finish,
    EndpointProofConnection, EndpointSessionStep, EndpointStartupRole, KeeperAdoptedOffer,
    KeeperChallengeReceiver, KeeperChallengeSender,
};
use crate::linux_spawn::{current_executable, ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "linux_supervisor_start/active_io.rs"]
mod active_io;
#[path = "linux_supervisor_start/cancellation.rs"]
mod cancellation;
#[path = "linux_supervisor_start/discoverable.rs"]
mod discoverable;
#[path = "linux_supervisor_start/foreground.rs"]
mod foreground;

use cancellation::{cancellation_pending, Cancellation};

const KEEPER: &str = "--internal-linux-keeper";
const SESSION_KEEPER: &str = "--internal-linux-session-keeper";
pub(crate) const SUPERVISOR: &str = "--internal-linux-supervisor";
pub(crate) const VERIFIER: &str = "--internal-linux-verifier";
const DISCOVER: &str = "--internal-linux-discover";
pub(crate) const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
const PROOF_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const SECRET_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpeningOutcome {
    Closed,
    Refused,
    OpeningUncertain,
    Interrupted,
    PublicationUncertain,
}

impl OpeningOutcome {
    fn after_failure(self, error: &io::Error) -> Self {
        if matches!(
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<PublishError>()),
            Some(PublishError::PublishedDurabilityUnknown(_)),
        ) {
            Self::PublicationUncertain
        } else {
            self
        }
    }
}

pub(crate) fn open_public(credential: &str) -> OpeningOutcome {
    let mut progress = OpeningOutcome::Refused;
    match keeper(Some(credential), true, true, &mut progress) {
        Ok(()) => OpeningOutcome::Closed,
        Err(error) => progress.after_failure(&error),
    }
}

fn refused() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}

fn descriptor(argument: &OsStr) -> io::Result<RawFd> {
    let text = argument.to_str().ok_or_else(refused)?;
    let descriptor = text.parse::<RawFd>().map_err(|_| refused())?;
    if descriptor < 3 || descriptor.to_string() != text {
        return Err(refused());
    }
    Ok(descriptor)
}

fn take_inherited(descriptor: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: F_GETFD only queries the caller-selected descriptor.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags < 0 || flags & libc::FD_CLOEXEC != 0 {
        return Err(refused());
    }
    // SAFETY: the inherited descriptor is live and startup is single-threaded.
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the selected startup descriptor is transferred exactly once.
    let original = unsafe { OwnedFd::from_raw_fd(descriptor) };
    // SAFETY: successful duplication returned a fresh exclusively owned descriptor.
    let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
    drop(original);
    Ok(duplicate)
}

fn null_stdio() -> io::Result<OwnedFd> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open("/dev/null")?;
    // SAFETY: the file remains owned and F_DUPFD_CLOEXEC returns a fresh descriptor.
    let duplicate = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful duplication transfers this descriptor to the caller.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

pub(crate) fn launch(
    owner: &mut SpawnOwner,
    command: &str,
    session: Option<(&str, &Path)>,
) -> io::Result<(Receipt, OwnedFd)> {
    let (socket, child_socket) = packet_pair()?;
    let mut arguments = vec![command.into(), child_socket.as_raw_fd().to_string().into()];
    if let Some((credential, runtime)) = session {
        arguments.push(credential.into());
        arguments.push(runtime.as_os_str().to_owned());
    }
    let receipt = owner.spawn(SpawnSpec {
        executable: current_executable()?,
        arg0: "vivac".into(),
        arguments,
        environment: Vec::new(),
        stdio: [null_stdio()?, null_stdio()?, null_stdio()?],
        inherited: vec![child_socket],
        retained: Vec::new(),
        new_session: false,
        controlling_tty: None,
        probe: None,
    })?;
    Ok((receipt, socket))
}

fn observed_exit(owner: &mut SpawnOwner, receipt: Receipt) -> io::Result<Option<ExitReceipt>> {
    let exit = owner.try_wait(receipt.token)?;
    if exit.is_some_and(|exit| exit.token != receipt.token || exit.pid != receipt.pid) {
        return Err(refused());
    }
    Ok(exit)
}

pub(crate) fn wait_for_success(
    owner: &mut SpawnOwner,
    receipt: Receipt,
    deadline: Instant,
) -> io::Result<()> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        if let Some(exit) = observed_exit(owner, receipt)? {
            if exit.code != Some(0) || exit.signal.is_some() {
                return Err(refused());
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_for_cancelled_children(
    owner: &mut SpawnOwner,
    supervisor: Receipt,
    verifier: Receipt,
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let supervisor_exited = observed_exit(owner, supervisor)?.is_some();
        let verifier_exited = observed_exit(owner, verifier)?.is_some();
        if supervisor_exited && verifier_exited {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn block_hangup() -> io::Result<()> {
    let mut set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: sigemptyset initializes the set before it is populated and used.
    if unsafe { libc::sigemptyset(set.as_mut_ptr()) } != 0
        || unsafe { libc::sigaddset(set.as_mut_ptr(), libc::SIGHUP) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // Private commands exit this process. Children start with a reset mask.
    // SAFETY: the signal set is initialized and no previous mask is requested.
    let code =
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, set.as_ptr(), std::ptr::null_mut()) };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code));
    }
    Ok(())
}

pub(crate) fn short_deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}

fn keeper(
    credential: Option<&str>,
    hold: bool,
    discoverable: bool,
    progress: &mut OpeningOutcome,
) -> io::Result<()> {
    let _cancellation = credential.map(|_| Cancellation::block()).transpose()?;
    let foreground = if credential.is_some() {
        let terminal = AttendedUnlockTerminal::open().map_err(|_| refused())?;
        if hold {
            terminal.validate_foreground_hold()?;
            Some(terminal)
        } else {
            None
        }
    } else {
        None
    };
    let admission = credential
        .map(CurrentAdmission::open_fixed_namespace)
        .transpose()?;
    if let Some(admission) = &admission {
        // This preflight is read-only. The supervisor consumes the permit under its reservation.
        admission.observe_opening_permit()?;
        if discoverable {
            admission.with_endpoint_location(|_, _| Ok(()))?;
        }
    }
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut owner = SpawnOwner::new_dedicated()?;
    let parent = credential
        .map(|_| foreground::create_runtime_parent())
        .transpose()?;
    let runtime = match &parent {
        Some(parent) => RuntimeRoot::create_in(parent)?,
        None => RuntimeRoot::create()?,
    };
    let session = credential.zip(parent.as_deref());
    let (supervisor, supervisor_channel) = launch(&mut owner, SUPERVISOR, session)?;
    let (verifier, verifier_channel) = launch(&mut owner, VERIFIER, session)?;
    for (child, channel, peer, role) in [
        (
            supervisor,
            &supervisor_channel,
            verifier,
            EndpointStartupRole::Supervisor,
        ),
        (
            verifier,
            &verifier_channel,
            supervisor,
            EndpointStartupRole::Verifier,
        ),
    ] {
        authorize_endpoint_start(
            channel,
            &owner,
            child.token,
            peer.token,
            runtime.directory(),
            role,
            deadline,
        )?;
    }
    if let Some(admission) = admission {
        let outcome = keeper_session(
            &mut owner,
            [
                (supervisor, &supervisor_channel),
                (verifier, &verifier_channel),
            ],
            admission,
            deadline,
            foreground.as_ref(),
            discoverable,
            progress,
        );
        let mut adopted = match outcome {
            Ok(adopted) => adopted,
            Err(error) => {
                drop(supervisor_channel);
                drop(verifier_channel);
                // The supervisor blocks hangup. Closing each channel above ends
                // its waits, including the one for the unlock secret, before
                // teardown.
                let _ = owner.signal(supervisor.token, libc::SIGHUP);
                if wait_for_cancelled_children(&mut owner, supervisor, verifier).is_ok() {
                    let _ = runtime.remove_empty();
                    if let Some(parent) = &parent {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
                return Err(error);
            }
        };
        runtime.remove_empty()?;
        if let Some(parent) = &parent {
            std::fs::remove_dir(parent)?;
        }
        let authorize_rename = || {
            if let Some(terminal) = &foreground {
                terminal.validate_foreground_hold()?;
            }
            if cancellation_pending()? {
                return Err(refused());
            }
            Ok(())
        };
        authorize_rename()?;
        return adopted
            .publish_clean_close(authorize_rename)
            .map_err(|error| match error {
                PublishError::Unpublished(error) => error,
                uncertain => io::Error::other(uncertain),
            });
    }
    for (child, channel) in [
        (supervisor, &supervisor_channel),
        (verifier, &verifier_channel),
    ] {
        await_endpoint_ready(channel, &owner, child.token, deadline)?;
    }
    // The verifier also holds the directory. Observe its exit before the
    // supervisor is released to remove the identified socket.
    finish_endpoint_start(&verifier_channel, &owner, verifier.token, deadline)?;
    wait_for_success(&mut owner, verifier, deadline)?;
    finish_endpoint_start(&supervisor_channel, &owner, supervisor.token, deadline)?;
    wait_for_success(&mut owner, supervisor, deadline)?;
    runtime.remove_empty()
}

fn keeper_session(
    owner: &mut SpawnOwner,
    [(supervisor, supervisor_channel), (verifier, verifier_channel)]: [(Receipt, &OwnedFd); 2],
    admission: CurrentAdmission,
    startup_deadline: Instant,
    foreground: Option<&AttendedUnlockTerminal>,
    discoverable: bool,
    progress: &mut OpeningOutcome,
) -> io::Result<KeeperAdoptedOffer> {
    use EndpointSessionStep::*;
    receive_keeper_child_step(
        verifier_channel,
        owner,
        verifier.token,
        Prepared,
        startup_deadline,
    )?;
    if let Some(terminal) = foreground {
        terminal.validate_foreground_hold()?;
    }
    if cancellation_pending()? {
        return Err(refused());
    }
    *progress = OpeningOutcome::OpeningUncertain;
    send_session_keeper_step(
        supervisor_channel,
        owner,
        supervisor.token,
        Start,
        short_deadline(),
    )?;
    receive_keeper_child_step(
        supervisor_channel,
        owner,
        supervisor.token,
        Register,
        Instant::now() + Duration::from_secs(20),
    )?;
    let mut adopted = KeeperChallengeSender::for_owned_child(
        supervisor_channel.try_clone()?,
        owner,
        supervisor.token,
        short_deadline(),
    )?
    .receive_launch_context(admission)?
    .receive_offer()?
    .adopt()?;
    adopted.confirm()?;
    if adopted.confirmed_opening_authority() != Some(OpeningAuthority::Permit) {
        return Err(refused());
    }
    *progress = OpeningOutcome::Interrupted;
    // Temporary until open-linux hands the secret to the keeper over its
    // control socket: the foreground keeper reads it and passes it on.
    let input = AttendedUnlockTerminal::open()
        .and_then(AttendedUnlockTerminal::read)
        .map_err(|_| refused())?;
    let secret = <&[u8; 64]>::try_from(input.as_bytes()).map_err(|_| refused())?;
    send_session_secret(
        supervisor_channel,
        owner,
        supervisor.token,
        secret,
        short_deadline(),
    )?;
    drop(input);
    receive_keeper_child_step(
        supervisor_channel,
        owner,
        supervisor.token,
        Unlocked,
        Instant::now() + Duration::from_secs(30),
    )?;
    send_session_keeper_step(
        verifier_channel,
        owner,
        verifier.token,
        Prove,
        short_deadline(),
    )?;
    let proof_deadline = Instant::now() + Duration::from_secs(22);
    receive_keeper_child_step(
        supervisor_channel,
        owner,
        supervisor.token,
        Proved,
        proof_deadline,
    )?;
    receive_keeper_child_step(
        verifier_channel,
        owner,
        verifier.token,
        Proved,
        proof_deadline,
    )?;
    finish_endpoint_start(verifier_channel, owner, verifier.token, short_deadline())?;
    wait_for_success(owner, verifier, short_deadline())?;
    if let Some(terminal) = foreground {
        if discoverable {
            terminal.validate_foreground_hold()?;
            if cancellation_pending()? {
                return Err(refused());
            }
            send_session_keeper_step(
                supervisor_channel,
                owner,
                supervisor.token,
                Activate,
                short_deadline(),
            )?;
            receive_keeper_child_step(
                supervisor_channel,
                owner,
                supervisor.token,
                Active,
                short_deadline(),
            )?;
        }
        let supervisor_token = supervisor.token;
        let owner = &*owner;
        terminal.wait_for_explicit_close(
            supervisor_channel.as_fd(),
            adopted.active_agent()?,
            || match receive_any_session_child_step(
                supervisor_channel,
                owner,
                supervisor_token,
                short_deadline(),
            )? {
                Served => Ok(()),
                _ => Err(refused()),
            },
        )?;
    }
    send_session_keeper_step(
        supervisor_channel,
        owner,
        supervisor.token,
        Close,
        short_deadline(),
    )?;
    // A proof served just before Close was sent is reported ahead of Closed.
    let closed_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match receive_keeper_any_child_step(
            supervisor_channel,
            owner,
            supervisor.token,
            closed_deadline,
        )? {
            Served => continue,
            Closed => break,
            _ => return Err(refused()),
        }
    }
    adopted.probe_receive_active_closure(short_deadline())?;
    adopted.confirm_active_closure(short_deadline())?;
    wait_for_success(owner, supervisor, short_deadline())?;
    if !adopted.observe_registered_exits()? {
        return Err(refused());
    }
    Ok(adopted)
}

fn receive_keeper_child_step(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    step: EndpointSessionStep,
    deadline: Instant,
) -> io::Result<()> {
    await_keeper_child_frame(socket, deadline)?;
    receive_session_child_step(socket, owner, token, step, deadline)
}

fn receive_keeper_any_child_step(
    socket: &OwnedFd,
    owner: &SpawnOwner,
    token: u64,
    deadline: Instant,
) -> io::Result<EndpointSessionStep> {
    await_keeper_child_frame(socket, deadline)?;
    receive_any_session_child_step(socket, owner, token, deadline)
}

fn await_keeper_child_frame(socket: &OwnedFd, deadline: Instant) -> io::Result<()> {
    loop {
        if cancellation_pending()? {
            return Err(refused());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut watched = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = remaining.as_millis().saturating_add(1).min(20) as i32;
        // SAFETY: watched is writable for one initialized poll descriptor.
        let ready = unsafe { libc::poll(&mut watched, 1, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if watched.revents != 0 {
            if cancellation_pending()? {
                return Err(refused());
            }
            return Ok(());
        }
    }
}

fn session_child(
    controller: &OwnedFd,
    connected: OwnedFd,
    expected: &OwnedFd,
    role: EndpointStartupRole,
    (credential, runtime): (&str, &Path),
    startup_deadline: Instant,
) -> io::Result<()> {
    use EndpointSessionStep::*;
    EndpointProofConnection::for_expected_peer(connected.try_clone()?, expected, startup_deadline)?;
    match role {
        EndpointStartupRole::Verifier => {
            let admission = CurrentAdmission::open_fixed_namespace(credential)?;
            send_session_child_step(controller, Prepared, startup_deadline)?;
            receive_session_keeper_step(
                controller,
                Prove,
                Instant::now() + Duration::from_secs(60),
            )?;
            let mut connection = EndpointProofConnection::for_expected_peer(
                connected,
                expected,
                Instant::now() + PROOF_TIMEOUT,
            )?;
            connection.verify_endpoint_possession(&admission)?;
            send_session_child_step(controller, Proved, short_deadline())?;
            wait_session_keeper_finish(controller, Instant::now() + Duration::from_secs(12))?;
            drop(connection);
            Ok(())
        }
        EndpointStartupRole::Supervisor => {
            let authority = receive_session_keeper_opening(controller, startup_deadline)?;
            let admission = CurrentAdmission::open_fixed_namespace(credential)?;
            let reserved = admission.prepare_native_material()?.reserve()?;
            let home = CreationRuntime::create_session_in(runtime).map_err(|_| refused())?;
            let opening = match authority {
                OpeningAuthority::Permit => reserved.prepare_service_opening(),
                OpeningAuthority::Reopened => reserved.prepare_service_reopening(),
            };
            let opening = match opening {
                Ok(opening) => opening,
                Err(error) => {
                    let _ = home.cleanup_after_native_teardown();
                    return Err(error);
                }
            };
            let outcome = opening.probe_unlock_registered_active(
                home.path(),
                |registration| {
                    send_session_child_step(controller, Register, short_deadline())?;
                    KeeperChallengeReceiver::for_parent(controller.try_clone()?, short_deadline())?
                        .send_launch_context(registration)?
                        .send_offer()?
                        .receive_confirmation()?
                        .into_active_closure_sender()
                },
                || {
                    // While a terminal prompts for the secret, its cancellation
                    // signals reach this process group too. Hold them so that a
                    // prompt ended by cancellation ends this wait through the
                    // closed channel and the cleanup below still runs.
                    let _cancellation = Cancellation::block()?;
                    receive_session_secret(controller, Instant::now() + SECRET_TIMEOUT)
                },
            );
            let (mut session, provider, closure) = match outcome {
                Ok(active) => active,
                Err(error) => {
                    if !SpawnOwner::admission_uncertain() {
                        let _ = home.cleanup_after_native_teardown();
                    }
                    return Err(error);
                }
            };
            let agent_socket = home.capture_agent_socket().map_err(|_| refused());
            let close = (|| {
                agent_socket.as_ref().map_err(|_| refused())?;
                send_session_child_step(controller, Unlocked, short_deadline())?;
                let mut connection = EndpointProofConnection::for_expected_peer(
                    connected,
                    expected,
                    Instant::now() + PROOF_TIMEOUT,
                )?;
                provider.revalidate()?;
                session.serve_endpoint_proof(
                    &mut connection,
                    CurrentAdmission::open_fixed_namespace(credential)?,
                )?;
                provider.revalidate()?;
                send_session_child_step(controller, Proved, short_deadline())?;
                match await_session_keeper_action(controller)? {
                    Close => Ok(()),
                    Activate => discoverable::serve(controller, credential, &session, &provider),
                    _ => Err(refused()),
                }
            })();
            let admission_closed = session
                .gate()
                .stop_endpoint_admission(Duration::from_secs(1));
            session
                .close_on_creator(Duration::from_secs(5))
                .map_err(|_| refused())?;
            home.cleanup_after_session(agent_socket?)
                .map_err(|_| refused())?;
            close?;
            admission_closed.map_err(|_| refused())?;
            send_session_child_step(controller, Closed, short_deadline())?;
            closure
                .send(&mut session, short_deadline())?
                .receive_confirmation(short_deadline())
        }
    }
}

fn endpoint_child(
    descriptor: RawFd,
    role: EndpointStartupRole,
    session: Option<(&str, &Path)>,
) -> io::Result<()> {
    if session.is_some() && role == EndpointStartupRole::Supervisor {
        block_hangup()?;
    }
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let socket = take_inherited(descriptor)?;
    let [peer, directory] = receive_endpoint_start(&socket, role, deadline)?;
    let runtime = EndpointRuntime::from_directory(directory)?;
    let (connected, owned_socket) = match role {
        EndpointStartupRole::Supervisor => {
            let (connected, owned_socket) = runtime.listen_and_accept(deadline)?;
            (connected, Some(owned_socket))
        }
        EndpointStartupRole::Verifier => (runtime.connect(deadline)?, None),
    };
    let result = if let Some(session) = session {
        session_child(&socket, connected, &peer, role, session, deadline)
    } else {
        let connection = EndpointProofConnection::for_expected_peer(connected, &peer, deadline)?;
        acknowledge_supervisor_ready(&socket, deadline)?;
        drop(connection);
        Ok(())
    };
    let cleanup = owned_socket.map_or(Ok(()), |socket| runtime.remove_socket(socket));
    result.and(cleanup)
}

fn credential(argument: Option<std::ffi::OsString>) -> io::Result<Option<String>> {
    argument
        .map(|value| {
            let value = value.into_string().map_err(|_| refused())?;
            if value.len() != 26 || !value.is_ascii() {
                return Err(refused());
            }
            Ok(value)
        })
        .transpose()
}

/// Accepts only an absolute path written in its one canonical form.
fn runtime_directory(argument: Option<OsString>) -> io::Result<Option<PathBuf>> {
    let Some(argument) = argument else {
        return Ok(None);
    };
    let path = PathBuf::from(argument);
    let mut components = path.components();
    if components.next() != Some(Component::RootDir)
        || !components.all(|component| matches!(component, Component::Normal(_)))
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
        || path.as_os_str().len() == 1
    {
        return Err(refused());
    }
    Ok(Some(path))
}

pub(crate) fn verify_discovered_endpoint(credential: &str) -> io::Result<()> {
    let deadline = Instant::now() + DISCOVERY_TIMEOUT;
    let admission = CurrentAdmission::open_fixed_namespace(credential)?;
    admission.with_endpoint_location(|name, directory| {
        let runtime = EndpointRuntime::from_named_directory(directory, name)?;
        runtime.with_connection(deadline, |connected| {
            EndpointProofConnection::new(connected, deadline)?
                .verify_endpoint_possession(&admission)
                .map(|_| ())
        })
    })?;
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(())
}

pub(crate) fn dispatch() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments.next()?;
    if command == SESSION_KEEPER {
        return Some(crate::linux_session_keeper::run(arguments));
    }
    let result = if command == KEEPER {
        let credential = credential(arguments.next());
        let mode = arguments.next();
        if arguments.next().is_some()
            || mode
                .as_deref()
                .is_some_and(|mode| mode != "--hold" && mode != "--hold-discoverable")
        {
            Err(refused())
        } else {
            credential.and_then(|credential| {
                keeper(
                    credential.as_deref(),
                    mode.is_some(),
                    mode.as_deref() == Some(OsStr::new("--hold-discoverable")),
                    &mut OpeningOutcome::Refused,
                )
            })
        }
    } else if command == DISCOVER {
        let result = credential(arguments.next()).and_then(|value| {
            if arguments.next().is_some() {
                return Err(refused());
            }
            verify_discovered_endpoint(value.as_deref().ok_or_else(refused)?)
        });
        return Some(match result {
            Ok(()) => 0,
            Err(_) => {
                let _ = io::stderr()
                    .lock()
                    .write_all(b"Linux custody endpoint proof refused.\n");
                1
            }
        });
    } else if command == SUPERVISOR || command == VERIFIER {
        let role = if command == SUPERVISOR {
            EndpointStartupRole::Supervisor
        } else {
            EndpointStartupRole::Verifier
        };
        let descriptor = arguments
            .next()
            .ok_or_else(refused)
            .and_then(|argument| descriptor(&argument));
        let credential = credential(arguments.next());
        let runtime = runtime_directory(arguments.next());
        if arguments.next().is_some() {
            Err(refused())
        } else {
            descriptor.and_then(|fd| match (credential?, runtime?) {
                (None, None) => endpoint_child(fd, role, None),
                (Some(credential), Some(runtime)) => {
                    endpoint_child(fd, role, Some((&credential, &runtime)))
                }
                _ => Err(refused()),
            })
        }
    } else {
        return None;
    };
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            let message: &[u8] = if matches!(
                error
                    .get_ref()
                    .and_then(|inner| inner.downcast_ref::<PublishError>()),
                Some(PublishError::PublishedDurabilityUnknown(_)),
            ) {
                b"Linux custody permit publication durability is uncertain.\n"
            } else {
                b"Linux custody startup refused.\n"
            };
            // A disconnected controlling terminal can also close stderr.
            let _ = io::stderr().lock().write_all(message);
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_failure_preserves_authenticated_progress() {
        let failure = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(
            OpeningOutcome::Refused.after_failure(&failure),
            OpeningOutcome::Refused
        );
        assert_eq!(
            OpeningOutcome::OpeningUncertain.after_failure(&failure),
            OpeningOutcome::OpeningUncertain
        );
        assert_eq!(
            OpeningOutcome::Interrupted.after_failure(&failure),
            OpeningOutcome::Interrupted
        );
        let uncertain = io::Error::other(PublishError::PublishedDurabilityUnknown(
            io::Error::from(io::ErrorKind::PermissionDenied),
        ));
        assert_eq!(
            OpeningOutcome::Interrupted.after_failure(&uncertain),
            OpeningOutcome::PublicationUncertain
        );
    }
}
