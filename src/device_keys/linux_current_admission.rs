use super::linux_admission_storage::PublishError;
use super::linux_reservation::StoreReservation;
use super::linux_session_cgroup::EmptySessionCgroup;
use super::{linux_creation_record::Record, linux_resident_reader::CandidateResident};
use sha2::{Digest, Sha256};
use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[path = "linux_current_admission/native_material.rs"]
mod native_material;
pub(super) use native_material::seal_ciphertext;
pub(crate) use native_material::{
    ActiveClosureBinding, KeeperLaunchExpectation, OpeningAuthority, KEEPER_BOOTSTRAP_SIZE,
};
pub use native_material::{
    AdmittedChildRegistration, AdmittedLaunchPreparation, AdmittedSessionMaterial,
    OpeningPermitLaunch, ReservedAdmittedSessionMaterial,
};

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

struct Validity {
    invalidated: AtomicBool,
}

impl Validity {
    fn new() -> Self {
        Self {
            invalidated: AtomicBool::new(false),
        }
    }

    fn check<T>(&self, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        if self.invalidated.load(Ordering::Acquire) {
            return Err(invalid());
        }
        match operation() {
            Err(error) => {
                self.invalidated.store(true, Ordering::Release);
                Err(error)
            }
            Ok(value) => {
                if self.invalidated.load(Ordering::Acquire) {
                    Err(invalid())
                } else {
                    Ok(value)
                }
            }
        }
    }

    fn check_publication(
        &self,
        operation: impl FnOnce(&mut bool) -> io::Result<()>,
    ) -> Result<(), PublishError> {
        let mut published = false;
        self.check(|| operation(&mut published)).map_err(|error| {
            if published {
                PublishError::PublishedDurabilityUnknown(error)
            } else {
                PublishError::Unpublished(error)
            }
        })
    }
}

fn decode_public_key(encoded: &str) -> io::Result<[u8; 32]> {
    if encoded.len() != 64 {
        return Err(invalid());
    }
    let digit = |byte| match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(invalid()),
    };
    let mut public = [0; 32];
    for (output, pair) in public.iter_mut().zip(encoded.as_bytes().as_chunks::<2>().0) {
        *output = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Ok(public)
}

pub struct CurrentAdmission {
    candidate: CandidateResident,
    validity: Validity,
}

pub(super) struct EndpointRecord {
    pub canonical: Vec<u8>,
    pub public_key: [u8; 32],
    pub comment: Vec<u8>,
}

pub struct RetainedKeeperDirectory {
    admission: Arc<CurrentAdmission>,
    directory: OwnedFd,
}

impl RetainedKeeperDirectory {
    pub fn validate(&self) -> io::Result<()> {
        self.admission.checked_store(|_, identity| {
            verify_keeper_directory(&self.directory, [identity[0], identity[1]])
        })
    }

    pub(crate) fn publish_clean_opening_permit(
        &self,
        authorize_rename: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), PublishError> {
        self.admission.validity.check_publication(|published| {
            self.validate()?;
            let result = self.admission.candidate.publish_clean_opening_permit(
                || self.validate(),
                authorize_rename,
                published,
            );
            self.validate()?;
            result
        })
    }
}

fn verify_keeper_directory(directory: &OwnedFd, expected: [u64; 2]) -> io::Result<()> {
    let mut identity = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat queries the owned descriptor and initializes identity on success.
    if unsafe { libc::fstat(directory.as_raw_fd(), identity.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the preceding fstat call succeeded.
    let identity = unsafe { identity.assume_init() };
    if [identity.st_dev, identity.st_ino] != expected {
        return Err(invalid());
    }
    super::linux_reservation::verify_retained_flock(directory)
}

impl CurrentAdmission {
    pub fn retain_keeper_directory(
        self,
        directory: OwnedFd,
    ) -> io::Result<RetainedKeeperDirectory> {
        let retained = RetainedKeeperDirectory {
            admission: Arc::new(self),
            directory,
        };
        retained.validate()?;
        Ok(retained)
    }

    /// Publishes the opening permit again after the session cgroup is proven empty,
    /// when the supervisor never reached the keeper. A fresh, non-waiting
    /// reservation of the package directory stands in for the dead keeper's.
    #[allow(dead_code, reason = "The native probes never run a session keeper.")]
    pub(crate) fn republish_after_empty_session(
        &self,
        _evidence: &EmptySessionCgroup,
        authorize_rename: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), PublishError> {
        self.validity.check_publication(|published| {
            let reservation = self.reserve_package_directory()?;
            let result = self.candidate.publish_clean_opening_permit(
                || reservation.validate_owned(),
                authorize_rename,
                published,
            );
            reservation.validate_owned()?;
            result
        })
    }

    /// Takes the credential reservation on the package directory alone, without
    /// waiting, for work that needs no package lease, such as retiring a session
    /// record. A held reservation is refused.
    pub fn reserve_package_directory(&self) -> io::Result<StoreReservation> {
        self.validate()?;
        let package = self.candidate.package_directory()?;
        let reservation = StoreReservation::acquire_directory(&package)?;
        self.validate()?;
        Ok(reservation)
    }

    pub fn open_fixed_namespace(credential_id: &str) -> io::Result<Self> {
        let current = Self {
            candidate: CandidateResident::open_fixed_namespace(credential_id)?,
            validity: Validity::new(),
        };
        current.validate()?;
        Ok(current)
    }

    fn checked<T>(&self, operation: impl FnOnce(&Record) -> io::Result<T>) -> io::Result<T> {
        self.validity.check(|| {
            self.candidate.validate()?;
            let value = operation(self.candidate.record())?;
            self.candidate.validate()?;
            Ok(value)
        })
    }

    fn checked_store<T>(
        &self,
        operation: impl FnOnce(&Record, [u64; 6]) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validity
            .check(|| self.candidate.checked_ipc_identity(operation))
    }

    pub fn validate(&self) -> io::Result<()> {
        self.checked(|_| Ok(()))
    }

    pub fn canonical_record(&self) -> io::Result<Vec<u8>> {
        self.checked(Record::canonical_bytes)
    }

    pub fn with_endpoint_location<T>(
        &self,
        operation: impl FnOnce(&CStr, OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self.candidate.with_endpoint_location(operation);
        self.validate()?;
        result
    }

    pub fn with_opening_location<T>(
        &self,
        operation: impl FnOnce(&CStr, OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self.candidate.with_opening_location(operation);
        self.validate()?;
        result
    }

    pub fn has_session_directory(&self) -> io::Result<bool> {
        self.validate()?;
        let result = self.candidate.has_session_directory();
        self.validate()?;
        result
    }

    pub fn has_endpoint_directory(&self) -> io::Result<bool> {
        self.validate()?;
        let result = self.candidate.has_endpoint_directory();
        self.validate()?;
        result
    }

    pub fn has_opening_directory(&self) -> io::Result<bool> {
        self.validate()?;
        let result = self.candidate.has_opening_directory();
        self.validate()?;
        result
    }

    pub fn with_session_location<T>(
        &self,
        operation: impl FnOnce(&CStr, OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self.candidate.with_session_location(operation);
        self.validate()?;
        result
    }

    pub fn admission_digest(&self) -> io::Result<[u8; 32]> {
        Ok(Sha256::digest(self.canonical_record()?).into())
    }

    /// Inspects the candidate permit without consuming it or granting authority.
    pub fn observe_opening_permit(
        &self,
    ) -> io::Result<super::linux_resident_reader::OpeningPermitKind> {
        self.validity
            .check(|| self.candidate.observe_opening_permit())
    }

    /// Checks that the permit is gone, without consuming or granting anything.
    pub fn opening_permit_absent(&self) -> io::Result<bool> {
        self.validity
            .check(|| self.candidate.opening_permit_absent())
    }

    fn consume_opening_permit(&self) -> io::Result<()> {
        self.validity
            .check(|| self.candidate.consume_opening_permit())
    }

    pub fn matches_record(&self, expected: &Record) -> io::Result<bool> {
        self.checked(|record| Ok(record == expected))
    }

    pub fn expected_public_key(&self) -> io::Result<[u8; 32]> {
        self.checked(|record| decode_public_key(&record.public_key))
    }

    pub(super) fn endpoint_record(&self) -> io::Result<EndpointRecord> {
        self.checked(|record| {
            Ok(EndpointRecord {
                canonical: record.canonical_bytes()?,
                public_key: decode_public_key(&record.public_key)?,
                comment: format!(
                    "vivac.device-key.v1:{}:{}",
                    record.device_id, record.credential_id
                )
                .into_bytes(),
            })
        })
    }

    pub fn store_identity(&self) -> io::Result<[u64; 6]> {
        // The candidate already validates its retained and named members on
        // both sides of the descriptor identity read.
        self.validity.check(|| self.candidate.ipc_identity())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn opening_publication_preserves_rename_across_failed_revalidation() {
        let before_rename = Validity::new();
        let refused = before_rename
            .check_publication(|_| Err::<(), _>(io::Error::from_raw_os_error(libc::EIO)));
        assert!(matches!(refused, Err(PublishError::Unpublished(error))
            if error.raw_os_error() == Some(libc::EIO)));

        let failed_sync = Validity::new();
        let uncertain = failed_sync.check_publication(|published| {
            *published = true;
            Err(io::Error::from_raw_os_error(libc::EIO))
        });
        assert!(
            matches!(uncertain, Err(PublishError::PublishedDurabilityUnknown(error))
            if error.raw_os_error() == Some(libc::EIO))
        );

        let after_rename = Validity::new();
        let uncertain = after_rename.check_publication(|published| {
            *published = true;
            let invalidation =
                after_rename.check(|| Err::<(), _>(io::Error::from_raw_os_error(libc::EXDEV)));
            assert_eq!(invalidation.unwrap_err().raw_os_error(), Some(libc::EXDEV));
            Ok(())
        });
        assert!(
            matches!(uncertain, Err(PublishError::PublishedDurabilityUnknown(error))
            if error.kind() == io::ErrorKind::InvalidData)
        );

        let called = Cell::new(false);
        let retry = after_rename.check_publication(|_| {
            called.set(true);
            Ok(())
        });
        assert!(matches!(retry, Err(PublishError::Unpublished(_))));
        assert!(!called.get());
    }

    #[test]
    fn current_admission_never_recovers_after_failed_validation() {
        let validity = Validity::new();
        let first = validity.check(|| Err::<(), _>(io::Error::from_raw_os_error(libc::EXDEV)));
        assert_eq!(first.unwrap_err().raw_os_error(), Some(libc::EXDEV));
        let called = Cell::new(false);
        let second = validity.check(|| {
            called.set(true);
            Ok(())
        });
        assert!(second.is_err());
        assert_eq!(second.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(!called.get());
    }

    #[test]
    fn current_admission_preserves_success_and_first_native_error() {
        let validity = Validity::new();
        assert_eq!(validity.check(|| Ok(17)).unwrap(), 17);
        assert_eq!(validity.check(|| Ok(23)).unwrap(), 23);
        let first = validity.check(|| Err::<(), _>(io::Error::from_raw_os_error(libc::EXDEV)));
        assert_eq!(first.unwrap_err().raw_os_error(), Some(libc::EXDEV));
    }

    #[test]
    fn current_admission_checks_invalidation_after_a_successful_operation() {
        let validity = Validity::new();
        let outer = validity.check(|| {
            let first = validity.check(|| Err::<(), _>(io::Error::from_raw_os_error(libc::EXDEV)));
            assert_eq!(first.unwrap_err().raw_os_error(), Some(libc::EXDEV));
            Ok(17)
        });
        assert_eq!(outer.unwrap_err().kind(), io::ErrorKind::InvalidData);
        let called = Cell::new(false);
        let later = validity.check(|| {
            called.set(true);
            Ok(23)
        });
        assert_eq!(later.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(!called.get());
    }

    #[test]
    fn current_admission_decodes_bounded_canonical_public_keys() {
        let encoded = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
        let expected = [
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ];
        assert_eq!(decode_public_key(encoded).unwrap(), expected);
        for value in [
            "".to_owned(),
            encoded[..62].to_owned(),
            encoded[..63].to_owned(),
            format!("{encoded}0"),
            encoded.to_ascii_uppercase(),
            format!("g{}", &encoded[1..]),
            format!("é{}", &encoded[2..]),
        ] {
            assert_eq!(
                decode_public_key(&value).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn current_admission_latches_the_first_public_key_decode_rejection() {
        let validity = Validity::new();
        let first = validity.check(|| decode_public_key("invalid"));
        assert_eq!(first.unwrap_err().kind(), io::ErrorKind::InvalidData);
        let called = Cell::new(false);
        let later = validity.check(|| {
            called.set(true);
            decode_public_key(&"00".repeat(32))
        });
        assert_eq!(later.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(!called.get());
    }
}
