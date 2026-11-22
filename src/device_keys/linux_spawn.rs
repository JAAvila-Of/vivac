use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::linux_ipc::registration::{send_own_pidfd, ChildPidfdChannel};
use super::linux_ipc::require_procfs;
use super::linux_reservation::StoreReservation;
use super::linux_session::PackageLease;

#[path = "linux_spawn/exit_watch.rs"]
mod exit_watch;
pub(crate) use exit_watch::retained_process_exited;
pub use exit_watch::{ChildExitWatch, ObservedChildExit};

#[path = "linux_spawn/child_registration.rs"]
mod child_registration;
pub use child_registration::OwnedChildRegistration;

#[path = "linux_spawn/task_restriction.rs"]
mod task_restriction;

const MAX_CHILDREN: usize = 4;
const MAX_OWNERS: usize = 4;
const OWNER_SLOTS: usize = (1 << MAX_OWNERS) - 1;
const UNCERTAIN: usize = 1 << MAX_OWNERS;
const MAX_INHERITED: usize = 8;
const MAX_RETAINED: usize = 8;
const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
const RESET_SIGNALS: [libc::c_int; 8] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGPIPE,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
];

struct Admission {
    state: AtomicUsize,
}

impl Admission {
    const fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
        }
    }

    fn acquire(&self) -> io::Result<usize> {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & UNCERTAIN != 0 {
                return Err(raw_error(libc::EOWNERDEAD));
            }
            let available = !state & OWNER_SLOTS;
            if available == 0 {
                return Err(raw_error(libc::EBUSY));
            }
            let slot = 1 << available.trailing_zeros();
            match self.state.compare_exchange_weak(
                state,
                state | slot,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(slot),
                Err(observed) => state = observed,
            }
        }
    }

    fn release(&self, slot: usize) {
        self.state.fetch_and(!slot, Ordering::AcqRel);
    }

    fn poison(&self) {
        self.state.fetch_or(UNCERTAIN, Ordering::AcqRel);
    }

    fn uncertain(&self) -> bool {
        self.state.load(Ordering::Acquire) & UNCERTAIN != 0
    }
}

static ADMISSION: Admission = Admission::new();

/// Prepared descriptors and non-secret process metadata for one native ELF.
/// Provider admission and the meaning of arguments/environment belong to the
/// caller. Every descriptor is owned, at least 3, and initially close-on-exec.
pub struct SpawnSpec {
    pub executable: OwnedFd,
    pub arg0: OsString,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub stdio: [OwnedFd; 3],
    pub inherited: Vec<OwnedFd>,
    /// Parent-owned capabilities which must remain leased but never survive exec.
    pub retained: Vec<OwnedFd>,
    pub new_session: bool,
    pub controlling_tty: Option<OwnedFd>,
    pub probe: Option<ProbeSeams>,
}

/// Disposable synchronization only; neither descriptor survives native exec.
pub struct ProbeSeams {
    pub events: OwnedFd,
    pub gate: OwnedFd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Receipt {
    pub token: u64,
    pub pid: u32,
    pub creator_tid: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitReceipt {
    pub token: u64,
    pub pid: u32,
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

struct Entry {
    receipt: Receipt,
    tasks_restricted: bool,
    child: Child,
    pidfd: Option<OwnedFd>,
    exit: Option<ExitReceipt>,
    // Retain all selected objects until native exit has been observed.
    _resources: SpawnSpec,
}

/// A thread-affine owner. Its creator thread must remain alive for the session.
/// Failed shutdown retains ownership. Drop makes one stop/poll pass, then retains
/// uncertain entries and permanently closes process-wide admission. Neither Drop
/// nor abandonment acknowledges closure or provides a real-time syscall bound.
pub struct SpawnOwner {
    parent: libc::pid_t,
    creator_tid: libc::pid_t,
    next_token: u64,
    closing: bool,
    registration_frozen: bool,
    entries: Vec<Entry>,
    admission_slot: usize,
    reservation: Option<StoreReservation>,
    session_generation: Option<[u8; 32]>,
    dedicated_host_invalidated: Option<Cell<bool>>,
    _thread_affine: PhantomData<Rc<()>>,
}

impl SpawnOwner {
    pub fn new() -> io::Result<Self> {
        // SAFETY: these calls only query the calling process/thread identity.
        let parent = unsafe { libc::getpid() };
        // SAFETY: gettid has no pointer arguments.
        let creator_tid = unsafe { libc::syscall(libc::SYS_gettid) };
        if parent <= 0 || creator_tid <= 0 || creator_tid > libc::pid_t::MAX as libc::c_long {
            return Err(raw_error(libc::ESRCH));
        }
        let admission_slot = ADMISSION.acquire()?;
        Ok(Self {
            parent,
            creator_tid: creator_tid as libc::pid_t,
            next_token: 1,
            closing: false,
            registration_frozen: false,
            entries: Vec::new(),
            admission_slot,
            reservation: None,
            session_generation: None,
            dedicated_host_invalidated: None,
            _thread_affine: PhantomData,
        })
    }

    pub fn new_reserved(reservation: StoreReservation) -> io::Result<Self> {
        reservation.validate_owned()?;
        let mut owner = Self::new()?;
        let generation = new_session_generation()?;
        reservation.validate_owned()?;
        owner.reservation = Some(reservation);
        owner.session_generation = Some(generation);
        Ok(owner)
    }

    pub fn new_dedicated() -> io::Result<Self> {
        Self::new()?.into_dedicated()
    }

    pub(crate) fn new_reserved_dedicated(reservation: StoreReservation) -> io::Result<Self> {
        Self::new_reserved(reservation)?.into_dedicated()
    }

    pub(crate) fn new_reserved_successor(
        reservation: StoreReservation,
        generation: [u8; 32],
    ) -> io::Result<Self> {
        if generation == [0; 32] {
            return Err(raw_error(libc::EINVAL));
        }
        reservation.validate_owned()?;
        let mut owner = Self::new()?;
        reservation.validate_owned()?;
        owner.reservation = Some(reservation);
        owner.session_generation = Some(generation);
        owner.into_dedicated()
    }

    fn into_dedicated(mut self) -> io::Result<Self> {
        self.dedicated_host_invalidated = Some(Cell::new(false));
        self.check_thread()?;
        Ok(self)
    }

    // The endpoint fixture uses worker threads only after the dedicated
    // launch has ended and the owner's roster has been frozen.
    pub(crate) fn enter_endpoint_probe_session(&mut self, lease: &PackageLease) -> io::Result<()> {
        self.validate_generation_context(lease)?;
        if self.dedicated_host_invalidated.is_none()
            || !self.registration_frozen
            || self.entries.len() != 2
            || self.entries.iter().any(|entry| !entry.tasks_restricted)
        {
            return Err(raw_error(libc::EINVAL));
        }
        self.dedicated_host_invalidated = None;
        Ok(())
    }

    pub fn validate_reservation(&self, lease: &PackageLease) -> io::Result<()> {
        self.check_thread()?;
        self.reservation
            .as_ref()
            .ok_or_else(|| raw_error(libc::EINVAL))?
            .validate(lease)
    }

    pub(crate) fn session_generation(&self, lease: &PackageLease) -> io::Result<[u8; 32]> {
        self.validate_generation_context(lease)?;
        let generation = self
            .session_generation
            .ok_or_else(|| raw_error(libc::EINVAL))?;
        if generation == [0; 32] {
            return Err(raw_error(libc::EINVAL));
        }
        self.validate_generation_context(lease)?;
        Ok(generation)
    }

    fn validate_generation_context(&self, lease: &PackageLease) -> io::Result<()> {
        self.validate_reservation(lease)?;
        if ADMISSION.uncertain() {
            return Err(raw_error(libc::EOWNERDEAD));
        }
        if self.closing {
            return Err(raw_error(libc::EBUSY));
        }
        Ok(())
    }

    fn check_thread(&self) -> io::Result<()> {
        // SAFETY: identity queries have no output pointers.
        if unsafe { libc::getpid() } != self.parent
            || unsafe { libc::syscall(libc::SYS_gettid) } != libc::c_long::from(self.creator_tid)
        {
            return Err(raw_error(libc::EPERM));
        }
        if let Some(invalidated) = &self.dedicated_host_invalidated {
            if invalidated.get() {
                return Err(raw_error(libc::EPERM));
            }
            let result = require_dedicated_host(self.parent, self.creator_tid);
            invalidated.set(result.is_err());
            return result;
        }
        Ok(())
    }

    pub fn spawn(&mut self, spec: SpawnSpec) -> io::Result<Receipt> {
        self.spawn_inner(spec, false, false)
    }

    /// Starts a native process whose kernel filter forbids creating tasks.
    pub fn spawn_task_restricted(&mut self, spec: SpawnSpec) -> io::Result<Receipt> {
        task_restriction::require_supported()?;
        self.spawn_inner(spec, false, true)
    }

    /// Disposable fault injection after successful spawn and ownership storage.
    pub fn spawn_with_pidfd_probe_failure(&mut self, spec: SpawnSpec) -> io::Result<Receipt> {
        self.spawn_inner(spec, true, false)
    }

    pub fn admission_uncertain() -> bool {
        ADMISSION.uncertain()
    }

    fn spawn_inner(
        &mut self,
        spec: SpawnSpec,
        fail_pidfd: bool,
        tasks_restricted: bool,
    ) -> io::Result<Receipt> {
        self.check_thread()?;
        if ADMISSION.uncertain() {
            return Err(raw_error(libc::EOWNERDEAD));
        }
        if self.closing || self.registration_frozen || self.entries.len() >= MAX_CHILDREN {
            return Err(raw_error(libc::EBUSY));
        }
        validate_shape(
            spec.inherited.len(),
            spec.retained.len(),
            spec.new_session,
            spec.controlling_tty.is_some(),
        )?;
        validate_metadata(&spec.arg0, &spec.arguments, &spec.environment)?;
        validate_descriptors(&spec)?;
        validate_elf(&spec.executable)?;
        let following_token = advance_token(self.next_token)?;
        self.entries
            .try_reserve(1)
            .map_err(|_| raw_error(libc::ENOMEM))?;

        let mut command = Command::new(format!("/proc/self/fd/{}", spec.executable.as_raw_fd()));
        command.arg0(&spec.arg0).args(&spec.arguments).env_clear();
        for (key, value) in &spec.environment {
            command.env(key, value);
        }
        command.stdin(Stdio::from(spec.stdio[0].try_clone()?));
        command.stdout(Stdio::from(spec.stdio[1].try_clone()?));
        command.stderr(Stdio::from(spec.stdio[2].try_clone()?));
        let mut setup = ChildSetup::prepare(self.parent, &spec)?;
        setup.tasks_restricted = tasks_restricted;
        let pidfd_channel = if fail_pidfd {
            None
        } else {
            Some(ChildPidfdChannel::new()?)
        };
        let child_socket = pidfd_channel.as_ref().map(ChildPidfdChannel::child_socket);
        // SAFETY: setup owns only precomputed scalar/array data. The callback
        // performs fixed syscalls, never allocation, environment access, locks,
        // formatting, logging or Rust unwinding in the forked child.
        unsafe { command.pre_exec(move || setup.apply(child_socket)) };
        if ADMISSION.uncertain() {
            return Err(raw_error(libc::EOWNERDEAD));
        }
        if let Some(reservation) = &self.reservation {
            reservation.validate_owned()?;
        }
        self.check_thread()?;
        require_retained_child_status()?;
        let child = command.spawn()?;
        let receipt = Receipt {
            token: self.next_token,
            pid: child.id(),
            creator_tid: self.creator_tid,
        };
        self.next_token = following_token;
        // Store ownership before any fallible operation after successful spawn.
        self.entries.push(Entry {
            receipt,
            tasks_restricted,
            child,
            pidfd: None,
            exit: None,
            _resources: spec,
        });
        let pidfd = match pidfd_channel {
            Some(channel) => accept_child_pidfd(receipt, channel),
            None => Err(raw_error(libc::EMFILE)),
        }
        .and_then(|pidfd| {
            self.check_thread()?;
            Ok(pidfd)
        });
        match pidfd {
            Ok(pidfd) => {
                self.entries
                    .last_mut()
                    .ok_or_else(|| raw_error(libc::EIO))?
                    .pidfd = Some(pidfd);
            }
            Err(error) => {
                // No PID-based kill fallback: an uncertain child and all its
                // resources remain owned, and admission closes immediately.
                self.closing = true;
                ADMISSION.poison();
                return Err(error);
            }
        }
        Ok(receipt)
    }

    /// Preserve an abandoned owner's complete resource bundle at its caller.
    /// No wait or closure acknowledgement is performed; admission stays closed
    /// until process exit even if a signalled child subsequently exits.
    pub fn abandon(&mut self) {
        ADMISSION.poison();
        self.closing = true;
        if self.check_thread().is_err() {
            return;
        }
        for entry in &self.entries {
            if entry.exit.is_none() {
                if let Some(pidfd) = &entry.pidfd {
                    let _ = send_pidfd_signal(pidfd, libc::SIGKILL);
                }
            }
        }
    }

    fn entry(&self, token: u64) -> io::Result<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.receipt.token == token)
            .ok_or_else(|| raw_error(libc::ENOENT))
    }

    pub fn receipt(&self, token: u64) -> io::Result<Receipt> {
        self.check_thread()?;
        Ok(self.entry(token)?.receipt)
    }

    pub fn signal(&self, token: u64, signal: libc::c_int) -> io::Result<()> {
        self.check_thread()?;
        let entry = self.entry(token)?;
        if entry.exit.is_some() {
            return Err(raw_error(libc::ESRCH));
        }
        send_pidfd_signal(
            entry
                .pidfd
                .as_ref()
                .ok_or_else(|| raw_error(libc::ENOTSUP))?,
            signal,
        )
    }

    pub fn duplicate_pidfd(&self, token: u64) -> io::Result<OwnedFd> {
        self.check_thread()?;
        self.entry(token)?
            .pidfd
            .as_ref()
            .ok_or_else(|| raw_error(libc::ENOTSUP))?
            .try_clone()
    }

    pub fn try_wait(&mut self, token: u64) -> io::Result<Option<ExitReceipt>> {
        self.check_thread()?;
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.receipt.token == token)
            .ok_or_else(|| raw_error(libc::ENOENT))?;
        let exit = poll_entry(entry)?;
        self.check_thread()?;
        Ok(exit)
    }

    pub(crate) fn observe_direct_exits(&mut self) -> io::Result<Option<Vec<ExitReceipt>>> {
        self.check_thread()?;
        let mut exits = Vec::with_capacity(self.entries.len());
        for entry in &mut self.entries {
            if let Some(exit) = poll_entry(entry)? {
                exits.push(exit);
            }
        }
        self.check_thread()?;
        Ok((exits.len() == self.entries.len()).then_some(exits))
    }

    pub fn shutdown(&mut self, timeout: Duration) -> io::Result<Vec<ExitReceipt>> {
        self.check_thread()?;
        self.closing = true;
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| raw_error(libc::EINVAL))?;
        loop {
            self.check_thread()?;
            self.stop_and_poll();
            if self.entries.iter().all(|entry| entry.exit.is_some()) {
                self.check_thread()?;
                let receipts = self.entries.iter().filter_map(|entry| entry.exit).collect();
                self.entries.clear();
                drop(self.reservation.take());
                return Ok(receipts);
            }
            if Instant::now() >= deadline {
                return Err(raw_error(libc::ETIMEDOUT));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn stop_and_poll(&mut self) {
        // Try every child even if signalling or waiting for another one fails.
        for entry in &mut self.entries {
            if entry.exit.is_none() {
                if let Some(pidfd) = &entry.pidfd {
                    let _ = send_pidfd_signal(pidfd, libc::SIGKILL);
                }
                let _ = poll_entry(entry);
            }
        }
    }
}

pub(crate) fn new_session_generation() -> io::Result<[u8; 32]> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or_else(|| raw_error(libc::EINVAL))?;
    loop {
        let mut generation = [0u8; 32];
        let mut filled = 0;
        while filled < generation.len() {
            if Instant::now() >= deadline {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            // SAFETY: the remaining buffer is writable and its size is bounded.
            let count = unsafe {
                libc::getrandom(
                    generation[filled..].as_mut_ptr().cast(),
                    generation.len() - filled,
                    libc::GRND_NONBLOCK,
                )
            };
            if count > 0 {
                filled += count as usize;
            } else if count < 0 {
                let error = io::Error::last_os_error();
                if !matches!(error.raw_os_error(), Some(libc::EINTR | libc::EAGAIN)) {
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(1));
            } else {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        if generation.iter().any(|byte| *byte != 0) {
            return Ok(generation);
        }
    }
}

impl Drop for SpawnOwner {
    fn drop(&mut self) {
        self.closing = true;
        if self.check_thread().is_ok() {
            self.stop_and_poll();
        }
        if self.entries.iter().any(|entry| entry.exit.is_none()) {
            ADMISSION.poison();
            // Keep the occupied slot and every selected capability. The fixed
            // owner cap also bounds concurrent owners admitted before poison.
            std::mem::forget(std::mem::take(&mut self.entries));
            std::mem::forget(self.reservation.take());
            return;
        }
        self.entries.clear();
        drop(self.reservation.take());
        ADMISSION.release(self.admission_slot);
    }
}

fn poll_entry(entry: &mut Entry) -> io::Result<Option<ExitReceipt>> {
    if let Some(exit) = entry.exit {
        return Ok(Some(exit));
    }
    let pidfd = entry
        .pidfd
        .as_ref()
        .ok_or_else(|| raw_error(libc::ENOTSUP))?;
    entry.exit = wait_pidfd(entry.receipt, pidfd, 0)?;
    Ok(entry.exit)
}

fn require_retained_child_status() -> io::Result<()> {
    let mut action = std::mem::MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: the null replacement leaves the disposition unchanged. The
    // output points to writable space for one complete sigaction.
    if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), action.as_mut_ptr()) } < 0 {
        return Err(last_error());
    }
    // SAFETY: the preceding sigaction query succeeded.
    let action = unsafe { action.assume_init() };
    if action.sa_sigaction != libc::SIG_DFL || action.sa_flags & libc::SA_NOCLDWAIT != 0 {
        return Err(raw_error(libc::EPERM));
    }
    Ok(())
}

const MAX_HOST_STATUS: u64 = 8192;

fn require_dedicated_host(pid: libc::pid_t, tid: libc::pid_t) -> io::Result<()> {
    if pid != tid {
        return Err(raw_error(libc::EPERM));
    }
    let status = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open("/proc/self/status")?;
    require_procfs(&status)?;
    if !status.metadata()?.is_file() {
        return Err(raw_error(libc::EPERM));
    }
    let mut bytes = Vec::new();
    status.take(MAX_HOST_STATUS + 1).read_to_end(&mut bytes)?;
    if !single_thread_status(&bytes) {
        return Err(raw_error(libc::EPERM));
    }
    require_retained_child_status()
}

fn single_thread_status(bytes: &[u8]) -> bool {
    if bytes.len() as u64 > MAX_HOST_STATUS || bytes.last() != Some(&b'\n') {
        return false;
    }
    let mut rows = bytes
        .split(|byte| *byte == b'\n')
        .filter(|row| row.starts_with(b"Threads:"));
    rows.next() == Some(b"Threads:\t1") && rows.next().is_none()
}

fn wait_pidfd(
    receipt: Receipt,
    pidfd: &OwnedFd,
    flags: libc::c_int,
) -> io::Result<Option<ExitReceipt>> {
    validate_descriptor(pidfd.as_raw_fd())?;
    // SAFETY: zeroed storage is valid and covers waitid's output. With
    // WNOHANG a zero PID means the child has no pending exit status.
    let mut status: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: pidfd remains owned and status is writable for one siginfo.
    if unsafe {
        libc::waitid(
            libc::P_PIDFD,
            pidfd.as_raw_fd() as libc::id_t,
            &mut status,
            libc::WEXITED | libc::WNOHANG | flags,
        )
    } < 0
    {
        return Err(last_error());
    }
    // SAFETY: waitid succeeded and initializes these SIGCHLD fields.
    let pid = unsafe { status.si_pid() };
    if pid == 0 {
        return Ok(None);
    }
    if u32::try_from(pid) != Ok(receipt.pid) || status.si_signo != libc::SIGCHLD {
        return Err(raw_error(libc::EIO));
    }
    // SAFETY: a SIGCHLD status from waitid has an initialized status value.
    let value = unsafe { status.si_status() };
    let (code, signal) = match status.si_code {
        libc::CLD_EXITED => (Some(value), None),
        libc::CLD_KILLED | libc::CLD_DUMPED => (None, Some(value)),
        _ => return Err(raw_error(libc::EIO)),
    };
    Ok(Some(ExitReceipt {
        token: receipt.token,
        pid: receipt.pid,
        code,
        signal,
    }))
}

fn advance_token(token: u64) -> io::Result<u64> {
    token
        .checked_add(1)
        .ok_or_else(|| raw_error(libc::EOVERFLOW))
}

fn validate_shape(
    inherited: usize,
    retained: usize,
    new_session: bool,
    tty: bool,
) -> io::Result<()> {
    if inherited > MAX_INHERITED
        || retained > MAX_RETAINED
        || inherited
            .checked_add(retained)
            .is_none_or(|count| count > MAX_INHERITED + MAX_RETAINED)
        || (tty && !new_session)
    {
        return Err(raw_error(libc::EINVAL));
    }
    Ok(())
}

fn validate_metadata(
    arg0: &OsStr,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
) -> io::Result<()> {
    if arg0.is_empty()
        || arg0.as_bytes().contains(&0)
        || arguments
            .iter()
            .any(|argument| argument.as_bytes().contains(&0))
        || environment.iter().any(|(key, value)| {
            key.is_empty()
                || key.as_bytes().contains(&0)
                || key.as_bytes().contains(&b'=')
                || value.as_bytes().contains(&0)
        })
    {
        return Err(raw_error(libc::EINVAL));
    }
    Ok(())
}

fn validate_descriptor(fd: RawFd) -> io::Result<()> {
    if fd < 3 {
        return Err(raw_error(libc::EBADF));
    }
    // SAFETY: F_GETFD only queries the specified descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(last_error());
    }
    if flags & libc::FD_CLOEXEC == 0 {
        return Err(raw_error(libc::EINVAL));
    }
    Ok(())
}

fn validate_descriptors(spec: &SpawnSpec) -> io::Result<()> {
    validate_descriptor(spec.executable.as_raw_fd())?;
    for fd in spec
        .stdio
        .iter()
        .chain(spec.inherited.iter())
        .chain(spec.retained.iter())
    {
        validate_descriptor(fd.as_raw_fd())?;
    }
    if let Some(tty) = &spec.controlling_tty {
        validate_descriptor(tty.as_raw_fd())?;
    }
    if let Some(probe) = &spec.probe {
        validate_descriptor(probe.events.as_raw_fd())?;
        validate_descriptor(probe.gate.as_raw_fd())?;
    }
    Ok(())
}

fn validate_elf(fd: &OwnedFd) -> io::Result<()> {
    let file = File::from(fd.try_clone()?);
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(raw_error(libc::EINVAL));
    }
    let mut magic = [0u8; 4];
    if file.read_at(&mut magic, 0)? != magic.len() || magic != *b"\x7fELF" {
        return Err(raw_error(libc::ENOEXEC));
    }
    Ok(())
}

pub fn current_executable() -> io::Result<OwnedFd> {
    Ok(running_executable(std::process::id())?.into())
}

pub(crate) fn running_executable(pid: u32) -> io::Result<File> {
    if pid == 0 || pid > libc::pid_t::MAX as u32 {
        return Err(raw_error(libc::EINVAL));
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(format!("/proc/{pid}"))?;
    validate_descriptor(directory.as_raw_fd())?;
    require_procfs(&directory)?;
    if !directory.metadata()?.is_dir() {
        return Err(raw_error(libc::EINVAL));
    }
    // SAFETY: the retained directory is a process directory on procfs. Only
    // this fixed kernel executable link is followed, with read-only access.
    let raw = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c"exe".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if raw < 0 {
        return Err(last_error());
    }
    // SAFETY: successful openat returned a fresh exclusively owned descriptor.
    let executable = unsafe { OwnedFd::from_raw_fd(raw) };
    validate_descriptor(executable.as_raw_fd())?;
    validate_elf(&executable)?;
    Ok(File::from(executable))
}

#[cfg(test)]
fn open_pidfd(pid: u32) -> io::Result<OwnedFd> {
    // SAFETY: pidfd_open has no pointer arguments and creates a new descriptor.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
    if descriptor < 0 {
        return Err(last_error());
    }
    // SAFETY: the syscall returned a new exclusively owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor as RawFd) })
}

fn accept_child_pidfd(receipt: Receipt, channel: ChildPidfdChannel) -> io::Result<OwnedFd> {
    require_retained_child_status()?;
    let pidfd = channel.receive(receipt.pid)?;
    let _ = wait_pidfd(receipt, &pidfd, libc::WNOWAIT)?;
    require_retained_child_status()?;
    Ok(pidfd)
}

fn send_pidfd_signal(fd: &OwnedFd, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: the descriptor stays owned, and a null siginfo requests the normal
    // kernel-generated signal. Identity is retained by the pidfd, never a PID.
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0u32,
        )
    };
    if result < 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}

fn raw_error(code: libc::c_int) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn last_error() -> io::Error {
    // SAFETY: Linux exposes thread-local errno through this libc accessor.
    raw_error(unsafe { *libc::__errno_location() })
}

struct ChildSetup {
    parent: libc::pid_t,
    tasks_restricted: bool,
    inherited: [RawFd; MAX_INHERITED],
    inherited_count: usize,
    new_session: bool,
    tty: Option<RawFd>,
    probe: Option<(RawFd, RawFd)>,
    empty_mask: libc::sigset_t,
    default_action: libc::sigaction,
    image: PreparedExecImage,
}

struct PreparedExecImage {
    path: CString,
    // CString data buffers keep stable addresses when these owners move.
    _arguments: Vec<CString>,
    _environment: Vec<CString>,
    argv: Vec<*const libc::c_char>,
    envp: Vec<*const libc::c_char>,
}

// SAFETY: pointer tables refer only to the CString buffers owned by this same
// value, or their null terminator. Construction finishes before publication;
// no method mutates or exposes those buffers/tables. Moving the owners does not
// move their heap buffers. The child reads them only until exec or _exit, and
// Command::spawn does not return to the parent before that boundary.
unsafe impl Send for PreparedExecImage {}
// SAFETY: the same immutable ownership invariants permit concurrent shared reads.
unsafe impl Sync for PreparedExecImage {}

impl PreparedExecImage {
    fn prepare(spec: &SpawnSpec) -> io::Result<Self> {
        Self::from_metadata(
            spec.executable.as_raw_fd(),
            &spec.arg0,
            &spec.arguments,
            &spec.environment,
        )
    }

    fn from_metadata(
        executable: RawFd,
        arg0: &OsStr,
        arguments: &[OsString],
        environment: &[(OsString, OsString)],
    ) -> io::Result<Self> {
        let path = CString::new(format!("/proc/self/fd/{executable}"))
            .map_err(|_| raw_error(libc::EINVAL))?;
        let arguments = std::iter::once(arg0)
            .chain(arguments.iter().map(OsString::as_os_str))
            .map(|argument| CString::new(argument.as_bytes()).map_err(|_| raw_error(libc::EINVAL)))
            .collect::<io::Result<Vec<_>>>()?;
        // Match Command's last-value-wins environment selection in the parent.
        let mut selected = BTreeMap::new();
        for (key, value) in environment {
            selected.insert(key.as_os_str(), value.as_os_str());
        }
        let mut environment = Vec::with_capacity(selected.len());
        for (key, value) in selected {
            let capacity = key
                .len()
                .checked_add(1)
                .and_then(|length| length.checked_add(value.len()))
                .ok_or_else(|| raw_error(libc::EOVERFLOW))?;
            let mut entry = Vec::with_capacity(capacity);
            entry.extend_from_slice(key.as_bytes());
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            environment.push(CString::new(entry).map_err(|_| raw_error(libc::EINVAL))?);
        }
        let argv = arguments
            .iter()
            .map(|value| value.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let envp = environment
            .iter()
            .map(|value| value.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        Ok(Self {
            path,
            _arguments: arguments,
            _environment: environment,
            argv,
            envp,
        })
    }
}

impl ChildSetup {
    fn prepare(parent: libc::pid_t, spec: &SpawnSpec) -> io::Result<Self> {
        // SAFETY: zero is a valid initial storage representation; sigemptyset
        // initializes the signal sets before either is used by a syscall.
        let mut empty_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        // SAFETY: the initialized action is completed below before use.
        let mut default_action: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: both pointers refer to complete writable signal sets.
        if unsafe { libc::sigemptyset(&mut empty_mask) } != 0
            || unsafe { libc::sigemptyset(&mut default_action.sa_mask) } != 0
        {
            return Err(last_error());
        }
        default_action.sa_sigaction = libc::SIG_DFL;
        let mut inherited = [-1; MAX_INHERITED];
        for (target, source) in inherited.iter_mut().zip(&spec.inherited) {
            *target = source.as_raw_fd();
        }
        Ok(Self {
            parent,
            tasks_restricted: false,
            inherited,
            inherited_count: spec.inherited.len(),
            new_session: spec.new_session,
            tty: spec.controlling_tty.as_ref().map(AsRawFd::as_raw_fd),
            probe: spec
                .probe
                .as_ref()
                .map(|probe| (probe.events.as_raw_fd(), probe.gate.as_raw_fd())),
            empty_mask,
            default_action,
            image: PreparedExecImage::prepare(spec)?,
        })
    }

    fn parent_unchanged(&self) -> io::Result<()> {
        // SAFETY: getppid is an async-signal-safe identity query.
        if unsafe { libc::getppid() } == self.parent {
            Ok(())
        } else {
            self.probe_event(b'P')?;
            Err(raw_error(libc::ESRCH))
        }
    }

    fn barrier(&self, phase: u8) -> io::Result<()> {
        let Some((_, gate)) = self.probe else {
            return Ok(());
        };
        self.probe_event(phase)?;
        let mut command = [0u8; 1];
        loop {
            // SAFETY: the fixed byte buffer is writable and gate stays owned.
            let count =
                unsafe { libc::syscall(libc::SYS_read, gate, command.as_mut_ptr(), command.len()) };
            if count == 1 {
                break;
            }
            if count < 0 && errno() == libc::EINTR {
                continue;
            }
            return Err(if count < 0 {
                last_error()
            } else {
                raw_error(libc::EPIPE)
            });
        }
        if command[0] == b'G' {
            Ok(())
        } else {
            Err(raw_error(libc::EPROTO))
        }
    }

    fn probe_event(&self, phase: u8) -> io::Result<()> {
        let Some((events, _)) = self.probe else {
            return Ok(());
        };
        // SAFETY: getpid is async-signal-safe and has no pointer arguments.
        let packet = barrier_packet(phase, unsafe { libc::getpid() } as u32);
        loop {
            // SAFETY: the owned descriptor and fixed packet are live here.
            let count =
                unsafe { libc::syscall(libc::SYS_write, events, packet.as_ptr(), packet.len()) };
            if count == packet.len() as libc::c_long {
                break;
            }
            if count < 0 && errno() == libc::EINTR {
                continue;
            }
            return Err(if count < 0 {
                last_error()
            } else {
                raw_error(libc::EPROTO)
            });
        }
        Ok(())
    }

    fn apply(&self, child_socket: Option<RawFd>) -> io::Result<()> {
        self.parent_unchanged()?;
        self.barrier(b'B')?;
        self.parent_unchanged()?;
        // SAFETY: fixed prctl operations use scalar arguments and no borrowed
        // libc state. No privilege-changing operation follows them here.
        if unsafe {
            libc::syscall(
                libc::SYS_prctl,
                libc::PR_SET_NO_NEW_PRIVS,
                1usize,
                0usize,
                0usize,
                0usize,
            )
        } < 0
        {
            return Err(last_error());
        }
        // SAFETY: PR_GET_NO_NEW_PRIVS takes no output pointer.
        if unsafe {
            libc::syscall(
                libc::SYS_prctl,
                libc::PR_GET_NO_NEW_PRIVS,
                0usize,
                0usize,
                0usize,
                0usize,
            )
        } != 1
        {
            return Err(raw_error(libc::EPERM));
        }
        // SAFETY: PR_SET_PDEATHSIG accepts a signal number and scalar padding.
        if unsafe {
            libc::syscall(
                libc::SYS_prctl,
                libc::PR_SET_PDEATHSIG,
                libc::SIGKILL as usize,
                0usize,
                0usize,
                0usize,
            )
        } < 0
        {
            return Err(last_error());
        }
        let mut signal: libc::c_int = 0;
        // SAFETY: signal is writable for one integer and remains live.
        if unsafe {
            libc::syscall(
                libc::SYS_prctl,
                libc::PR_GET_PDEATHSIG,
                &mut signal as *mut libc::c_int,
                0usize,
                0usize,
                0usize,
            )
        } < 0
        {
            return Err(last_error());
        }
        if signal != libc::SIGKILL {
            return Err(raw_error(libc::EPERM));
        }
        self.parent_unchanged()?;
        self.barrier(b'A')?;
        self.parent_unchanged()?;
        if self.new_session {
            // SAFETY: setsid is async-signal-safe and has no pointer arguments.
            if unsafe { libc::setsid() } < 0 {
                return Err(last_error());
            }
        }
        if let Some(tty) = self.tty {
            // SAFETY: the retained terminal descriptor stays open until exec.
            if unsafe { libc::syscall(libc::SYS_ioctl, tty, libc::TIOCSCTTY, 0usize) } < 0 {
                return Err(last_error());
            }
        }
        for signal in RESET_SIGNALS {
            // SAFETY: sigaction is async-signal-safe. The complete default
            // action was prepared in the parent; no old action is requested.
            if unsafe { libc::sigaction(signal, &self.default_action, std::ptr::null_mut()) } != 0 {
                return Err(last_error());
            }
        }
        // SAFETY: sigprocmask is async-signal-safe; this child has one thread.
        if unsafe { libc::sigprocmask(libc::SIG_SETMASK, &self.empty_mask, std::ptr::null_mut()) }
            != 0
        {
            return Err(last_error());
        }
        // SAFETY: marking, rather than closing, preserves std's private exec
        // error channel until exec succeeds. Unsupported kernels refuse.
        if unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, CLOSE_RANGE_CLOEXEC) } < 0
        {
            return Err(last_error());
        }
        for fd in self.inherited.iter().take(self.inherited_count) {
            // SAFETY: these are pre-existing owned descriptors, not fixed
            // destination numbers that could collide with std's error channel.
            let flags = unsafe { libc::fcntl(*fd, libc::F_GETFD) };
            if flags < 0 {
                return Err(last_error());
            }
            // SAFETY: only the selected child's close-on-exec bit is changed.
            if unsafe { libc::fcntl(*fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
                return Err(last_error());
            }
        }
        if self.tasks_restricted {
            task_restriction::install()?;
        }
        if let Some(socket) = child_socket {
            send_own_pidfd(socket)?;
        }
        // SAFETY: path and the null-terminated pointer tables were completed in
        // the parent and remain owned by this callback. A direct kernel execve
        // never falls back to a shell when the selected ELF is malformed.
        unsafe {
            libc::syscall(
                libc::SYS_execve,
                self.image.path.as_ptr(),
                self.image.argv.as_ptr(),
                self.image.envp.as_ptr(),
            )
        };
        // execve returns only on error. Returning that errno lets std retain its
        // existing exec-error protocol and reap the failed child.
        Err(last_error())
    }
}

fn errno() -> libc::c_int {
    // SAFETY: Linux exposes errno for this thread through the libc accessor.
    unsafe { *libc::__errno_location() }
}

fn barrier_packet(phase: u8, pid: u32) -> [u8; 8] {
    let bytes = pid.to_be_bytes();
    [phase, 0, 0, 0, bytes[0], bytes[1], bytes[2], bytes[3]]
}

#[cfg(test)]
mod session_generation_tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::PathBuf;

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove owned generation fixture");
        }
    }

    fn public_lease() -> (TestDirectory, PackageLease) {
        let temporary_root = std::env::temp_dir();
        assert!(temporary_root.is_absolute(), "TMPDIR must be absolute");
        let path = temporary_root.join(format!(
            "vivac-session-generation-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("new generation fixture directory");
        let directory = TestDirectory(path);
        std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700))
            .expect("private generation fixture directory");
        for (name, bytes) in [
            ("key.openssh", b"public ciphertext fixture".as_slice()),
            ("manifest.json", b"public manifest fixture".as_slice()),
        ] {
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.0.join(name))
                .expect("new public generation fixture");
            file.write_all(bytes)
                .expect("public generation fixture bytes");
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .expect("private generation fixture file");
        }
        let capability: OwnedFd = File::open(&directory.0)
            .expect("generation directory capability")
            .into();
        let cipher: OwnedFd = File::open(directory.0.join("key.openssh"))
            .expect("public ciphertext capability")
            .into();
        let lease = PackageLease::capture(capability, &cipher).expect("public package snapshot");
        (directory, lease)
    }

    fn reserved(lease: &PackageLease) -> SpawnOwner {
        SpawnOwner::new_reserved(StoreReservation::acquire(lease).unwrap()).unwrap()
    }

    #[test]
    fn generation_requires_reserved_owner() {
        let (_directory, lease) = public_lease();
        let owner = SpawnOwner::new().unwrap();
        assert!(owner.session_generation(&lease).is_err());
    }

    #[test]
    fn generation_exists_before_native_entries_and_is_stable() {
        let (_directory, lease) = public_lease();
        let owner = reserved(&lease);
        assert!(owner.entries.is_empty());
        let reserved_generation = owner.session_generation.expect("generation already exists");
        let generation = owner.session_generation(&lease).unwrap();
        assert_ne!(generation, [0; 32]);
        assert_eq!(generation, reserved_generation);
        assert_eq!(owner.session_generation(&lease).unwrap(), generation);
        assert!(owner.entries.is_empty());
    }

    #[test]
    fn generation_is_distinct_for_separate_reserved_owners() {
        let (_first_directory, first_lease) = public_lease();
        let (_second_directory, second_lease) = public_lease();
        let first = reserved(&first_lease);
        let second = reserved(&second_lease);
        assert_ne!(
            first.session_generation(&first_lease).unwrap(),
            second.session_generation(&second_lease).unwrap()
        );
    }

    #[test]
    fn generation_rejects_another_store() {
        let (_directory, lease) = public_lease();
        let (_other_directory, other) = public_lease();
        let owner = reserved(&lease);
        assert!(owner.session_generation(&other).is_err());
    }

    #[test]
    fn generation_rejects_changed_retained_store() {
        let (directory, lease) = public_lease();
        let owner = reserved(&lease);
        let mut manifest = File::options()
            .append(true)
            .open(directory.0.join("manifest.json"))
            .unwrap();
        manifest.write_all(b" changed").unwrap();
        assert!(owner.session_generation(&lease).is_err());
    }

    #[test]
    fn generation_rejects_wrong_origin() {
        let (_directory, lease) = public_lease();
        let mut owner = reserved(&lease);
        let origin = owner.creator_tid;
        owner.creator_tid += 1;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            owner.session_generation(&lease)
        }));
        owner.creator_tid = origin;
        let error = result.unwrap().unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }

    #[test]
    fn generation_rejects_closing_and_released_owner() {
        let (_directory, lease) = public_lease();
        let mut owner = reserved(&lease);
        owner.closing = true;
        assert!(owner.session_generation(&lease).is_err());
        owner.closing = false;
        owner.shutdown(Duration::ZERO).unwrap();
        assert!(owner.session_generation(&lease).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_current_executable_matches_the_running_image() {
        use std::os::unix::fs::MetadataExt;

        let selected = File::from(current_executable().expect("retain current executable"));
        let current = File::open("/proc/self/exe").expect("open running image");
        let selected_metadata = selected.metadata().unwrap();
        let current_metadata = current.metadata().unwrap();
        assert_eq!(selected_metadata.dev(), current_metadata.dev());
        assert_eq!(selected_metadata.ino(), current_metadata.ino());
        validate_descriptor(selected.as_raw_fd()).unwrap();
    }

    #[test]
    fn running_executable_refuses_invalid_and_absent_processes() {
        for pid in [0, u32::MAX, i32::MAX as u32] {
            assert!(running_executable(pid).is_err());
        }
    }

    #[test]
    fn dedicated_host_status_requires_exactly_one_thread_field() {
        assert!(single_thread_status(b"Name:\ttest\nThreads:\t1\n"));
        assert!(!single_thread_status(b"Threads:\t2\n"));
        assert!(!single_thread_status(b"Threads:\t1\nThreads:\t1\n"));
        assert!(!single_thread_status(b"Threads:\t1"));
    }

    #[test]
    fn child_acquisition_refuses_a_pidfd_for_a_non_child() {
        let receipt = Receipt {
            token: 1,
            pid: std::process::id(),
            creator_tid: 0,
        };
        let pidfd = open_pidfd(receipt.pid).unwrap();
        assert_eq!(
            wait_pidfd(receipt, &pidfd, libc::WNOWAIT)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn admission_caps_live_owners_and_reuses_only_released_slots() {
        let admission = Admission::new();
        let slots = (0..MAX_OWNERS)
            .map(|_| admission.acquire().expect("owner should fit the fixed cap"))
            .collect::<Vec<_>>();
        assert_eq!(
            admission.acquire().unwrap_err().raw_os_error(),
            Some(libc::EBUSY)
        );
        admission.release(slots[1]);
        assert_eq!(admission.acquire().unwrap(), slots[1]);
        assert_eq!(
            admission.acquire().unwrap_err().raw_os_error(),
            Some(libc::EBUSY)
        );
    }

    #[test]
    fn uncertainty_is_irreversible_even_when_existing_slots_are_released() {
        let admission = Admission::new();
        let slot = admission.acquire().unwrap();
        admission.poison();
        admission.release(slot);
        admission.poison();
        assert!(admission.uncertain());
        assert_eq!(
            admission.acquire().unwrap_err().raw_os_error(),
            Some(libc::EOWNERDEAD)
        );
    }

    #[test]
    fn concurrent_admission_never_exceeds_the_owner_cap() {
        let admission = std::sync::Arc::new(Admission::new());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(MAX_OWNERS * 2));
        let workers = (0..MAX_OWNERS * 2)
            .map(|_| {
                let admission = std::sync::Arc::clone(&admission);
                let barrier = std::sync::Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    admission.acquire()
                })
            })
            .collect::<Vec<_>>();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            MAX_OWNERS
        );
        assert!(results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| { error.raw_os_error() == Some(libc::EBUSY) }));
    }

    #[test]
    fn shape_limits_refuse_unbounded_inheritance_or_an_unowned_terminal() {
        assert!(validate_shape(0, 0, false, false).is_ok());
        assert!(validate_shape(MAX_INHERITED, MAX_RETAINED, true, true).is_ok());
        assert!(validate_shape(MAX_INHERITED + 1, 0, true, false).is_err());
        assert!(validate_shape(0, MAX_RETAINED + 1, true, false).is_err());
        assert!(validate_shape(0, 0, false, true).is_err());
    }

    #[test]
    fn metadata_refuses_embedded_nul_and_invalid_environment_names() {
        let valid = vec![(OsString::from("HOME"), OsString::from("/private"))];
        assert!(validate_metadata(OsStr::new("/usr/bin/native"), &[], &valid).is_ok());
        assert!(validate_metadata(OsStr::new(""), &[], &valid).is_err());
        assert!(validate_metadata(OsStr::new("native\0tail"), &[], &valid).is_err());
        assert!(
            validate_metadata(OsStr::new("native"), &[OsString::from("x\0y")], &valid).is_err()
        );
        for key in ["", "X=Y", "X\0Y"] {
            let environment = vec![(OsString::from(key), OsString::from("value"))];
            assert!(validate_metadata(OsStr::new("native"), &[], &environment).is_err());
        }
        let environment = vec![(OsString::from("X"), OsString::from("x\0y"))];
        assert!(validate_metadata(OsStr::new("native"), &[], &environment).is_err());
    }

    #[test]
    fn token_exhaustion_never_reuses_an_existing_identity() {
        assert_eq!(advance_token(1).unwrap(), 2);
        assert!(advance_token(u64::MAX).is_err());
    }

    #[test]
    fn barrier_packet_has_a_fixed_network_byte_order_identity() {
        assert_eq!(
            barrier_packet(b'B', 0x01020304),
            [b'B', 0, 0, 0, 1, 2, 3, 4]
        );
        assert_eq!(barrier_packet(b'A', 9), [b'A', 0, 0, 0, 0, 0, 0, 9]);
    }

    #[test]
    fn prepared_exec_image_owns_stable_terminated_argument_and_environment_tables() {
        let arguments = vec![OsString::from("--receipt")];
        let environment = vec![
            (OsString::from("A"), OsString::from("old")),
            (OsString::from("A"), OsString::from("selected")),
            (OsString::from("B"), OsString::from("metadata")),
        ];
        let image = PreparedExecImage::from_metadata(
            123,
            OsStr::new("/test/native"),
            &arguments,
            &environment,
        )
        .expect("public metadata should prepare");
        let arguments_before = image.argv.clone();
        let environment_before = image.envp.clone();
        let image = Box::new(image);
        assert_eq!(image.path.as_bytes(), b"/proc/self/fd/123");
        assert_eq!(image.argv, arguments_before);
        assert_eq!(image.envp, environment_before);
        assert_eq!(image.argv.len(), image._arguments.len() + 1);
        assert_eq!(image.envp.len(), image._environment.len() + 1);
        assert!(image.argv.last().unwrap().is_null());
        assert!(image.envp.last().unwrap().is_null());
        for (pointer, value) in image.argv.iter().zip(&image._arguments) {
            assert_eq!(*pointer, value.as_ptr());
        }
        for (pointer, value) in image.envp.iter().zip(&image._environment) {
            assert_eq!(*pointer, value.as_ptr());
        }
        assert_eq!(
            image
                ._environment
                .iter()
                .map(|value| value.as_bytes())
                .collect::<Vec<_>>(),
            vec![b"A=selected".as_slice(), b"B=metadata".as_slice()]
        );
        let empty = PreparedExecImage::from_metadata(123, OsStr::new("native"), &[], &[])
            .expect("empty explicit environment should prepare");
        assert_eq!(empty.envp.len(), 1);
        assert!(empty.envp[0].is_null());
    }
}
