use crate::attended_terminal::{attributes, AttendedUnlockTerminal};
use std::io;
use std::os::fd::AsRawFd;

impl AttendedUnlockTerminal {
    pub(crate) fn validate_foreground_hold(&self) -> io::Result<()> {
        self.validate().map_err(|_| hold_refused())?;
        let current = attributes(self.as_raw_fd()).map_err(|_| hold_refused())?;
        if current.c_lflag & libc::ICANON == 0 {
            return Err(hold_refused());
        }
        Ok(())
    }
}

fn hold_refused() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}
