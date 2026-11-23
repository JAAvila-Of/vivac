//! The record of an open session, kept under `sessions-v1/<credential_id>`, and
//! the decision of whether the kernel shows that a lost session can be
//! reopened.
//!
//! The record names what the kernel can later confirm gone: the boot, the unit
//! cgroup and its inode. It holds no secret and no file content.
#![allow(
    dead_code,
    reason = "The session start and close commands consume these pieces in later steps."
)]

use super::linux_admission_storage::PublishError;
use super::linux_current_admission::CurrentAdmission;
use super::linux_retained_removal::{
    entry_identity, random_hex, remove_retained_entry, rename_no_replace, sync_directory,
    EntryIdentity,
};
use super::linux_session_cgroup::{self as cgroup, CgroupIdentity, CgroupObservation};
use super::linux_user_manager::SessionUnit;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const MAX_RECORD: usize = 1024;

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn missing_collection() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "the session collection does not exist",
    )
}

fn require(condition: bool) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionRecord {
    pub version: u8,
    pub credential_id: String,
    pub generation: String,
    pub unit: String,
    pub unit_cgroup: String,
    pub unit_cgroup_inode: u64,
    pub session_cgroup_inode: u64,
    pub boot_id: String,
    pub admission_sha256: String,
}

fn canonical_boot_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn lowercase_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn plain_cgroup_path(value: &str) -> bool {
    value.starts_with('/')
        && value.bytes().all(|byte| (0x21..0x7f).contains(&byte))
        && value[1..]
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

impl SessionRecord {
    fn validate(&self) -> io::Result<()> {
        require(self.version == 1)?;
        require(self.unit()?.name() == self.unit)?;
        require(plain_cgroup_path(&self.unit_cgroup))?;
        require(self.unit_cgroup.ends_with(&format!("/{}", self.unit)))?;
        require(canonical_boot_id(&self.boot_id))?;
        require(lowercase_digest(&self.admission_sha256))
    }

    pub(crate) fn canonical_bytes(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        require(bytes.len() <= MAX_RECORD)?;
        Ok(bytes)
    }

    pub(crate) fn parse(bytes: &[u8]) -> io::Result<Self> {
        require(bytes.len() <= MAX_RECORD)?;
        let value: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        require(value.canonical_bytes()? == bytes)?;
        Ok(value)
    }

    pub(crate) fn session_cgroup(&self) -> String {
        format!("{}/session", self.unit_cgroup)
    }

    pub(crate) fn unit(&self) -> io::Result<SessionUnit> {
        SessionUnit::from_parts(&self.credential_id, &self.generation)
    }
}

fn process_ids() -> (u32, u32) {
    // SAFETY: these calls only query this process's credentials.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

fn open_stage(directory: &OwnedFd, name: &CStr) -> io::Result<std::fs::File> {
    // SAFETY: the descriptor is live and the name is a terminated component.
    let raw = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned one fresh descriptor that this file now owns.
    Ok(unsafe { std::fs::File::from_raw_fd(raw) })
}

fn unlink_stage(directory: &OwnedFd, name: &CStr) {
    // SAFETY: the name is terminated; this only removes the stage created here.
    unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
}

/// Publishes `bytes` under `name` without replacing anything: the record is
/// staged, synchronized and renamed with `RENAME_NOREPLACE`, then read back.
pub(crate) fn publish_in(
    directory: &OwnedFd,
    name: &CStr,
    bytes: &[u8],
) -> Result<EntryIdentity, PublishError> {
    let stage = CString::new(format!(
        ".pending-session-{}",
        random_hex().map_err(PublishError::Unpublished)?
    ))
    .map_err(|_| PublishError::Unpublished(invalid()))?;
    let staged = (|| -> io::Result<EntryIdentity> {
        let mut file = open_stage(directory, &stage)?;
        // SAFETY: fchmod only changes this descriptor's own file.
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } < 0 {
            return Err(io::Error::last_os_error());
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        let file = OwnedFd::from(file);
        let identity = EntryIdentity::from_descriptor(&file)?;
        require(identity.links == 1)?;
        rename_no_replace(directory, &stage, name)?;
        Ok(identity)
    })();
    let identity = match staged {
        Ok(identity) => identity,
        Err(error) => {
            unlink_stage(directory, &stage);
            return Err(PublishError::Unpublished(error));
        }
    };
    (|| -> io::Result<EntryIdentity> {
        sync_directory(directory)?;
        let (read, found) = read_in(directory, name, process_ids())?.ok_or_else(invalid)?;
        require(read == bytes && found == identity)?;
        Ok(identity)
    })()
    .map_err(PublishError::PublishedDurabilityUnknown)
}

/// Reads one retained record: a regular file owned by `ids`, mode 0600, a
/// single link, read through the identity that was inspected.
pub(crate) fn read_in(
    directory: &OwnedFd,
    name: &CStr,
    ids: (u32, u32),
) -> io::Result<Option<(Vec<u8>, EntryIdentity)>> {
    let Some(before) = entry_identity(directory, name)? else {
        return Ok(None);
    };
    require(
        before.mode & libc::S_IFMT == libc::S_IFREG
            && (before.uid, before.gid) == ids
            && before.mode & 0o7777 == 0o600
            && before.links == 1,
    )?;
    // SAFETY: the descriptor is live and the name is a terminated component.
    let raw = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned one fresh descriptor that this value now owns.
    let file = unsafe { OwnedFd::from_raw_fd(raw) };
    require(EntryIdentity::from_descriptor(&file)? == before)?;
    let mut bytes = Vec::new();
    std::fs::File::from(file.try_clone()?)
        .take(MAX_RECORD as u64 + 1)
        .read_to_end(&mut bytes)?;
    require(bytes.len() <= MAX_RECORD)?;
    require(EntryIdentity::from_descriptor(&file)? == before)?;
    require(entry_identity(directory, name)? == Some(before))?;
    Ok(Some((bytes, before)))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoredSessionRecord {
    pub record: SessionRecord,
    pub identity: EntryIdentity,
}

fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Reads the record for `credential` and refuses one that belongs to another
/// credential or another admission; the file is left untouched either way.
fn read_checked(
    directory: &OwnedFd,
    name: &CStr,
    ids: (u32, u32),
    credential: &str,
    digest: &[u8; 32],
) -> io::Result<Option<StoredSessionRecord>> {
    let Some((bytes, identity)) = read_in(directory, name, ids)? else {
        return Ok(None);
    };
    let record = SessionRecord::parse(&bytes)?;
    require(record.credential_id == credential && record.admission_sha256 == hex(digest))?;
    Ok(Some(StoredSessionRecord { record, identity }))
}

fn remove_stored(directory: &OwnedFd, name: &CStr, stored: &StoredSessionRecord) -> io::Result<()> {
    remove_retained_entry(directory, name, &stored.identity)?;
    sync_directory(directory)
}

/// The session record of one admission, reached through its own location.
pub(crate) struct SessionRecords<'a> {
    admission: &'a CurrentAdmission,
}

impl<'a> SessionRecords<'a> {
    pub(crate) fn new(admission: &'a CurrentAdmission) -> Self {
        Self { admission }
    }

    /// `None` means the collection exists and holds nothing under this name.
    /// A missing collection is an error: it is not the absence of a record.
    pub(crate) fn read(&self) -> io::Result<Option<StoredSessionRecord>> {
        let digest = self.admission.admission_digest()?;
        if !self.admission.has_session_directory()? {
            return Err(missing_collection());
        }
        self.admission.with_session_location(|name, directory| {
            let credential = name.to_str().map_err(|_| invalid())?;
            read_checked(&directory, name, process_ids(), credential, &digest)
        })
    }

    pub(crate) fn publish(
        &self,
        record: &SessionRecord,
    ) -> Result<StoredSessionRecord, PublishError> {
        let digest = self
            .admission
            .admission_digest()
            .map_err(PublishError::Unpublished)?;
        let bytes = record
            .canonical_bytes()
            .map_err(PublishError::Unpublished)?;
        if record.admission_sha256 != hex(&digest) {
            return Err(PublishError::Unpublished(invalid()));
        }
        let outcome = RefCell::new(None);
        let result = self.admission.with_session_location(|name, directory| {
            if name.to_bytes() != record.credential_id.as_bytes() {
                return Err(invalid());
            }
            *outcome.borrow_mut() = Some(publish_in(&directory, name, &bytes));
            Ok(())
        });
        match (outcome.into_inner(), result) {
            (Some(Ok(identity)), Ok(())) => Ok(StoredSessionRecord {
                record: record.clone(),
                identity,
            }),
            (Some(Ok(_)), Err(error)) => Err(PublishError::PublishedDurabilityUnknown(error)),
            (Some(Err(error)), _) => Err(error),
            (None, Err(error)) => Err(PublishError::Unpublished(error)),
            (None, Ok(())) => Err(PublishError::Unpublished(invalid())),
        }
    }

    pub(crate) fn remove(&self, stored: &StoredSessionRecord) -> io::Result<()> {
        self.admission
            .with_session_location(|name, directory| remove_stored(&directory, name, stored))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReopeningEvidence {
    NewBoot,
    CgroupGone,
    CgroupEmpty,
}

/// Whether the observations prove that no process of the recorded session
/// survives. The decision is about the whole unit, keeper included: a session
/// cgroup that is empty under a populated unit is a session being started or
/// closed, so the session observation never counts on its own.
pub(crate) fn reopening_evidence(
    record: &SessionRecord,
    boot_id: &str,
    unit: CgroupObservation,
    _session: CgroupObservation,
) -> Option<ReopeningEvidence> {
    if boot_id != record.boot_id {
        return Some(ReopeningEvidence::NewBoot);
    }
    match unit {
        CgroupObservation::Absent | CgroupObservation::Replaced => {
            Some(ReopeningEvidence::CgroupGone)
        }
        CgroupObservation::Empty => Some(ReopeningEvidence::CgroupEmpty),
        CgroupObservation::Populated => None,
    }
}

pub(crate) fn observe_evidence(record: &SessionRecord) -> io::Result<Option<ReopeningEvidence>> {
    let boot_id = cgroup::boot_id()?;
    if boot_id != record.boot_id {
        return Ok(Some(ReopeningEvidence::NewBoot));
    }
    let unit = cgroup::observe(
        &record.unit_cgroup,
        CgroupIdentity {
            inode: record.unit_cgroup_inode,
        },
    )?;
    let session = cgroup::observe(
        &record.session_cgroup(),
        CgroupIdentity {
            inode: record.session_cgroup_inode,
        },
    )?;
    Ok(reopening_evidence(record, &boot_id, unit, session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_retained_removal::tests::scratch;
    use crate::linux_session_cgroup::CgroupObservation::{Absent, Empty, Populated, Replaced};

    const CREDENTIAL: &str = "00000000000000000000000004";
    const GENERATION: &str = "00000000000000000000000005";
    const BOOT: &str = "01234567-89ab-cdef-0123-456789abcdef";

    fn record() -> SessionRecord {
        let unit = format!("vivac-custody-{CREDENTIAL}-{GENERATION}.service");
        SessionRecord {
            version: 1,
            credential_id: CREDENTIAL.to_owned(),
            generation: GENERATION.to_owned(),
            unit_cgroup: format!("/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}"),
            unit,
            unit_cgroup_inode: 11,
            session_cgroup_inode: 12,
            boot_id: BOOT.to_owned(),
            admission_sha256: "ab".repeat(32),
        }
    }

    fn canonical() -> String {
        let unit = format!("vivac-custody-{CREDENTIAL}-{GENERATION}.service");
        format!(
            concat!(
                "{{\"version\":1,\"credential_id\":\"{}\",\"generation\":\"{}\",",
                "\"unit\":\"{}\",",
                "\"unit_cgroup\":\"/user.slice/user-1000.slice/user@1000.service/app.slice/{}\",",
                "\"unit_cgroup_inode\":11,\"session_cgroup_inode\":12,",
                "\"boot_id\":\"{}\",\"admission_sha256\":\"{}\"}}"
            ),
            CREDENTIAL,
            GENERATION,
            unit,
            unit,
            BOOT,
            "ab".repeat(32)
        )
    }

    #[test]
    fn session_record_round_trips_canonical_bytes() {
        let value = record();
        let bytes = value.canonical_bytes().unwrap();
        assert_eq!(bytes, canonical().as_bytes());
        assert_eq!(SessionRecord::parse(&bytes).unwrap(), value);
        assert_eq!(
            value.session_cgroup(),
            format!("{}/session", value.unit_cgroup)
        );
        assert_eq!(value.unit().unwrap().name(), value.unit);
    }

    #[test]
    fn session_record_rejects_noncanonical_unknown_or_inconsistent_fields() {
        let good = canonical();
        let refused = |bytes: &str| {
            assert_eq!(
                SessionRecord::parse(bytes.as_bytes()).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{bytes}"
            );
        };
        refused(&good.replacen('}', ",\"extra\":1}", 1).replace("}}", "}"));
        refused(&good.replacen("{\"version\"", "{ \"version\"", 1));
        refused(&good.replacen("\"version\":1", "\"version\":2", 1));
        refused(&good.replacen(
            &format!("-{CREDENTIAL}-"),
            "-00000000000000000000000009-",
            1,
        ));
        refused(&good.replace(
            "\"unit\":\"vivac-custody-00000000000000000000000004-00000000000000000000000005.service\"",
            "\"unit\":\"vivac-custody-00000000000000000000000009-00000000000000000000000005.service\"",
        ));
        refused(&good.replace(
            ".service\",\"unit_cgroup_inode",
            ".scope\",\"unit_cgroup_inode",
        ));
        refused(&good.replacen(BOOT, &BOOT.to_ascii_uppercase(), 1));
        refused(&good.replacen(&"ab".repeat(32), &"AB".repeat(32), 1));
        refused(&good.replacen(&"ab".repeat(32), &"ab".repeat(31), 1));
        refused(&format!("{good}\n"));
        refused(&"x".repeat(1025));
        let mut padded = good.clone();
        padded.push_str(&" ".repeat(1025));
        refused(&padded);
    }

    #[test]
    fn reopening_evidence_follows_the_kernel_observations() {
        let r = record();
        let other = "ffffffff-89ab-cdef-0123-456789abcdef";
        let all = [Absent, Replaced, Populated, Empty];
        for unit in all {
            for session in all {
                let same = reopening_evidence(&r, BOOT, unit, session);
                let expected = match unit {
                    Absent | Replaced => Some(ReopeningEvidence::CgroupGone),
                    Empty => Some(ReopeningEvidence::CgroupEmpty),
                    Populated => None,
                };
                assert_eq!(same, expected, "{unit:?} {session:?}");
                assert_eq!(
                    reopening_evidence(&r, other, unit, session),
                    Some(ReopeningEvidence::NewBoot)
                );
            }
        }
    }

    #[test]
    fn live_keeper_with_empty_session_cgroup_is_not_evidence() {
        let r = record();
        assert_eq!(reopening_evidence(&r, &r.boot_id, Populated, Empty), None);
    }

    fn published(
        directory: &OwnedFd,
        value: &SessionRecord,
    ) -> Result<EntryIdentity, PublishError> {
        let name = CString::new(value.credential_id.as_bytes()).unwrap();
        publish_in(directory, &name, &value.canonical_bytes().unwrap())
    }

    fn ids() -> (u32, u32) {
        // SAFETY: these calls only query this process's credentials.
        unsafe { (libc::geteuid(), libc::getegid()) }
    }

    #[test]
    fn session_record_publication_reads_back_and_refuses_an_occupied_name() {
        let (dir, fd) = scratch();
        let value = record();
        let identity = published(&fd, &value).unwrap();
        let name = CString::new(CREDENTIAL).unwrap();
        let (bytes, read) = read_in(&fd, &name, ids()).unwrap().unwrap();
        assert_eq!(bytes, value.canonical_bytes().unwrap());
        assert_eq!(read, identity);
        assert_eq!(identity.mode & 0o7777, 0o600);
        let before = std::fs::read(dir.0.join(CREDENTIAL)).unwrap();

        let mut other = record();
        other.generation = "00000000000000000000000006".to_owned();
        other.unit = format!("vivac-custody-{CREDENTIAL}-{}.service", other.generation);
        other.unit_cgroup = format!("/user.slice/app.slice/{}", other.unit);
        let error = published(&fd, &other).unwrap_err();
        assert!(matches!(error, PublishError::Unpublished(error)
            if error.raw_os_error() == Some(libc::EEXIST)));
        assert_eq!(std::fs::read(dir.0.join(CREDENTIAL)).unwrap(), before);
        let names: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(CREDENTIAL)]);
    }

    #[test]
    fn session_record_read_refuses_unsafe_entries() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, fd) = scratch();
        let name = CString::new(CREDENTIAL).unwrap();
        assert!(read_in(&fd, &name, ids()).unwrap().is_none());
        let path = dir.0.join(CREDENTIAL);
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_in(&fd, &name, ids()).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_in(&fd, &name, ids()).unwrap().is_some());
        std::fs::hard_link(&path, dir.0.join("alias")).unwrap();
        assert!(read_in(&fd, &name, ids()).is_err());
        std::fs::remove_file(dir.0.join("alias")).unwrap();
        assert!(read_in(&fd, &name, (ids().0 + 1, ids().1)).is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("elsewhere", &path).unwrap();
        assert!(read_in(&fd, &name, ids()).is_err());
    }

    #[test]
    fn record_for_another_admission_is_refused_and_left() {
        let (dir, fd) = scratch();
        let value = record();
        let identity = published(&fd, &value).unwrap();
        let path = dir.0.join(CREDENTIAL);
        let before = std::fs::read(&path).unwrap();
        let name = CString::new(CREDENTIAL).unwrap();

        let digest = [0xab_u8; 32];
        let stored = read_checked(&fd, &name, ids(), CREDENTIAL, &digest)
            .unwrap()
            .unwrap();
        assert_eq!(stored.record, value);
        assert_eq!(stored.identity, identity);

        let error = read_checked(&fd, &name, ids(), CREDENTIAL, &[0xcd_u8; 32]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let error =
            read_checked(&fd, &name, ids(), "00000000000000000000000009", &digest).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(entry_identity(&fd, &name).unwrap(), Some(identity));
    }

    #[test]
    fn a_missing_collection_is_not_the_absence_of_a_record() {
        let error = missing_collection();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_ne!(error.raw_os_error(), Some(libc::ENOENT));
        let (_dir, fd) = scratch();
        let name = CString::new(CREDENTIAL).unwrap();
        assert!(read_checked(&fd, &name, ids(), CREDENTIAL, &[0xab_u8; 32])
            .unwrap()
            .is_none());
    }

    #[test]
    fn stored_record_removal_is_identity_checked() {
        let (dir, fd) = scratch();
        let value = record();
        let identity = published(&fd, &value).unwrap();
        let stored = StoredSessionRecord {
            record: value,
            identity,
        };
        let name = CString::new(CREDENTIAL).unwrap();
        remove_stored(&fd, &name, &stored).unwrap();
        assert!(std::fs::read_dir(&dir.0).unwrap().next().is_none());
    }
}
