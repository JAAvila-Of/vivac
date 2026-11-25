use super::linux_spawn::{ExitReceipt, Receipt, SpawnOwner, SpawnSpec};
use std::fs::File;
use std::io::{self, Read};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const MAX_OUTPUT: usize = 32 * 1024;
const MAX_STATUS: usize = 16 * 1024;
const MAX_CIPHERTEXT: u64 = 65536;
const REQUIRED_SEALS: libc::c_int =
    libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
// Linux's atomic slave-open ioctl. The admitted fixture is Ubuntu 24.04 amd64.
const TIOCGPTPEER: libc::c_ulong = 0x5441;

/// A private native loader. Provider/package admission remains the caller's
/// responsibility. The owner retains a master and ciphertext duplicate even
/// when this object's local resources are dropped before native exit.
pub struct PtyLoader {
    receipt: Receipt,
    master: OwnedFd,
    executable_device: u64,
    executable_inode: u64,
    parent: libc::pid_t,
    uid: libc::uid_t,
    gid: libc::gid_t,
    output: Zeroizing<Vec<u8>>,
    prompt_seen: bool,
    handoff_started: bool,
    handed_off: bool,
    /// The length of `output` when the secret was delivered.
    handoff_output: usize,
}

impl PtyLoader {
    pub fn start(
        owner: &mut SpawnOwner,
        executable: OwnedFd,
        cipher: OwnedFd,
        socket: &Path,
        home: &Path,
    ) -> io::Result<Self> {
        Self::start_with(owner, executable, cipher, socket, home, SpawnOwner::spawn)
    }

    pub(crate) fn start_task_restricted(
        owner: &mut SpawnOwner,
        executable: OwnedFd,
        cipher: OwnedFd,
        socket: &Path,
        home: &Path,
    ) -> io::Result<Self> {
        Self::start_with(
            owner,
            executable,
            cipher,
            socket,
            home,
            SpawnOwner::spawn_task_restricted,
        )
    }

    fn start_with(
        owner: &mut SpawnOwner,
        executable: OwnedFd,
        cipher: OwnedFd,
        socket: &Path,
        home: &Path,
        spawn: impl FnOnce(&mut SpawnOwner, SpawnSpec) -> io::Result<Receipt>,
    ) -> io::Result<Self> {
        if !socket.is_absolute() || !home.is_absolute() {
            return Err(refused());
        }
        let executable_copy = duplicate(executable.as_raw_fd())?;
        drop(executable);
        let executable_metadata = File::from(executable_copy.try_clone()?).metadata()?;
        let cipher_copy = duplicate(cipher.as_raw_fd())?;
        drop(cipher);
        let cipher_metadata = File::from(cipher_copy.try_clone()?).metadata()?;
        // SAFETY: F_GET_SEALS only queries the retained descriptor.
        let seals = unsafe { libc::fcntl(cipher_copy.as_raw_fd(), libc::F_GET_SEALS) };
        if seals < 0 {
            return Err(io::Error::last_os_error());
        }
        validate_cipher_shape(cipher_metadata.is_file(), cipher_metadata.len(), seals)?;
        let (master, slave) = open_private_pty()?;
        let inherited_path = format!("/proc/self/fd/{}", cipher_copy.as_raw_fd());
        let spec = SpawnSpec {
            executable: executable_copy,
            arg0: "/usr/bin/ssh-add".into(),
            arguments: vec!["-k".into(), inherited_path.into()],
            environment: vec![
                ("HOME".into(), home.as_os_str().to_owned()),
                ("LANG".into(), "C".into()),
                ("LC_ALL".into(), "C".into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("SSH_AUTH_SOCK".into(), socket.as_os_str().to_owned()),
            ],
            stdio: [
                duplicate(slave.as_raw_fd())?,
                duplicate(slave.as_raw_fd())?,
                duplicate(slave.as_raw_fd())?,
            ],
            inherited: vec![cipher_copy],
            retained: vec![duplicate(master.as_raw_fd())?],
            new_session: true,
            controlling_tty: Some(slave),
            probe: None,
        };
        let receipt = spawn(owner, spec)?;
        Ok(Self {
            receipt,
            master,
            executable_device: executable_metadata.dev(),
            executable_inode: executable_metadata.ino(),
            // SAFETY: these calls only query this process's current identity.
            parent: unsafe { libc::getpid() },
            // SAFETY: effective credential queries have no pointer arguments.
            uid: unsafe { libc::geteuid() },
            // SAFETY: effective credential queries have no pointer arguments.
            gid: unsafe { libc::getegid() },
            output: Zeroizing::new(Vec::with_capacity(1024)),
            prompt_seen: false,
            handoff_started: false,
            handed_off: false,
            handoff_output: 0,
        })
    }

    pub fn receipt(&self) -> Receipt {
        self.receipt
    }

    pub fn token(&self) -> u64 {
        self.receipt.token
    }

    pub fn prompt_seen(&self) -> bool {
        self.prompt_seen
    }

    pub fn handed_off(&self) -> bool {
        self.handed_off
    }

    pub fn duplicate_master(&self) -> io::Result<OwnedFd> {
        duplicate(self.master.as_raw_fd())
    }

    pub fn wait_prompt(&mut self, owner: &mut SpawnOwner, timeout: Duration) -> io::Result<()> {
        if self.prompt_seen || self.handoff_started {
            return Err(refused());
        }
        let deadline = deadline(timeout)?;
        loop {
            require_before(deadline)?;
            self.require_live(owner)?;
            self.drain_output(deadline)?;
            if prompt_complete(&self.output) {
                self.validate_native(owner)?;
                self.validate_terminal()?;
                require_before(deadline)?;
                self.prompt_seen = true;
                return Ok(());
            }
            poll_tick(self.master.as_raw_fd(), libc::POLLIN, deadline)?;
        }
    }

    pub fn handoff_stopped(
        &mut self,
        owner: &mut SpawnOwner,
        secret: &[u8],
        timeout: Duration,
    ) -> io::Result<()> {
        validate_secret(secret)?;
        if !self.prompt_seen || self.handoff_started {
            return Err(refused());
        }
        let deadline = deadline(timeout)?;
        self.validate_native(owner)?;
        self.validate_terminal()?;
        // A partial delivery can never be retried as another handoff.
        require_before(deadline)?;
        self.handoff_started = true;
        owner.signal(self.token(), libc::SIGSTOP)?;
        let pidfd = owner.duplicate_pidfd(self.token())?;
        observe_stopped(pidfd.as_raw_fd(), self.receipt.pid, deadline)?;
        self.validate_native(owner)?;
        self.validate_terminal()?;
        let mut payload = Zeroizing::new(Vec::with_capacity(secret.len() + 1));
        payload.extend_from_slice(secret);
        payload.push(b'\n');
        let mut position = 0;
        while position < payload.len() {
            self.require_live(owner)?;
            self.validate_terminal()?;
            poll_tick(self.master.as_raw_fd(), libc::POLLOUT, deadline)?;
            require_before(deadline)?;
            // SAFETY: this private master and remaining payload are live.
            let written = unsafe {
                libc::write(
                    self.master.as_raw_fd(),
                    payload[position..].as_ptr().cast(),
                    payload.len() - position,
                )
            };
            if written > 0 {
                position += written as usize;
            } else if written < 0 && matches!(errno(), libc::EINTR | libc::EAGAIN) {
                continue;
            } else {
                return Err(if written < 0 {
                    io::Error::last_os_error()
                } else {
                    refused()
                });
            }
        }
        self.require_live(owner)?;
        require_before(deadline)?;
        self.handoff_output = self.output.len();
        self.handed_off = true;
        Ok(())
    }

    pub fn complete(
        &mut self,
        owner: &mut SpawnOwner,
        timeout: Duration,
    ) -> io::Result<ExitReceipt> {
        if !self.handed_off {
            return Err(refused());
        }
        let deadline = deadline(timeout)?;
        self.validate_native(owner)?;
        self.validate_terminal()?;
        require_before(deadline)?;
        owner.signal(self.token(), libc::SIGCONT)?;
        loop {
            require_before(deadline)?;
            self.drain_output(deadline)?;
            if passphrase_refused(&self.output[self.handoff_output..]) {
                return Err(refused());
            }
            if let Some(exit) = owner.try_wait(self.token())? {
                return if exit.code == Some(0) && exit.signal.is_none() {
                    Ok(exit)
                } else {
                    Err(refused())
                };
            }
            poll_tick(self.master.as_raw_fd(), libc::POLLIN, deadline)?;
        }
    }

    fn require_live(&self, owner: &mut SpawnOwner) -> io::Result<()> {
        if owner.try_wait(self.token())?.is_some() {
            return Err(refused());
        }
        Ok(())
    }

    fn validate_native(&self, owner: &mut SpawnOwner) -> io::Result<()> {
        self.require_live(owner)?;
        let metadata = std::fs::metadata(format!("/proc/{}/exe", self.receipt.pid))?;
        if metadata.dev() != self.executable_device || metadata.ino() != self.executable_inode {
            return Err(refused());
        }
        let mut status = Vec::with_capacity(4096);
        File::open(format!("/proc/{}/status", self.receipt.pid))?
            .take((MAX_STATUS + 1) as u64)
            .read_to_end(&mut status)?;
        validate_native_status(&status, self.receipt.pid, self.parent, self.uid, self.gid)?;
        self.require_live(owner)
    }

    fn validate_terminal(&self) -> io::Result<()> {
        let mut attributes = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: the owned master is live and output fits one termios.
        if unsafe { libc::tcgetattr(self.master.as_raw_fd(), attributes.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful tcgetattr initialized the complete value.
        let attributes = unsafe { attributes.assume_init() };
        // SAFETY: these calls only query terminal/session identities.
        let session = unsafe { libc::tcgetsid(self.master.as_raw_fd()) };
        // SAFETY: querying the process session does not mutate it.
        let process_session = unsafe { libc::getsid(self.receipt.pid as libc::pid_t) };
        // SAFETY: this queries the retained master terminal's foreground group.
        let foreground = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        if attributes.c_lflag & (libc::ECHO | libc::ECHONL) != 0
            || session != self.receipt.pid as libc::pid_t
            || process_session != self.receipt.pid as libc::pid_t
            || foreground != self.receipt.pid as libc::pid_t
        {
            return Err(refused());
        }
        Ok(())
    }

    fn drain_output(&mut self, deadline: Instant) -> io::Result<()> {
        let mut chunk = Zeroizing::new([0u8; 4096]);
        loop {
            require_before(deadline)?;
            // SAFETY: the writable chunk and retained master remain live.
            let count = unsafe {
                libc::read(
                    self.master.as_raw_fd(),
                    chunk.as_mut_ptr().cast(),
                    chunk.len(),
                )
            };
            if count > 0 {
                let count = count as usize;
                if self
                    .output
                    .len()
                    .checked_add(count)
                    .is_none_or(|length| length > MAX_OUTPUT)
                {
                    return Err(refused());
                }
                self.output.extend_from_slice(&chunk[..count]);
            } else if count < 0 && errno() == libc::EINTR {
                continue;
            } else if count < 0 && errno() == libc::EAGAIN {
                return Ok(());
            } else {
                return Err(if count < 0 {
                    io::Error::last_os_error()
                } else {
                    refused()
                });
            }
        }
    }
}

fn validate_secret(secret: &[u8]) -> io::Result<()> {
    if !(16..=512).contains(&secret.len())
        || secret.iter().any(|byte| matches!(*byte, 0 | b'\r' | b'\n'))
        || std::str::from_utf8(secret).is_err()
    {
        return Err(refused());
    }
    Ok(())
}

fn prompt_complete(output: &[u8]) -> bool {
    let Some(start) = output
        .windows(b"Enter passphrase".len())
        .rposition(|part| part == b"Enter passphrase")
    else {
        return false;
    };
    let prompt = &output[start..];
    prompt.starts_with(b"Enter passphrase for ")
        && (prompt.ends_with(b":") || prompt.ends_with(b": "))
}

/// Whether the native loader, after the secret, asks for it again: it
/// refused the secret and waits for another, which it never gets.
fn passphrase_refused(after_handoff: &[u8]) -> bool {
    const AGAIN: &[u8] = b"Bad passphrase, try again for ";
    after_handoff.windows(AGAIN.len()).any(|part| part == AGAIN)
}

fn validate_cipher_shape(regular: bool, size: u64, seals: libc::c_int) -> io::Result<()> {
    if !regular || !(1..=MAX_CIPHERTEXT).contains(&size) || seals & REQUIRED_SEALS != REQUIRED_SEALS
    {
        return Err(refused());
    }
    Ok(())
}

fn validate_native_status(
    status: &[u8],
    pid: u32,
    parent: libc::pid_t,
    uid: libc::uid_t,
    gid: libc::gid_t,
) -> io::Result<()> {
    if status.len() > MAX_STATUS {
        return Err(refused());
    }
    let status = std::str::from_utf8(status).map_err(|_| refused())?;
    let one = |name: &str| -> io::Result<Vec<u32>> {
        let mut matches = status.lines().filter_map(|line| line.strip_prefix(name));
        let raw = matches.next().ok_or_else(refused)?;
        if matches.next().is_some() {
            return Err(refused());
        }
        raw.split_ascii_whitespace()
            .map(|part| part.parse().map_err(|_| refused()))
            .collect()
    };
    if one("Pid:")? != [pid]
        || one("PPid:")? != [parent as u32]
        || one("Uid:")? != [uid; 4]
        || one("Gid:")? != [gid; 4]
        || one("NoNewPrivs:")? != [1]
    {
        return Err(refused());
    }
    Ok(())
}

fn open_private_pty() -> io::Result<(OwnedFd, OwnedFd)> {
    // SAFETY: posix_openpt creates a new descriptor with atomic CLOEXEC.
    let master = unsafe {
        libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK)
    };
    if master < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: posix_openpt returned a new exclusively owned descriptor.
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    // SAFETY: these operations address only this retained private PTY.
    if unsafe { libc::grantpt(master.as_raw_fd()) } != 0
        || unsafe { libc::unlockpt(master.as_raw_fd()) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Linux TIOCGPTPEER atomically opens this master's slave without a path.
    let slave = unsafe {
        libc::ioctl(
            master.as_raw_fd(),
            TIOCGPTPEER,
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if slave < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: TIOCGPTPEER returned a new exclusively owned descriptor.
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    // grantpt may leave group-write permission. Restrict this unique slave,
    // never the shared ptmx node, before handing it to a native process.
    // SAFETY: the owned slave is live and only permission bits are supplied.
    if unsafe { libc::fchmod(slave.as_raw_fd(), 0o600) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut metadata = MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: fstat initializes the complete live stat storage.
    if unsafe { libc::fstat(slave.as_raw_fd(), metadata.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstat initialized the structure; geteuid is a query.
    let metadata = unsafe { metadata.assume_init() };
    validate_slave_shape(metadata.st_mode, metadata.st_uid, unsafe {
        libc::geteuid()
    })?;
    Ok((master, slave))
}

fn validate_slave_shape(
    mode: libc::mode_t,
    uid: libc::uid_t,
    expected_uid: libc::uid_t,
) -> io::Result<()> {
    if mode & libc::S_IFMT == libc::S_IFCHR && mode & 0o7777 == 0o600 && uid == expected_uid {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(libc::EACCES))
    }
}

fn observe_stopped(pidfd: RawFd, pid: u32, deadline: Instant) -> io::Result<()> {
    loop {
        require_before(deadline)?;
        let mut information = MaybeUninit::<libc::siginfo_t>::zeroed();
        // SAFETY: the pidfd is retained and the full siginfo storage is writable.
        // WNOWAIT observes without consuming Child's eventual exit status.
        if unsafe {
            libc::waitid(
                libc::P_PIDFD,
                pidfd as libc::id_t,
                information.as_mut_ptr(),
                libc::WSTOPPED | libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful waitid initialized siginfo (or left its zero PID).
        let information = unsafe { information.assume_init() };
        // SAFETY: these accessors read the initialized SIGCHLD layout.
        let actual_pid = unsafe { information.si_pid() };
        if actual_pid != 0 {
            if actual_pid != pid as libc::pid_t
                || information.si_code != libc::CLD_STOPPED
                || unsafe { information.si_status() } != libc::SIGSTOP
            {
                return Err(refused());
            }
            return Ok(());
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(timed_out)?;
        std::thread::sleep(remaining.min(Duration::from_millis(5)));
    }
}

fn poll_tick(fd: RawFd, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(timed_out)?;
        let millis = remaining.as_millis().min(50) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: the single writable pollfd and retained fd remain live.
        let result = unsafe { libc::poll(&mut descriptor, 1, millis.max(1)) };
        if result < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(refused());
        }
        if descriptor.revents & events != 0 {
            return Ok(());
        }
        // A retained slave can suppress HUP after native exit. Return a bounded
        // wake-up so the caller checks the owner's exit receipt each iteration.
        if result == 0 {
            return Ok(());
        }
    }
}

fn duplicate(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates the live borrowed descriptor with atomic CLOEXEC.
    let descriptor = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl returned a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

fn deadline(timeout: Duration) -> io::Result<Instant> {
    if timeout.is_zero() {
        return Err(timed_out());
    }
    Instant::now().checked_add(timeout).ok_or_else(refused)
}

fn require_before(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        return Err(timed_out());
    }
    Ok(())
}

fn refused() -> io::Error {
    io::Error::from_raw_os_error(libc::EPROTO)
}

fn timed_out() -> io::Error {
    io::Error::from_raw_os_error(libc::ETIMEDOUT)
}

fn errno() -> libc::c_int {
    // SAFETY: this accessor returns this thread's errno storage.
    unsafe { *libc::__errno_location() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slave_requires_its_owner_and_exact_private_character_device_permissions() {
        assert!(validate_slave_shape(libc::S_IFCHR | 0o600, 7, 7).is_ok());
        for (mode, uid) in [
            (libc::S_IFCHR | 0o620, 7),
            (libc::S_IFCHR | 0o660, 7),
            (libc::S_IFREG | 0o600, 7),
            (libc::S_IFCHR | 0o600, 8),
        ] {
            assert!(validate_slave_shape(mode, uid, 7).is_err());
        }
    }

    #[test]
    fn secret_transport_requires_exact_bounded_utf8_without_terminal_controls() {
        assert!(validate_secret(&[b'a'; 16]).is_ok());
        assert!(validate_secret(&[b'b'; 512]).is_ok());
        assert!(validate_secret("á".repeat(8).as_bytes()).is_ok());
        for invalid in [
            vec![],
            vec![b'a'; 15],
            vec![b'a'; 513],
            vec![0xff; 16],
            [vec![b'a'; 16], vec![0]].concat(),
            [vec![b'a'; 16], vec![b'\r']].concat(),
            [vec![b'a'; 16], vec![b'\n']].concat(),
        ] {
            assert!(validate_secret(&invalid).is_err());
        }
    }

    #[test]
    fn prompt_detection_requires_a_complete_native_passphrase_prompt() {
        assert!(prompt_complete(b"Enter passphrase for /proc/self/fd/42: "));
        assert!(prompt_complete(
            b"notice\r\nEnter passphrase for /proc/self/fd/42:"
        ));
        assert!(!prompt_complete(b"Enter passphrase for /proc/self/fd/42"));
        assert!(!prompt_complete(b"Enter password: "));
        assert!(!prompt_complete(
            b"Enter passphrase for /proc/self/fd/42: trailing"
        ));
    }

    #[test]
    fn a_second_passphrase_prompt_is_a_refused_secret() {
        assert!(passphrase_refused(
            b"\r\nBad passphrase, try again for /proc/self/fd/42: "
        ));
        // Printed before the prompt text is complete, it is already a refusal.
        assert!(passphrase_refused(b"\r\nBad passphrase, try again for "));
        for accepted in [
            &b""[..],
            b"\r\n",
            b"\r\nIdentity added: /proc/self/fd/42 (vivac)\r\n",
        ] {
            assert!(!passphrase_refused(accepted), "{accepted:?}");
        }
    }

    #[test]
    fn ciphertext_requires_the_complete_seal_set_and_bounded_regular_bytes() {
        assert!(validate_cipher_shape(true, 1, REQUIRED_SEALS).is_ok());
        assert!(validate_cipher_shape(true, 65536, REQUIRED_SEALS).is_ok());
        for (regular, size, seals) in [
            (false, 4, REQUIRED_SEALS),
            (true, 0, REQUIRED_SEALS),
            (true, 65537, REQUIRED_SEALS),
            (true, 4, REQUIRED_SEALS & !libc::F_SEAL_WRITE),
            (true, 4, REQUIRED_SEALS & !libc::F_SEAL_SEAL),
        ] {
            assert!(validate_cipher_shape(regular, size, seals).is_err());
        }
    }

    #[test]
    fn native_status_requires_exact_pid_parent_credentials_and_nnp() {
        let valid = b"Name:\tssh-add\nPid:\t123\nPPid:\t99\nUid:\t7\t7\t7\t7\nGid:\t8\t8\t8\t8\nNoNewPrivs:\t1\n";
        assert!(validate_native_status(valid, 123, 99, 7, 8).is_ok());
        for invalid in [
            String::from_utf8(valid.to_vec())
                .unwrap()
                .replace("PPid:\t99", "PPid:\t98"),
            String::from_utf8(valid.to_vec())
                .unwrap()
                .replace("Uid:\t7\t7\t7\t7", "Uid:\t7\t0\t7\t7"),
            String::from_utf8(valid.to_vec())
                .unwrap()
                .replace("NoNewPrivs:\t1", "NoNewPrivs:\t0"),
            String::from_utf8(valid.to_vec()).unwrap() + "Pid:\t123\n",
        ] {
            assert!(validate_native_status(invalid.as_bytes(), 123, 99, 7, 8).is_err());
        }
    }
}
