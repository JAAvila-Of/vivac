use super::optional_collection::{CollectionProvisionError, ENDPOINTS};
use super::*;

impl CandidateRoot {
    pub fn with_endpoint_directory<T>(
        &self,
        operation: impl FnOnce(OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.with_optional_collection(&ENDPOINTS, operation)
    }

    pub fn endpoint_directory(&self) -> io::Result<Option<OwnedFd>> {
        self.optional_collection(&ENDPOINTS)
    }

    pub fn provision_endpoint_directory(&self) -> Result<(), CollectionProvisionError> {
        self.provision_optional_collection(&ENDPOINTS)
    }
}

#[cfg(test)]
mod tests {
    use super::super::optional_collection::{
        publish as publish_collection, read_optional as read_collection,
        with_directory as with_collection, ProvisionCheckpoint,
    };
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn with_directory<T>(
        state: &OwnedFd,
        ids: (u32, u32),
        operation: impl FnOnce(OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        with_collection(&ENDPOINTS, state, ids, operation)
    }

    fn read_optional(state: &OwnedFd, ids: (u32, u32)) -> io::Result<Option<Directory>> {
        read_collection(&ENDPOINTS, state, ids)
    }

    fn publish(
        state: &OwnedFd,
        ids: (u32, u32),
        validate_root: impl Fn() -> io::Result<()>,
        checkpoint: impl FnMut(ProvisionCheckpoint) -> io::Result<()>,
    ) -> Result<(), CollectionProvisionError> {
        publish_collection(&ENDPOINTS, state, ids, validate_root, checkpoint)
    }

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
            "vivac-endpoint-directory-{}-{stamp}-{serial}",
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

    #[test]
    fn endpoint_directory_publication_preserves_an_occupied_destination() {
        let (directory, fd, ids) = fixture();
        std::fs::create_dir(directory.path().join("endpoints-v1")).unwrap();
        std::fs::write(directory.path().join("endpoints-v1/foreign"), b"retained").unwrap();
        let error = publish(&fd, ids, || Ok(()), |_| Ok(())).unwrap_err();
        assert!(matches!(error, CollectionProvisionError::Unpublished(error)
            if error.raw_os_error() == Some(libc::EEXIST)));
        assert_eq!(
            std::fs::read(directory.path().join("endpoints-v1/foreign")).unwrap(),
            b"retained"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn endpoint_directory_failure_before_rename_removes_only_its_stage() {
        let (directory, fd, ids) = fixture();
        std::fs::write(directory.path().join("foreign"), b"retained").unwrap();
        let error = publish(
            &fd,
            ids,
            || Ok(()),
            |stage| {
                assert_eq!(stage, ProvisionCheckpoint::BeforeRename);
                Err(io::Error::from_raw_os_error(libc::ECANCELED))
            },
        )
        .unwrap_err();
        assert!(matches!(error, CollectionProvisionError::Unpublished(error)
            if error.raw_os_error() == Some(libc::ECANCELED)));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(directory.path().join("foreign")).unwrap(),
            b"retained"
        );
    }

    #[test]
    fn endpoint_directory_failure_after_rename_preserves_the_final() {
        let (directory, fd, ids) = fixture();
        let error = publish(
            &fd,
            ids,
            || Ok(()),
            |stage| {
                if stage == ProvisionCheckpoint::BeforeParentSync {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CollectionProvisionError::PublishedDurabilityUnknown(error)
                if error.raw_os_error() == Some(libc::EIO)
        ));
        assert!(directory.path().join("endpoints-v1").is_dir());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn endpoint_location_returns_the_operation_result_with_the_retained_identity() {
        let (_directory, fd, ids) = fixture();
        publish(&fd, ids, || Ok(()), |_| Ok(())).unwrap();
        let original = read_optional(&fd, ids).unwrap().unwrap();
        let result = with_directory(&fd, ids, |duplicate| {
            assert_eq!(
                Identity::directory(duplicate.as_raw_fd(), 8, ids)?,
                original.identity
            );
            Ok(7)
        })
        .unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn endpoint_location_absence_does_not_run_the_operation_or_create_a_directory() {
        let (directory, fd, ids) = fixture();
        let error = with_directory(&fd, ids, |_| -> io::Result<()> {
            panic!("absent endpoint directory reached the operation")
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn endpoint_location_retains_the_original_and_refuses_a_named_replacement() {
        let (directory, fd, ids) = fixture();
        publish(&fd, ids, || Ok(()), |_| Ok(())).unwrap();
        let original = read_optional(&fd, ids).unwrap().unwrap();
        let path = directory.path().join("endpoints-v1");
        let previous = directory.path().join("previous");
        let error = with_directory(&fd, ids, |duplicate| {
            std::fs::rename(&path, &previous)?;
            std::fs::create_dir(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            assert_eq!(
                Identity::directory(duplicate.as_raw_fd(), 8, ids)?,
                original.identity
            );
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
        assert!(path.is_dir() && previous.is_dir());
    }

    #[test]
    fn endpoint_location_revalidates_policy_even_when_the_operation_refuses() {
        let (directory, fd, ids) = fixture();
        publish(&fd, ids, || Ok(()), |_| Ok(())).unwrap();
        let path = directory.path().join("endpoints-v1");
        let error = with_directory(&fd, ids, |_| -> io::Result<()> {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            Err(io::Error::from_raw_os_error(libc::ECANCELED))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().starts_with("directory-policy:"));
        assert!(path.is_dir());
    }
}
