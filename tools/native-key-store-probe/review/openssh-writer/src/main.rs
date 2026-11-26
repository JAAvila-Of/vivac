#[cfg(all(test, not(target_os = "linux")))]
#[path = "../../../../../src/device_keys/linux_generated_secret.rs"]
mod generated_secret;
#[cfg(all(target_os = "linux", feature = "heap-probe"))]
mod heap_probe;
#[cfg(target_os = "linux")]
use rust_launcher_probe::{
    linux_admission_storage, linux_creation_binding, linux_creation_record, linux_creation_storage,
    linux_current_admission, linux_custody_root, linux_gate, linux_loader, linux_openssh_package,
    linux_reservation, linux_resident_reader, linux_session, linux_spawn, ubuntu_provider,
};
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_creation_producer.rs"]
mod creation_producer;
#[cfg(target_os = "linux")]
mod disposable_faults;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_opening_publication.rs"]
mod opening_publication;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_package_storage.rs"]
mod package;
#[cfg(target_os = "linux")]
mod terminal;
#[cfg(target_os = "linux")]
use terminal as creation_input;
#[cfg(any(target_os = "linux", test))]
#[path = "../../../../../src/device_keys/linux_creation_ciphertext.rs"]
mod creation_ciphertext;

#[cfg(any(target_os = "linux", test))]
mod writer {
    #[cfg(target_os = "linux")]
    mod creation_draft_probe;
    #[cfg(target_os = "linux")]
    mod creation_native_probe;
    #[cfg(target_os = "linux")]
    pub(crate) use crate::creation_ciphertext::valid_identifier;
    #[cfg(all(test, unix))]
    use crate::creation_ciphertext::ROUNDS;
    use crate::creation_ciphertext::{serialize_encrypted, validate_comment};
    #[cfg(target_os = "linux")]
    use crate::creation_producer;
    #[cfg(all(test, unix))]
    use ssh_key::{Cipher, Kdf, PrivateKey};
    use std::io::Read;
    #[cfg(unix)]
    use std::io::Write;
    use std::path::Path;
    use zeroize::Zeroizing;

    type Result<T> = std::result::Result<T, ()>;
    const MAX_INPUT: usize = 1026;
    const MIN_PASSWORD: usize = 16;
    const MAX_PASSWORD: usize = 512;

    #[cfg(target_os = "linux")]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum RunError {
        Refused,
        CommittedIndeterminate,
    }

    #[cfg(target_os = "linux")]
    impl From<()> for RunError {
        fn from(_: ()) -> Self {
            Self::Refused
        }
    }

    #[cfg(target_os = "linux")]
    impl From<crate::package::CreateError> for RunError {
        fn from(error: crate::package::CreateError) -> Self {
            match error {
                crate::package::CreateError::Refused => Self::Refused,
                crate::package::CreateError::CommittedIndeterminate => Self::CommittedIndeterminate,
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl RunError {
        pub(crate) fn exit_code(self) -> i32 {
            match self {
                Self::Refused => 1,
                Self::CommittedIndeterminate => 2,
            }
        }

        pub(crate) fn message(self) -> &'static str {
            match self {
                Self::Refused => "Experimental OpenSSH writer refused or failed.",
                Self::CommittedIndeterminate => "Experimental OpenSSH package committed; durability is indeterminate. Do not overwrite or retry automatically.",
            }
        }
    }

    fn read_password_pair(source: &mut impl Read) -> Result<Zeroizing<Vec<u8>>> {
        let mut input = Zeroizing::new(Vec::with_capacity(MAX_INPUT + 1));
        source
            .take((MAX_INPUT + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|_| ())?;
        if input.len() > MAX_INPUT || !input.ends_with(b"\n") {
            return Err(());
        }
        let mut lines = input.split(|byte| *byte == b'\n');
        let password = lines.next().ok_or(())?;
        let confirmation = lines.next().ok_or(())?;
        if lines.next() != Some(b"".as_slice()) || lines.next().is_some() {
            return Err(());
        }
        for line in [password, confirmation] {
            if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&line.len())
                || line.contains(&0)
                || line.contains(&b'\r')
                || std::str::from_utf8(line).is_err()
            {
                return Err(());
            }
        }
        if password != confirmation {
            return Err(());
        }
        Ok(Zeroizing::new(password.to_vec()))
    }

    #[cfg(unix)]
    fn publish_encrypted(destination: &Path, encoded: &[u8]) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)
            .map_err(|_| ())?;
        file.write_all(encoded).map_err(|_| ())?;
        file.sync_all().map_err(|_| ())
    }

    #[cfg(not(unix))]
    fn publish_encrypted(_destination: &Path, _encoded: &[u8]) -> Result<()> {
        Err(())
    }

    fn write_from_reader(destination: &Path, comment: &str, source: &mut impl Read) -> Result<()> {
        let password = read_password_pair(source)?;
        validate_comment(comment)?;
        let encoded = serialize_encrypted(comment, &password)?;
        publish_encrypted(destination, encoded.as_bytes())
    }

    fn generated_package_arguments(
        mut arguments: impl Iterator<Item = std::ffi::OsString>,
    ) -> Result<(std::path::PathBuf, String)> {
        let destination = std::path::PathBuf::from(arguments.next().ok_or(())?);
        let comment = arguments.next().ok_or(())?.into_string().map_err(|_| ())?;
        if arguments.next().is_some() || !destination.is_absolute() {
            return Err(());
        }
        validate_comment(&comment)?;
        Ok((destination, comment))
    }

    #[cfg(target_os = "linux")]
    struct AttendedPreparation {
        confirmation: crate::terminal::ConfirmedGeneratedSecret,
        package: crate::package::PreparedPackage,
    }

    #[cfg(target_os = "linux")]
    fn prepare_attended(
        location: crate::package::Location,
        comment: &str,
    ) -> std::result::Result<AttendedPreparation, RunError> {
        let (confirmation, package) = creation_producer::prepare_package(location, comment)?;
        Ok(AttendedPreparation {
            confirmation,
            package,
        })
    }

    #[cfg(target_os = "linux")]
    fn prepare_attended_using(
        comment: &str,
        prepare: impl FnOnce(
            &[u8],
        ) -> std::result::Result<
            crate::package::PreparedPackage,
            crate::package::CreateError,
        >,
    ) -> std::result::Result<AttendedPreparation, RunError> {
        let ciphertext = crate::creation_ciphertext::AttendedCiphertext::read(comment)?;
        let (confirmation, encoded) = ciphertext.into_parts();
        let package = prepare(encoded.as_bytes())?;
        Ok(AttendedPreparation {
            confirmation,
            package,
        })
    }

    #[cfg(target_os = "linux")]
    impl AttendedPreparation {
        fn publish_experimental(
            self,
            fault: crate::disposable_faults::FaultPoint,
        ) -> std::result::Result<(), RunError> {
            let Self {
                confirmation,
                package,
            } = self;
            let result = crate::disposable_faults::publish(package, fault);
            drop(confirmation);
            result.map_err(RunError::from)
        }
    }

    #[cfg(target_os = "linux")]
    pub fn run() -> std::result::Result<(), RunError> {
        use std::os::unix::fs::FileTypeExt;

        if std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() != Ok("1") {
            return Err(RunError::Refused);
        }
        let mut arguments = std::env::args_os().skip(1);
        let first = arguments.next().ok_or(())?;
        if first == "--fixed-root-foreground-create" {
            let device = arguments.next().ok_or(())?;
            let credential = arguments.next().ok_or(())?;
            if arguments.next().is_some() {
                return Err(RunError::Refused);
            }
            // SAFETY: these calls only query the disposable process's identity and flag.
            if !unsafe {
                libc::getuid() == 65534
                    && libc::geteuid() == 65534
                    && libc::getgid() == 65534
                    && libc::getegid() == 65534
                    && libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1
            } {
                return Err(RunError::Refused);
            }
            let result = creation_producer::create_attended(
                device.to_str().ok_or(())?,
                credential.to_str().ok_or(())?,
            )?;
            use creation_producer::PublicationOutcome;
            let mut outcome = result.outcome();
            let record = if outcome == PublicationOutcome::Durable {
                match result
                    .record()
                    .ok()
                    .and_then(|record| serde_json::to_value(record).ok())
                {
                    Some(record) => Some(record),
                    None => {
                        outcome = PublicationOutcome::RecordDurabilityUnknown;
                        None
                    }
                }
            } else {
                None
            };
            let mut report = serde_json::json!({"outcome": outcome.as_str()});
            if let Some(record) = record {
                report["record"] = record;
            }
            let delivery_error = if outcome == PublicationOutcome::Unpublished {
                RunError::Refused
            } else {
                RunError::CommittedIndeterminate
            };
            let mut packet = serde_json::to_vec(&report).map_err(|_| delivery_error)?;
            packet.push(b'\n');
            let mut stdout = std::io::stdout().lock();
            stdout
                .write_all(&packet)
                .and_then(|()| stdout.flush())
                .map_err(|_| delivery_error)?;
            return match outcome {
                PublicationOutcome::Durable => Ok(()),
                PublicationOutcome::Unpublished => Err(RunError::Refused),
                _ => Err(RunError::CommittedIndeterminate),
            };
        }
        if first == "--attended-unlock-input-probe" {
            if arguments.next().is_some() {
                return Err(RunError::Refused);
            }
            // SAFETY: these calls only query the disposable process's identity and flag.
            if !unsafe {
                libc::getuid() == 65534
                    && libc::geteuid() == 65534
                    && libc::getgid() == 65534
                    && libc::getegid() == 65534
                    && libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1
            } {
                return Err(RunError::Refused);
            }
            let input = crate::terminal::read_existing_unlock()?;
            if input.as_bytes().len() != crate::terminal::ENCODED_LENGTH {
                return Err(RunError::Refused);
            }
            drop(input);
            return std::io::stdout()
                .lock()
                .write_all(b"{\"version\":1,\"attended_input_accepted\":true,\"terminal_restored\":true,\"credential_unlocked\":false,\"session_activated\":false}\n")
                .map_err(|_| RunError::Refused);
        }
        if first == "--fixed-root-record-draft" {
            return creation_draft_probe::run(arguments);
        }
        if first == "--fixed-root-native-bind" {
            return creation_native_probe::run(arguments);
        }
        if first == "--package-check" {
            let destination = arguments.next().ok_or(())?;
            if arguments.next().is_some() {
                return Err(RunError::Refused);
            }
            return crate::package::check(Path::new(&destination)).map_err(RunError::from);
        }
        if first == "--generated-package-create" {
            let (destination, comment) = generated_package_arguments(arguments)?;
            let fault = crate::disposable_faults::FaultPoint::from_environment()?;
            let location = crate::package::Location::preflight(&destination)?;
            let prepared = if fault == crate::disposable_faults::FaultPoint::None {
                prepare_attended(location, &comment)?
            } else {
                prepare_attended_using(&comment, |encoded| {
                    crate::package::prepare_observed(location, &comment, encoded, |stage| {
                        fault.observe_preparation(stage)
                    })
                })?
            };
            return prepared.publish_experimental(fault);
        }
        let terminal = first == "--terminal";
        let package_create = first == "--package-create";
        let destination = if terminal || package_create {
            arguments.next().ok_or(())?
        } else {
            first
        };
        let comment = arguments.next().ok_or(())?;
        if arguments.next().is_some() {
            return Err(RunError::Refused);
        }
        let comment = comment.to_str().ok_or(())?;
        validate_comment(comment)?;
        let package = if package_create {
            let fault = crate::disposable_faults::FaultPoint::from_environment()?;
            Some((
                crate::package::Location::preflight(Path::new(&destination))?,
                fault,
            ))
        } else {
            None
        };
        if terminal {
            let password = crate::terminal::read_confirmed()?;
            let encoded = serialize_encrypted(comment, &password)?;
            return publish_encrypted(Path::new(&destination), encoded.as_bytes())
                .map_err(RunError::from);
        }
        // Development fixture only: stdin is the probe's owned pipe, not a
        // terminal password reader or a supported product password channel.
        if !std::fs::metadata("/proc/self/fd/0")
            .map_err(|_| ())?
            .file_type()
            .is_fifo()
        {
            return Err(RunError::Refused);
        }
        if let Some((location, fault)) = package {
            let password = read_password_pair(&mut std::io::stdin().lock())?;
            let encoded = serialize_encrypted(comment, &password)?;
            return crate::disposable_faults::create(location, comment, encoded.as_bytes(), fault)
                .map_err(|_| RunError::Refused);
        }
        write_from_reader(
            Path::new(&destination),
            comment,
            &mut std::io::stdin().lock(),
        )
        .map_err(RunError::from)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Cursor;
        use std::sync::atomic::{AtomicU64, Ordering};

        const COMMENT: &str =
            "vivac.device-key.v1:00000000000000000000000001:00000000000000000000000002";
        const PASSWORD: &[u8] = b"public-writer-fixture-password";
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

        #[cfg(target_os = "linux")]
        #[test]
        fn generated_publication_errors_have_distinct_constant_outcomes() {
            let refused = RunError::from(crate::package::CreateError::Refused);
            assert_eq!(refused, RunError::Refused);
            assert_eq!(refused.exit_code(), 1);
            assert_eq!(
                refused.message(),
                "Experimental OpenSSH writer refused or failed."
            );
            let committed = RunError::from(crate::package::CreateError::CommittedIndeterminate);
            assert_eq!(committed, RunError::CommittedIndeterminate);
            assert_eq!(committed.exit_code(), 2);
            assert_eq!(committed.message(), "Experimental OpenSSH package committed; durability is indeterminate. Do not overwrite or retry automatically.");
            assert_eq!(RunError::from(()), RunError::Refused);
        }

        fn pair(first: &[u8], second: &[u8]) -> Zeroizing<Vec<u8>> {
            let mut result = Zeroizing::new(Vec::new());
            result.extend_from_slice(first);
            result.push(b'\n');
            result.extend_from_slice(second);
            result.push(b'\n');
            result
        }

        fn invalid_inputs() -> Vec<Zeroizing<Vec<u8>>> {
            let mut invalid_utf8 = vec![b'a'; 16];
            invalid_utf8[3] = 0xff;
            let mut nul = vec![b'a'; 16];
            nul[3] = 0;
            let mut carriage_return = vec![b'a'; 16];
            carriage_return[3] = b'\r';
            let mut extra = pair(PASSWORD, PASSWORD);
            extra.extend_from_slice(b"extra\n");
            vec![
                Zeroizing::new(Vec::new()),
                pair(b"", b""),
                pair(b"too-short", b"too-short"),
                pair(&[b'a'; 513], &[b'a'; 513]),
                pair(&invalid_utf8, &invalid_utf8),
                pair(&nul, &nul),
                pair(&carriage_return, &carriage_return),
                pair(PASSWORD, b"different-public-fixture-password"),
                Zeroizing::new([PASSWORD, b"\n"].concat()),
                extra,
                Zeroizing::new([PASSWORD, b"\n", PASSWORD].concat()),
            ]
        }

        struct TemporaryDirectory(std::path::PathBuf);

        impl TemporaryDirectory {
            fn new() -> Self {
                let counter = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
                let instant = std::time::SystemTime::UNIX_EPOCH
                    .elapsed()
                    .expect("test clock unavailable")
                    .as_nanos();
                let path = std::env::temp_dir()
                    .join(format!("vivac-openssh-writer-test-{instant}-{counter}"));
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&path)
                        .expect("test directory creation failed");
                }
                #[cfg(not(unix))]
                std::fs::create_dir(&path).expect("test directory creation failed");
                Self(path)
            }

            fn destination(&self) -> std::path::PathBuf {
                self.0.join("key")
            }
        }

        impl Drop for TemporaryDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.destination());
                let _ = std::fs::remove_dir(&self.0);
            }
        }

        #[test]
        fn malformed_password_inputs_are_refused() {
            for input in invalid_inputs() {
                assert!(read_password_pair(&mut Cursor::new(&*input)).is_err());
            }
        }

        #[test]
        fn accepted_passwords_preserve_exact_bytes() {
            for password in [vec![b'a'; 16], vec![b'b'; 512], "á".repeat(8).into_bytes()] {
                let input = pair(&password, &password);
                let accepted = read_password_pair(&mut Cursor::new(&*input))
                    .expect("public fixture should be accepted");
                assert_eq!(accepted.as_slice(), password.as_slice());
            }
        }

        #[test]
        fn generated_package_arguments_never_fall_back_to_another_mode() {
            let destination = std::env::temp_dir().join("generated-package-public-fixture");
            let accepted = generated_package_arguments(
                [destination.as_os_str().to_owned(), COMMENT.into()].into_iter(),
            )
            .expect("complete explicit arguments should be accepted");
            assert_eq!(accepted, (destination.clone(), COMMENT.to_owned()));
            for invalid in [
                Vec::new(),
                vec![destination.as_os_str().to_owned()],
                vec![
                    destination.as_os_str().to_owned(),
                    COMMENT.into(),
                    "extra".into(),
                ],
                vec!["--terminal".into(), COMMENT.into()],
                vec!["relative-package".into(), COMMENT.into()],
                vec![destination.as_os_str().to_owned(), "invalid-comment".into()],
            ] {
                assert!(generated_package_arguments(invalid.into_iter()).is_err());
            }
        }

        #[test]
        fn only_canonical_opaque_comments_are_accepted() {
            assert!(validate_comment(COMMENT).is_ok());
            for invalid in [
                "",
                "vivac.device-key.v1:short:short",
                "vivac.device-key.v1:80000000000000000000000001:00000000000000000000000002",
                "vivac.device-key.v1:0000000000000000000000000i:00000000000000000000000002",
                "vivac.device-key.v1:00000000000000000000000001:00000000000000000000000002:extra",
                "vivac.device-key.v1:0000000000000000000000000á:00000000000000000000000002",
            ] {
                assert!(validate_comment(invalid).is_err());
            }
        }

        #[test]
        fn invalid_input_never_creates_destination() {
            let directory = TemporaryDirectory::new();
            for input in invalid_inputs() {
                assert!(write_from_reader(
                    &directory.destination(),
                    COMMENT,
                    &mut Cursor::new(&*input)
                )
                .is_err());
                assert!(!directory.destination().exists());
            }
        }

        #[cfg(unix)]
        #[test]
        fn valid_creation_publishes_only_an_encrypted_private_file() {
            use std::os::unix::fs::PermissionsExt;

            let directory = TemporaryDirectory::new();
            let input = pair(PASSWORD, PASSWORD);
            write_from_reader(&directory.destination(), COMMENT, &mut Cursor::new(&*input))
                .expect("public fixture creation failed");
            let metadata = directory
                .destination()
                .metadata()
                .expect("test output absent");
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            let encoded = std::fs::read(directory.destination()).expect("test read failed");
            let encrypted = PrivateKey::from_openssh(encoded).expect("test parse failed");
            assert!(encrypted.is_encrypted());
            assert_eq!(encrypted.cipher(), Cipher::Aes256Gcm);
            assert!(
                matches!(encrypted.kdf(), Kdf::Bcrypt { salt, rounds } if salt.len() == 16 && *rounds == ROUNDS)
            );
        }
    }
}

fn main() {
    #[cfg(all(target_os = "linux", feature = "heap-probe"))]
    if let Some(code) = heap_probe::maybe_run() {
        std::process::exit(code);
    }
    #[cfg(target_os = "linux")]
    if let Err(error) = writer::run() {
        use std::io::Write;
        let _ = writeln!(std::io::stderr().lock(), "{}", error.message());
        std::process::exit(error.exit_code());
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("Experimental OpenSSH writer refused or failed.");
        std::process::exit(1);
    }
}
