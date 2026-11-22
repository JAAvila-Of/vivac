use super::*;
use crate::linux_creation_record::Record;
use crate::linux_custody_root::CandidateRoot;
use std::ffi::OsStr;
use std::os::fd::{FromRawFd, OwnedFd};

#[path = "rooted/native_material.rs"]
mod native_material;

pub(super) struct RootedLocation {
    root: CandidateRoot,
    filesystem: FilesystemIdentity,
}

fn capture(fd: RawFd) -> Result<libc::statx> {
    // SAFETY: fcntl only queries this retained descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if fd < 3 || flags < 0 || flags & libc::FD_CLOEXEC == 0 {
        return Err(());
    }
    let mut value = MaybeUninit::<libc::statx>::uninit();
    let mask = libc::STATX_BASIC_STATS | libc::STATX_MNT_ID;
    // SAFETY: AT_EMPTY_PATH selects fd and statx initializes the output.
    if unsafe {
        libc::syscall(
            libc::SYS_statx,
            fd,
            c"".as_ptr(),
            libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
            mask,
            value.as_mut_ptr(),
        )
    } < 0
    {
        return Err(());
    }
    // SAFETY: statx succeeded.
    let value = unsafe { value.assume_init() };
    if !complete_mask(value.stx_mask) {
        return Err(());
    }
    Ok(value)
}

fn complete_mask(mask: u32) -> bool {
    let required = libc::STATX_BASIC_STATS | libc::STATX_MNT_ID;
    mask & required == required
}

fn filesystem_identity(value: &libc::statx) -> FilesystemIdentity {
    FilesystemIdentity {
        device: (value.stx_dev_major, value.stx_dev_minor),
        mount: value.stx_mnt_id,
    }
}

fn open_at(parent: &Dir, name: &OsStr, directory: bool) -> Result<OwnedFd> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let name = CString::new(name.as_bytes()).map_err(|_| ())?;
    let how = OpenHow {
        flags: (libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | libc::O_NOFOLLOW
            | if directory { libc::O_DIRECTORY } else { 0 }) as u64,
        mode: 0,
        resolve: libc::RESOLVE_BENEATH
            | libc::RESOLVE_NO_SYMLINKS
            | libc::RESOLVE_NO_MAGICLINKS
            | libc::RESOLVE_NO_XDEV,
    };
    // SAFETY: name is terminated and how has the kernel open_how layout.
    let raw = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            parent.as_raw_fd(),
            name.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if raw < 0 {
        return Err(());
    }
    // SAFETY: successful openat2 returned one fresh owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw as RawFd) };
    capture(descriptor.as_raw_fd())?;
    Ok(descriptor)
}

pub(super) fn open_directory(parent: &Dir, name: &OsStr) -> Result<Dir> {
    Ok(Dir::from_std_file(std::fs::File::from(open_at(
        parent, name, true,
    )?)))
}

pub(super) fn open_leaf(parent: &Dir, name: &str) -> Result<File> {
    let file = File::from_std(std::fs::File::from(open_at(
        parent,
        OsStr::new(name),
        false,
    )?));
    private_file(&file)?;
    Ok(file)
}

impl RootedLocation {
    pub(super) fn open() -> Result<(Self, Dir)> {
        let root = CandidateRoot::open_fixed_namespace().map_err(|_| ())?;
        let parent = Dir::from_std_file(std::fs::File::from(
            root.packages_descriptor().map_err(|_| ())?,
        ));
        let filesystem = filesystem_identity(&capture(parent.as_raw_fd())?);
        let rooted = Self { root, filesystem };
        rooted.verify_directory(&parent)?;
        Ok((rooted, parent))
    }

    fn verify_object(&self, fd: RawFd, directory: bool) -> Result<()> {
        let stat = capture(fd)?;
        let binding = self.root.binding();
        if !self.filesystem.same_filesystem(filesystem_identity(&stat))
            || (stat.stx_uid, stat.stx_gid) != (binding.owner_uid, binding.owner_gid)
            || u32::from(stat.stx_mode) & libc::S_IFMT
                != if directory {
                    libc::S_IFDIR
                } else {
                    libc::S_IFREG
                }
            || stat.stx_mode & 0o7777 != if directory { 0o700 } else { 0o600 }
            || (!directory && stat.stx_nlink != 1)
        {
            return Err(());
        }
        filesystem(fd)?;
        no_posix_acl(fd, directory)
    }

    pub(super) fn verify_directory(&self, directory: &Dir) -> Result<()> {
        self.verify_object(directory.as_raw_fd(), true)
    }

    pub(super) fn verify_file(&self, file: &File) -> Result<()> {
        self.verify_object(file.as_raw_fd(), false)
    }

    pub(super) fn validate_namespace(&self, parent: &Dir) -> Result<()> {
        self.root.validate().map_err(|_| ())?;
        let fresh = Dir::from_std_file(std::fs::File::from(
            self.root.packages_descriptor().map_err(|_| ())?,
        ));
        self.verify_directory(parent)?;
        self.verify_directory(&fresh)?;
        if Identity::of(&parent.dir_metadata().map_err(|_| ())?)
            != Identity::of(&fresh.dir_metadata().map_err(|_| ())?)
        {
            return Err(());
        }
        Ok(())
    }

    pub(super) fn recheck(&self, parent: &Dir, credential: &OsStr) -> Result<()> {
        self.validate_namespace(parent)?;
        let admissions = Dir::from_std_file(std::fs::File::from(
            self.root.admissions_descriptor().map_err(|_| ())?,
        ));
        match admissions.symlink_metadata(credential) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(()),
        }
    }
}

fn encode_identifier(bytes: [u8; 16]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for byte in encoded.iter_mut().rev() {
        *byte = ALPHABET[(value & 31) as usize];
        value >>= 5;
    }
    encoded.into_iter().map(char::from).collect()
}

pub(crate) struct CreationDraft {
    prepared: PreparedPackage,
    record: Record,
}

/// Retained published package data, without admission or producer authority.
pub(crate) struct PublishedDraft {
    prepared: PreparedPackage,
    record: Record,
}

pub(crate) struct OpeningStorage<'a> {
    package: &'a PublishedDraft,
}

impl OpeningStorage<'_> {
    fn rooted(&self) -> std::io::Result<&RootedLocation> {
        self.package
            .prepared
            .preparation
            .location
            .rooted
            .as_ref()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))
    }

    pub(crate) fn duplicate_material(&self) -> std::io::Result<(OwnedFd, OwnedFd)> {
        let duplicate = |fd| {
            // SAFETY: fcntl duplicates this retained descriptor atomically.
            let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
            if raw < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: successful fcntl returned one fresh owned descriptor.
            Ok(unsafe { OwnedFd::from_raw_fd(raw) })
        };
        let directory = duplicate(self.package.prepared.preparation.directory.as_raw_fd())?;
        let cipher = duplicate(self.package.prepared.key.file.as_raw_fd())?;
        Ok((directory, cipher))
    }

    pub(crate) fn verify_object(&self, fd: RawFd, directory: bool) -> Result<()> {
        self.rooted().map_err(|_| ())?.verify_object(fd, directory)
    }

    pub(crate) fn capture(&self, fd: RawFd) -> Result<libc::statx> {
        capture(fd)
    }

    pub(crate) fn with_opening_directory<T>(
        &self,
        operation: impl FnOnce(&OwnedFd) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        self.rooted()?.root.with_opening_directory(operation)
    }

    pub(crate) fn open_named(&self, directory: &OwnedFd) -> std::io::Result<OwnedFd> {
        let parent = Dir::from_std_file(std::fs::File::from(directory.try_clone()?));
        open_at(
            &parent,
            OsStr::new(&self.package.record.credential_id),
            false,
        )
        .map_err(|()| std::io::Error::from(std::io::ErrorKind::InvalidData))
    }
}

impl PublishedDraft {
    pub(crate) fn opening_storage(&self) -> OpeningStorage<'_> {
        OpeningStorage { package: self }
    }

    pub(crate) fn record(&self) -> &Record {
        &self.record
    }

    pub(crate) fn ipc_identity(&self) -> Result<[u64; 6]> {
        self.validate()?;
        let directory = self
            .prepared
            .preparation
            .directory
            .dir_metadata()
            .map_err(|_| ())?;
        let key = self.prepared.key.file.metadata().map_err(|_| ())?;
        let manifest = self.prepared.manifest.file.metadata().map_err(|_| ())?;
        let identity = [
            directory.dev(),
            directory.ino(),
            key.dev(),
            key.ino(),
            manifest.dev(),
            manifest.ino(),
        ];
        self.validate()?;
        Ok(identity)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.prepared.revalidate_published()?;
        let expected =
            CreationDraft::expected_record(&self.prepared, self.record.creation_id.clone())?;
        if expected != self.record {
            return Err(());
        }
        self.prepared.revalidate_published()
    }

    pub(crate) fn admissions_descriptor(&self) -> std::result::Result<OwnedFd, CreateError> {
        let descriptor = (|| -> Result<OwnedFd> {
            self.validate()?;
            let rooted = self
                .prepared
                .preparation
                .location
                .rooted
                .as_ref()
                .ok_or(())?;
            let descriptor = rooted.root.admissions_descriptor().map_err(|_| ())?;
            self.validate()?;
            Ok(descriptor)
        })();
        descriptor.map_err(|()| CreateError::CommittedIndeterminate)
    }
}

impl CreationDraft {
    pub(super) fn from_prepared(prepared: PreparedPackage) -> Result<Self> {
        prepared.preparation.location.rooted.as_ref().ok_or(())?;
        prepared.revalidate(&prepared.preparation.name)?;
        prepared.preparation.location.require_absent()?;
        let mut nonce = [0; 16];
        getrandom::getrandom(&mut nonce).map_err(|_| ())?;
        let record = Self::expected_record(&prepared, encode_identifier(nonce))?;
        let draft = Self { prepared, record };
        draft.validate()?;
        Ok(draft)
    }

    fn expected_record(prepared: &PreparedPackage, creation_id: String) -> Result<Record> {
        let location = &prepared.preparation.location;
        let rooted = location.rooted.as_ref().ok_or(())?;
        let manifest: Manifest =
            serde_json::from_slice(&prepared.encoded_manifest).map_err(|_| ())?;
        manifest.validate().map_err(|_| ())?;
        validate_key_bytes(&prepared.encoded_key, &manifest).map_err(|_| ())?;
        if manifest.credential_id.as_bytes() != location.name.as_bytes()
            || serde_json::to_vec(&manifest).map_err(|_| ())? != prepared.encoded_manifest
        {
            return Err(());
        }
        let record = Record {
            version: 2,
            installation_id: rooted.root.binding().installation_id.clone(),
            creation_id,
            device_id: manifest.device_id,
            credential_id: manifest.credential_id,
            public_key: manifest.public_key,
            ciphertext_sha256: hex(&Sha256::digest(&prepared.encoded_key)),
            manifest_sha256: hex(&Sha256::digest(&prepared.encoded_manifest)),
            ciphertext_len: prepared.encoded_key.len() as u64,
            manifest_len: prepared.encoded_manifest.len() as u64,
            creation_profile_revision: 1,
            secret_policy_revision: 1,
        };
        record.canonical_bytes().map_err(|_| ())?;
        Ok(record)
    }

    pub(crate) fn record(&self) -> &Record {
        &self.record
    }

    /// Publish only package objects; this operation grants no admission.
    pub(crate) fn publish_package(self) -> std::result::Result<PublishedDraft, CreateError> {
        self.publish_package_using(PreparedPackage::publish_retained)
    }

    pub(crate) fn publish_package_with(
        self,
        checkpoint: &mut impl FnMut(PackagePublishStage) -> Result<()>,
    ) -> std::result::Result<PublishedDraft, CreateError> {
        self.publish_package_using(|prepared| prepared.publish_retained_with(checkpoint))
    }

    fn publish_package_using(
        self,
        publish: impl FnOnce(PreparedPackage) -> std::result::Result<PreparedPackage, CreateError>,
    ) -> std::result::Result<PublishedDraft, CreateError> {
        self.validate()?;
        self.prepared.key.file.sync_all().map_err(|_| ())?;
        self.prepared.manifest.file.sync_all().map_err(|_| ())?;
        sync_directory(&self.prepared.preparation.directory)?;
        self.validate()?;
        let Self { prepared, record } = self;
        let published = PublishedDraft {
            prepared: publish(prepared)?,
            record,
        };
        published
            .validate()
            .map_err(|()| CreateError::CommittedIndeterminate)?;
        Ok(published)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.prepared.revalidate(&self.prepared.preparation.name)?;
        self.prepared.preparation.location.require_absent()?;
        let expected = Self::expected_record(&self.prepared, self.record.creation_id.clone())?;
        if expected != self.record {
            return Err(());
        }
        self.prepared.revalidate(&self.prepared.preparation.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_encoding_preserves_all_128_random_bits() {
        for (bytes, expected) in [
            ([0; 16], "00000000000000000000000000"),
            ([255; 16], "7zzzzzzzzzzzzzzzzzzzzzzzzz"),
            (1u128.to_be_bytes(), "00000000000000000000000001"),
            ((1u128 << 127).to_be_bytes(), "40000000000000000000000000"),
        ] {
            let encoded = encode_identifier(bytes);
            assert_eq!(encoded, expected);
            assert!(crate::creation_ciphertext::valid_identifier(&encoded));
        }
        let mut distinct = std::collections::BTreeSet::new();
        for bit in 0..128 {
            let encoded = encode_identifier((1u128 << bit).to_be_bytes());
            assert_ne!(encoded, encode_identifier([0; 16]));
            assert!(distinct.insert(encoded));
        }
        assert_eq!(distinct.len(), 128);
    }

    #[test]
    fn rooted_statx_requires_every_requested_identity_field() {
        let complete = libc::STATX_BASIC_STATS | libc::STATX_MNT_ID;
        assert!(complete_mask(complete));
        for bit in 0..32 {
            let field = 1u32 << bit;
            if complete & field != 0 {
                assert!(!complete_mask(complete & !field));
            }
        }
    }
}
