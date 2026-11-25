//! Kernel evidence for a session: the delegated `session` cgroup under the
//! keeper's user service. The keeper stays in the unit cgroup root and never
//! writes `cgroup.subtree_control`; the session's processes run in
//! `<unit cgroup>/session`, and only systemd removes cgroups. Identity is the
//! inode of the cgroup directory.

use std::ffi::{CStr, CString};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

const CGROUP_ROOT: &CStr = c"/sys/fs/cgroup";
const CGROUP2_SUPER_MAGIC: u64 = 0x6367_7270;
const SESSION_NAME: &CStr = c"session";
const MEMBERSHIP_LIMIT: u64 = 4096;
const EVENTS_LIMIT: usize = 512;

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn errno(code: libc::c_int) -> io::Error {
    io::Error::from_raw_os_error(code)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CgroupIdentity {
    pub inode: u64,
}

fn read_limited(path: &str, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    Ok(bytes)
}

/// The single unified-hierarchy entry of a `/proc/<pid>/cgroup` file.
fn parse_membership(bytes: &[u8]) -> io::Result<&str> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let line = text.strip_suffix('\n').ok_or_else(invalid)?;
    let path = line.strip_prefix("0::").ok_or_else(invalid)?;
    if !path.starts_with('/') || path.contains('\n') || path.contains('\0') {
        return Err(invalid());
    }
    Ok(path)
}

/// The `populated` value of a `cgroup.events` file, read from its own line.
fn parse_populated(bytes: &[u8]) -> io::Result<bool> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut found = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("populated ") {
            let value = match value {
                "0" => false,
                "1" => true,
                _ => return Err(invalid()),
            };
            if found.replace(value).is_some() {
                return Err(invalid());
            }
        }
    }
    found.ok_or_else(invalid)
}

pub(crate) fn validate_unit_path(path: &str, uid: u32, unit: &str) -> io::Result<()> {
    let prefix = format!("/user.slice/user-{uid}.slice/user@{uid}.service/");
    let rest = path.strip_prefix(prefix.as_str()).ok_or_else(invalid)?;
    let mut last = None;
    for component in rest.split('/') {
        if component.is_empty() || component == "." || component == ".." || component.contains('\0')
        {
            return Err(invalid());
        }
        last = Some(component);
    }
    if last != Some(unit) {
        return Err(invalid());
    }
    Ok(())
}

fn parse_boot_id(bytes: &[u8]) -> io::Result<String> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let id = text.strip_suffix('\n').ok_or_else(invalid)?;
    let canonical = id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        });
    if !canonical {
        return Err(invalid());
    }
    Ok(id.to_owned())
}

pub(crate) fn boot_id() -> io::Result<String> {
    parse_boot_id(&read_limited("/proc/sys/kernel/random/boot_id", 64)?)
}

fn peer_matches(bytes: &[u8], path: &str) -> io::Result<bool> {
    Ok(parse_membership(bytes)? == path)
}

/// Whether `pid` is in exactly the cgroup at `path`, not below it: the keeper
/// lives in the unit cgroup itself and the session's processes one level down.
pub(crate) fn peer_in_cgroup(pid: libc::pid_t, path: &str) -> io::Result<bool> {
    if pid <= 0 || !path.starts_with('/') {
        return Err(errno(libc::EINVAL));
    }
    let bytes = read_limited(&format!("/proc/{pid}/cgroup"), MEMBERSHIP_LIMIT)?;
    peer_matches(&bytes, path)
}

fn filesystem_is_cgroup2(fd: &OwnedFd) -> io::Result<()> {
    let mut value = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs initializes `value` on success and `fd` is live.
    if unsafe { libc::fstatfs(fd.as_raw_fd(), value.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatfs succeeded.
    let value = unsafe { value.assume_init() };
    if value.f_type as u64 != CGROUP2_SUPER_MAGIC {
        return Err(errno(libc::ENODEV));
    }
    Ok(())
}

fn open_at(parent: libc::c_int, name: &CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: (flags | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64,
        mode: 0,
        resolve: libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS,
    };
    // SAFETY: `name` is terminated and `how` has the kernel's open_how layout.
    let raw = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            parent,
            name.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful openat2 returned a fresh descriptor owned only here.
    Ok(unsafe { OwnedFd::from_raw_fd(raw as libc::c_int) })
}

fn open_root() -> io::Result<OwnedFd> {
    open_root_at(CGROUP_ROOT)
}

fn open_root_at(root: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: the path is terminated; a successful open returns a fresh descriptor.
    let raw = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `open` returned a fresh descriptor owned only here.
    let root = unsafe { OwnedFd::from_raw_fd(raw) };
    filesystem_is_cgroup2(&root)?;
    Ok(root)
}

/// Opens a cgroup directory below the cgroup2 mount, never through a symlink.
fn open_cgroup(path: &str) -> io::Result<OwnedFd> {
    let relative = path.strip_prefix('/').ok_or_else(|| errno(libc::EINVAL))?;
    if relative.is_empty() {
        return Err(errno(libc::EINVAL));
    }
    let name = CString::new(relative).map_err(|_| errno(libc::EINVAL))?;
    let root = open_root()?;
    open_below(&root, &name)
}

fn open_below(root: &OwnedFd, name: &CStr) -> io::Result<OwnedFd> {
    let directory = open_at(root.as_raw_fd(), name, libc::O_RDONLY | libc::O_DIRECTORY)?;
    filesystem_is_cgroup2(&directory)?;
    Ok(directory)
}

fn inode(fd: &OwnedFd) -> io::Result<u64> {
    let mut value = std::mem::MaybeUninit::<libc::statx>::uninit();
    // SAFETY: AT_EMPTY_PATH selects this live descriptor; statx initializes `value`.
    if unsafe {
        libc::syscall(
            libc::SYS_statx,
            fd.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
            libc::STATX_INO,
            value.as_mut_ptr(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statx succeeded.
    let value = unsafe { value.assume_init() };
    if value.stx_mask & libc::STATX_INO == 0 {
        return Err(errno(libc::EIO));
    }
    Ok(value.stx_ino)
}

fn owner_uid(fd: &OwnedFd) -> io::Result<u32> {
    let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes `value` on success and `fd` is live.
    if unsafe { libc::fstat(fd.as_raw_fd(), value.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstat succeeded.
    Ok(unsafe { value.assume_init() }.st_uid)
}

/// A control file must exist below the cgroup and belong to the account.
fn require_owned_file(directory: &OwnedFd, name: &CStr, uid: u32) -> io::Result<()> {
    let file = open_at(directory.as_raw_fd(), name, libc::O_PATH)?;
    if owner_uid(&file)? != uid {
        return Err(errno(libc::EACCES));
    }
    Ok(())
}

fn read_events(directory: &OwnedFd) -> io::Result<bool> {
    let file = open_at(directory.as_raw_fd(), c"cgroup.events", libc::O_RDONLY)?;
    read_populated(&file)
}

fn read_populated(file: &OwnedFd) -> io::Result<bool> {
    let mut buffer = [0_u8; EVENTS_LIMIT];
    // SAFETY: the buffer is writable for its whole length; offset 0 re-reads
    // the file from its start.
    let count = unsafe {
        libc::pread(
            file.as_raw_fd(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
        )
    };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    let count = count as usize;
    if count == buffer.len() {
        return Err(invalid());
    }
    parse_populated(&buffer[..count])
}

pub(crate) struct UnitCgroup {
    directory: OwnedFd,
    path: String,
    identity: CgroupIdentity,
    uid: u32,
}

impl UnitCgroup {
    /// The cgroup of the calling unit, accepted only when it is the named
    /// unit below this account's user manager.
    pub(crate) fn current(unit_name: &str) -> io::Result<Self> {
        // SAFETY: geteuid only reads this process's credentials.
        let uid = unsafe { libc::geteuid() };
        let bytes = read_limited("/proc/self/cgroup", MEMBERSHIP_LIMIT)?;
        let path = parse_membership(&bytes)?;
        validate_unit_path(path, uid, unit_name)?;
        let directory = open_cgroup(path)?;
        require_owned_file(&directory, c"cgroup.kill", uid)?;
        require_owned_file(&directory, c"cgroup.procs", uid)?;
        let identity = CgroupIdentity {
            inode: inode(&directory)?,
        };
        Ok(Self {
            directory,
            path: path.to_owned(),
            identity,
            uid,
        })
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn identity(&self) -> CgroupIdentity {
        self.identity
    }

    /// Creates the `session` child. An existing one is refused: nothing but
    /// systemd removes cgroups, so a leftover may still hold processes.
    pub(crate) fn create_session(&self) -> io::Result<SessionCgroup> {
        // SAFETY: the directory descriptor is live and the name is terminated.
        if unsafe { libc::mkdirat(self.directory.as_raw_fd(), SESSION_NAME.as_ptr(), 0o755) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let directory = open_at(
            self.directory.as_raw_fd(),
            SESSION_NAME,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        filesystem_is_cgroup2(&directory)?;
        for name in [c"cgroup.kill", c"cgroup.procs", c"cgroup.events"] {
            require_owned_file(&directory, name, self.uid)?;
        }
        let identity = CgroupIdentity {
            inode: inode(&directory)?,
        };
        Ok(SessionCgroup {
            directory,
            identity,
        })
    }
}

pub(crate) struct SessionCgroup {
    directory: OwnedFd,
    identity: CgroupIdentity,
}

impl SessionCgroup {
    pub(crate) fn identity(&self) -> CgroupIdentity {
        self.identity
    }

    /// The placement file a child writes `0` to before exec.
    pub(crate) fn procs(&self) -> io::Result<OwnedFd> {
        open_at(self.directory.as_raw_fd(), c"cgroup.procs", libc::O_WRONLY)
    }

    pub(crate) fn kill(&self) -> io::Result<()> {
        let file = open_at(self.directory.as_raw_fd(), c"cgroup.kill", libc::O_WRONLY)?;
        // SAFETY: the buffer is one static byte and the descriptor is live.
        let written = unsafe { libc::write(file.as_raw_fd(), b"1".as_ptr().cast(), 1) };
        match written {
            1 => Ok(()),
            n if n < 0 => Err(io::Error::last_os_error()),
            _ => Err(errno(libc::EIO)),
        }
    }

    /// Kills every process in the session and waits for `populated 0`. The
    /// events file is read, then polled with `POLLPRI`, and re-read from
    /// offset 0 after each wake.
    pub(crate) fn kill_and_wait_empty(&self, deadline: Instant) -> io::Result<EmptySessionCgroup> {
        let events = open_at(self.directory.as_raw_fd(), c"cgroup.events", libc::O_RDONLY)?;
        self.kill()?;
        loop {
            if !read_populated(&events)? {
                return Ok(EmptySessionCgroup {
                    identity: self.identity,
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(errno(libc::ETIMEDOUT));
            }
            let milliseconds = remaining
                .saturating_add(Duration::from_millis(1))
                .as_millis()
                .min(libc::c_int::MAX as u128) as libc::c_int;
            let mut watch = libc::pollfd {
                fd: events.as_raw_fd(),
                events: libc::POLLPRI,
                revents: 0,
            };
            // SAFETY: `watch` is one live pollfd.
            if unsafe { libc::poll(&mut watch, 1, milliseconds) } < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EINTR) {
                    return Err(error);
                }
            }
        }
    }
}

/// Proof that the session cgroup was observed empty after a kill.
pub(crate) struct EmptySessionCgroup {
    identity: CgroupIdentity,
}

impl EmptySessionCgroup {
    #[cfg(test)]
    pub(crate) fn for_test(inode: u64) -> Self {
        Self {
            identity: CgroupIdentity { inode },
        }
    }

    pub(crate) fn identity(&self) -> CgroupIdentity {
        self.identity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CgroupObservation {
    Absent,
    Replaced,
    Populated,
    Empty,
}

/// Looks at a recorded cgroup: gone, a different directory under the same
/// name, or the same one with or without processes.
pub(crate) fn observe(path: &str, expected: CgroupIdentity) -> io::Result<CgroupObservation> {
    observe_below(CGROUP_ROOT, path, expected)
}

fn observe_below(
    root: &CStr,
    path: &str,
    expected: CgroupIdentity,
) -> io::Result<CgroupObservation> {
    observe_in(open_root_at(root), path, expected)
}

/// Only a failed lookup beneath a root that opened and validated counts as
/// absence; a root that cannot be opened is no evidence about the path.
fn observe_in(
    root: io::Result<OwnedFd>,
    path: &str,
    expected: CgroupIdentity,
) -> io::Result<CgroupObservation> {
    let root = root?;
    let relative = path.strip_prefix('/').ok_or_else(|| errno(libc::EINVAL))?;
    if relative.is_empty() {
        return Err(errno(libc::EINVAL));
    }
    let name = CString::new(relative).map_err(|_| errno(libc::EINVAL))?;
    let directory = match open_below(&root, &name) {
        Ok(directory) => directory,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
            return Ok(CgroupObservation::Absent)
        }
        Err(error) => return Err(error),
    };
    if inode(&directory)? != expected.inode {
        return Ok(CgroupObservation::Replaced);
    }
    match read_events(&directory) {
        Ok(true) => Ok(CgroupObservation::Populated),
        Ok(false) => Ok(CgroupObservation::Empty),
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(CgroupObservation::Absent),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgroup_membership_requires_a_single_unified_entry() {
        assert_eq!(parse_membership(b"0::/a\n").unwrap(), "/a");
        assert_eq!(
            parse_membership(b"0::/a/b.service\n").unwrap(),
            "/a/b.service"
        );
        for bad in [
            &b"1:name=systemd:/a\n"[..],
            b"0::/a\n0::/b\n",
            b"0::/a\n1:cpu:/b\n",
            b"0::/a",
            b"0::a\n",
            b"0::\n",
            b"",
            b"\n",
            b"0::/a\n\n",
            b"0::/\xff\n",
        ] {
            assert!(parse_membership(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn populated_is_read_from_its_own_line() {
        assert!(!parse_populated(b"populated 0\nfrozen 0\n").unwrap());
        assert!(parse_populated(b"populated 1\n").unwrap());
        assert!(parse_populated(b"frozen 1\npopulated 1\n").unwrap());
        for bad in [
            &b"populated 2"[..],
            b"frozen 0\n",
            b"",
            b"populated\n",
            b"populated 1\npopulated 0\n",
            b"populated 01\n",
            b"xpopulated 1\n",
        ] {
            assert!(parse_populated(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn unit_path_must_sit_under_the_account_manager() {
        let base = "/user.slice/user-1000.slice/user@1000.service";
        let unit = "vivac-session.service";
        assert!(validate_unit_path(&format!("{base}/app.slice/{unit}"), 1000, unit).is_ok());
        assert!(validate_unit_path(&format!("{base}/{unit}"), 1000, unit).is_ok());
        assert!(validate_unit_path(&format!("{base}/app.slice/{unit}"), 1001, unit).is_err());
        for bad in [
            format!("{base}/app.slice/../{unit}"),
            format!("{base}/./{unit}"),
            format!("{base}/app.slice/{unit}/session"),
            format!("{base}/app.slice/{unit}/"),
            format!("{base}/app.slice/other.service"),
            format!("{base}//{unit}"),
            format!("{base}/"),
            base.to_string(),
            format!("/user.slice/user-1000.slice/session-3.scope/{unit}"),
            format!("/system.slice/{unit}"),
            format!("user.slice/user-1000.slice/user@1000.service/{unit}"),
        ] {
            assert!(validate_unit_path(&bad, 1000, unit).is_err(), "{bad}");
        }
        assert!(validate_unit_path(&format!("{base}/app.slice/{unit}"), 1000, "other").is_err());
    }

    #[test]
    fn boot_id_accepts_only_canonical_uuid_text() {
        let good = "0123abcd-4567-89ab-cdef-0123456789ab";
        assert_eq!(parse_boot_id(format!("{good}\n").as_bytes()).unwrap(), good);
        for bad in [
            good.to_string(),
            format!("{good}\n\n"),
            format!("{}\n", good.to_uppercase()),
            format!("{}\n", good.replace('-', "")),
            "0123abcd-4567-89ab-cdef-0123456789a\n".to_string(),
            "0123abcd-4567-89ab-cdef-0123456789abc\n".to_string(),
            "0123abcd-4567-89ab-cdef-0123456789ag\n".to_string(),
            "\n".to_string(),
            String::new(),
        ] {
            assert!(parse_boot_id(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn empty_proof_carries_only_its_identity() {
        assert_eq!(
            EmptySessionCgroup::for_test(7).identity(),
            CgroupIdentity { inode: 7 }
        );
    }

    #[test]
    fn peer_matches_only_the_exact_cgroup() {
        let path = "/user.slice/user-1000.slice/user@1000.service/app.slice/u.service";
        assert!(peer_matches(format!("0::{path}\n").as_bytes(), path).unwrap());
        assert!(!peer_matches(format!("0::{path}/session\n").as_bytes(), path).unwrap());
        assert!(!peer_matches(format!("0::{path}x\n").as_bytes(), path).unwrap());
        assert!(!peer_matches(b"0::/other\n", path).unwrap());
        assert!(peer_matches(b"1:name=x:/a\n", path).is_err());
    }

    #[test]
    fn peer_path_must_be_absolute_and_non_empty() {
        // SAFETY: getpid has no arguments.
        let pid = unsafe { libc::getpid() };
        for bad in ["", "relative/path"] {
            let error = peer_in_cgroup(pid, bad).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EINVAL), "{bad:?}");
        }
    }

    #[test]
    fn a_missing_cgroup_root_is_not_evidence_of_absence() {
        let expected = CgroupIdentity { inode: 1 };
        let error = observe_below(c"/nonexistent-cgroup-root", "/a/b", expected).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
        let failed = Err(errno(libc::EACCES));
        let error = observe_in(failed, "/a/b", expected).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EACCES));
    }

    #[test]
    fn a_missing_path_below_a_present_root_is_absent() {
        let root = std::env::temp_dir();
        let root = OwnedFd::from(std::fs::File::open(&root).expect("open temp directory"));
        let seen = observe_in(
            Ok(root),
            "/vivac-no-such-cgroup/session",
            CgroupIdentity { inode: 1 },
        );
        assert_eq!(seen.unwrap(), CgroupObservation::Absent);
    }
}
