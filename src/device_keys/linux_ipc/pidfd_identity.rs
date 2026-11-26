use super::super::require_procfs;
use super::{invalid, validate_reference};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

const MAX_FDINFO: u64 = 4096;

/// Compares ordered live process references in one trusted procfs namespace.
/// The expected references must come from the local launch context.
pub fn verify_live_process_pair(expected: &[OwnedFd; 2], offered: &[OwnedFd; 2]) -> io::Result<()> {
    let [first, second, offered_first, offered_second] =
        read_live_processes([&expected[0], &expected[1], &offered[0], &offered[1]])?;
    if first == second || first != offered_first || second != offered_second {
        return Err(invalid());
    }
    Ok(())
}

/// Compares a connected process reference with the retained local expectation.
pub fn verify_live_process(expected: &OwnedFd, offered: &OwnedFd) -> io::Result<()> {
    let [expected, offered] = read_live_processes([expected, offered])?;
    if expected != offered {
        return Err(invalid());
    }
    Ok(())
}

/// The pid of one live process reference, read while the reference stays live.
pub fn live_pid(reference: &OwnedFd) -> io::Result<libc::pid_t> {
    let [pid] = read_live_processes([reference])?;
    Ok(pid)
}

fn read_live_processes<const N: usize>(references: [&OwnedFd; N]) -> io::Result<[libc::pid_t; N]> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/proc/self/fdinfo")?;
    require_procfs(&directory)?;
    let device = directory.metadata()?.dev();

    for reference in references {
        validate_process_reference(reference)?;
    }
    let mut observed = [0; N];
    for (pid, reference) in observed.iter_mut().zip(references) {
        *pid = read_pid(&directory, device, reference)?;
    }
    // All references stayed live between the readings. A numeric PID could only
    // have been recycled if a retained reference ceased to refer to a live task.
    for reference in references {
        validate_process_reference(reference)?;
    }
    Ok(observed)
}

pub(super) fn validate_process_reference(reference: &OwnedFd) -> io::Result<()> {
    validate_reference(reference)?;
    // SAFETY: F_GETFL only queries the borrowed file description's flags.
    let flags = unsafe { libc::fcntl(reference.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // Linux uses O_EXCL for a thread pidfd and O_TRUNC for an autokill pidfd.
    if flags & (libc::O_EXCL | libc::O_TRUNC) != 0 {
        return Err(invalid());
    }
    Ok(())
}

fn read_pid(directory: &File, device: u64, reference: &OwnedFd) -> io::Result<libc::pid_t> {
    let slot = CString::new(reference.as_raw_fd().to_string()).map_err(|_| invalid())?;
    // SAFETY: slot is a single NUL-terminated numeric component, relative to
    // the retained procfs directory. O_RDONLY does not use a mode argument.
    let raw = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            slot.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat returned a fresh, owned file descriptor.
    let file = unsafe { File::from_raw_fd(raw) };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.dev() != device {
        return Err(invalid());
    }
    require_procfs(&file)?;
    let mut bytes = Vec::new();
    file.take(MAX_FDINFO + 1).read_to_end(&mut bytes)?;
    parse_pid(&bytes)
}

fn parse_pid(bytes: &[u8]) -> io::Result<libc::pid_t> {
    if bytes.len() as u64 > MAX_FDINFO || bytes.last() != Some(&b'\n') {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut rows = text
        .split_terminator('\n')
        .filter(|line| line.starts_with("Pid:"));
    let field = rows
        .next()
        .and_then(|line| line.strip_prefix("Pid:\t"))
        .ok_or_else(invalid)?;
    if rows.next().is_some()
        || field.starts_with('0')
        || !field.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    field.parse().map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pidfd(pid: libc::pid_t) -> OwnedFd {
        // SAFETY: pidfd_open uses scalar arguments and returns an owned descriptor.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
        assert!(raw >= 3);
        // SAFETY: successful pidfd_open returned this fresh descriptor.
        unsafe { OwnedFd::from_raw_fd(raw as libc::c_int) }
    }

    #[test]
    fn live_process_comparison_requires_the_same_process_reference() {
        // SAFETY: getpid and getppid only query process identity.
        let (current, parent) = unsafe { (libc::getpid(), libc::getppid()) };
        assert!(parent > 1 && current != parent);
        let expected = pidfd(current);
        let independently_opened = pidfd(current);
        let other = pidfd(parent);
        verify_live_process(&expected, &independently_opened).unwrap();
        assert!(verify_live_process(&expected, &other).is_err());
        let ordinary_file: OwnedFd = File::open("/dev/null").unwrap().into();
        assert!(verify_live_process(&expected, &ordinary_file).is_err());
        assert!(verify_live_process(&ordinary_file, &expected).is_err());
    }

    #[test]
    fn parses_only_a_single_canonical_live_pid_row() {
        assert_eq!(
            parse_pid(b"pos:\t0\nPid:\t21\nNSpid:\t21\t1\n").unwrap(),
            21
        );
        for malformed in [
            "NSpid:\t21\n",
            "Pid:\t0\n",
            "Pid:\t-1\n",
            "Pid:\t021\n",
            "Pid:\t+21\n",
            "Pid:\t2147483648\n",
            "Pid:\t21\nPid:\t21\n",
            "Pid:\t21\nPid:22\n",
            "Pid:\t21",
        ] {
            assert!(parse_pid(malformed.as_bytes()).is_err(), "{malformed:?}");
        }
        assert!(parse_pid(&[b'\n'; (MAX_FDINFO + 1) as usize]).is_err());
    }
}
