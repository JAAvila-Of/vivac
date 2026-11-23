//! Identity-checked removal of an entry in a retained directory.
//!
//! The name is first moved aside, so the check and the removal act on the same
//! object and a replacement racing the caller is never deleted.
#![allow(
    dead_code,
    reason = "The session commands consume these pieces in later steps."
)]

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntryIdentity {
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub links: u64,
}

impl EntryIdentity {
    // The field widths differ between Linux targets.
    #[allow(clippy::unnecessary_cast)]
    fn from_stat(stat: &libc::stat) -> Self {
        Self {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
            mode: stat.st_mode as u32,
            uid: stat.st_uid,
            gid: stat.st_gid,
            links: stat.st_nlink as u64,
        }
    }

    pub(crate) fn from_descriptor(fd: &OwnedFd) -> io::Result<Self> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
        // SAFETY: fstat writes one stat record for this live descriptor.
        if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful fstat initialised the record.
        Ok(Self::from_stat(&unsafe { stat.assume_init() }))
    }

    fn is_directory(&self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFDIR
    }
}

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

/// The identity of `name` without following a symbolic link, or `None` when
/// the name is absent.
pub(crate) fn entry_identity(
    directory: &OwnedFd,
    name: &CStr,
) -> io::Result<Option<EntryIdentity>> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: the descriptor is live, the name is terminated and fstatat
    // writes one stat record on success.
    let status = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status < 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    // SAFETY: a successful fstatat initialised the record.
    Ok(Some(EntryIdentity::from_stat(&unsafe {
        stat.assume_init()
    })))
}

pub(crate) fn rename_no_replace(directory: &OwnedFd, from: &CStr, to: &CStr) -> io::Result<()> {
    // SAFETY: both names are terminated single components of one live directory.
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn sync_directory(directory: &OwnedFd) -> io::Result<()> {
    // SAFETY: fsync only operates on this live descriptor.
    if unsafe { libc::fsync(directory.as_raw_fd()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn random_hex() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| io::Error::other("random bytes"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Removes `name` only if it is still the object described by `expected`.
///
/// A mismatch puts the object back and fails with `InvalidData`. If it cannot
/// be put back, that error is returned and the object stays under its aside
/// name: nothing that failed the check is ever deleted. The caller
/// synchronizes the directory.
pub(crate) fn remove_retained_entry(
    directory: &OwnedFd,
    name: &CStr,
    expected: &EntryIdentity,
) -> io::Result<()> {
    let aside = CString::new(format!(".removing-{}", random_hex()?)).map_err(|_| invalid())?;
    rename_no_replace(directory, name, &aside)?;
    let failure = match entry_identity(directory, &aside) {
        Ok(Some(actual)) if actual == *expected => {
            let flags = if actual.is_directory() {
                libc::AT_REMOVEDIR
            } else {
                0
            };
            // SAFETY: the aside name is terminated and relative to this
            // directory, and it was just checked to be the expected object.
            if unsafe { libc::unlinkat(directory.as_raw_fd(), aside.as_ptr(), flags) } >= 0 {
                return Ok(());
            }
            io::Error::last_os_error()
        }
        Ok(_) => invalid(),
        Err(error) => error,
    };
    rename_no_replace(directory, &aside, name)?;
    Err(failure)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(crate) struct Scratch(pub std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn scratch() -> (Scratch, OwnedFd) {
        let path = std::env::temp_dir().join(format!("vivac-retained-{}", random_hex().unwrap()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fd = std::fs::File::open(&path).unwrap().into();
        (Scratch(path), fd)
    }

    fn names(path: &std::path::Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn retained_removal_removes_the_retained_entry() {
        let (dir, fd) = scratch();
        std::fs::write(dir.0.join("entry"), b"bytes").unwrap();
        let identity = entry_identity(&fd, c"entry").unwrap().unwrap();
        remove_retained_entry(&fd, c"entry", &identity).unwrap();
        assert!(names(&dir.0).is_empty());
        assert_eq!(entry_identity(&fd, c"entry").unwrap(), None);
    }

    #[test]
    fn retained_removal_removes_an_empty_directory() {
        let (dir, fd) = scratch();
        std::fs::create_dir(dir.0.join("entry")).unwrap();
        let identity = entry_identity(&fd, c"entry").unwrap().unwrap();
        remove_retained_entry(&fd, c"entry", &identity).unwrap();
        assert!(names(&dir.0).is_empty());
    }

    #[test]
    fn retained_removal_refuses_a_replaced_entry_and_leaves_it() {
        let (dir, fd) = scratch();
        std::fs::write(dir.0.join("entry"), b"first").unwrap();
        let identity = entry_identity(&fd, c"entry").unwrap().unwrap();
        std::fs::write(dir.0.join("other"), b"second").unwrap();
        std::fs::rename(dir.0.join("other"), dir.0.join("entry")).unwrap();
        let error = remove_retained_entry(&fd, c"entry", &identity).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(dir.0.join("entry")).unwrap(), b"second");
        assert_eq!(names(&dir.0), ["entry"]);
    }

    #[test]
    fn retained_removal_restores_an_entry_it_cannot_unlink() {
        let (dir, fd) = scratch();
        std::fs::create_dir(dir.0.join("entry")).unwrap();
        std::fs::write(dir.0.join("entry/inner"), b"kept").unwrap();
        let identity = entry_identity(&fd, c"entry").unwrap().unwrap();
        let error = remove_retained_entry(&fd, c"entry", &identity).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOTEMPTY));
        assert_eq!(names(&dir.0), ["entry"]);
        assert_eq!(std::fs::read(dir.0.join("entry/inner")).unwrap(), b"kept");
    }

    #[test]
    fn retained_removal_refuses_an_absent_entry() {
        let (dir, fd) = scratch();
        std::fs::write(dir.0.join("entry"), b"first").unwrap();
        let identity = entry_identity(&fd, c"entry").unwrap().unwrap();
        std::fs::remove_file(dir.0.join("entry")).unwrap();
        let error = remove_retained_entry(&fd, c"entry", &identity).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
        assert!(names(&dir.0).is_empty());
    }

    #[test]
    fn entry_identity_does_not_follow_a_symbolic_link() {
        let (dir, fd) = scratch();
        std::fs::write(dir.0.join("target"), b"x").unwrap();
        std::os::unix::fs::symlink("target", dir.0.join("link")).unwrap();
        let link = entry_identity(&fd, c"link").unwrap().unwrap();
        let target = entry_identity(&fd, c"target").unwrap().unwrap();
        assert_ne!(link.inode, target.inode);
        assert_eq!(link.mode & libc::S_IFMT, libc::S_IFLNK);
    }
}
