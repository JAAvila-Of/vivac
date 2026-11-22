use crate::fixed_root_probe::Channel;
use crate::linux_creation_record::Record;
use crate::linux_current_admission::{AdmittedLaunchPreparation, CurrentAdmission};
use crate::linux_resident_reader::OpeningPermitKind;
use crate::loader_probe as baseline;
use crate::registration_probe::{marker_spec, sleep_spec};
use rust_launcher_probe::{probe_launch_admitted_pair, probe_observe_admitted_children};
use serde_json::json;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File};
use std::io::{self, Write};
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static OPENSSH_HOME_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) struct TemporaryOpenSshHome {
    path: PathBuf,
}

impl TemporaryOpenSshHome {
    pub(super) fn create() -> io::Result<Self> {
        let temporary = std::env::temp_dir();
        let parent = fs::symlink_metadata(&temporary)?;
        baseline::require(
            temporary.is_absolute() && parent.is_dir() && !parent.file_type().is_symlink(),
        )?;
        for _ in 0..8 {
            let sequence = OPENSSH_HOME_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = temporary.join(format!("vivac-openssh-{}-{sequence}", std::process::id()));
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => {
                    let metadata = fs::symlink_metadata(&path)?;
                    baseline::require(
                        metadata.is_dir()
                            && !metadata.file_type().is_symlink()
                            && metadata.mode() & 0o7777 == 0o700
                            && metadata.uid() == unsafe { libc::geteuid() }
                            && metadata.gid() == unsafe { libc::getegid() },
                    )?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryOpenSshHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

struct Arguments {
    credential: String,
    observer: RawFd,
    prepare_launch: bool,
}

fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<Arguments> {
    let mut arguments = arguments.into_iter();
    baseline::require(arguments.next().as_deref() == Some(OsStr::new("--creation-read")))?;
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
    baseline::require(arguments.next().as_deref() == Some(OsStr::new("--observer-fd")))?;
    let descriptor = arguments
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    baseline::require(
        !descriptor.is_empty() && descriptor.bytes().all(|byte| byte.is_ascii_digit()),
    )?;
    let observer = descriptor
        .parse::<RawFd>()
        .map_err(|_| baseline::refused())?;
    baseline::require(observer >= 3 && observer.to_string() == descriptor)?;
    let prepare_launch = match arguments.next() {
        None => false,
        Some(value) if value == OsStr::new("--prepare-launch") => true,
        _ => return Err(baseline::refused()),
    };
    baseline::require(arguments.next().is_none())?;
    Ok(Arguments {
        credential,
        observer,
        prepare_launch,
    })
}

fn error_label(error: &io::Error) -> &'static str {
    match error.raw_os_error() {
        Some(libc::EXDEV) => "crosses-devices",
        Some(libc::ELOOP) => "symlink",
        _ => match error.kind() {
            io::ErrorKind::NotFound => "not-found",
            io::ErrorKind::InvalidData => "invalid-data",
            io::ErrorKind::PermissionDenied => "permission-denied",
            _ => "other",
        },
    }
}

fn channel(descriptor: OwnedFd) -> io::Result<Channel> {
    let mut kind: libc::c_int = 0;
    let mut length = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: getsockopt writes only the initialized scalar option storage.
    baseline::require(
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
    // SAFETY: zero is a valid initial sockaddr_storage representation.
    let mut address: libc::sockaddr_storage = unsafe { zeroed() };
    let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: getsockname writes within the live complete address storage.
    baseline::require(
        unsafe {
            libc::getsockname(
                descriptor.as_raw_fd(),
                std::ptr::addr_of_mut!(address).cast(),
                &mut length,
            )
        } == 0
            && length as usize >= size_of::<libc::sa_family_t>()
            && address.ss_family as libc::c_int == libc::AF_UNIX,
    )?;
    Channel::new(descriptor)
}

fn validated_record(candidate: &CurrentAdmission) -> io::Result<String> {
    let bytes = candidate.canonical_record()?;
    let record = Record::parse(&bytes)?;
    baseline::require(candidate.matches_record(&record)?)?;
    let public_key = candidate.expected_public_key()?;
    baseline::require(record.public_key == crate::linux_openssh_package::hex(&public_key))?;
    String::from_utf8(bytes).map_err(|_| baseline::refused())
}

fn verify_keeper_directory_binding(credential: &str) -> io::Result<()> {
    let packages = "/var/lib/vivac/custody-v1/users/65534/state/packages";
    let package = format!("{packages}/{credential}");
    let open_directory = |path: &str| -> io::Result<File> {
        File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
    };
    {
        let different: OwnedFd = open_directory(packages)?.into();
        // SAFETY: this disposable fixture owns the open laboratory directory.
        baseline::require(unsafe {
            libc::flock(different.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0
        })?;
        crate::linux_reservation::verify_retained_flock(&different)?;
        baseline::require(
            CurrentAdmission::open_fixed_namespace(credential)?
                .retain_keeper_directory(different)
                .is_err(),
        )?;
    }
    let locked = open_directory(&package)?;
    let independent = open_directory(&package)?;
    let locked_identity = locked.metadata()?;
    let independent_identity = independent.metadata()?;
    baseline::require(
        locked_identity.dev() == independent_identity.dev()
            && locked_identity.ino() == independent_identity.ino(),
    )?;
    let locked: OwnedFd = locked.into();
    // SAFETY: the fixture owns this disposable package directory descriptor.
    baseline::require(unsafe {
        libc::flock(locked.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0
    })?;
    crate::linux_reservation::verify_retained_flock(&locked)?;
    baseline::require(
        CurrentAdmission::open_fixed_namespace(credential)?
            .retain_keeper_directory(independent.into())
            .is_err(),
    )?;
    let retained = CurrentAdmission::open_fixed_namespace(credential)?
        .retain_keeper_directory(locked.try_clone()?)?;
    retained.validate()?;
    let control = locked.try_clone()?;
    // SAFETY: this fixture duplicate shares the retained open file description.
    baseline::require(unsafe { libc::flock(control.as_raw_fd(), libc::LOCK_UN) == 0 })?;
    baseline::require(retained.validate().is_err())?;
    // SAFETY: reacquire only on the same disposable open file description.
    baseline::require(unsafe {
        libc::flock(control.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0
    })?;
    crate::linux_reservation::verify_retained_flock(&control)?;
    baseline::require(retained.validate().is_err())?;
    Ok(())
}

pub(super) fn launch_preparation(credential: &str) -> io::Result<AdmittedLaunchPreparation> {
    CurrentAdmission::open_fixed_namespace(credential)?
        .prepare_native_material()?
        .reserve()?
        .prepare_launch()
}

fn verify_admitted_openssh_roles(launch: &mut AdmittedLaunchPreparation) -> io::Result<()> {
    let home = TemporaryOpenSshHome::create()?;
    rust_launcher_probe::probe_admitted_openssh_roles(launch, home.path())?;
    let retry = rust_launcher_probe::probe_admitted_openssh_roles(launch, home.path())
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(retry.kind() == io::ErrorKind::InvalidData)?;
    baseline::require(fs::read_dir(home.path())?.next().is_none())?;
    fs::remove_dir(home.path())
}

fn verify_no_admitted_pair_retry(launch: &mut AdmittedLaunchPreparation) -> io::Result<()> {
    let (_first_gate, first) = marker_spec()?;
    let (_second_gate, second) = marker_spec()?;
    let error = probe_launch_admitted_pair(launch, [first, second])
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::InvalidData)
}

pub(super) fn verify_admitted_exit_observation(
    credential: &str,
    launch: &mut AdmittedLaunchPreparation,
    expected_count: usize,
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let exits = loop {
        baseline::require(Instant::now() < deadline)?;
        if let Some(exits) = probe_observe_admitted_children(launch)? {
            break exits;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    baseline::require(exits.len() == expected_count)?;
    baseline::require(
        exits
            .iter()
            .all(|exit| exit.code == Some(0) && exit.signal.is_none()),
    )?;
    let competing =
        CurrentAdmission::open_fixed_namespace(credential)?.prepare_native_material()?;
    let error = competing.reserve().err().ok_or_else(baseline::refused)?;
    baseline::require(error.kind() == io::ErrorKind::WouldBlock)
}

fn verify_admitted_pair(credential: &str) -> io::Result<()> {
    let mut launch = launch_preparation(credential)?;
    let (first_gate, first) = marker_spec()?;
    let (second_gate, second) = marker_spec()?;
    {
        let registration = probe_launch_admitted_pair(&mut launch, [first, second])?;
        first_gate.expect(b'R')?;
        second_gate.expect(b'R')?;
        registration.validate()?;
    }
    baseline::require(probe_observe_admitted_children(&mut launch)?.is_none())?;
    verify_no_admitted_pair_retry(&mut launch)?;
    first_gate.send(b'G')?;
    second_gate.send(b'G')?;
    verify_admitted_exit_observation(credential, &mut launch, 2)
}

fn verify_admitted_executable_order(credential: &str) -> io::Result<()> {
    let mut launch = launch_preparation(credential)?;
    let (gate, first) = marker_spec()?;
    let second = sleep_spec()?;
    let first_image = first.executable.try_clone()?;
    let second_image = second.executable.try_clone()?;
    {
        let registration = probe_launch_admitted_pair(&mut launch, [first, second])?;
        gate.expect(b'R')?;
        registration.validate_executable_order([&first_image, &second_image])?;
        baseline::require(
            registration
                .validate_executable_order([&second_image, &first_image])
                .is_err(),
        )?;
        baseline::require(
            registration
                .validate_executable_order([&first_image, &second_image])
                .is_err(),
        )?;
    }
    gate.send(b'G')?;
    verify_admitted_exit_observation(credential, &mut launch, 2)
}

fn verify_partial_admitted_pair(credential: &str) -> io::Result<()> {
    let mut launch = launch_preparation(credential)?;
    let (first_gate, first) = marker_spec()?;
    let (_second_gate, mut second) = marker_spec()?;
    second.arg0 = OsString::new();
    let error = probe_launch_admitted_pair(&mut launch, [first, second])
        .err()
        .ok_or_else(baseline::refused)?;
    baseline::require(error.raw_os_error() == Some(libc::EINVAL))?;
    first_gate.expect(b'R')?;
    baseline::require(launch.validate().is_err())?;
    baseline::require(probe_observe_admitted_children(&mut launch)?.is_none())?;
    verify_no_admitted_pair_retry(&mut launch)?;
    first_gate.send(b'G')?;
    verify_admitted_exit_observation(credential, &mut launch, 1)?;
    baseline::require(launch.validate().is_err())
}

pub(super) fn opening_permit() -> io::Result<bool> {
    let observed = (|| {
        baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
        // SAFETY: these operations only query the disposable process credentials.
        baseline::require(unsafe {
            libc::getuid() == 65534
                && libc::geteuid() == 65534
                && libc::getgid() == 65534
                && libc::getegid() == 65534
                && libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1
        })?;
        let mut arguments = std::env::args_os().skip(2);
        let credential = arguments.next().ok_or_else(baseline::refused)?;
        baseline::require(arguments.next().is_none())?;
        let credential = credential.to_str().ok_or_else(baseline::refused)?;
        CurrentAdmission::open_fixed_namespace(credential)?.observe_opening_permit()
    })();
    let label = match observed {
        Ok(OpeningPermitKind::FirstOpen) => "first-open",
        Ok(OpeningPermitKind::CleanClose) => "clean-close",
        Err(_) => "refused",
    };
    writeln!(
        io::stdout().lock(),
        "{}",
        json!({"opening_permit": label, "authority": false})
    )?;
    Ok(observed.is_ok())
}

pub(super) fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: these calls only query the current process's credentials and flag.
    baseline::require(unsafe {
        libc::getuid() == 65534
            && libc::geteuid() == 65534
            && libc::getgid() == 65534
            && libc::getegid() == 65534
            && libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1
    })?;
    // SAFETY: this bounds only the explicitly disposable probe process.
    unsafe { libc::alarm(45) };
    let arguments = parse(std::env::args_os().skip(1))?;
    let observer = channel(baseline::take_inherited(arguments.observer)?)?;
    let (candidate, initial_error) =
        match CurrentAdmission::open_fixed_namespace(&arguments.credential) {
            Ok(candidate) => (Some(candidate), "none"),
            Err(error) => (None, error_label(&error)),
        };
    let record = candidate.as_ref().map(validated_record).transpose()?;
    let store_identity = candidate
        .as_ref()
        .map(CurrentAdmission::store_identity)
        .transpose()?;
    let keeper_restricted_successor_verified = if candidate.is_some() && arguments.prepare_launch {
        crate::registration_probe::verify_sigchld_admission()?;
        crate::registration_probe::verify_dedicated_host_admission()?;
        crate::registration_probe::verify_task_creation_restriction()?;
        verify_keeper_directory_binding(&arguments.credential)?;
        verify_admitted_pair(&arguments.credential)?;
        verify_admitted_executable_order(&arguments.credential)?;
        verify_partial_admitted_pair(&arguments.credential)?;
        crate::registration_probe::verify_admitted_bootstrap(&arguments.credential)?;
        crate::registration_probe::verify_openssh_keeper_successor(&arguments.credential)?;
        true
    } else {
        false
    };
    let admitted_openssh_roles_verified = if candidate.is_some() && arguments.prepare_launch {
        let mut roles_probe = CurrentAdmission::open_fixed_namespace(&arguments.credential)?
            .prepare_native_material()?
            .reserve()?
            .prepare_launch()?;
        verify_admitted_openssh_roles(&mut roles_probe)?;
        true
    } else {
        false
    };
    let (material_witness, reserved_witness, launch_witness) = if candidate.is_some() {
        let material = CurrentAdmission::open_fixed_namespace(&arguments.credential)?
            .prepare_native_material()?;
        material.validate()?;
        let reserved = CurrentAdmission::open_fixed_namespace(&arguments.credential)?
            .prepare_native_material()?
            .reserve()?;
        reserved.validate()?;
        if arguments.prepare_launch {
            let launch = reserved.prepare_launch()?;
            launch.validate()?;
            (Some(material), None, Some(launch))
        } else {
            (Some(material), Some(reserved), None)
        }
    } else {
        (None, None, None)
    };
    let launch_challenge = [1; 32];
    let launch_commitment = if let Some(launch) = &launch_witness {
        let error = launch
            .keeper_admission_commitment(&[0; 32])
            .err()
            .ok_or_else(baseline::refused)?;
        baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
        let commitment = launch.keeper_admission_commitment(&launch_challenge)?;
        baseline::require(launch.keeper_admission_commitment(&launch_challenge)? == commitment)?;
        baseline::require(launch.keeper_admission_commitment(&[2; 32])? != commitment)?;
        Some(commitment)
    } else {
        None
    };
    let mut result = json!({
        "launch_prepared": launch_witness.is_some(),
        "launch_snapshot_revalidated": false, "launch_failure_sticky": false,
        "launch_reservation_contention_rejected": false, "launch_reservation_reacquired": false,
        "launch_validation_error": if arguments.prepare_launch { "not-run" } else { "not-required" },
        "version": 1, "stage": "opened", "pid": unsafe { libc::getpid() },
        "credential_id": arguments.credential, "initial_opened": candidate.is_some(),
        "initial_error": initial_error, "record_canonical": record,
        "snapshot_revalidated": false, "validation_error": "not-run",
        "repeat_snapshot_revalidated": false, "repeat_validation_error": "not-run", "read_only": true,
        "creation_admission": false, "custody_accredited": false,
        "producer_origin_checked": false, "durable_ack_inferred": false,
        "admitted_material_prepared": false, "admitted_material_validated": false,
        "admitted_reservation_validated": false,
        "admitted_reservation_contention_rejected": false,
        "admitted_reservation_reacquired": false,
        "material_snapshot_revalidated": false,
        "reserved_snapshot_revalidated": false,
        "material_failure_sticky": false, "reserved_failure_sticky": false,
        "current_namespace_revalidated": false,
        "material_validation_error": "not-run", "reserved_validation_error": if arguments.prepare_launch { "not-required" } else { "not-run" },
    });
    if arguments.prepare_launch {
        result["admitted_openssh_roles_verified"] = json!(admitted_openssh_roles_verified);
        result["keeper_restricted_successor_verified"] =
            json!(keeper_restricted_successor_verified);
    }
    observer.send(&result)?;
    observer.go()?;
    result["stage"] = json!("validated");
    if let Some(material) = &material_witness {
        match material.validate() {
            Ok(()) => {
                result["material_snapshot_revalidated"] = json!(true);
                result["material_validation_error"] = json!("none");
            }
            Err(error) => result["material_validation_error"] = json!(error_label(&error)),
        }
    } else {
        result["material_validation_error"] = json!("unavailable");
    }
    if let Some(reserved) = &reserved_witness {
        match reserved.validate() {
            Ok(()) => {
                result["reserved_snapshot_revalidated"] = json!(true);
                result["reserved_validation_error"] = json!("none");
            }
            Err(error) => result["reserved_validation_error"] = json!(error_label(&error)),
        }
    } else {
        result["reserved_validation_error"] = json!(if arguments.prepare_launch {
            "not-required"
        } else {
            "unavailable"
        });
    }
    if let Some(launch) = &launch_witness {
        let commitment = launch.keeper_admission_commitment(&launch_challenge);
        match (commitment, launch.validate()) {
            (Ok(commitment), Ok(())) => {
                baseline::require(Some(commitment) == launch_commitment)?;
                result["launch_snapshot_revalidated"] = json!(true);
                result["launch_validation_error"] = json!("none");
            }
            (Err(commitment_error), Err(error)) => {
                baseline::require(commitment_error.kind() == error.kind())?;
                result["launch_validation_error"] = json!(error_label(&error));
            }
            _ => return Err(baseline::refused()),
        }
    }
    if let Some(candidate) = &candidate {
        match candidate.validate() {
            Ok(()) => {
                result["snapshot_revalidated"] = json!(true);
                result["validation_error"] = json!("none");
            }
            Err(error) => result["validation_error"] = json!(error_label(&error)),
        }
    } else {
        result["validation_error"] = json!("unavailable");
    }
    observer.send(&result)?;
    observer.go()?;
    if let Some(material) = &material_witness {
        let repeated = material.validate();
        if result["material_snapshot_revalidated"] == true {
            repeated?;
        } else {
            let error = repeated.err().ok_or_else(baseline::refused)?;
            baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
            result["material_failure_sticky"] = json!(true);
        }
    }
    if let Some(reserved) = &reserved_witness {
        let repeated = reserved.validate();
        if result["reserved_snapshot_revalidated"] == true {
            repeated?;
        } else {
            let error = repeated.err().ok_or_else(baseline::refused)?;
            baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
            result["reserved_failure_sticky"] = json!(true);
        }
    }
    if let Some(launch) = &launch_witness {
        let repeated_commitment = launch.keeper_admission_commitment(&launch_challenge);
        if result["launch_snapshot_revalidated"] == true {
            baseline::require(Some(repeated_commitment?) == launch_commitment)?;
            launch.validate()?;
            let competing = CurrentAdmission::open_fixed_namespace(&arguments.credential)?
                .prepare_native_material()?;
            let error = competing.reserve().err().ok_or_else(baseline::refused)?;
            baseline::require(error.kind() == io::ErrorKind::WouldBlock)?;
            launch.validate()?;
            result["launch_reservation_contention_rejected"] = json!(true);
        } else {
            let error = repeated_commitment.err().ok_or_else(baseline::refused)?;
            baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
            let error = launch.validate().err().ok_or_else(baseline::refused)?;
            baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
            result["launch_failure_sticky"] = json!(true);
        }
    }
    result["current_namespace_revalidated"] = json!(CurrentAdmission::open_fixed_namespace(
        &arguments.credential
    )
    .and_then(|current| current.validate())
    .is_ok());
    if let Some(candidate) = &candidate {
        let repeated = candidate.validate();
        if result["snapshot_revalidated"] == true {
            repeated?;
            baseline::require(Some(validated_record(candidate)?) == record)?;
            baseline::require(Some(candidate.store_identity()?) == store_identity)?;
            result["repeat_snapshot_revalidated"] = json!(true);
            result["repeat_validation_error"] = json!("none");
        } else {
            let error = repeated.err().ok_or_else(baseline::refused)?;
            baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
            for error in [
                candidate
                    .canonical_record()
                    .err()
                    .ok_or_else(baseline::refused)?,
                candidate
                    .expected_public_key()
                    .err()
                    .ok_or_else(baseline::refused)?,
                candidate
                    .store_identity()
                    .err()
                    .ok_or_else(baseline::refused)?,
            ] {
                baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
            }
            result["repeat_validation_error"] = json!("invalid-data");
        }
    } else {
        result["repeat_validation_error"] = json!("unavailable");
    }
    drop(material_witness);
    drop(reserved_witness);
    drop(launch_witness);
    if arguments.prepare_launch && result["repeat_snapshot_revalidated"] == true {
        let fresh = CurrentAdmission::open_fixed_namespace(&arguments.credential)?
            .prepare_native_material()?
            .reserve()?
            .prepare_launch()?;
        fresh.validate()?;
        baseline::require(
            Some(fresh.keeper_admission_commitment(&launch_challenge)?) != launch_commitment,
        )?;
        result["launch_reservation_reacquired"] = json!(true);
        drop(fresh);
        drop(candidate);
    } else if !arguments.prepare_launch && result["repeat_snapshot_revalidated"] == true {
        let candidate = candidate.ok_or_else(baseline::refused)?;
        let material = candidate.prepare_native_material()?;
        result["admitted_material_prepared"] = json!(true);
        material.validate()?;
        result["admitted_material_validated"] = json!(true);
        let reserved = material.reserve()?;
        reserved.validate()?;
        result["admitted_reservation_validated"] = json!(true);
        let competing = CurrentAdmission::open_fixed_namespace(&arguments.credential)?
            .prepare_native_material()?;
        competing.validate()?;
        let error = competing.reserve().err().ok_or_else(baseline::refused)?;
        baseline::require(error.kind() == io::ErrorKind::WouldBlock)?;
        reserved.validate()?;
        result["admitted_reservation_contention_rejected"] = json!(true);
        drop(reserved);
        let reacquired = CurrentAdmission::open_fixed_namespace(&arguments.credential)?
            .prepare_native_material()?
            .reserve()?;
        reacquired.validate()?;
        result["admitted_reservation_reacquired"] = json!(true);
        drop(reacquired);
    } else {
        drop(candidate);
    }
    result["stage"] = json!("final");
    result["snapshot_released"] = json!(true);
    observer.send(&result)?;
    let mut bytes = serde_json::to_vec(&result).map_err(|_| baseline::refused())?;
    baseline::require(bytes.len() <= 8192)?;
    bytes.push(b'\n');
    io::stdout().write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn arguments() -> Vec<OsString> {
        [
            "--creation-read",
            "00000000000000000000000004",
            "--observer-fd",
            "3",
        ]
        .into_iter()
        .map(Into::into)
        .collect()
    }

    #[test]
    fn fresh_reader_cli_accepts_only_a_credential_and_inherited_observer() {
        let parsed = parse(arguments()).expect("accept the exact reader invocation");
        assert_eq!(parsed.credential, "00000000000000000000000004");
        assert_eq!(parsed.observer, 3);
    }

    #[test]
    fn fresh_reader_cli_accepts_only_the_fixed_launch_mode() {
        let mut values = arguments();
        values.push("--prepare-launch".into());
        assert!(parse(values.clone()).unwrap().prepare_launch);
        values.push("--prepare-launch".into());
        assert!(parse(values).is_err());
        let mut values = arguments();
        values.push("--prepare-launch=1".into());
        assert!(parse(values).is_err());
    }

    #[test]
    fn fresh_reader_cli_refuses_paths_records_noncanonical_descriptors_and_extras() {
        for (index, value) in [
            (0, "--resident-case"),
            (0, "--creation-read-path"),
            (1, "/var/lib/vivac"),
            (1, "0000000000000000000000000i"),
            (1, "80000000000000000000000000"),
            (1, "0000000000000000000000000A"),
            (1, "0000000000000000000000000"),
            (2, "--record"),
            (2, "--root"),
            (2, "--expected"),
            (2, "--case"),
            (3, "0"),
            (3, "1"),
            (3, "2"),
            (3, "03"),
            (3, "+3"),
            (3, "-3"),
            (3, "2147483648"),
            (3, "3 "),
            (3, ""),
        ] {
            let mut values = arguments();
            values[index] = value.into();
            assert!(parse(values).is_err(), "{index}: {value}");
        }
        for length in 0..4 {
            assert!(parse(arguments().into_iter().take(length)).is_err());
        }
        let mut extra = arguments();
        extra.push("--record".into());
        assert!(parse(extra).is_err());
        for index in 0..4 {
            let mut non_utf8 = arguments();
            non_utf8[index] = OsString::from_vec(vec![0xff]);
            assert!(parse(non_utf8).is_err());
        }
    }
}
