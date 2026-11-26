use super::linux_session::PackageLease;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::rc::Rc;

/// Cooperative reservation of one physical package directory on the approved
/// ext filesystem family. It does not arbitrate copies in other directories.
/// Closing the descriptor releases the reservation, never acknowledges exit.
pub struct StoreReservation {
    directory: OwnedFd,
    snapshot: Option<PackageLease>,
    identity: [u64; 6],
    directory_identity: Option<(u64, u64)>,
    origin: libc::pid_t,
    adopted_lock: bool,
    _retained_context: Option<Box<dyn std::any::Any>>,
    _thread_affine: PhantomData<Rc<()>>,
}

impl StoreReservation {
    /// Retains resources until this reservation is released, without granting authority.
    pub fn acquire_retaining<T: 'static>(lease: &PackageLease, context: T) -> io::Result<Self> {
        let mut reservation = Self::acquire(lease)?;
        reservation._retained_context = Some(Box::new(context));
        Ok(reservation)
    }

    pub fn acquire(lease: &PackageLease) -> io::Result<Self> {
        let identity = lease.ipc_identity().map_err(|_| invalid())?;
        let snapshot = lease.duplicate_for_reservation()?;
        let directory = snapshot.open_independent_directory()?;
        require_supported_filesystem(&directory)?;
        // SAFETY: the descriptor is owned and refers to the validated directory.
        // A single nonblocking call preserves EINTR and contention as failures.
        if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: getpid only queries the current process identity.
        let origin = unsafe { libc::getpid() };
        let reservation = Self {
            directory,
            snapshot: Some(snapshot),
            identity,
            directory_identity: None,
            origin,
            adopted_lock: false,
            _retained_context: None,
            _thread_affine: PhantomData,
        };
        reservation.validate(lease)?;
        Ok(reservation)
    }

    /// Reserves a package directory that is anchored by a bare descriptor: a
    /// fresh open of `.` gives each caller its own open file description,
    /// and one nonblocking exclusive flock never waits, retries or falls back.
    pub(crate) fn acquire_directory(package: &OwnedFd) -> io::Result<Self> {
        let before = descriptor_identity(package.as_raw_fd())?;
        if before.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err(invalid());
        }
        // SAFETY: the fixed relative name is NUL terminated. openat creates a
        // new open file description rather than sharing one through dup.
        let raw = unsafe {
            libc::openat(
                package.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat returned a fresh owned descriptor.
        let directory = unsafe { OwnedFd::from_raw_fd(raw) };
        require_supported_filesystem(&directory)?;
        // SAFETY: the descriptor is owned and refers to the validated directory.
        // A single nonblocking call preserves EINTR and contention as failures.
        if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: getpid only queries the current process identity.
        let origin = unsafe { libc::getpid() };
        let reservation = Self {
            directory,
            snapshot: None,
            identity: [0; 6],
            directory_identity: Some((before.st_dev as u64, before.st_ino as u64)),
            origin,
            adopted_lock: false,
            _retained_context: None,
            _thread_affine: PhantomData,
        };
        reservation.validate_owned()?;
        Ok(reservation)
    }

    pub(crate) fn from_retained_directory<T: 'static>(
        lease: &PackageLease,
        directory: OwnedFd,
        context: T,
    ) -> io::Result<Self> {
        let identity = lease.ipc_identity().map_err(|_| invalid())?;
        let snapshot = lease.duplicate_for_reservation()?;
        snapshot.validate_reservation_directory(&directory)?;
        verify_retained_flock(&directory)?;
        // SAFETY: getpid only queries the current process identity.
        let origin = unsafe { libc::getpid() };
        let reservation = Self {
            directory,
            snapshot: Some(snapshot),
            identity,
            directory_identity: None,
            origin,
            adopted_lock: true,
            _retained_context: Some(Box::new(context)),
            _thread_affine: PhantomData,
        };
        reservation.validate(lease)?;
        Ok(reservation)
    }

    pub(super) fn validate_owned(&self) -> io::Result<()> {
        match &self.snapshot {
            Some(snapshot) => self.validate(snapshot),
            None => self.validate_directory_only(),
        }
    }

    /// Spawn gates need the retained package snapshot, not just the directory.
    pub(super) fn validate_for_spawn(&self) -> io::Result<()> {
        if self.snapshot.is_none() {
            return Err(invalid());
        }
        self.validate_owned()
    }

    fn validate_directory_only(&self) -> io::Result<()> {
        // SAFETY: getpid only queries the current process identity.
        if unsafe { libc::getpid() } != self.origin {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        let identity = descriptor_identity(self.directory.as_raw_fd())?;
        if identity.st_mode & libc::S_IFMT != libc::S_IFDIR
            || Some((identity.st_dev as u64, identity.st_ino as u64)) != self.directory_identity
        {
            return Err(invalid());
        }
        require_supported_filesystem(&self.directory)
    }

    pub(super) fn duplicate_locked_directory(&self) -> io::Result<OwnedFd> {
        self.validate_owned()?;
        verify_retained_flock(&self.directory)?;
        let duplicate = self.directory.try_clone()?;
        self.validate_owned()?;
        Ok(duplicate)
    }

    pub fn validate(&self, lease: &PackageLease) -> io::Result<()> {
        // SAFETY: getpid only queries the current process identity.
        if unsafe { libc::getpid() } != self.origin {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        let snapshot = self.snapshot.as_ref().ok_or_else(invalid)?;
        if lease.ipc_identity().map_err(|_| invalid())? != self.identity
            || snapshot.ipc_identity().map_err(|_| invalid())? != self.identity
        {
            return Err(invalid());
        }
        snapshot.validate_reservation_directory(&self.directory)?;
        if self.adopted_lock {
            verify_retained_flock(&self.directory)?;
        }
        require_supported_filesystem(&self.directory)
    }
}

fn invalid() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

/// Observes the exclusive flock retained by this descriptor without changing it.
pub fn verify_retained_flock(directory: &OwnedFd) -> io::Result<()> {
    // SAFETY: F_GETFD only queries flags on the borrowed descriptor.
    let flags = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::FD_CLOEXEC == 0 {
        return Err(invalid());
    }
    let identity = descriptor_identity(directory.as_raw_fd())?;
    if identity.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(invalid());
    }
    require_supported_filesystem(directory)?;
    let mut fdinfo = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(format!("/proc/self/fdinfo/{}", directory.as_raw_fd()))?;
    let source = descriptor_identity(fdinfo.as_raw_fd())?;
    if source.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(invalid());
    }
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs initializes the complete structure on success.
    if unsafe { libc::fstatfs(fdinfo.as_raw_fd(), filesystem.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the preceding fstatfs call succeeded.
    if unsafe { filesystem.assume_init() }.f_type != 0x9FA0 {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(4097);
    fdinfo.by_ref().take(4097).read_to_end(&mut bytes)?;
    parse_retained_flock(&bytes, identity.st_dev, identity.st_ino)
}

fn descriptor_identity(descriptor: libc::c_int) -> io::Result<libc::stat> {
    let mut identity = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes the complete structure on success.
    if unsafe { libc::fstat(descriptor, identity.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the preceding fstat call succeeded.
    Ok(unsafe { identity.assume_init() })
}

fn parse_retained_flock(bytes: &[u8], device: libc::dev_t, inode: libc::ino_t) -> io::Result<()> {
    if bytes.len() > 4096
        || bytes.last() != Some(&b'\n')
        || !bytes
            .iter()
            .all(|byte| matches!(byte, b' '..=b'~' | b'\t' | b'\n'))
    {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut retained = false;
    for line in text.lines() {
        let line = line.trim_start_matches([' ', '\t']);
        if !line.starts_with("lock:") {
            continue;
        }
        if retained {
            return Err(invalid());
        }
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if fields.len() != 9
            || fields[0] != "lock:"
            || fields[2] != "FLOCK"
            || fields[3] != "ADVISORY"
            || fields[4] != "WRITE"
            || fields[7] != "0"
            || fields[8] != "EOF"
        {
            return Err(invalid());
        }
        let ordinal = fields[1].strip_suffix(':').ok_or_else(invalid)?;
        if parse_lock_number(ordinal, 10)? == 0 {
            return Err(invalid());
        }
        let pid = fields[5];
        let digits = pid
            .strip_prefix('+')
            .or_else(|| pid.strip_prefix('-'))
            .unwrap_or(pid);
        if digits.is_empty()
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
            || pid.parse::<libc::pid_t>().is_err()
        {
            return Err(invalid());
        }
        let identity: Vec<_> = fields[6].split(':').collect();
        if identity.len() != 3
            || parse_lock_number(identity[0], 16)? != u64::from(libc::major(device))
            || parse_lock_number(identity[1], 16)? != u64::from(libc::minor(device))
            || parse_lock_number(identity[2], 10)? != inode
        {
            return Err(invalid());
        }
        retained = true;
    }
    if retained {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn parse_lock_number(text: &str, radix: u32) -> io::Result<u64> {
    if text.is_empty()
        || !text.bytes().all(|byte| match radix {
            10 => byte.is_ascii_digit(),
            16 => byte.is_ascii_hexdigit(),
            _ => false,
        })
    {
        return Err(invalid());
    }
    u64::from_str_radix(text, radix).map_err(|_| invalid())
}

fn require_supported_filesystem(directory: &OwnedFd) -> io::Result<()> {
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs initializes the complete statfs structure on success.
    if unsafe { libc::fstatfs(directory.as_raw_fd(), filesystem.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the preceding fstatfs call succeeded.
    let filesystem = unsafe { filesystem.assume_init() };
    if supported_filesystem(filesystem.f_type) {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP))
    }
}

/// `magic` is `statfs.f_type`, signed with glibc and not with musl.
fn supported_filesystem<T: TryInto<u64>>(magic: T) -> bool {
    magic.try_into().is_ok_and(|magic| magic == 0xEF53)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove owned reservation fixture");
        }
    }

    struct DropWitness(Arc<AtomicUsize>);

    impl Drop for DropWitness {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn public_lease() -> (TestDirectory, PackageLease) {
        let temporary_root = std::env::temp_dir();
        assert!(temporary_root.is_absolute(), "TMPDIR must be absolute");
        let path = temporary_root.join(format!(
            "vivac-reservation-context-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("new reservation fixture directory");
        let directory = TestDirectory(path);
        std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700))
            .expect("private reservation fixture directory");
        // PackageLease snapshots public bytes; it does not validate their crypto format.
        for (name, bytes) in [
            ("key.openssh", b"public ciphertext fixture".as_slice()),
            ("manifest.json", b"public manifest fixture".as_slice()),
        ] {
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.0.join(name))
                .expect("new public reservation fixture");
            file.write_all(bytes)
                .expect("public reservation fixture bytes");
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .expect("private reservation fixture file");
        }
        let capability: OwnedFd = File::open(&directory.0)
            .expect("reservation directory capability")
            .into();
        let cipher: OwnedFd = File::open(directory.0.join("key.openssh"))
            .expect("public ciphertext capability")
            .into();
        let lease = PackageLease::capture(capability, &cipher).expect("public package snapshot");
        (directory, lease)
    }

    #[test]
    fn reservation_retains_context_until_release() {
        let (_directory, lease) = public_lease();
        let drops = Arc::new(AtomicUsize::new(0));
        let reservation = StoreReservation::acquire_retaining(&lease, DropWitness(drops.clone()))
            .expect("ext filesystem reservation");
        drop(lease);
        reservation
            .validate_owned()
            .expect("retained package snapshot");
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(reservation);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn contended_reservation_releases_only_its_own_context() {
        let (_directory, lease) = public_lease();
        let first_drops = Arc::new(AtomicUsize::new(0));
        let first = StoreReservation::acquire_retaining(&lease, DropWitness(first_drops.clone()))
            .expect("first ext filesystem reservation");
        let refused_drops = Arc::new(AtomicUsize::new(0));
        let refused =
            StoreReservation::acquire_retaining(&lease, DropWitness(refused_drops.clone()));
        let error = refused.err().expect("physical reservation must contend");
        assert_eq!(error.raw_os_error(), Some(libc::EWOULDBLOCK));
        assert_eq!(refused_drops.load(Ordering::SeqCst), 1);
        assert_eq!(first_drops.load(Ordering::SeqCst), 0);
        first
            .validate_owned()
            .expect("first reservation remains valid");
        drop(first);
        assert_eq!(first_drops.load(Ordering::SeqCst), 1);
        let released = StoreReservation::acquire(&lease).expect("released physical reservation");
        drop(released);
    }

    #[test]
    fn directory_reservation_is_exclusive_and_never_waits() {
        let (directory, _lease) = public_lease();
        let package: OwnedFd = File::open(&directory.0)
            .expect("package directory capability")
            .into();
        let first = StoreReservation::acquire_directory(&package)
            .expect("ext filesystem directory reservation");
        first.validate_owned().expect("held directory reservation");
        let started = std::time::Instant::now();
        let second = StoreReservation::acquire_directory(&package);
        let error = second.err().expect("directory reservation must contend");
        assert_eq!(error.raw_os_error(), Some(libc::EWOULDBLOCK));
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        drop(first);
        StoreReservation::acquire_directory(&package).expect("released directory reservation");
    }

    #[test]
    fn reservation_accepts_the_ext_family() {
        // `f_type` is signed with glibc and not with musl.
        assert!(supported_filesystem(0xEF53_i64));
        assert!(supported_filesystem(0xEF53_u64));
    }

    #[test]
    fn reservation_refuses_unapproved_filesystems() {
        for magic in [0_i64, 0x0102_1994, 0x6969, 0x794c_7630, 0x9123_683e, -1] {
            assert!(!supported_filesystem(magic));
        }
    }

    #[test]
    fn retained_flock_observes_the_open_description() {
        let (directory, lease) = public_lease();
        let reservation = StoreReservation::acquire(&lease).expect("exclusive fixture flock");
        verify_retained_flock(&reservation.directory).expect("exclusive retained flock");
        let duplicate = reservation
            .directory
            .try_clone()
            .expect("duplicate fixture descriptor");
        verify_retained_flock(&duplicate).expect("duplicate retains the exclusive flock");
        let independent: OwnedFd = File::open(&directory.0)
            .expect("independent directory")
            .into();
        assert!(verify_retained_flock(&independent).is_err());
        // SAFETY: this fixture deliberately releases its own flock.
        assert_eq!(
            unsafe { libc::flock(duplicate.as_raw_fd(), libc::LOCK_UN) },
            0
        );
        assert!(verify_retained_flock(&duplicate).is_err());
        assert!(verify_retained_flock(&reservation.directory).is_err());
    }

    #[test]
    fn adopted_reservation_preserves_the_original_lock_description() {
        let (_directory, lease) = public_lease();
        let original = StoreReservation::acquire(&lease).expect("original exclusive flock");
        let adopted = StoreReservation::from_retained_directory(
            &lease,
            original
                .directory
                .try_clone()
                .expect("same lock description"),
            (),
        )
        .expect("adopt retained flock");
        adopted
            .validate(&lease)
            .expect("adopted snapshot remains valid");
        drop(adopted);
        assert_eq!(
            StoreReservation::acquire(&lease)
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EWOULDBLOCK)
        );
        let adopted = StoreReservation::from_retained_directory(
            &lease,
            original
                .directory
                .try_clone()
                .expect("same lock description"),
            (),
        )
        .expect("adopt another retained duplicate");
        drop(original);
        adopted
            .validate(&lease)
            .expect("duplicate retains the flock");
        assert_eq!(
            StoreReservation::acquire(&lease)
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EWOULDBLOCK)
        );
        drop(adopted);
        StoreReservation::acquire(&lease).expect("last description released");
    }

    #[test]
    fn adopted_reservation_rejects_independent_or_unlocked_descriptions() {
        let (directory, lease) = public_lease();
        let original = StoreReservation::acquire(&lease).expect("original exclusive flock");
        let independent = File::open(&directory.0)
            .expect("independent directory")
            .into();
        assert!(StoreReservation::from_retained_directory(&lease, independent, ()).is_err());
        let adopted = StoreReservation::from_retained_directory(
            &lease,
            original
                .directory
                .try_clone()
                .expect("same lock description"),
            (),
        )
        .expect("adopt retained flock");
        // SAFETY: this fixture deliberately unlocks its own shared description.
        assert_eq!(
            unsafe { libc::flock(original.directory.as_raw_fd(), libc::LOCK_UN) },
            0
        );
        assert!(adopted.validate(&lease).is_err());
    }

    #[test]
    fn retained_flock_refuses_shared_unlocked_and_non_directory_descriptors() {
        let (directory, _lease) = public_lease();
        let descriptor: OwnedFd = File::open(&directory.0).expect("fixture directory").into();
        assert!(verify_retained_flock(&descriptor).is_err());
        // SAFETY: this fixture acquires a shared flock on its owned descriptor.
        assert_eq!(
            unsafe { libc::flock(descriptor.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
            0
        );
        assert!(verify_retained_flock(&descriptor).is_err());
        let file: OwnedFd = File::open(directory.0.join("manifest.json"))
            .expect("fixture file")
            .into();
        assert!(verify_retained_flock(&file).is_err());
    }

    #[test]
    fn retained_flock_refuses_inheritable_descriptors() {
        let (_directory, lease) = public_lease();
        let reservation = StoreReservation::acquire(&lease).expect("exclusive fixture flock");
        let duplicate = reservation
            .directory
            .try_clone()
            .expect("duplicate fixture descriptor");
        // SAFETY: this fixture changes only its owned duplicate's descriptor flags.
        assert_eq!(
            unsafe { libc::fcntl(duplicate.as_raw_fd(), libc::F_SETFD, 0) },
            0
        );
        assert!(verify_retained_flock(&duplicate).is_err());
        verify_retained_flock(&reservation.directory).expect("original flags remain private");
    }

    #[test]
    fn retained_flock_parser_accepts_exact_identity_and_signed_pid() {
        let device = libc::makedev(8, 17);
        for pid in ["123", "-1", "+42", "0"] {
            let row =
                format!("pos:\t0\nlock:\t  1:  FLOCK\tADVISORY  WRITE {pid} 08:11:987 0 EOF\n");
            parse_retained_flock(row.as_bytes(), device, 987).expect("matching exclusive row");
        }
    }

    #[test]
    fn retained_flock_parser_refuses_ambiguous_shape_and_identity() {
        let device = libc::makedev(8, 17);
        let valid = "lock: 1: FLOCK ADVISORY WRITE 123 08:11:987 0 EOF\n";
        for bytes in [
            String::new(),
            "pos: 0\n".to_owned(),
            valid.trim_end().to_owned(),
            format!("{valid}{valid}"),
            valid.replace("1:", "0:"),
            valid.replace("1:", "-1:"),
            valid.replace("1:", "1"),
            valid.replace("FLOCK", "POSIX"),
            valid.replace("ADVISORY", "MANDATORY"),
            valid.replace("WRITE", "READ"),
            valid.replace("123", "pid"),
            valid.replace("123", "2147483648"),
            valid.replace("08:11", "09:11"),
            valid.replace("08:11", "08:12"),
            valid.replace("08:11", "0x8:11"),
            valid.replace("987", "988"),
            valid.replace("987", "-987"),
            valid.replace("987", "18446744073709551616"),
            valid.replace(" 0 EOF", " 1 EOF"),
            valid.replace("EOF", "99"),
            valid.replace("EOF", "EOF extra"),
            valid.replace("FLOCK", "-> FLOCK"),
            valid.replace(' ', "\r"),
            format!("pos: \0\n{valid}"),
            format!("pos: é\n{valid}"),
        ] {
            assert!(
                parse_retained_flock(bytes.as_bytes(), device, 987).is_err(),
                "accepted {bytes:?}"
            );
        }
    }

    #[test]
    fn retained_flock_parser_enforces_the_read_budget() {
        let device = libc::makedev(8, 17);
        let row = "lock: 1: FLOCK ADVISORY WRITE 123 08:11:987 0 EOF\n";
        let mut bytes = vec![b' '; 4096 - row.len() - 1];
        bytes.push(b'\n');
        bytes.extend_from_slice(row.as_bytes());
        parse_retained_flock(&bytes, device, 987).expect("exactly bounded fdinfo");
        bytes.push(b'\n');
        assert!(parse_retained_flock(&bytes, device, 987).is_err());
    }
}
