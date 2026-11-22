use serde::{Deserialize, Serialize};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(crate) const MAX_BINDING: usize = 512;

fn invalid() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Public installation binding. A valid encoding does not prove random origin.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub version: u64,
    pub installation_id: String,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub profile_revision: u64,
}

impl Binding {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        require(bytes.len() <= MAX_BINDING)?;
        let value: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        require(value.canonical_bytes()? == bytes)?;
        Ok(value)
    }

    pub fn canonical_bytes(&self) -> io::Result<Vec<u8>> {
        require(self.version == 1 && self.profile_revision == 1)?;
        require(
            self.owner_uid != 0
                && self.owner_gid != 0
                && self.owner_uid != u32::MAX
                && self.owner_gid != u32::MAX,
        )?;
        let id = self.installation_id.as_bytes();
        require(
            id.len() == 26
                && matches!(id[0], b'0'..=b'7')
                && id
                    .iter()
                    .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(byte)),
        )?;
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        require(bytes.len() <= MAX_BINDING)?;
        Ok(bytes)
    }
}

pub(super) fn duplicate(fd: i32) -> io::Result<OwnedFd> {
    // SAFETY: this atomically creates a fresh CLOEXEC descriptor at least 3.
    let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fcntl returned one fresh owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

pub(super) fn namespace_root() -> io::Result<OwnedFd> {
    // SAFETY: the fixed terminated path selects this process's filesystem root.
    let raw = unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY
                | libc::O_DIRECTORY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful open returned one fresh owned descriptor.
    let root = unsafe { OwnedFd::from_raw_fd(raw) };
    if raw < 3 {
        duplicate(root.as_raw_fd())
    } else {
        Ok(root)
    }
}
