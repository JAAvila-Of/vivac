use super::optional_collection::{CollectionProvisionError, SESSIONS};
use super::*;

#[allow(dead_code)]
impl CandidateRoot {
    pub fn with_session_directory<T>(
        &self,
        operation: impl FnOnce(OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.with_optional_collection(&SESSIONS, operation)
    }

    pub fn session_directory(&self) -> io::Result<Option<OwnedFd>> {
        self.optional_collection(&SESSIONS)
    }

    pub fn provision_session_directory(&self) -> Result<(), CollectionProvisionError> {
        self.provision_optional_collection(&SESSIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::super::optional_collection::{
        publish as publish_collection, with_directory as with_collection, ProvisionCheckpoint,
    };
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct TemporaryDirectory(std::path::PathBuf);

    impl TemporaryDirectory {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn fixture() -> (TemporaryDirectory, OwnedFd, (u32, u32)) {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "vivac-session-directory-{}-{stamp}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        let directory = TemporaryDirectory(path);
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let fd = std::fs::File::open(directory.path()).unwrap().into();
        // SAFETY: both functions only query this process's credentials.
        let ids = unsafe { (libc::getuid(), libc::getgid()) };
        (directory, fd, ids)
    }

    fn publish(
        state: &OwnedFd,
        ids: (u32, u32),
        checkpoint: impl FnMut(ProvisionCheckpoint) -> io::Result<()>,
    ) -> Result<(), CollectionProvisionError> {
        publish_collection(&SESSIONS, state, ids, || Ok(()), checkpoint)
    }

    #[test]
    fn session_directory_publication_preserves_an_occupied_destination() {
        let (directory, fd, ids) = fixture();
        std::fs::create_dir(directory.path().join("sessions-v1")).unwrap();
        std::fs::write(directory.path().join("sessions-v1/foreign"), b"retained").unwrap();
        let error = publish(&fd, ids, |_| Ok(())).unwrap_err();
        assert!(matches!(error, CollectionProvisionError::Unpublished(error)
            if error.raw_os_error() == Some(libc::EEXIST)));
        assert_eq!(
            std::fs::read(directory.path().join("sessions-v1/foreign")).unwrap(),
            b"retained"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn session_directory_failure_before_rename_removes_only_its_stage() {
        let (directory, fd, ids) = fixture();
        std::fs::write(directory.path().join("foreign"), b"retained").unwrap();
        let error = publish(&fd, ids, |stage| {
            assert_eq!(stage, ProvisionCheckpoint::BeforeRename);
            Err(io::Error::from_raw_os_error(libc::ECANCELED))
        })
        .unwrap_err();
        assert!(matches!(error, CollectionProvisionError::Unpublished(error)
            if error.raw_os_error() == Some(libc::ECANCELED)));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(!directory.path().join("sessions-v1").exists());
        assert_eq!(
            std::fs::read(directory.path().join("foreign")).unwrap(),
            b"retained"
        );
    }

    #[test]
    fn session_directory_failure_after_rename_preserves_the_final() {
        let (directory, fd, ids) = fixture();
        let error = publish(&fd, ids, |stage| {
            if stage == ProvisionCheckpoint::BeforeParentSync {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(matches!(
            error,
            CollectionProvisionError::PublishedDurabilityUnknown(error)
                if error.raw_os_error() == Some(libc::EIO)
        ));
        assert!(directory.path().join("sessions-v1").is_dir());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn session_location_absence_does_not_run_the_operation_or_create_a_directory() {
        let (directory, fd, ids) = fixture();
        let error = with_collection(&SESSIONS, &fd, ids, |_| -> io::Result<()> {
            panic!("absent session directory reached the operation")
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
