use crate::connected_probe;
use crate::linux_current_admission::CurrentAdmission;
use crate::linux_gate::{Error, State};
use crate::linux_ipc::registration::EndpointProofConnection;
use crate::linux_session::NativeSession;
use crate::loader_probe as baseline;
use crate::registration_probe::ActiveRelayCase;
use rust_launcher_probe::ubuntu_provider::UbuntuProvider;
use serde_json::json;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{OwnedFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

enum Invocation {
    Custodian(String, PathBuf, RawFd),
    AttendedCustodian(String, PathBuf),
    PermittedCustodian(String, PathBuf),
    ActiveCustodian(String, PathBuf, ActiveRelayCase),
    Verifier(String, PathBuf),
}

fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<Invocation> {
    let mut arguments = arguments.into_iter();
    let mode = arguments.next().ok_or_else(baseline::refused)?;
    let credential = arguments
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    baseline::require(
        credential.len() == 26
            && matches!(credential.as_bytes().first(), Some(b'0'..=b'7'))
            && credential
                .bytes()
                .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&byte)),
    )?;
    let home = PathBuf::from(arguments.next().ok_or_else(baseline::refused)?);
    baseline::require(home.is_absolute())?;
    let invocation = if mode == "--endpoint-native-custodian" {
        let text = arguments
            .next()
            .ok_or_else(baseline::refused)?
            .into_string()
            .map_err(|_| baseline::refused())?;
        let descriptor: RawFd = text.parse().map_err(|_| baseline::refused())?;
        baseline::require(descriptor >= 3 && descriptor.to_string() == text)?;
        Invocation::Custodian(credential, home, descriptor)
    } else if mode == "--endpoint-attended-custodian" {
        Invocation::AttendedCustodian(credential, home)
    } else if mode == "--endpoint-permitted-custodian" {
        Invocation::PermittedCustodian(credential, home)
    } else if mode == "--endpoint-active-custodian" {
        let case = arguments.next().ok_or_else(baseline::refused)?;
        Invocation::ActiveCustodian(credential, home, ActiveRelayCase::parse(&case)?)
    } else {
        baseline::require(mode == "--endpoint-native-verifier")?;
        Invocation::Verifier(credential, home)
    };
    baseline::require(arguments.next().is_none())?;
    Ok(invocation)
}

fn private_home(home: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(home)?;
    baseline::require(
        home.is_absolute()
            && metadata.is_dir()
            && metadata.mode() & 0o7777 == 0o700
            && metadata.uid() == 65534
            && metadata.gid() == 65534,
    )
}

fn secret_from_pipe(descriptor: RawFd) -> io::Result<Zeroizing<Vec<u8>>> {
    let descriptor = baseline::take_inherited(descriptor)?;
    let file = File::from(descriptor);
    baseline::require(file.metadata()?.file_type().is_fifo())?;
    let mut secret = Zeroizing::new(Vec::new());
    file.take(65).read_to_end(&mut secret)?;
    baseline::require(
        secret.len() == 64
            && secret
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)),
    )?;
    Ok(secret)
}

pub(super) fn wait_release() -> io::Result<()> {
    let mut item = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll borrows one initialized writable descriptor.
    baseline::require(
        unsafe { libc::poll(&mut item, 1, 15000) } == 1
            && item.revents & libc::POLLIN != 0
            && item.revents & (libc::POLLERR | libc::POLLNVAL) == 0,
    )?;
    let mut command = [0; 1];
    io::stdin().read_exact(&mut command)?;
    baseline::require(command == *b"G")
}

fn report(role: &str, generation: [u8; 32], closed: bool) -> io::Result<()> {
    let value = json!({
        "endpoint_role": role,
        "generation": baseline::hex_encode(&generation),
        "signature_verified": true,
        "session_closed": closed,
    });
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, &value).map_err(|_| baseline::refused())?;
    output.write_all(b"\n")?;
    output.flush()
}

fn gate_io<T>(value: Result<T, Error>) -> io::Result<T> {
    value.map_err(|_| baseline::refused())
}

fn stage_error(label: &'static str) -> impl FnOnce(io::Error) -> io::Error {
    move |error| io::Error::new(error.kind(), label)
}

pub(super) fn failure(error: &io::Error) {
    let message = error.to_string();
    let phase = match message.as_str() {
        "accept"
        | "connection"
        | "reserve-attempt"
        | "receive-challenge"
        | "consume-attempt"
        | "prepare-context"
        | "prepare-nonce"
        | "prepare-agent-connect"
        | "prepare-agent-peer"
        | "prepare-agent-write"
        | "prepare-agent-header"
        | "prepare-agent-body"
        | "prepare-agent-deadline"
        | "prepare-agent-identity"
        | "prepare-revalidate"
        | "prepare-final-deadline"
        | "sign-proof"
        | "validate-before-send"
        | "send-proof"
        | "validate-after-send"
        | "check-result"
        | "close-session"
        | "remove-sockets"
        | "attended-unlock"
        | "opening-permit"
        | "keeper-owner"
        | "keeper-launch"
        | "keeper-ready"
        | "keeper-predecessor-material"
        | "keeper-predecessor-openssh"
        | "keeper-predecessor-terminal"
        | "keeper-terminal"
        | "keeper-bootstrap"
        | "keeper-offer"
        | "keeper-adopt"
        | "keeper-confirm"
        | "keeper-confirm-received"
        | "keeper-stop"
        | "keeper-collect"
        | "keeper-advance"
        | "active-old-ready"
        | "active-old-closed"
        | "active-close-frame"
        | "active-close-confirm"
        | "active-close-confirmed"
        | "active-close-refusal"
        | "active-close-publish"
        | "active-old-exit"
        | "active-advance"
        | "verify-proof" => message.as_str(),
        _ => "other",
    };
    println!(
        "{}",
        json!({"endpoint_failure": phase, "kind": format!("{:?}", error.kind())})
    );
}

fn remove_socket(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    baseline::require(metadata.file_type().is_socket() && metadata.uid() == 65534)?;
    std::fs::remove_file(path)
}

fn custodian(credential: &str, home: &Path, descriptor: RawFd) -> io::Result<()> {
    private_home(home)?;
    let secret = secret_from_pipe(descriptor)?;
    let preparation = CurrentAdmission::open_fixed_namespace(credential)?
        .prepare_native_material()?
        .reserve()?
        .prepare_launch()?;
    let (mut session, provider) =
        rust_launcher_probe::probe_unlock_admitted_openssh(preparation, home, &secret)?;
    drop(secret);
    let (generation, connection) =
        exchange_as_custodian(credential, home, &mut session, &provider, None)?;
    report("custodian", generation, true)?;
    wait_release()?;
    drop(connection);
    Ok(())
}

fn attended_custodian(credential: &str, home: &Path) -> io::Result<()> {
    private_home(home)?;
    let (generation, connection) =
        crate::registration_probe::with_restricted_keeper_successor(credential, |successor| {
            let (_, generation) = successor.generations();
            let (mut session, provider) =
                rust_launcher_probe::probe_unlock_attended_successor(successor, home)
                    .map_err(stage_error("attended-unlock"))?;
            exchange_as_custodian(credential, home, &mut session, &provider, Some(generation))
        })?;
    report("custodian", generation, true)?;
    wait_release()?;
    drop(connection);
    Ok(())
}

fn permitted_custodian(credential: &str, home: &Path) -> io::Result<()> {
    private_home(home)?;
    let launch = CurrentAdmission::open_fixed_namespace(credential)?
        .prepare_native_material()?
        .reserve()?
        .prepare_attended_opening()
        .map_err(stage_error("opening-permit"))?;
    let (mut session, provider) = launch
        .probe_unlock_attended(home)
        .map_err(stage_error("attended-unlock"))?;
    let (generation, connection) =
        exchange_as_custodian(credential, home, &mut session, &provider, None)?;
    report("custodian", generation, true)?;
    wait_release()?;
    drop(connection);
    Ok(())
}

fn active_custodian(credential: &str, home: &Path, case: ActiveRelayCase) -> io::Result<()> {
    private_home(home)?;
    let result = crate::registration_probe::with_active_keeper_successor(
        credential,
        home,
        case,
        |successor| {
            let (_, generation) = successor.generations();
            let (mut session, provider) =
                rust_launcher_probe::probe_unlock_attended_successor(successor, home)
                    .map_err(stage_error("attended-unlock"))?;
            exchange_as_custodian(credential, home, &mut session, &provider, Some(generation))
        },
    )?;
    if let Some((generation, connection)) = result {
        report("custodian", generation, true)?;
        wait_release()?;
        drop(connection);
    } else if case == ActiveRelayCase::Missing {
        println!("{}", json!({"active_closure_missing_refused": true}));
    } else if case == ActiveRelayCase::PermitClose {
        println!("{}", json!({"clean_close_permit_published": true}));
        io::stdout().flush()?;
        wait_release()?;
    } else if matches!(
        case,
        ActiveRelayCase::PermitMissing | ActiveRelayCase::PermitUnconfirmed
    ) {
        println!("{}", json!({"clean_close_permit_refused": true}));
    } else {
        let refusal = case.refusal().ok_or_else(baseline::refused)?;
        println!("{}", json!({"active_closure_refusal": refusal}));
        if case == ActiveRelayCase::ConnectionLost {
            wait_release()?;
        }
    }
    Ok(())
}

pub(super) fn exchange_as_custodian(
    credential: &str,
    home: &Path,
    session: &mut NativeSession,
    provider: &UbuntuProvider,
    expected_generation: Option<[u8; 32]>,
) -> io::Result<([u8; 32], EndpointProofConnection)> {
    exchange_as_custodian_with_cut(
        credential,
        home,
        session,
        provider,
        expected_generation,
        || Ok(()),
    )
}

pub(super) fn exchange_as_custodian_with_cut(
    credential: &str,
    home: &Path,
    session: &mut NativeSession,
    provider: &UbuntuProvider,
    expected_generation: Option<[u8; 32]>,
    before_close: impl FnOnce() -> io::Result<()>,
) -> io::Result<([u8; 32], EndpointProofConnection)> {
    let socket = home.join("endpoint.sock");
    let mut endpoint_created = false;
    let result = (|| {
        let generation = session.endpoint_witness()?.generation()?;
        if let Some(expected_generation) = expected_generation {
            baseline::require(generation == expected_generation)?;
            crate::registration_probe::expect_adopted_reservation(credential)?;
        }
        baseline::require(!socket.exists())?;
        let listener = connected_probe::listen(&socket)?;
        endpoint_created = true;
        let connected = connected_probe::accept(&listener).map_err(stage_error("accept"))?;
        drop(listener);
        let mut connection =
            EndpointProofConnection::new(connected, Instant::now() + Duration::from_secs(15))
                .map_err(stage_error("connection"))?;
        provider.revalidate()?;
        session.serve_endpoint_proof(
            &mut connection,
            CurrentAdmission::open_fixed_namespace(credential)?,
        )?;
        provider.revalidate()?;
        let snapshot = gate_io(session.gate().snapshot())?;
        baseline::require(
            snapshot.state == State::Ready
                && snapshot.proofs_verified == 1
                && snapshot.proofs_published == 1,
        )
        .map_err(stage_error("check-result"))?;
        before_close()?;
        Ok((generation, connection))
    })();
    gate_io(session.close_on_creator(Duration::from_secs(5)))
        .map_err(stage_error("close-session"))?;
    remove_socket(&home.join("agent.sock")).map_err(stage_error("remove-sockets"))?;
    if endpoint_created {
        remove_socket(&socket).map_err(stage_error("remove-sockets"))?;
    }
    result
}

fn connect_when_ready(path: &Path) -> io::Result<OwnedFd> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match connected_probe::connect(path) {
            Ok(connection) => return Ok(connection),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn verifier(credential: &str, home: &Path) -> io::Result<()> {
    private_home(home)?;
    let admission = CurrentAdmission::open_fixed_namespace(credential)?;
    let connected =
        connect_when_ready(&home.join("endpoint.sock")).map_err(stage_error("connection"))?;
    let mut connection =
        EndpointProofConnection::new(connected, Instant::now() + Duration::from_secs(15))?;
    let verified = connection
        .verify_endpoint_possession(&admission)
        .map_err(stage_error("verify-proof"))?;
    report("verifier", verified.generation(), false)?;
    wait_release()?;
    drop(connection);
    Ok(())
}

pub(super) fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: these calls query this disposable process's identities and flag.
    baseline::require(unsafe {
        libc::getuid() == 65534
            && libc::geteuid() == 65534
            && libc::getgid() == 65534
            && libc::getegid() == 65534
            && libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1
    })?;
    let invocation = parse(std::env::args_os().skip(1))?;
    // SAFETY: alarm bounds only this explicitly selected disposable process.
    unsafe {
        libc::alarm(if matches!(invocation, Invocation::ActiveCustodian(..)) {
            85
        } else if matches!(
            invocation,
            Invocation::AttendedCustodian(..) | Invocation::PermittedCustodian(..)
        ) {
            55
        } else {
            40
        })
    };
    match invocation {
        Invocation::Custodian(credential, home, descriptor) => {
            custodian(&credential, &home, descriptor)
        }
        Invocation::AttendedCustodian(credential, home) => attended_custodian(&credential, &home),
        Invocation::PermittedCustodian(credential, home) => permitted_custodian(&credential, &home),
        Invocation::ActiveCustodian(credential, home, case) => {
            active_custodian(&credential, &home, case)
        }
        Invocation::Verifier(credential, home) => verifier(&credential, &home),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_requires_explicit_role_credential_runtime_and_anonymous_input_descriptor() {
        let credential = "00000000000000000000000001";
        assert!(matches!(
            parse(
                [
                    "--endpoint-native-custodian",
                    credential,
                    "/tmp/native",
                    "3"
                ]
                .map(Into::into)
            ),
            Ok(Invocation::Custodian(_, _, 3))
        ));
        assert!(matches!(
            parse(["--endpoint-native-verifier", credential, "/tmp/native"].map(Into::into)),
            Ok(Invocation::Verifier(_, _))
        ));
        assert!(matches!(
            parse(["--endpoint-attended-custodian", credential, "/tmp/native"].map(Into::into)),
            Ok(Invocation::AttendedCustodian(_, _))
        ));
        for (case, expected) in [
            ("normal", ActiveRelayCase::Normal),
            ("missing", ActiveRelayCase::Missing),
            ("incomplete", ActiveRelayCase::Incomplete),
            ("different-generation", ActiveRelayCase::DifferentGeneration),
            ("connection-lost", ActiveRelayCase::ConnectionLost),
            ("keeper-lost", ActiveRelayCase::KeeperLost),
            ("permit-close", ActiveRelayCase::PermitClose),
            ("permit-missing", ActiveRelayCase::PermitMissing),
            ("permit-keeper-lost", ActiveRelayCase::PermitKeeperLost),
        ] {
            assert!(matches!(
                parse(["--endpoint-active-custodian", credential, "/tmp/native", case].map(Into::into)),
                Ok(Invocation::ActiveCustodian(_, _, observed)) if observed == expected
            ));
        }
        for arguments in [
            vec![
                "--endpoint-native-custodian",
                credential,
                "/tmp/native",
                "03",
            ],
            vec![
                "--endpoint-native-custodian",
                credential,
                "/tmp/native",
                "0",
            ],
            vec!["--endpoint-native-custodian", credential, "/tmp/native"],
            vec!["--endpoint-native-verifier", credential, "relative"],
            vec!["--endpoint-native-verifier", "wrong", "/tmp/native"],
            vec!["--endpoint-native-verifier", credential, "/tmp/native", "3"],
            vec![
                "--endpoint-attended-custodian",
                credential,
                "/tmp/native",
                "3",
            ],
            vec!["--endpoint-active-custodian", credential, "/tmp/native"],
            vec![
                "--endpoint-active-custodian",
                credential,
                "/tmp/native",
                "other",
            ],
        ] {
            assert!(parse(arguments.into_iter().map(Into::into)).is_err());
        }
    }
}
