use crate::linux_spawn::{SpawnOwner, SpawnSpec};
use md5::{Digest as MdDigest, Md5};
use sha2::{Digest as ShaDigest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const BINARY_LIMIT: u64 = 16 * 1024 * 1024;
const METADATA_LIMIT: usize = 1024 * 1024;
const SYSTEM_PATHS: [&str; 5] = [
    "/usr/bin/ssh",
    "/usr/bin/ssh-keygen",
    "/usr/bin/ssh-agent",
    "/usr/bin/ssh-add",
    "/usr/bin/dpkg-query",
];
const METADATA_PATHS: [&str; 4] = [
    "/etc/os-release",
    "/var/lib/dpkg/status",
    "/var/lib/dpkg/info/openssh-client.list",
    "/var/lib/dpkg/info/openssh-client.md5sums",
];
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn require(condition: bool) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Ubuntu provider was refused",
        ))
    }
}

fn ascii_metadata(data: &[u8], limit: usize) -> io::Result<&str> {
    require(data.len() <= limit && data.is_ascii())?;
    std::str::from_utf8(data).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

fn metadata_lines(data: &str) -> Vec<&str> {
    let bytes = data.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut position = 0;
    while position < bytes.len() {
        if matches!(bytes[position], b'\n' | b'\r' | 0x0b | 0x0c | 0x1c..=0x1e) {
            lines.push(&data[start..position]);
            if bytes[position] == b'\r' && bytes.get(position + 1) == Some(&b'\n') {
                position += 1;
            }
            start = position + 1;
        }
        position += 1;
    }
    if start < bytes.len() {
        lines.push(&data[start..]);
    }
    lines
}

fn parse_os_release(data: &[u8]) -> io::Result<()> {
    let mut values = BTreeMap::new();
    for line in metadata_lines(ascii_metadata(data, METADATA_LIMIT)?) {
        let (key, raw) = line.split_once('=').unwrap_or((line, ""));
        if !matches!(key, "ID" | "VERSION_ID") {
            continue;
        }
        let value = if raw.starts_with('"') {
            raw.strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or("")
        } else {
            raw
        };
        require(
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
        )?;
        require(values.insert(key, value).is_none())?;
    }
    require(values.get("ID") == Some(&"ubuntu") && values.get("VERSION_ID") == Some(&"24.04"))
}

fn parse_package(data: &[u8]) -> io::Result<()> {
    let text = ascii_metadata(data, 512)?;
    let fields: Vec<_> = text.strip_suffix('\n').unwrap_or("").split('\t').collect();
    require(
        cfg!(target_arch = "x86_64")
            && fields
                == [
                    "openssh-client",
                    "1:9.6p1-3ubuntu13.20",
                    "amd64",
                    "installed",
                ],
    )
}

fn parse_md5(data: &[u8]) -> io::Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for line in metadata_lines(ascii_metadata(data, METADATA_LIMIT)?) {
        let (checksum, path) = line.split_once("  ").unwrap_or(("", ""));
        require(
            checksum.len() == 32
                && checksum
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        )?;
        require(
            !path.is_empty()
                && !path.starts_with('/')
                && path
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_+./:@-".contains(&byte)),
        )?;
        require(path.split('/').all(|part| !matches!(part, "" | "." | "..")))?;
        require(
            values
                .insert(path.to_owned(), checksum.to_owned())
                .is_none(),
        )?;
    }
    require(!values.is_empty())?;
    Ok(values)
}

fn package_owners(data: &[u8]) -> io::Result<()> {
    let lines = metadata_lines(ascii_metadata(data, 2048)?);
    require(lines.len() == 4)?;
    for path in &SYSTEM_PATHS[..4] {
        let expected = format!("openssh-client: {path}");
        require(lines.iter().filter(|line| **line == expected).count() == 1)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Identity {
    fn from_metadata(value: &Metadata) -> Self {
        Self {
            device: value.dev(),
            inode: value.ino(),
            mode: value.mode(),
            uid: value.uid(),
            gid: value.gid(),
            links: value.nlink(),
            size: value.size(),
            modified: (value.mtime(), value.mtime_nsec()),
            changed: (value.ctime(), value.ctime_nsec()),
        }
    }

    fn validate(&self, executable: bool, allow_setgid: bool) -> io::Result<()> {
        require(
            self.mode & libc::S_IFMT == libc::S_IFREG && self.uid == 0 && self.mode & 0o022 == 0,
        )?;
        require(
            self.mode & libc::S_ISUID == 0 && (allow_setgid || self.mode & libc::S_ISGID == 0),
        )?;
        require(
            self.size > 0
                && self.size
                    <= if executable {
                        BINARY_LIMIT
                    } else {
                        METADATA_LIMIT as u64
                    },
        )?;
        require(!executable || self.mode & 0o111 != 0)
    }
}

fn check_directory_chain(path: &Path) -> io::Result<()> {
    require(path.is_absolute())?;
    let mut parents: Vec<_> = path.ancestors().skip(1).collect();
    parents.reverse();
    for parent in parents {
        let metadata = fs::symlink_metadata(parent)?;
        require(metadata.uid() == 0)?;
        if !metadata.file_type().is_symlink() {
            require(metadata.is_dir() && metadata.mode() & 0o022 == 0)?;
        }
    }
    Ok(())
}

struct RetainedFile {
    original: PathBuf,
    resolved: PathBuf,
    file: File,
    identity: Identity,
    sha256: [u8; 32],
    executable: bool,
    allow_setgid: bool,
}

impl RetainedFile {
    fn open(original: &Path, executable: bool, allow_setgid: bool) -> io::Result<Self> {
        check_directory_chain(original)?;
        require(fs::symlink_metadata(original)?.uid() == 0)?;
        let resolved = fs::canonicalize(original)?;
        check_directory_chain(&resolved)?;
        let identity = Identity::from_metadata(&fs::symlink_metadata(&resolved)?);
        identity.validate(executable, allow_setgid)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&resolved)?;
        require(
            Identity::from_metadata(&file.metadata()?) == identity
                && Identity::from_metadata(&fs::symlink_metadata(&resolved)?) == identity,
        )?;
        let mut retained = Self {
            original: original.to_owned(),
            resolved,
            file,
            identity,
            sha256: [0; 32],
            executable,
            allow_setgid,
        };
        if executable {
            let mut magic = [0; 4];
            retained.file.read_exact_at(&mut magic, 0)?;
            require(magic == *b"\x7fELF")?;
        }
        retained.sha256 = retained.sha256_digest()?;
        retained.revalidate()?;
        Ok(retained)
    }

    fn check_identity(&self) -> io::Result<()> {
        check_directory_chain(&self.original)?;
        require(
            fs::symlink_metadata(&self.original)?.uid() == 0
                && fs::canonicalize(&self.original)? == self.resolved,
        )?;
        check_directory_chain(&self.resolved)?;
        let current = Identity::from_metadata(&self.file.metadata()?);
        require(
            Identity::from_metadata(&fs::symlink_metadata(&self.resolved)?) == self.identity
                && current == self.identity,
        )?;
        current.validate(self.executable, self.allow_setgid)
    }

    fn stream(&self, mut consume: impl FnMut(&[u8])) -> io::Result<()> {
        let mut buffer = [0; 65536];
        let mut position = 0;
        while position < self.identity.size {
            let length = (self.identity.size - position).min(buffer.len() as u64) as usize;
            self.file.read_exact_at(&mut buffer[..length], position)?;
            consume(&buffer[..length]);
            position += length as u64;
        }
        let mut extra = [0];
        require(self.file.read_at(&mut extra, position)? == 0)
    }

    fn sha256_digest(&self) -> io::Result<[u8; 32]> {
        let mut digest = Sha256::new();
        self.stream(|chunk| ShaDigest::update(&mut digest, chunk))?;
        Ok(ShaDigest::finalize(digest).into())
    }

    fn md5_digest(&self) -> io::Result<String> {
        self.revalidate()?;
        let mut digest = Md5::new();
        self.stream(|chunk| MdDigest::update(&mut digest, chunk))?;
        self.revalidate()?;
        let bytes = MdDigest::finalize(digest);
        let alphabet = b"0123456789abcdef";
        let mut encoded = String::with_capacity(32);
        for byte in bytes {
            encoded.push(char::from(alphabet[(byte >> 4) as usize]));
            encoded.push(char::from(alphabet[(byte & 15) as usize]));
        }
        Ok(encoded)
    }

    fn revalidate(&self) -> io::Result<()> {
        self.check_identity()?;
        require(self.sha256_digest()? == self.sha256)?;
        self.check_identity()
    }

    fn read_metadata(&self) -> io::Result<Vec<u8>> {
        require(!self.executable)?;
        self.revalidate()?;
        let mut data = vec![0; self.identity.size as usize];
        self.file.read_exact_at(&mut data, 0)?;
        self.revalidate()?;
        Ok(data)
    }

    fn duplicate(&self) -> io::Result<OwnedFd> {
        self.revalidate()?;
        let descriptor = duplicate(self.file.as_raw_fd())?;
        self.revalidate()?;
        Ok(descriptor)
    }
}

pub struct UbuntuProvider {
    executables: [RetainedFile; 5],
    metadata: [RetainedFile; 4],
}

impl UbuntuProvider {
    pub fn select() -> io::Result<Self> {
        let provider = Self {
            executables: [
                RetainedFile::open(Path::new(SYSTEM_PATHS[0]), true, false)?,
                RetainedFile::open(Path::new(SYSTEM_PATHS[1]), true, false)?,
                RetainedFile::open(Path::new(SYSTEM_PATHS[2]), true, true)?,
                RetainedFile::open(Path::new(SYSTEM_PATHS[3]), true, false)?,
                RetainedFile::open(Path::new(SYSTEM_PATHS[4]), true, false)?,
            ],
            metadata: [
                RetainedFile::open(Path::new(METADATA_PATHS[0]), false, false)?,
                RetainedFile::open(Path::new(METADATA_PATHS[1]), false, false)?,
                RetainedFile::open(Path::new(METADATA_PATHS[2]), false, false)?,
                RetainedFile::open(Path::new(METADATA_PATHS[3]), false, false)?,
            ],
        };
        parse_os_release(&provider.metadata[0].read_metadata()?)?;
        let package = provider.query(
            &[
                "--admindir=/var/lib/dpkg",
                "-W",
                "-f=${Package}\t${Version}\t${Architecture}\t${db:Status-Status}\n",
                "openssh-client",
            ],
            512,
        )?;
        parse_package(&package)?;
        let owners = provider.query(
            &[
                "--admindir=/var/lib/dpkg",
                "-S",
                SYSTEM_PATHS[0],
                SYSTEM_PATHS[1],
                SYSTEM_PATHS[2],
                SYSTEM_PATHS[3],
            ],
            2048,
        )?;
        package_owners(&owners)?;
        let checksums = parse_md5(&provider.metadata[3].read_metadata()?)?;
        for (path, file) in SYSTEM_PATHS[..4].iter().zip(&provider.executables[..4]) {
            let checksum = file.md5_digest()?;
            require(checksums.get(path.trim_start_matches('/')) == Some(&checksum))?;
        }
        provider.revalidate()?;
        Ok(provider)
    }

    pub fn revalidate(&self) -> io::Result<()> {
        for file in self.executables.iter().chain(&self.metadata) {
            file.revalidate()?;
        }
        Ok(())
    }

    pub fn launch_executables(&self) -> io::Result<[OwnedFd; 2]> {
        self.revalidate()?;
        let agent = self.executables[2].duplicate()?;
        let loader = self.executables[3].duplicate()?;
        self.revalidate()?;
        Ok([agent, loader])
    }

    fn query(&self, arguments: &[&str], limit: usize) -> io::Result<Vec<u8>> {
        self.revalidate()?;
        let mut retained = Vec::with_capacity(8);
        for file in self.executables[..4].iter().chain(&self.metadata) {
            retained.push(file.duplicate()?);
        }
        let result = bounded_query(self.executables[4].duplicate()?, retained, arguments, limit);
        self.revalidate()?;
        result
    }
}

pub fn agent_spec(executable: &OwnedFd, socket: &Path, home: &Path) -> io::Result<SpawnSpec> {
    require(socket.is_absolute() && home.is_absolute())?;
    let null = || {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .map(OwnedFd::from)
    };
    Ok(SpawnSpec {
        executable: duplicate(executable.as_raw_fd())?,
        arg0: SYSTEM_PATHS[2].into(),
        arguments: vec!["-D".into(), "-a".into(), socket.as_os_str().to_owned()],
        environment: vec![
            ("HOME".into(), home.as_os_str().to_owned()),
            ("LANG".into(), "C".into()),
            ("LC_ALL".into(), "C".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ],
        stdio: [null()?, null()?, null()?],
        inherited: Vec::new(),
        retained: Vec::new(),
        new_session: true,
        controlling_tty: None,
        probe: None,
    })
}

fn duplicate(descriptor: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl creates a distinct descriptor with close-on-exec set.
    let result = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful call transferred a new descriptor to this scope.
    Ok(unsafe { OwnedFd::from_raw_fd(result) })
}

fn pipe() -> io::Result<(File, OwnedFd)> {
    let mut descriptors = [-1; 2];
    // SAFETY: descriptors points to two writable integers.
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe2 created both descriptors, each owned once here.
    let read = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    // SAFETY: this is the independent write descriptor returned by pipe2.
    let write = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    let read = duplicate(read.as_raw_fd())?;
    let write = duplicate(write.as_raw_fd())?;
    // SAFETY: F_GETFL queries the live descriptor; F_SETFL changes only its flags.
    let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((File::from(read), write))
}

fn drain(file: &mut File, output: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    let mut buffer = [0; 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                require(
                    output
                        .len()
                        .checked_add(count)
                        .is_some_and(|length| length <= limit),
                )?;
                output.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn bounded_query(
    executable: OwnedFd,
    retained: Vec<OwnedFd>,
    arguments: &[&str],
    limit: usize,
) -> io::Result<Vec<u8>> {
    let (mut stdout, stdout_write) = pipe()?;
    let (mut stderr, stderr_write) = pipe()?;
    let null = File::open("/dev/null")?;
    let mut owner = SpawnOwner::new()?;
    let result = (|| {
        let deadline = Instant::now() + QUERY_TIMEOUT;
        let receipt = owner.spawn(SpawnSpec {
            executable,
            arg0: OsString::from("/usr/bin/dpkg-query"),
            arguments: arguments
                .iter()
                .map(|argument| OsString::from(*argument))
                .collect(),
            environment: [
                ("LC_ALL", "C"),
                ("LANG", "C"),
                ("PATH", "/usr/bin:/bin"),
                ("HOME", "/nonexistent"),
            ]
            .into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect(),
            stdio: [duplicate(null.as_raw_fd())?, stdout_write, stderr_write],
            inherited: Vec::new(),
            retained,
            new_session: false,
            controlling_tty: None,
            probe: None,
        })?;
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let exit = loop {
            if Instant::now() >= deadline {
                return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
            }
            drain(&mut stdout, &mut output, limit)?;
            drain(&mut stderr, &mut errors, 512)?;
            require(errors.is_empty())?;
            if let Some(exit) = owner.try_wait(receipt.token)? {
                require(
                    exit.token == receipt.token
                        && exit.pid == receipt.pid
                        && exit.code == Some(0)
                        && exit.signal.is_none(),
                )?;
                break exit;
            }
            thread::sleep(Duration::from_millis(2));
        };
        require(owner.shutdown(SHUTDOWN_TIMEOUT)? == [exit])?;
        loop {
            let stdout_eof = drain(&mut stdout, &mut output, limit)?;
            let stderr_eof = drain(&mut stderr, &mut errors, 512)?;
            require(errors.is_empty())?;
            if stdout_eof && stderr_eof {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
            }
            thread::sleep(Duration::from_millis(2));
        }
        Ok(output)
    })();
    if result.is_err() {
        let _ = owner.shutdown(SHUTDOWN_TIMEOUT);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ubuntu-provider-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            check_directory_chain(&path.join("fixture")).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, data: &[u8], mode: u32) -> PathBuf {
            let path = self.0.join(name);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            std::io::Write::write_all(&mut file, data).unwrap();
            file.set_permissions(fs::Permissions::from_mode(mode))
                .unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn ubuntu_profile_accepts_the_observed_build_and_complete_metadata() {
        parse_os_release(b"NAME=\"Ubuntu\"\nID=ubuntu\nVERSION_ID=\"24.04\"\n").unwrap();
        parse_package(b"openssh-client\t1:9.6p1-3ubuntu13.20\tamd64\tinstalled\n").unwrap();
        let sums = parse_md5(b"900150983cd24fb0d6963f7d28e17f72  usr/bin/ssh\nd41d8cd98f00b204e9800998ecf8427e  usr/share/example\n").unwrap();
        assert_eq!(sums.len(), 2);
        assert_eq!(
            sums.get("usr/bin/ssh").unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        package_owners(b"openssh-client: /usr/bin/ssh-add\nopenssh-client: /usr/bin/ssh\nopenssh-client: /usr/bin/ssh-agent\nopenssh-client: /usr/bin/ssh-keygen\n").unwrap();
    }

    #[test]
    fn ubuntu_profile_bounds_every_metadata_receipt() {
        assert!(parse_os_release(&vec![b'a'; METADATA_LIMIT + 1]).is_err());
        assert!(parse_md5(&vec![b'a'; METADATA_LIMIT + 1]).is_err());
        assert!(parse_package(&vec![b'a'; 513]).is_err());
        assert!(package_owners(&vec![b'a'; 2049]).is_err());
    }

    #[test]
    #[ignore = "needs root: the fixture must sit under a root-owned chain; CI runs it with sudo"]
    fn retained_metadata_reads_and_hashes_the_owned_object() {
        let fixture = Fixture::new();
        let path = fixture.file("metadata", b"abc", 0o644);
        let retained = RetainedFile::open(&path, false, false).unwrap();
        assert_eq!(retained.read_metadata().unwrap(), b"abc");
        assert_eq!(
            retained.md5_digest().unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        let duplicate = retained.duplicate().unwrap();
        assert!(duplicate.as_raw_fd() >= 3);
        // SAFETY: F_GETFD only queries this live, owned descriptor.
        assert_ne!(
            unsafe { libc::fcntl(duplicate.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }

    #[test]
    #[ignore = "needs root: the fixture must sit under a root-owned chain; CI runs it with sudo"]
    fn retained_executable_streams_beyond_one_buffer_and_checks_elf() {
        let fixture = Fixture::new();
        let mut bytes = vec![b'x'; 2 * 65536 + 17];
        bytes[..4].copy_from_slice(b"\x7fELF");
        let path = fixture.file("elf", &bytes, 0o755);
        let retained = RetainedFile::open(&path, true, false).unwrap();
        let expected: [u8; 32] = Sha256::digest(&bytes).into();
        assert_eq!(retained.sha256, expected);
        assert!(retained.read_metadata().is_err());
        let not_elf = fixture.file("not-elf", b"plain executable", 0o755);
        assert!(RetainedFile::open(&not_elf, true, false).is_err());
    }

    #[test]
    #[ignore = "needs root: the fixture must sit under a root-owned chain; CI runs it with sudo"]
    fn retained_file_refuses_unsafe_permissions_and_privilege_bits() {
        let fixture = Fixture::new();
        for (index, mode) in [0o777, 0o775, 0o4755, 0o2755, 0o644]
            .into_iter()
            .enumerate()
        {
            let path = fixture.file(&format!("unsafe-{index}"), b"\x7fELFfixture", mode);
            assert!(RetainedFile::open(&path, true, false).is_err());
        }
        let agent = fixture.file("agent", b"\x7fELFfixture", 0o2755);
        RetainedFile::open(&agent, true, true).unwrap();
        let retained = RetainedFile::open(&agent, true, true).unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o2777)).unwrap();
        assert!(retained.revalidate().is_err());
    }

    #[test]
    #[ignore = "needs root: the fixture must sit under a root-owned chain; CI runs it with sudo"]
    fn retained_file_refuses_wrong_owner_and_size_profiles() {
        let mut identity = Identity {
            device: 1,
            inode: 1,
            mode: libc::S_IFREG | 0o755,
            uid: 0,
            gid: 0,
            links: 1,
            size: 4,
            modified: (0, 0),
            changed: (0, 0),
        };
        identity.validate(true, false).unwrap();
        identity.uid = 1;
        assert!(identity.validate(true, false).is_err());
        identity.uid = 0;
        for size in [0, BINARY_LIMIT + 1] {
            identity.size = size;
            assert!(identity.validate(true, false).is_err());
        }
        identity.size = METADATA_LIMIT as u64 + 1;
        assert!(identity.validate(false, false).is_err());
        let fixture = Fixture::new();
        assert!(RetainedFile::open(&fixture.0, false, false).is_err());
        let empty = fixture.file("empty", b"", 0o644);
        assert!(RetainedFile::open(&empty, false, false).is_err());
    }

    #[test]
    #[ignore = "needs root: the fixture must sit under a root-owned chain; CI runs it with sudo"]
    fn retained_file_detects_content_and_object_replacement() {
        let fixture = Fixture::new();
        let path = fixture.file("metadata", b"abc", 0o644);
        let retained = RetainedFile::open(&path, false, false).unwrap();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all_at(b"xyz", 0).unwrap();
        assert!(retained.revalidate().is_err());
        let path = fixture.file("original", b"abc", 0o644);
        let retained = RetainedFile::open(&path, false, false).unwrap();
        let replacement = fixture.file("replacement", b"abc", 0o644);
        fs::rename(replacement, path).unwrap();
        assert!(retained.revalidate().is_err());
    }

    #[test]
    #[ignore = "needs root: the fixture must sit under a root-owned chain; CI runs it with sudo"]
    fn retained_file_accepts_root_alias_and_refuses_retarget_and_unsafe_chain() {
        let fixture = Fixture::new();
        let first = fixture.file("first", b"abc", 0o644);
        let second = fixture.file("second", b"abc", 0o644);
        let alias = fixture.0.join("alias");
        symlink(&first, &alias).unwrap();
        let retained = RetainedFile::open(&alias, false, false).unwrap();
        retained.revalidate().unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&second, &alias).unwrap();
        assert!(retained.revalidate().is_err());
        let retained = RetainedFile::open(&first, false, false).unwrap();
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(retained.revalidate().is_err());
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn ubuntu_profile_refuses_duplicate_and_wrong_os_identity() {
        for data in [
            &b"ID=ubuntu\nID=ubuntu\nVERSION_ID=24.04\n"[..],
            &b"ID=ubuntu\nVERSION_ID=22.04\n"[..],
            &b"ID=debian\nVERSION_ID=24.04\n"[..],
            &b"ID='ubuntu'\nVERSION_ID=24.04\n"[..],
            &b"ID=ubuntu\nVERSION_ID=24.04\nOTHER=\xff\n"[..],
        ] {
            assert!(parse_os_release(data).is_err());
        }
    }

    #[test]
    fn ubuntu_profile_refuses_unknown_package_and_malformed_receipts() {
        for data in [
            &b"openssh-client\t1:9.6p1-3ubuntu13.19\tamd64\tinstalled\n"[..],
            &b"openssh-client\t1:9.6p1-3ubuntu13.20\tarm64\tinstalled\n"[..],
            &b"openssh-client\t1:9.6p1-3ubuntu13.20\tamd64\tunpacked\n"[..],
            &b"openssh-client\t1:9.6p1-3ubuntu13.20\tamd64\tinstalled\nextra\n"[..],
        ] {
            assert!(parse_package(data).is_err());
        }
    }

    #[test]
    fn ubuntu_profile_refuses_checksum_duplicates_traversal_and_encoding() {
        for data in [
            &b"00000000000000000000000000000000  usr/bin/ssh\n00000000000000000000000000000000  usr/bin/ssh\n"[..],
            &b"00000000000000000000000000000000  usr/../bin/ssh\n"[..],
            &b"00000000000000000000000000000000  /usr/bin/ssh\n"[..],
            &b"00000000000000000000000000000000  usr//bin/ssh\n"[..],
            &b"00000000000000000000000000000000  usr/bin/\xff\n"[..],
            &b"00000000000000000000000000000000 usr/bin/ssh\n"[..],
            &b""[..],
        ] {
            assert!(parse_md5(data).is_err());
        }
    }

    #[test]
    fn ubuntu_profile_refuses_wrong_or_duplicate_package_ownership() {
        for data in [
            &b"other: /usr/bin/ssh\nopenssh-client: /usr/bin/ssh-keygen\nopenssh-client: /usr/bin/ssh-agent\nopenssh-client: /usr/bin/ssh-add\n"[..],
            &b"openssh-client: /usr/bin/ssh\nopenssh-client: /usr/bin/ssh\nopenssh-client: /usr/bin/ssh-agent\nopenssh-client: /usr/bin/ssh-add\n"[..],
        ] {
            assert!(package_owners(data).is_err());
        }
    }
}
