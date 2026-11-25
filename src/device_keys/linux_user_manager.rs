use super::linux_resident_reader::identifier;
use std::ffi::{CStr, CString, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LINGER_DIRECTORY: &str = "/var/lib/systemd/linger";
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const START_DEADLINE: Duration = Duration::from_secs(20);
const STOP_DEADLINE: Duration = Duration::from_secs(45);
const IDLE_VARIABLE: &str = "VIVAC_LINUX_SESSION_IDLE_SECONDS";
const MAXIMUM_VARIABLE: &str = "VIVAC_LINUX_SESSION_MAXIMUM_SECONDS";

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

#[derive(Clone, Debug)]
pub(crate) struct Account {
    pub uid: u32,
    pub name: CString,
}

/// The account of the effective user, as the password database names it.
pub(crate) fn current_account() -> io::Result<Account> {
    // SAFETY: `geteuid` and `getuid` only read this process's credentials.
    let (uid, real) = unsafe { (libc::geteuid(), libc::getuid()) };
    if uid != real {
        return Err(invalid());
    }
    let mut buffer = vec![0_u8; 1024];
    loop {
        // SAFETY: an all-zero `passwd` is a valid out-parameter for `getpwuid_r`.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: `entry`, `buffer` and `found` are live for the call, and the
        // buffer length is passed unchanged.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status));
        }
        if found.is_null() || entry.pw_uid != uid || entry.pw_name.is_null() {
            return Err(invalid());
        }
        // SAFETY: a successful `getpwuid_r` points `pw_name` at a NUL-terminated
        // string inside the buffer, which is still live.
        let name = unsafe { CStr::from_ptr(entry.pw_name) }.to_owned();
        if name.as_bytes().is_empty() || name.as_bytes().contains(&b'/') {
            return Err(invalid());
        }
        return Ok(Account { uid, name });
    }
}

pub(crate) fn linger_enabled(account: &Account) -> io::Result<bool> {
    linger_enabled_in(Path::new(LINGER_DIRECTORY), account)
}

fn linger_enabled_in(directory: &Path, account: &Account) -> io::Result<bool> {
    let name = std::ffi::OsStr::from_bytes(account.name.as_bytes());
    match std::fs::symlink_metadata(directory.join(name)) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
pub(crate) enum ManagerRefusal {
    LingerDisabled(Account),
    Unsupported,
}

pub(crate) struct UserManager {
    runtime: PathBuf,
}

fn runtime_directory_in(path: &Path, uid: u32) -> Result<PathBuf, ManagerRefusal> {
    let owned_directory = |path: &Path, mode: Option<u32>| {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_dir()
                && metadata.uid() == uid
                && mode.is_none_or(|mode| metadata.permissions().mode() & 0o7777 == mode)
        })
    };
    if !owned_directory(path, Some(0o700)) || !owned_directory(&path.join("systemd"), None) {
        return Err(ManagerRefusal::Unsupported);
    }
    let private = std::fs::symlink_metadata(path.join("systemd/private"))
        .map_err(|_| ManagerRefusal::Unsupported)?;
    if !private.file_type().is_socket() {
        return Err(ManagerRefusal::Unsupported);
    }
    Ok(path.to_path_buf())
}

/// Whether reaching the user manager requires lingering. A session starts
/// only where lingering keeps its manager running; a session that is already
/// open is closed through whatever manager runs it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Linger {
    Required,
    NotRequired,
}

impl UserManager {
    /// The manager that starts a session: lingering must be enabled.
    pub(crate) fn open() -> Result<Self, ManagerRefusal> {
        Self::open_in(
            Path::new(LINGER_DIRECTORY),
            Path::new("/run/user"),
            Linger::Required,
        )
    }

    /// The manager that runs an open session, for closing it. Every check
    /// but lingering applies.
    pub(crate) fn open_running() -> Result<Self, ManagerRefusal> {
        Self::open_in(
            Path::new(LINGER_DIRECTORY),
            Path::new("/run/user"),
            Linger::NotRequired,
        )
    }

    fn open_in(
        linger: &Path,
        runtime_root: &Path,
        linger_required: Linger,
    ) -> Result<Self, ManagerRefusal> {
        let account = current_account().map_err(|_| ManagerRefusal::Unsupported)?;
        if linger_required == Linger::Required {
            match linger_enabled_in(linger, &account) {
                Ok(true) => {}
                Ok(false) => return Err(ManagerRefusal::LingerDisabled(account)),
                Err(_) => return Err(ManagerRefusal::Unsupported),
            }
        }
        let runtime =
            runtime_directory_in(&runtime_root.join(account.uid.to_string()), account.uid)?;
        Ok(Self { runtime })
    }

    pub(crate) fn runtime_directory(&self) -> &Path {
        &self.runtime
    }

    pub(crate) fn start_session_unit(
        &self,
        unit: &SessionUnit,
        keeper: &KeeperInvocation,
    ) -> io::Result<()> {
        self.run(
            SYSTEMD_RUN,
            systemd_run_arguments(unit, keeper),
            START_DEADLINE,
        )
    }

    pub(crate) fn stop_unit(&self, unit: &SessionUnit) -> io::Result<()> {
        self.run(SYSTEMCTL, systemctl_stop_arguments(unit), STOP_DEADLINE)
    }

    fn run(&self, program: &str, arguments: Vec<OsString>, deadline: Duration) -> io::Result<()> {
        trusted_program(Path::new(program))?;
        let mut child = Command::new(program)
            .args(arguments)
            .env_clear()
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let limit = Instant::now() + deadline;
        loop {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other("the service manager refused the request"))
                };
            }
            if Instant::now() >= limit {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Refuses a program that is not a root-owned regular file nobody else can write.
fn trusted_program(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_file()
        && metadata.uid() == 0
        && metadata.permissions().mode() & 0o022 == 0
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "the service manager program is not a trusted file",
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionUnit {
    credential: String,
    generation: String,
}

impl SessionUnit {
    pub(crate) fn new(credential: &str) -> io::Result<Self> {
        let mut bytes = [0_u8; 16];
        getrandom::getrandom(&mut bytes).map_err(|_| io::Error::other("random bytes"))?;
        Self::from_parts(
            credential,
            &super::linux_custody_anchor::encode_identifier(bytes),
        )
    }

    pub(crate) fn from_parts(credential: &str, generation: &str) -> io::Result<Self> {
        if !identifier(credential) || !identifier(generation) {
            return Err(invalid());
        }
        Ok(Self {
            credential: credential.to_owned(),
            generation: generation.to_owned(),
        })
    }

    pub(crate) fn name(&self) -> String {
        format!(
            "vivac-custody-{}-{}.service",
            self.credential, self.generation
        )
    }

    pub(crate) fn runtime_name(&self) -> String {
        format!("vivac-custody-{}", self.generation)
    }

    pub(crate) fn generation(&self) -> &str {
        &self.generation
    }

    pub(crate) fn credential(&self) -> &str {
        &self.credential
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SessionLimits {
    pub idle: Duration,
    pub maximum: Duration,
}

impl SessionLimits {
    pub(crate) const DEFAULT: Self = Self {
        idle: Duration::from_secs(43_200),
        maximum: Duration::from_secs(604_800),
    };

    pub(crate) fn from_environment() -> Result<Self, ()> {
        let read = |name: &str| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(()),
        };
        let idle = read(IDLE_VARIABLE)?;
        let maximum = read(MAXIMUM_VARIABLE)?;
        Self::from_values(idle.as_deref(), maximum.as_deref())
    }

    /// Each limit may only shorten its default, to no less than 60 seconds.
    pub(crate) fn from_values(idle: Option<&str>, maximum: Option<&str>) -> Result<Self, ()> {
        let shorten = |value: Option<&str>, ceiling: Duration| match value {
            None => Ok(ceiling),
            Some(text) => {
                let canonical = !text.is_empty()
                    && text.len() <= 10
                    && text.bytes().all(|byte| byte.is_ascii_digit())
                    && !text.starts_with('0');
                let seconds = text.parse::<u64>().map_err(|_| ())?;
                if canonical && (60..=ceiling.as_secs()).contains(&seconds) {
                    Ok(Duration::from_secs(seconds))
                } else {
                    Err(())
                }
            }
        };
        Ok(Self {
            idle: shorten(idle, Self::DEFAULT.idle)?,
            maximum: shorten(maximum, Self::DEFAULT.maximum)?,
        })
    }
}

pub(crate) struct KeeperInvocation {
    pub executable: PathBuf,
    pub identity: (u64, u64),
    pub limits: SessionLimits,
}

impl KeeperInvocation {
    pub(crate) fn current(limits: SessionLimits) -> io::Result<Self> {
        let executable = std::fs::read_link("/proc/self/exe")?;
        if !executable.is_absolute() || executable.as_os_str().as_bytes().ends_with(b" (deleted)") {
            return Err(invalid());
        }
        let by_path = std::fs::metadata(&executable)?;
        let by_descriptor = std::fs::File::open("/proc/self/exe")?.metadata()?;
        let identity = (by_descriptor.dev(), by_descriptor.ino());
        if (by_path.dev(), by_path.ino()) != identity {
            return Err(invalid());
        }
        Ok(Self {
            executable,
            identity,
            limits,
        })
    }
}

fn systemd_run_arguments(unit: &SessionUnit, keeper: &KeeperInvocation) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "--user",
        "--quiet",
        "--no-ask-password",
        "--collect",
        "--expand-environment=no",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    arguments.push(format!("--unit={}", unit.name()).into());
    arguments.extend(
        [
            "--service-type=notify",
            "--property=NotifyAccess=main",
            "--property=Delegate=yes",
            "--property=KillMode=mixed",
            "--property=TimeoutStartSec=15s",
            "--property=TimeoutStopSec=30s",
        ]
        .iter()
        .map(OsString::from),
    );
    arguments.push(format!("--property=RuntimeDirectory={}", unit.runtime_name()).into());
    arguments.extend(
        [
            "--property=RuntimeDirectoryMode=0700",
            "--property=Restart=no",
            "--",
        ]
        .iter()
        .map(OsString::from),
    );
    arguments.push(keeper.executable.clone().into_os_string());
    arguments.push("--internal-linux-session-keeper".into());
    arguments.push(unit.credential().into());
    arguments.push(unit.generation().into());
    arguments.push(format!("{}:{}", keeper.identity.0, keeper.identity.1).into());
    arguments.push(keeper.limits.idle.as_secs().to_string().into());
    arguments.push(keeper.limits.maximum.as_secs().to_string().into());
    arguments
}

fn systemctl_stop_arguments(unit: &SessionUnit) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = ["--user", "--quiet", "--no-ask-password", "stop", "--"]
        .iter()
        .map(OsString::from)
        .collect();
    arguments.push(unit.name().into());
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CString, OsString};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};

    const CREDENTIAL: &str = "0123456789abcdefghjkmnpqrs";
    const GENERATION: &str = "1123456789abcdefghjkmnpqrt";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("vivac-um-{}-{serial}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn mode(path: &Path, bits: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits)).unwrap();
    }

    fn own_uid() -> u32 {
        // SAFETY: `geteuid` only reads this process's credentials.
        unsafe { libc::geteuid() }
    }

    fn os(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn systemd_run_arguments_pin_the_session_unit_properties() {
        let unit = SessionUnit::from_parts(CREDENTIAL, GENERATION).unwrap();
        let keeper = KeeperInvocation {
            executable: PathBuf::from("/usr/bin/vivac"),
            identity: (7, 9),
            limits: SessionLimits::DEFAULT,
        };
        let name = format!("--unit=vivac-custody-{CREDENTIAL}-{GENERATION}.service");
        let runtime = format!("--property=RuntimeDirectory=vivac-custody-{GENERATION}");
        let expected = os(&[
            "--user",
            "--quiet",
            "--no-ask-password",
            "--collect",
            "--expand-environment=no",
            &name,
            "--service-type=notify",
            "--property=NotifyAccess=main",
            "--property=Delegate=yes",
            "--property=KillMode=mixed",
            "--property=TimeoutStartSec=15s",
            "--property=TimeoutStopSec=30s",
            &runtime,
            "--property=RuntimeDirectoryMode=0700",
            "--property=Restart=no",
            "--",
            "/usr/bin/vivac",
            "--internal-linux-session-keeper",
            CREDENTIAL,
            GENERATION,
            "7:9",
            "43200",
            "604800",
        ]);
        assert_eq!(systemd_run_arguments(&unit, &keeper), expected);
        assert_eq!(
            systemctl_stop_arguments(&unit),
            os(&[
                "--user",
                "--quiet",
                "--no-ask-password",
                "stop",
                "--",
                &unit.name()
            ])
        );
    }

    #[test]
    fn session_unit_names_use_two_canonical_identifiers() {
        let unit = SessionUnit::from_parts(CREDENTIAL, GENERATION).unwrap();
        assert_eq!(
            unit.name(),
            format!("vivac-custody-{CREDENTIAL}-{GENERATION}.service")
        );
        assert_eq!(unit.runtime_name(), format!("vivac-custody-{GENERATION}"));
        assert_eq!(unit.credential(), CREDENTIAL);
        assert_eq!(unit.generation(), GENERATION);
        assert!(SessionUnit::from_parts("8123456789abcdefghjkmnpqrs", GENERATION).is_err());
        assert!(SessionUnit::from_parts(CREDENTIAL, "8123456789abcdefghjkmnpqrs").is_err());
        assert!(SessionUnit::from_parts("0123456789ABCDEFGHJKMNPQRS", GENERATION).is_err());
        assert!(SessionUnit::from_parts(CREDENTIAL, "0123456789abcdefghjkmnpqr").is_err());
        let fresh = SessionUnit::new(CREDENTIAL).unwrap();
        assert!(crate::linux_resident_reader::identifier(fresh.generation()));
    }

    #[test]
    fn session_limits_accept_only_shortened_values() {
        let parse = SessionLimits::from_values;
        assert_eq!(parse(None, None), Ok(SessionLimits::DEFAULT));
        assert_eq!(
            parse(Some("60"), None),
            Ok(SessionLimits {
                idle: std::time::Duration::from_secs(60),
                maximum: SessionLimits::DEFAULT.maximum,
            })
        );
        assert_eq!(parse(None, Some("604800")), Ok(SessionLimits::DEFAULT));
        for bad in ["59", "43201", "060", "", "+60", "6 0", "-1"] {
            assert_eq!(parse(Some(bad), None), Err(()), "{bad:?}");
        }
        assert_eq!(parse(None, Some("604801")), Err(()));
        assert_eq!(SessionLimits::DEFAULT.idle.as_secs(), 43_200);
        assert_eq!(SessionLimits::DEFAULT.maximum.as_secs(), 604_800);
    }

    #[test]
    fn session_paths_fit_unix_socket_limit_for_largest_uid() {
        let unit = SessionUnit::from_parts(CREDENTIAL, GENERATION).unwrap();
        let root = format!("/run/user/4294967294/{}", unit.runtime_name());
        for tail in [
            "session/agent.sock",
            "startup/endpoint.sock",
            "control.sock",
        ] {
            let path = format!("{root}/{tail}");
            assert!(path.len() < 108, "{path}");
        }
    }

    #[test]
    fn runtime_directory_requires_the_account_owner_and_mode() {
        let scratch = Scratch::new();
        let runtime = scratch.0.join("run");
        let refused = |uid: u32| {
            matches!(
                runtime_directory_in(&runtime, uid),
                Err(ManagerRefusal::Unsupported)
            )
        };
        assert!(refused(own_uid()));
        std::fs::create_dir(&runtime).unwrap();
        mode(&runtime, 0o755);
        assert!(refused(own_uid()));
        mode(&runtime, 0o700);
        assert!(refused(own_uid()));
        std::fs::create_dir(runtime.join("systemd")).unwrap();
        std::fs::write(runtime.join("systemd/private"), b"").unwrap();
        assert!(refused(own_uid()));
        std::fs::remove_file(runtime.join("systemd/private")).unwrap();
        let _listener = UnixListener::bind(runtime.join("systemd/private")).unwrap();
        assert_eq!(
            runtime_directory_in(&runtime, own_uid()).ok(),
            Some(runtime.clone())
        );
        assert!(refused(own_uid().wrapping_add(1)));
    }

    #[test]
    fn only_a_start_requires_linger() {
        let scratch = Scratch::new();
        let linger = scratch.0.join("linger");
        std::fs::create_dir(&linger).unwrap();
        let root = scratch.0.join("user");
        let runtime = root.join(own_uid().to_string());
        std::fs::create_dir_all(runtime.join("systemd")).unwrap();
        mode(&runtime, 0o700);
        let _listener = UnixListener::bind(runtime.join("systemd/private")).unwrap();
        assert!(matches!(
            UserManager::open_in(&linger, &root, Linger::Required),
            Err(ManagerRefusal::LingerDisabled(_))
        ));
        let running = UserManager::open_in(&linger, &root, Linger::NotRequired).unwrap();
        assert_eq!(running.runtime_directory(), runtime);
        // Every other check still applies without lingering.
        mode(&runtime, 0o750);
        assert!(matches!(
            UserManager::open_in(&linger, &root, Linger::NotRequired),
            Err(ManagerRefusal::Unsupported)
        ));
    }

    #[test]
    fn linger_state_reads_only_the_named_regular_file() {
        let scratch = Scratch::new();
        let account = Account {
            uid: own_uid(),
            name: CString::new("someone").unwrap(),
        };
        assert!(!linger_enabled_in(&scratch.0, &account).unwrap());
        std::fs::write(scratch.0.join("someone"), b"").unwrap();
        assert!(linger_enabled_in(&scratch.0, &account).unwrap());
        std::fs::remove_file(scratch.0.join("someone")).unwrap();
        std::fs::create_dir(scratch.0.join("someone")).unwrap();
        assert!(linger_enabled_in(&scratch.0, &account).is_err());
    }

    #[test]
    fn service_manager_programs_must_be_root_owned_regular_files() {
        let scratch = Scratch::new();
        assert!(trusted_program(Path::new("/usr/bin/env")).is_ok());
        assert!(trusted_program(&scratch.0.join("missing")).is_err());
        assert!(trusted_program(&scratch.0).is_err());
        let file = scratch.0.join("program");
        std::fs::write(&file, b"").unwrap();
        mode(&file, 0o755);
        // Without root, the file already belongs to someone else. With root,
        // the owner changes if the sandbox allows it.
        if own_uid() != 0 || std::os::unix::fs::chown(&file, Some(65534), None).is_ok() {
            assert!(trusted_program(&file).is_err());
        }
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink("/usr/bin/env", &link).unwrap();
        assert!(trusted_program(&link).is_err());
        if own_uid() == 0 {
            let _ = std::os::unix::fs::chown(&file, Some(0), None);
            mode(&file, 0o755);
            assert!(trusted_program(&file).is_ok());
            for bits in [0o777, 0o775, 0o757] {
                mode(&file, bits);
                assert!(trusted_program(&file).is_err(), "{bits:o}");
            }
        }
    }
}
