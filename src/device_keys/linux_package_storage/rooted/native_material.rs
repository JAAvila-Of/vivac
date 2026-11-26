use super::*;
use std::os::unix::fs::MetadataExt;

const REQUIRED_SEALS: libc::c_int =
    libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;

pub(crate) struct NativeMaterial {
    directory: OwnedFd,
    cipher: OwnedFd,
}

impl NativeMaterial {
    pub(crate) fn into_parts(self) -> (OwnedFd, OwnedFd) {
        (self.directory, self.cipher)
    }
}

fn duplicate_descriptor(fd: RawFd) -> Result<OwnedFd> {
    // SAFETY: fcntl duplicates this retained descriptor with close-on-exec set.
    let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if raw < 0 {
        return Err(());
    }
    // SAFETY: successful fcntl returned one fresh owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
    verify_descriptor(descriptor.as_raw_fd())?;
    Ok(descriptor)
}

fn verify_descriptor(fd: RawFd) -> Result<()> {
    // SAFETY: fcntl only queries this retained descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if fd < 3 || flags < 0 || flags & libc::FD_CLOEXEC == 0 {
        return Err(());
    }
    Ok(())
}

impl CreationDraft {
    pub(crate) fn native_material(&self) -> std::result::Result<NativeMaterial, CreateError> {
        self.validate()?;
        if self.prepared.encoded_key.len() > 65_536 || self.prepared.encoded_manifest.len() > 2_048
        {
            return Err(CreateError::Refused);
        }
        let directory = duplicate_descriptor(self.prepared.preparation.directory.as_raw_fd())?;
        let cipher = seal_ciphertext(&self.prepared.encoded_key)?;
        self.validate()?;
        verify_descriptor(directory.as_raw_fd())?;
        verify_descriptor(cipher.as_raw_fd())?;
        Ok(NativeMaterial { directory, cipher })
    }
}

fn seal_ciphertext(bytes: &[u8]) -> Result<OwnedFd> {
    if bytes.is_empty() || bytes.len() > 65_536 {
        return Err(());
    }
    // SAFETY: the name is terminated and the flags request a private sealable memfd.
    let raw = unsafe {
        libc::memfd_create(
            c"vivac-ciphertext".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if raw < 0 {
        return Err(());
    }
    // SAFETY: successful memfd_create returned a fresh owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
    let descriptor = if descriptor.as_raw_fd() < 3 {
        let duplicate = duplicate_descriptor(descriptor.as_raw_fd())?;
        drop(descriptor);
        duplicate
    } else {
        descriptor
    };
    verify_descriptor(descriptor.as_raw_fd())?;
    // SAFETY: fchmod changes only this retained anonymous file.
    if unsafe { libc::fchmod(descriptor.as_raw_fd(), 0o600) } != 0 {
        return Err(());
    }
    let mut file = std::fs::File::from(descriptor);
    file.write_all(bytes).map_err(|_| ())?;
    file.seek(SeekFrom::Start(0)).map_err(|_| ())?;
    // SAFETY: fcntl seals only this owned anonymous file after the complete write.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, REQUIRED_SEALS) } != 0 {
        return Err(());
    }
    // SAFETY: fcntl only queries the seals of this retained descriptor.
    let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    let metadata = file.metadata().map_err(|_| ())?;
    if seals < 0
        || seals & REQUIRED_SEALS != REQUIRED_SEALS
        || !metadata.is_file()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.len() != bytes.len() as u64
        || file.stream_position().map_err(|_| ())? != 0
    {
        return Err(());
    }
    verify_descriptor(file.as_raw_fd())?;
    Ok(file.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciphertext_memfd_preserves_bytes_and_refuses_mutation() {
        let bytes = b"public synthetic ciphertext for descriptor verification";
        let descriptor = seal_ciphertext(bytes).expect("create ciphertext memfd");
        let fd = descriptor.as_raw_fd();
        // SAFETY: both fcntl calls only query the retained descriptor.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
        let mut file = std::fs::File::from(descriptor);
        let metadata = file.metadata().expect("read anonymous file metadata");
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(metadata.len(), bytes.len() as u64);
        assert!(metadata.is_file());
        assert_eq!(file.stream_position().expect("query initial offset"), 0);
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).expect("read ciphertext");
        assert_eq!(actual, bytes);
        // SAFETY: fcntl queries the seals on the still-retained file descriptor.
        let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
        assert!(seals >= 0 && seals & REQUIRED_SEALS == REQUIRED_SEALS);
        file.seek(SeekFrom::Start(0)).expect("reset offset");
        assert_eq!(
            file.write(b"x").unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            file.set_len(0).unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            file.set_len(bytes.len() as u64 + 1)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
    }
}
