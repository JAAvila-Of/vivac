#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_spawn as launcher;

#[cfg(target_os = "linux")]
use launcher as linux_spawn;
#[cfg(target_os = "linux")]
mod abandon_probe;
#[cfg(target_os = "linux")]
mod admission_storage_probe;
#[cfg(target_os = "linux")]
mod concurrent_probe;
#[cfg(target_os = "linux")]
mod connected_probe;
#[cfg(target_os = "linux")]
mod creation_probe;
#[cfg(target_os = "linux")]
mod creation_reader_probe;
#[cfg(target_os = "linux")]
mod endpoint_native_probe;
#[cfg(target_os = "linux")]
mod exit_watch_probe;
#[cfg(target_os = "linux")]
mod fixed_root_probe;
#[cfg(target_os = "linux")]
mod ipc_probe;
#[cfg(target_os = "linux")]
mod isolation_probe;
#[cfg(target_os = "linux")]
mod registration_probe;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_admission_storage;
#[cfg(target_os = "linux")]
use rust_launcher_probe::{linux_creation_binding, linux_creation_record};
// Experimental reader dependencies remain outside the product graph.
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_current_admission;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_custody_provision;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_custody_root;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_gate;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_ipc;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_loader;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_openssh_package;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_reservation;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_resident_reader;
#[cfg(target_os = "linux")]
use rust_launcher_probe::linux_session;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_ssh_lifetime.rs"]
mod linux_ssh_lifetime;
#[cfg(target_os = "linux")]
mod loader_probe;
#[cfg(target_os = "linux")]
mod provision_probe;
#[cfg(target_os = "linux")]
mod reservation_probe;
#[cfg(target_os = "linux")]
mod resident_probe;
#[cfg(target_os = "linux")]
mod root_reader_probe;

#[cfg(target_os = "linux")]
mod probe {
    use super::launcher::{ExitReceipt, ProbeSeams, Receipt, SpawnOwner, SpawnSpec};
    use serde_json::{json, Value};
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    const CASES: [&str; 7] = [
        "stable-thread",
        "exec-error",
        "creator-thread-exit",
        "owner-before-arm",
        "owner-after-arm",
        "native-agent",
        "native-agent-owner-death",
    ];

    fn refused() -> io::Error {
        io::Error::from_raw_os_error(libc::EINVAL)
    }

    fn require(condition: bool) -> io::Result<()> {
        if condition {
            Ok(())
        } else {
            Err(refused())
        }
    }

    fn duplicate(fd: RawFd) -> io::Result<OwnedFd> {
        // SAFETY: fcntl duplicates the borrowed descriptor without taking it.
        let copied = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if copied < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fcntl returned a newly owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(copied) })
    }

    fn null() -> io::Result<OwnedFd> {
        Ok(File::options()
            .read(true)
            .write(true)
            .open("/dev/null")?
            .into())
    }

    fn child_environment() -> Vec<(OsString, OsString)> {
        vec![
            ("VIVAC_OPENSSH_DISPOSABLE".into(), "1".into()),
            ("VIVAC_SPAWN_CHILD".into(), "1".into()),
        ]
    }

    fn open_fds() -> io::Result<Vec<i32>> {
        let candidates = std::fs::read_dir("/proc/self/fd")?
            .map(|entry| {
                entry?
                    .file_name()
                    .to_str()
                    .ok_or_else(refused)?
                    .parse::<i32>()
                    .map_err(|_| refused())
            })
            .collect::<io::Result<Vec<_>>>()?;
        // The directory stream has closed. Its former descriptor is excluded.
        let mut actual: Vec<_> = candidates
            .into_iter()
            .filter(|fd| {
                // SAFETY: F_GETFD only queries the descriptor table.
                unsafe { libc::fcntl(*fd, libc::F_GETFD) >= 0 }
            })
            .collect();
        actual.sort_unstable();
        Ok(actual)
    }

    fn status_value<'a>(status: &'a str, key: &str) -> io::Result<&'a str> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::trim)
            .ok_or_else(refused)
    }

    fn native_receipt(control_fd: RawFd) -> io::Result<()> {
        require(control_fd >= 3)?;
        require(std::env::var("VIVAC_SPAWN_CHILD").as_deref() == Ok("1"))?;
        let status = std::fs::read_to_string("/proc/self/status")?;
        let blocked =
            u128::from_str_radix(status_value(&status, "SigBlk:")?, 16).map_err(|_| refused())?;
        let keys: BTreeSet<_> = std::env::vars_os().map(|(key, _)| key).collect();
        let expected: BTreeSet<OsString> = [
            "VIVAC_OPENSSH_DISPOSABLE".into(),
            "VIVAC_SPAWN_CHILD".into(),
        ]
        .into_iter()
        .collect();
        let mut parent_signal: libc::c_int = 0;
        // SAFETY: prctl writes one live c_int; the remaining calls are queries.
        let signal_result =
            unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, &mut parent_signal, 0, 0, 0) };
        require(signal_result == 0)?;
        let receipt = json!({
            "kind": "native-receipt",
            "pid": unsafe { libc::getpid() },
            "ppid": unsafe { libc::getppid() },
            "tid": unsafe { libc::syscall(libc::SYS_gettid) },
            "nnp": unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) },
            "parent_death_signal": parent_signal,
            "sigmask_empty": blocked == 0,
            "open_fds": open_fds()?,
            "expected_test_env": keys == expected,
        });
        println!("{receipt}");
        io::stdout().flush()?;
        // SAFETY: this deadline belongs only to the disposable native fixture.
        unsafe { libc::alarm(30) };
        // SAFETY: the CLI takes exclusive ownership of its inherited control FD.
        let mut control = unsafe { File::from_raw_fd(control_fd) };
        let mut command = [0u8; 1];
        control.read_exact(&mut command)?;
        require(command == *b"X")
    }

    fn read_receipt(reader: &mut UnixStream) -> io::Result<Value> {
        reader.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut bytes = Vec::with_capacity(1024);
        loop {
            let mut byte = [0];
            reader.read_exact(&mut byte)?;
            require(bytes.len() < 8192)?;
            if byte == *b"\n" {
                break;
            }
            bytes.push(byte[0]);
        }
        serde_json::from_slice(&bytes).map_err(|_| refused())
    }

    struct ChildControl {
        receipt: Receipt,
        reader: UnixStream,
        command: UnixStream,
        control_fd: RawFd,
    }

    fn self_spec(
        probe: Option<ProbeSeams>,
    ) -> io::Result<(SpawnSpec, UnixStream, UnixStream, RawFd)> {
        let (reader, output) = UnixStream::pair()?;
        let (command, input) = UnixStream::pair()?;
        let inherited: OwnedFd = input.into();
        let control_fd = inherited.as_raw_fd();
        let spec = SpawnSpec {
            executable: File::open(std::env::current_exe()?)?.into(),
            arg0: "rust-launcher-probe".into(),
            arguments: vec![
                "--native-receipt".into(),
                "--control-fd".into(),
                control_fd.to_string().into(),
            ],
            environment: child_environment(),
            stdio: [null()?, output.into(), null()?],
            inherited: vec![inherited],
            retained: Vec::new(),
            new_session: false,
            controlling_tty: None,
            probe,
        };
        Ok((spec, reader, command, control_fd))
    }

    fn spawn_control(owner: &mut SpawnOwner) -> io::Result<ChildControl> {
        let (spec, reader, command, control_fd) = self_spec(None)?;
        let receipt = owner.spawn(spec)?;
        // SAFETY: gettid queries only this same creating thread.
        require(receipt.creator_tid == unsafe { libc::syscall(libc::SYS_gettid) } as i32)?;
        Ok(ChildControl {
            receipt,
            reader,
            command,
            control_fd,
        })
    }

    fn validate_receipt(control: &mut ChildControl) -> io::Result<()> {
        let value = read_receipt(&mut control.reader)?;
        let actual: Vec<i32> =
            serde_json::from_value(value["open_fds"].clone()).map_err(|_| refused())?;
        let mut expected = vec![0, 1, 2, control.control_fd];
        expected.sort_unstable();
        require(value["kind"] == "native-receipt")?;
        require(value["pid"] == control.receipt.pid)?;
        // SAFETY: getpid only queries the current process identity.
        require(value["ppid"] == unsafe { libc::getpid() })?;
        require(value["nnp"] == 1 && value["parent_death_signal"] == libc::SIGKILL)?;
        require(value["sigmask_empty"] == true && value["expected_test_env"] == true)?;
        require(actual == expected)
    }

    fn wait_owned(owner: &mut SpawnOwner, token: u64) -> io::Result<ExitReceipt> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(receipt) = owner.try_wait(token)? {
                return Ok(receipt);
            }
            require(Instant::now() < deadline)?;
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn completed(receipt: &ExitReceipt) -> io::Result<()> {
        require(receipt.code == Some(0) && receipt.signal.is_none())
    }

    pub(super) struct Pressure {
        stop: Arc<AtomicBool>,
        iterations: Arc<AtomicU64>,
        handles: Vec<thread::JoinHandle<()>>,
    }

    impl Pressure {
        pub(super) fn start() -> io::Result<Self> {
            let mut pressure = Self {
                stop: Arc::new(AtomicBool::new(false)),
                iterations: Arc::new(AtomicU64::new(0)),
                handles: Vec::new(),
            };
            let shared = Arc::new(Mutex::new(Vec::with_capacity(4096)));
            for _ in 0..4 {
                let stop = Arc::clone(&pressure.stop);
                let iterations = Arc::clone(&pressure.iterations);
                let shared = Arc::clone(&shared);
                pressure.handles.push(thread::Builder::new().spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        let allocation = vec![23u8; 4096];
                        std::hint::black_box(&allocation);
                        if let Ok(mut bytes) = shared.lock() {
                            bytes.extend_from_slice(&allocation[..256]);
                            bytes.clear();
                        }
                        iterations.fetch_add(1, Ordering::Relaxed);
                        thread::yield_now();
                    }
                })?);
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while pressure.iterations.load(Ordering::Relaxed) < 4 {
                require(Instant::now() < deadline)?;
                thread::yield_now();
            }
            Ok(pressure)
        }

        pub(super) fn finish(mut self) -> io::Result<u64> {
            self.stop.store(true, Ordering::Release);
            for handle in self.handles.drain(..) {
                handle.join().map_err(|_| refused())?;
            }
            Ok(self.iterations.load(Ordering::Relaxed))
        }
    }

    impl Drop for Pressure {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            for handle in self.handles.drain(..) {
                let _ = handle.join();
            }
        }
    }

    struct BlockedTerm(libc::sigset_t);

    impl BlockedTerm {
        fn new() -> io::Result<Self> {
            // SAFETY: zero is a valid initial representation for these Linux
            // signal sets; unused bytes remain initialized across libc calls.
            let mut blocked: libc::sigset_t = unsafe { std::mem::zeroed() };
            // SAFETY: same representation argument as the set above.
            let mut original: libc::sigset_t = unsafe { std::mem::zeroed() };
            // SAFETY: these calls use live initialized signal-set storage.
            unsafe {
                require(libc::sigemptyset(&mut blocked) == 0)?;
                require(libc::sigaddset(&mut blocked, libc::SIGTERM) == 0)?;
                let error = libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut original);
                if error != 0 {
                    return Err(io::Error::from_raw_os_error(error));
                }
                Ok(Self(original))
            }
        }
    }

    impl Drop for BlockedTerm {
        fn drop(&mut self) {
            // SAFETY: the saved mask belongs to this same disposable owner thread.
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut()) };
        }
    }

    enum Request {
        Spawn(mpsc::SyncSender<io::Result<ChildControl>>),
        Wait(u64, mpsc::SyncSender<io::Result<ExitReceipt>>),
        Close(mpsc::SyncSender<io::Result<Vec<ExitReceipt>>>),
    }

    fn stable_thread() -> io::Result<Value> {
        require(std::env::var("VIVAC_LAUNCHER_POISON").as_deref() == Ok("present"))?;
        let source = File::open("/dev/null")?;
        // SAFETY: this deliberately non-CLOEXEC canary is confined to the probe.
        let raw = unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD, 4096) };
        require(raw >= 4096)?;
        // SAFETY: fcntl returned a new exclusively owned descriptor.
        let _canary = unsafe { OwnedFd::from_raw_fd(raw) };
        let pressure = Pressure::start()?;
        let (send, receive) = mpsc::sync_channel::<Request>(4);
        let worker = thread::Builder::new().spawn(move || -> io::Result<()> {
            let _blocked = BlockedTerm::new()?;
            let mut owner = SpawnOwner::new()?;
            while let Ok(request) = receive.recv() {
                match request {
                    Request::Spawn(reply) => {
                        if reply.send(spawn_control(&mut owner)).is_err() {
                            break;
                        }
                    }
                    Request::Wait(token, reply) => {
                        if reply.send(wait_owned(&mut owner, token)).is_err() {
                            break;
                        }
                    }
                    Request::Close(reply) => {
                        let _ = reply.send(owner.shutdown(Duration::from_secs(3)));
                        break;
                    }
                }
            }
            Ok(())
        })?;
        let mut creator = None;
        for _ in 0..4 {
            let caller = send.clone();
            let ephemeral = thread::Builder::new().spawn(move || -> io::Result<ChildControl> {
                let (reply, result) = mpsc::sync_channel(1);
                caller.send(Request::Spawn(reply)).map_err(|_| refused())?;
                let control = result
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| refused())??;
                // SAFETY: gettid queries this disposable requesting thread.
                require(
                    control.receipt.creator_tid
                        != unsafe { libc::syscall(libc::SYS_gettid) } as i32,
                )?;
                Ok(control)
            })?;
            let mut control = ephemeral.join().map_err(|_| refused())??;
            validate_receipt(&mut control)?;
            if let Some(expected) = creator {
                require(control.receipt.creator_tid == expected)?;
            } else {
                creator = Some(control.receipt.creator_tid);
            }
            control.command.write_all(b"X")?;
            let (reply, result) = mpsc::sync_channel(1);
            send.send(Request::Wait(control.receipt.token, reply))
                .map_err(|_| refused())?;
            completed(
                &result
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| refused())??,
            )?;
        }
        let (reply, result) = mpsc::sync_channel(1);
        send.send(Request::Close(reply)).map_err(|_| refused())?;
        let exits = result
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| refused())??;
        require(exits.len() == 4)?;
        worker.join().map_err(|_| refused())??;
        let iterations = pressure.finish()?;
        require(iterations > 0)?;
        Ok(
            json!({"creator_tid": creator, "native_children_observed": 4,
                  "caller_threads_returned": 4, "pressure_threads": 4,
                  "pressure_iterations": iterations, "parent_poison_present": true,
                  "nnp_after_exec": true, "signal_mask_empty": true,
                  "fd_allowlist_verified": true, "child_environment_clean": true,
                  "canary_excluded": true, "shutdown_observed": true}),
        )
    }

    fn seam_packet(packet: &[u8; 8], expected: u8) -> io::Result<i32> {
        require(packet[0] == expected && packet[1..4] == [0, 0, 0])?;
        let pid = i32::from_be_bytes(packet[4..8].try_into().map_err(|_| refused())?);
        require(pid > 0)?;
        Ok(pid)
    }

    fn invalid_executable() -> io::Result<OwnedFd> {
        // SAFETY: the constant C string remains live for this syscall.
        let raw = unsafe {
            libc::syscall(
                libc::SYS_memfd_create,
                c"vivac-exec-refusal".as_ptr(),
                libc::MFD_CLOEXEC,
            )
        } as i32;
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: memfd_create returned a newly owned descriptor.
        let mut file = unsafe { File::from_raw_fd(raw) };
        file.write_all(b"\x7fELF")?;
        file.set_permissions(std::fs::Permissions::from_mode(0o700))?;
        Ok(file.into())
    }

    fn exec_error() -> io::Result<Value> {
        let mut owner = SpawnOwner::new()?;
        let (mut observer, child) = UnixStream::pair()?;
        observer.set_read_timeout(Some(Duration::from_secs(5)))?;
        let seams = ProbeSeams {
            events: duplicate(child.as_raw_fd())?,
            gate: child.into(),
        };
        let monitor = thread::Builder::new().spawn(move || -> io::Result<()> {
            let mut expected_pid = None;
            for phase in *b"BA" {
                let mut packet = [0; 8];
                observer.read_exact(&mut packet)?;
                let pid = seam_packet(&packet, phase)?;
                if let Some(expected) = expected_pid {
                    require(expected == pid)?;
                }
                expected_pid = Some(pid);
                observer.write_all(b"G")?;
            }
            Ok(())
        })?;
        let (output, writer) = UnixStream::pair()?;
        output.set_read_timeout(Some(Duration::from_secs(2)))?;
        let spec = SpawnSpec {
            executable: invalid_executable()?,
            arg0: "invalid-elf-fixture".into(),
            arguments: Vec::new(),
            environment: child_environment(),
            stdio: [null()?, writer.into(), null()?],
            inherited: Vec::new(),
            retained: Vec::new(),
            new_session: false,
            controlling_tty: None,
            probe: Some(seams),
        };
        let error = match owner.spawn(spec) {
            Ok(_) => return Err(refused()),
            Err(error) => error,
        };
        monitor.join().map_err(|_| refused())??;
        let mut captured = Vec::new();
        output.take(8193).read_to_end(&mut captured)?;
        require(captured.is_empty() && error.raw_os_error() == Some(libc::ENOEXEC))?;
        let mut control = spawn_control(&mut owner)?;
        validate_receipt(&mut control)?;
        owner.signal(control.receipt.token, libc::SIGCONT)?;
        control.command.write_all(b"X")?;
        completed(&wait_owned(&mut owner, control.receipt.token)?)?;
        let exits = owner.shutdown(Duration::from_secs(3))?;
        require(exits.len() == 1)?;
        Ok(
            json!({"exec_failure_observed": true, "exec_failure_raw_os_error": error.raw_os_error(),
                  "exec_preexec_seams_observed": ["B", "A"], "failed_exec_stdout_empty": true,
                  "owner_reusable_after_error": true, "shutdown_observed": true}),
        )
    }

    fn observed_kill(pid: u32, pidfd: RawFd) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut descriptor = libc::pollfd {
                fd: pidfd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll receives one initialized descriptor and a finite timeout.
            let result = unsafe { libc::poll(&mut descriptor, 1, 10) };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            if result > 0 {
                break;
            }
            require(Instant::now() < deadline)?;
        }
        let mut status = 0;
        // SAFETY: only this probe's known child PID is reaped.
        let result = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
        require(
            result == pid as i32
                && libc::WIFSIGNALED(status)
                && libc::WTERMSIG(status) == libc::SIGKILL,
        )
    }

    fn creator_thread_exit() -> io::Result<Value> {
        let (reply, result) = mpsc::sync_channel(1);
        let (finish, proceed) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().spawn(move || -> io::Result<()> {
            let mut owner = SpawnOwner::new()?;
            let control = spawn_control(&mut owner)?;
            let pidfd = owner.duplicate_pidfd(control.receipt.token)?;
            reply.send((control, pidfd)).map_err(|_| refused())?;
            proceed
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| refused())?;
            // Deliberate negative control: bypass the owner's Drop while keeping
            // its descriptors alive, then let only the creating thread exit.
            std::mem::forget(owner);
            Ok(())
        })?;
        let (mut control, pidfd) = result
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| refused())?;
        validate_receipt(&mut control)?;
        finish.send(()).map_err(|_| refused())?;
        thread.join().map_err(|_| refused())??;
        observed_kill(control.receipt.pid, pidfd.as_raw_fd())?;
        require(Path::new("/proc/self/status").exists())?;
        Ok(json!({"creator_tid": control.receipt.creator_tid,
                  "creator_thread_exit_observed": true, "parent_process_alive": true,
                  "child_sigkill_observed": true, "held_control_through_exit": true,
                  "deliberately_forgotten_owner": true}))
    }

    fn parent_death_seam(observer_fd: RawFd) -> io::Result<Value> {
        require(observer_fd >= 3)?;
        // SAFETY: the CLI consumes the inherited descriptor selected by its caller.
        let channel = unsafe { OwnedFd::from_raw_fd(observer_fd) };
        let control = duplicate(channel.as_raw_fd())?;
        let control_fd = control.as_raw_fd();
        let spec = SpawnSpec {
            executable: File::open(std::env::current_exe()?)?.into(),
            arg0: "rust-launcher-probe".into(),
            arguments: vec![
                "--native-receipt".into(),
                "--control-fd".into(),
                control_fd.to_string().into(),
            ],
            environment: child_environment(),
            stdio: [null()?, duplicate(libc::STDOUT_FILENO)?, null()?],
            inherited: vec![control],
            retained: Vec::new(),
            new_session: false,
            controlling_tty: None,
            probe: Some(ProbeSeams {
                events: duplicate(channel.as_raw_fd())?,
                gate: duplicate(channel.as_raw_fd())?,
            }),
        };
        drop(channel);
        let _pressure = Pressure::start()?;
        let mut owner = SpawnOwner::new()?;
        // The independent observer owns the other socket endpoint and kills this
        // process at B or A. Returning from spawn is not success for this case.
        let _ = owner.spawn(spec);
        Err(refused())
    }

    fn native_credentials(pid: u32) -> io::Result<()> {
        let own = std::fs::read_to_string("/proc/self/status")?;
        let child = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
        for key in ["Uid:", "Gid:", "Groups:"] {
            require(status_value(&own, key)? == status_value(&child, key)?)?;
        }
        require(status_value(&child, "NoNewPrivs:")? == "1")?;
        // SAFETY: getpid only queries the parent process identity.
        require(
            status_value(&child, "PPid:")?
                .parse::<i32>()
                .map_err(|_| refused())?
                == unsafe { libc::getpid() },
        )
    }

    fn empty_agent(socket: &Path) -> io::Result<UnixStream> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut connection = loop {
            match UnixStream::connect(socket) {
                Ok(connection) => break connection,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                Err(error) => return Err(error),
            }
        };
        connection.set_read_timeout(Some(Duration::from_secs(2)))?;
        connection.set_write_timeout(Some(Duration::from_secs(2)))?;
        connection.write_all(&[0, 0, 0, 1, 11])?;
        let mut size = [0; 4];
        connection.read_exact(&mut size)?;
        require(u32::from_be_bytes(size) == 5)?;
        let mut response = [0; 5];
        connection.read_exact(&mut response)?;
        require(response == [12, 0, 0, 0, 0])?;
        Ok(connection)
    }

    fn native_agent(agent_fd: RawFd, socket: &Path, owner_death: bool) -> io::Result<Value> {
        require(agent_fd >= 3 && socket.is_absolute() && !socket.exists())?;
        let parent = socket.parent().ok_or_else(refused)?;
        let metadata = parent.metadata()?;
        require(metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0)?;
        // SAFETY: the argument conveys an owned inherited descriptor to the probe.
        let original = unsafe { OwnedFd::from_raw_fd(agent_fd) };
        let spec = SpawnSpec {
            executable: duplicate(original.as_raw_fd())?,
            arg0: "/usr/bin/ssh-agent".into(),
            arguments: vec!["-D".into(), "-a".into(), socket.as_os_str().to_owned()],
            environment: vec![
                ("HOME".into(), parent.as_os_str().to_owned()),
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
        };
        drop(original);
        let pressure = Pressure::start()?;
        let mut owner = SpawnOwner::new()?;
        let receipt = owner.spawn(spec)?;
        let mut retained = empty_agent(socket)?;
        native_credentials(receipt.pid)?;
        if owner_death {
            let ready = json!({"version": 1, "case": "native-agent-owner-death", "status": "ready",
                               "pid": receipt.pid, "creator_tid": receipt.creator_tid,
                               "empty_identities_observed": true, "native_credentials_checked": true,
                               "custody_accredited": false, "launcher_accredited": false,
                               "threadsafe_fullflow": false});
            println!("{ready}");
            io::stdout().flush()?;
            let mut hold = [0; 1];
            io::stdin().read_exact(&mut hold)?;
            return Err(refused());
        }
        let exits = owner.shutdown(Duration::from_secs(3))?;
        require(exits.len() == 1 && exits[0].signal == Some(libc::SIGKILL))?;
        require(UnixStream::connect(socket).is_err())?;
        let after = retained.write_all(&[0, 0, 0, 1, 11]).and_then(|()| {
            let mut output = [0; 1];
            match retained.read(&mut output)? {
                0 => Err(io::Error::from_raw_os_error(libc::EPIPE)),
                _ => Ok(()),
            }
        });
        require(after.is_err())?;
        let iterations = pressure.finish()?;
        Ok(
            json!({"pid": receipt.pid, "creator_tid": receipt.creator_tid,
                  "empty_identities_observed": true, "native_credentials_checked": true,
                  "native_sigkill_observed": true, "shutdown_observed": true,
                  "retained_connection_refused": true, "new_connection_refused": true,
                  "pressure_iterations": iterations}),
        )
    }

    #[derive(Debug, PartialEq)]
    struct Arguments {
        case: String,
        agent_fd: Option<RawFd>,
        observer_fd: Option<RawFd>,
        socket: Option<PathBuf>,
    }

    fn arguments(values: impl Iterator<Item = OsString>) -> io::Result<Arguments> {
        let mut values = values;
        require(values.next().as_deref() == Some(std::ffi::OsStr::new("--case")))?;
        let case = values
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(refused)?;
        require(CASES.contains(&case.as_str()))?;
        let mut result = Arguments {
            case,
            agent_fd: None,
            observer_fd: None,
            socket: None,
        };
        while let Some(option) = values.next() {
            let value = values.next().ok_or_else(refused)?;
            match option.to_str() {
                Some("--agent-fd") if result.agent_fd.is_none() => {
                    result.agent_fd = Some(
                        value
                            .to_str()
                            .ok_or_else(refused)?
                            .parse()
                            .map_err(|_| refused())?,
                    );
                }
                Some("--observer-fd") if result.observer_fd.is_none() => {
                    result.observer_fd = Some(
                        value
                            .to_str()
                            .ok_or_else(refused)?
                            .parse()
                            .map_err(|_| refused())?,
                    );
                }
                Some("--socket") if result.socket.is_none() => result.socket = Some(value.into()),
                _ => return Err(refused()),
            }
        }
        let native = result.case.starts_with("native-agent");
        let seam = result.case.starts_with("owner-");
        require(result.agent_fd.is_some() == native && result.socket.is_some() == native)?;
        require(result.observer_fd.is_some() == seam)?;
        require(
            result.agent_fd.is_none_or(|fd| fd >= 3) && result.observer_fd.is_none_or(|fd| fd >= 3),
        )?;
        require(result.socket.as_ref().is_none_or(|path| path.is_absolute()))?;
        Ok(result)
    }

    pub fn run() -> io::Result<()> {
        require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
        let values: Vec<OsString> = std::env::args_os().skip(1).collect();
        if values
            .first()
            .is_some_and(|value| value == "--native-receipt")
        {
            require(values.len() == 3 && values[1] == "--control-fd")?;
            let fd = values[2]
                .to_str()
                .ok_or_else(refused)?
                .parse()
                .map_err(|_| refused())?;
            return native_receipt(fd);
        }
        // SAFETY: this is a disposable case process, never the product runtime.
        // A failed observer must not leave its coordination waits unbounded.
        unsafe { libc::alarm(15) };
        let args = arguments(values.into_iter())?;
        let mut report = match args.case.as_str() {
            "stable-thread" => stable_thread()?,
            "exec-error" => exec_error()?,
            "creator-thread-exit" => creator_thread_exit()?,
            "owner-before-arm" | "owner-after-arm" => {
                parent_death_seam(args.observer_fd.ok_or_else(refused)?)?
            }
            "native-agent" | "native-agent-owner-death" => native_agent(
                args.agent_fd.ok_or_else(refused)?,
                args.socket.as_deref().ok_or_else(refused)?,
                args.case == "native-agent-owner-death",
            )?,
            _ => return Err(refused()),
        };
        let object = report.as_object_mut().ok_or_else(refused)?;
        object.insert("version".into(), json!(1));
        object.insert("case".into(), json!(args.case));
        object.insert("status".into(), json!("observed"));
        object.insert("custody_accredited".into(), json!(false));
        object.insert("launcher_accredited".into(), json!(false));
        object.insert("threadsafe_fullflow".into(), json!(false));
        println!("{report}");
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn parse(values: &[&str]) -> io::Result<Arguments> {
            arguments(values.iter().map(OsString::from))
        }

        #[test]
        fn cases_require_only_their_own_explicit_descriptors() {
            assert!(parse(&["--case", "stable-thread"]).is_ok());
            assert!(parse(&["--case", "owner-before-arm", "--observer-fd", "9"]).is_ok());
            assert!(parse(&[
                "--case",
                "native-agent",
                "--agent-fd",
                "10",
                "--socket",
                "/private/agent.sock"
            ])
            .is_ok());
            for invalid in [
                vec!["--case", "unknown"],
                vec!["--case", "stable-thread", "--observer-fd", "9"],
                vec!["--case", "owner-after-arm"],
                vec![
                    "--case",
                    "native-agent",
                    "--agent-fd",
                    "2",
                    "--socket",
                    "/private/agent.sock",
                ],
                vec![
                    "--case",
                    "native-agent",
                    "--agent-fd",
                    "9",
                    "--socket",
                    "relative",
                ],
                vec!["--case", "exec-error", "--case", "exec-error"],
            ] {
                assert!(parse(&invalid).is_err());
            }
        }

        #[test]
        fn seam_packets_require_the_selected_phase_and_positive_big_endian_pid() {
            let packet = [b'B', 0, 0, 0, 0, 0, 1, 2];
            assert_eq!(
                seam_packet(&packet, b'B').expect("valid public packet"),
                258
            );
            assert!(seam_packet(&packet, b'A').is_err());
            assert!(seam_packet(&[b'B', 1, 0, 0, 0, 0, 1, 2], b'B').is_err());
            assert!(seam_packet(&[b'B', 0, 0, 0, 0, 0, 0, 0], b'B').is_err());
            assert!(seam_packet(&[b'B', 0, 0, 0, 255, 255, 255, 255], b'B').is_err());
        }
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--opening-permit-check"))
    {
        if !matches!(creation_reader_probe::opening_permit(), Ok(true)) {
            std::process::exit(1);
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some(
            "--endpoint-native-custodian"
                | "--endpoint-attended-custodian"
                | "--endpoint-permitted-custodian"
                | "--endpoint-active-custodian"
                | "--endpoint-native-verifier"
        )
    ) {
        if let Err(error) = endpoint_native_probe::run() {
            endpoint_native_probe::failure(&error);
            std::process::exit(1);
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some(
            "--registration-case"
                | "--registration-keeper-bootstrap"
                | "--registration-keeper-supervisor"
                | "--registration-admitted-bootstrap-supervisor"
                | "--registration-admitted-keeper"
                | "--registration-openssh-keeper"
                | "--registration-openssh-supervisor"
                | "--registration-active-supervisor"
                | "--registration-supervisor"
                | "--registration-marker"
                | "--registration-task-marker"
                | "--registration-forwarder"
        )
    ) {
        if registration_probe::run().is_err() {
            eprintln!("Rust process reference transport experiment failed.");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--exit-watch-case" | "--exit-watch-child")
    ) {
        if exit_watch_probe::run().is_err() {
            eprintln!("Rust retained child exit experiment failed.");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--creation-read")) {
        if creation_reader_probe::run().is_err() {
            eprintln!("Rust creation reader experiment failed.");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--fixed-root-setup" | "--fixed-root-case")
    ) {
        if fixed_root_probe::run().is_err() {
            eprintln!("Rust fixed namespace experiment failed.");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--provision-case" | "--provision-child" | "--provision-reader")
    ) {
        if provision_probe::run().is_err() {
            eprintln!("Rust administrative provisioning experiment failed.");
            std::process::exit(1);
        }
    } else if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--loader-case")) {
        if loader_probe::run().is_err() {
            eprintln!("Rust loader experiment failed.");
            std::process::exit(1);
        }
    } else if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new("--creation-binding-case"))
    {
        if creation_probe::run().is_err() {
            eprintln!("Rust local creation binding experiment failed.");
            std::process::exit(1);
        }
    } else if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new("--isolation-case"))
    {
        if isolation_probe::run().is_err() {
            eprintln!("Rust isolation experiment failed.");
            std::process::exit(1);
        }
    } else if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--resident-case"))
    {
        if resident_probe::run().is_err() {
            eprintln!("Rust resident reader experiment failed.");
            std::process::exit(1);
        }
    } else if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some(
            "--connected-case"
                | "--connected-server"
                | "--connected-client"
                | "--connected-injector"
                | "--connected-expected-peer"
                | "--connected-expected-child"
        )
    ) {
        if connected_probe::run().is_err() {
            eprintln!("Rust connected-peer experiment failed.");
            std::process::exit(1);
        }
    } else if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--root-reader-case")
    ) {
        if root_reader_probe::run().is_err() {
            eprintln!("Rust candidate root reader experiment failed.");
            std::process::exit(1);
        }
    } else if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--admission-storage-case" | "--admission-storage-child")
    ) {
        if admission_storage_probe::run().is_err() {
            eprintln!("Rust admission storage experiment failed.");
            std::process::exit(1);
        }
    } else if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--reservation-case" | "--reservation-worker" | "--reservation-child")
    ) {
        if reservation_probe::run().is_err() {
            eprintln!("Rust reservation experiment failed.");
            std::process::exit(1);
        }
    } else if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--ipc-case" | "--ipc-server" | "--ipc-injector")
    ) {
        if ipc_probe::run().is_err() {
            eprintln!("Rust IPC experiment failed.");
            std::process::exit(1);
        }
    } else if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--abandon-case" | "--abandon-child")
    ) {
        if abandon_probe::run().is_err() {
            eprintln!("Rust abandonment experiment failed.");
            std::process::exit(1);
        }
    } else if matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(std::ffi::OsStr::to_str),
        Some("--concurrent-case" | "--concurrent-ssh-case")
    ) {
        if concurrent_probe::run().is_err() {
            eprintln!("Rust concurrent experiment failed.");
            std::process::exit(1);
        }
    } else if let Err(error) = probe::run() {
        println!("{{\"version\":1,\"status\":\"failed\",\"custody_accredited\":false,\"launcher_accredited\":false,\"threadsafe_fullflow\":false,\"os_error\":{}}}", error.raw_os_error().unwrap_or(libc::EIO));
        std::process::exit(1);
    }
    #[cfg(not(target_os = "linux"))]
    std::process::exit(1);
}
