use crate::linux_openssh_package::{
    header_public_key, hex, validate_key_bytes, Manifest, MAX_KEY, MAX_MANIFEST, MAX_PACKAGE,
    PROFILE,
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::{
    Dir, DirBuilder, DirBuilderExt, File, Metadata, MetadataExt, OpenOptions, OpenOptionsExt,
};
use sha2::{Digest, Sha256};
use std::ffi::{CString, OsString};
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

#[path = "linux_package_storage/rooted.rs"]
mod rooted;
use rooted::RootedLocation;
pub(crate) use rooted::{CreationDraft, OpeningStorage, PublishedDraft};

type Result<T> = std::result::Result<T, ()>;
const KEY: &str = "key.openssh";
const MANIFEST: &str = "manifest.json";
const PREPARING: &str = ".vivac-openssh-preparing-";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FilesystemIdentity {
    device: (u32, u32),
    mount: u64,
}

impl FilesystemIdentity {
    fn same_filesystem(self, other: Self) -> bool {
        self.device == other.device && self.mount == other.mount
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

#[derive(Eq, PartialEq)]
struct Snapshot {
    identity: Identity,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Snapshot {
    fn of(metadata: &Metadata) -> Self {
        Self {
            identity: Identity::of(metadata),
            mode: metadata.mode(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            links: metadata.nlink(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

fn filesystem(fd: RawFd) -> Result<()> {
    let mut value = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fd is retained and value fits the native output structure.
    if unsafe { libc::fstatfs(fd, value.as_mut_ptr()) } != 0 {
        return Err(());
    }
    // SAFETY: successful fstatfs initialized the complete output structure.
    let value = unsafe { value.assume_init() };
    if value.f_type as u64 == 0xef53 {
        Ok(())
    } else {
        Err(())
    }
}

fn no_posix_acl(fd: RawFd, directory: bool) -> Result<()> {
    for name in [
        b"system.posix_acl_access\0".as_slice(),
        b"system.posix_acl_default\0".as_slice(),
    ]
    .into_iter()
    .take(if directory { 2 } else { 1 })
    {
        // SAFETY: fd is retained and name is NUL terminated; no value is read.
        let size = unsafe { libc::fgetxattr(fd, name.as_ptr().cast(), std::ptr::null_mut(), 0) };
        if size >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ENODATA) {
            return Err(());
        }
    }
    Ok(())
}

fn private_metadata(metadata: &Metadata, directory: bool) -> Result<()> {
    // SAFETY: geteuid only queries the calling process's effective identity.
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != if directory { 0o700 } else { 0o600 }
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(());
    }
    Ok(())
}

fn private_directory(directory: &Dir) -> Result<Metadata> {
    let metadata = directory.dir_metadata().map_err(|_| ())?;
    private_metadata(&metadata, true)?;
    let readable = readable_directory(directory)?;
    filesystem(readable.as_raw_fd())?;
    no_posix_acl(readable.as_raw_fd(), true)?;
    Ok(metadata)
}

fn readable_directory(directory: &Dir) -> Result<File> {
    let original = directory.dir_metadata().map_err(|_| ())?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .maybe_dir(true)
        .follow(FollowSymlinks::No);
    let readable = directory.open_with(".", &options).map_err(|_| ())?;
    let metadata = readable.metadata().map_err(|_| ())?;
    if !metadata.is_dir() || Identity::of(&original) != Identity::of(&metadata) {
        return Err(());
    }
    // Linux capability directories may use O_PATH, which cannot query ACLs or
    // fsync. Reopen the same retained object for those operations, not its path.
    Ok(readable)
}

fn private_file(file: &File) -> Result<Metadata> {
    let metadata = file.metadata().map_err(|_| ())?;
    private_metadata(&metadata, false)?;
    filesystem(file.as_raw_fd())?;
    no_posix_acl(file.as_raw_fd(), false)?;
    Ok(metadata)
}

fn reject_git(directory: &Dir) -> Result<()> {
    match directory.symlink_metadata(".git") {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(()),
    }
}

pub(crate) struct Location {
    ancestors: Vec<Dir>,
    components: Vec<OsString>,
    name: OsString,
    rooted: Option<RootedLocation>,
}

impl Location {
    fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() || path.as_os_str().as_bytes().len() > 4096 {
            return Err(());
        }
        let mut names = Vec::new();
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    let bytes = name.as_bytes();
                    if bytes.eq_ignore_ascii_case(b".git")
                        || bytes.eq_ignore_ascii_case(b".vivac")
                        || bytes.contains(&0)
                    {
                        return Err(());
                    }
                    names.push(name.to_owned());
                }
                _ => return Err(()),
            }
        }
        let name = names.pop().ok_or(())?;
        if name.as_bytes().starts_with(PREPARING.as_bytes()) {
            return Err(());
        }
        let root = Dir::open_ambient_dir("/", cap_std::ambient_authority()).map_err(|_| ())?;
        reject_git(&root)?;
        let mut ancestors = vec![root];
        for component in &names {
            let next = ancestors
                .last()
                .ok_or(())?
                .open_dir_nofollow(component)
                .map_err(|_| ())?;
            reject_git(&next)?;
            ancestors.push(next);
        }
        private_directory(ancestors.last().ok_or(())?)?;
        let location = Self {
            ancestors,
            components: names,
            name,
            rooted: None,
        };
        location.recheck()?;
        Ok(location)
    }

    pub(crate) fn preflight(path: &Path) -> Result<Self> {
        let location = Self::open(path)?;
        location.require_absent()?;
        Ok(location)
    }

    pub(crate) fn fixed_credential(credential: &str) -> Result<Self> {
        if !crate::creation_ciphertext::valid_identifier(credential) {
            return Err(());
        }
        let (rooted, parent) = RootedLocation::open()?;
        let location = Self {
            ancestors: vec![parent],
            components: Vec::new(),
            name: credential.into(),
            rooted: Some(rooted),
        };
        location.recheck()?;
        location.require_absent()?;
        Ok(location)
    }

    fn parent(&self) -> &Dir {
        // open always retains at least the root directory.
        &self.ancestors[self.ancestors.len() - 1]
    }

    fn recheck(&self) -> Result<()> {
        if let Some(rooted) = &self.rooted {
            return rooted.recheck(self.parent(), &self.name);
        }
        let mut current =
            Dir::open_ambient_dir("/", cap_std::ambient_authority()).map_err(|_| ())?;
        for (index, retained) in self.ancestors.iter().enumerate() {
            if Identity::of(&current.dir_metadata().map_err(|_| ())?)
                != Identity::of(&retained.dir_metadata().map_err(|_| ())?)
            {
                return Err(());
            }
            reject_git(&current)?;
            if let Some(component) = self.components.get(index) {
                current = current.open_dir_nofollow(component).map_err(|_| ())?;
            }
        }
        private_directory(&current)?;
        private_directory(self.parent())?;
        Ok(())
    }

    fn require_absent(&self) -> Result<()> {
        match self.parent().symlink_metadata(&self.name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(()),
        }
    }

    fn verify_named_directory(&self, name: &std::ffi::OsStr, held: &Dir) -> Result<()> {
        self.recheck()?;
        self.verify_named_directory_objects(name, held)
    }

    fn verify_published_directory(&self, name: &std::ffi::OsStr, held: &Dir) -> Result<()> {
        if let Some(rooted) = &self.rooted {
            rooted.validate_namespace(self.parent())?;
        } else {
            self.recheck()?;
        }
        self.verify_named_directory_objects(name, held)
    }

    fn verify_named_directory_objects(&self, name: &std::ffi::OsStr, held: &Dir) -> Result<()> {
        private_directory(held)?;
        let fresh = if self.rooted.is_some() {
            rooted::open_directory(self.parent(), name)?
        } else {
            self.parent().open_dir_nofollow(name).map_err(|_| ())?
        };
        private_directory(&fresh)?;
        if Identity::of(&fresh.dir_metadata().map_err(|_| ())?)
            != Identity::of(&held.dir_metadata().map_err(|_| ())?)
        {
            return Err(());
        }
        if let Some(rooted) = &self.rooted {
            rooted.verify_directory(held)?;
            rooted.verify_directory(&fresh)?;
        }
        Ok(())
    }
}

fn open_file(directory: &Dir, name: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = directory.open_with(name, &options).map_err(|_| ())?;
    private_file(&file)?;
    Ok(file)
}

struct HeldFile {
    name: &'static str,
    file: File,
    snapshot: Snapshot,
    maximum: u64,
}

impl HeldFile {
    fn open(directory: &Dir, name: &'static str, maximum: u64) -> Result<Self> {
        let file = open_file(directory, name)?;
        let metadata = private_file(&file)?;
        if metadata.len() == 0 || metadata.len() > maximum {
            return Err(());
        }
        let result = Self {
            name,
            file,
            snapshot: Snapshot::of(&metadata),
            maximum,
        };
        result.verify(directory)?;
        Ok(result)
    }

    fn verify(&self, directory: &Dir) -> Result<()> {
        for file in [&self.file, &open_file(directory, self.name)?] {
            if Snapshot::of(&private_file(file)?) != self.snapshot {
                return Err(());
            }
        }
        Ok(())
    }

    fn read(&self, directory: &Dir) -> Result<Vec<u8>> {
        self.verify(directory)?;
        let mut file = self.file.try_clone().map_err(|_| ())?;
        file.seek(SeekFrom::Start(0)).map_err(|_| ())?;
        let mut bytes = Vec::with_capacity(self.snapshot.size as usize);
        file.take(self.maximum + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        if bytes.len() as u64 != self.snapshot.size {
            return Err(());
        }
        self.verify(directory)?;
        Ok(bytes)
    }
}

fn exact_members(directory: &Dir, expected: &[&str]) -> Result<()> {
    let mut seen = Vec::new();
    for entry in directory.entries().map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(())?;
        if seen.len() >= expected.len()
            || !expected.contains(&name)
            || seen.iter().any(|current| current == name)
        {
            return Err(());
        }
        seen.push(name.to_owned());
    }
    if seen.len() == expected.len() {
        Ok(())
    } else {
        Err(())
    }
}

fn check_directory(directory: &Dir) -> Result<()> {
    let initial = Snapshot::of(&private_directory(directory)?);
    reject_git(directory)?;
    exact_members(directory, &[KEY, MANIFEST])?;
    let key = HeldFile::open(directory, KEY, MAX_KEY)?;
    let manifest_file = HeldFile::open(directory, MANIFEST, MAX_MANIFEST)?;
    if key.snapshot.size + manifest_file.snapshot.size > MAX_PACKAGE {
        return Err(());
    }
    let encoded_manifest = manifest_file.read(directory)?;
    let manifest: Manifest = serde_json::from_slice(&encoded_manifest).map_err(|_| ())?;
    manifest.validate().map_err(|_| ())?;
    let encoded_key = key.read(directory)?;
    validate_key_bytes(&encoded_key, &manifest).map_err(|_| ())?;
    // Re-read the retained objects after decoding; no provider or decryption is
    // involved. This check does not validate the protected identity comment.
    if key.read(directory)? != encoded_key || manifest_file.read(directory)? != encoded_manifest {
        return Err(());
    }
    exact_members(directory, &[KEY, MANIFEST])?;
    if Snapshot::of(&private_directory(directory)?) != initial {
        return Err(());
    }
    Ok(())
}

pub(crate) fn check(path: &Path) -> Result<()> {
    let location = Location::open(path)?;
    let directory = location
        .parent()
        .open_dir_nofollow(&location.name)
        .map_err(|_| ())?;
    location.verify_named_directory(&location.name, &directory)?;
    check_directory(&directory)?;
    location.verify_named_directory(&location.name, &directory)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackagePublishStage {
    BeforeRename,
    Renamed,
    ParentSynced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreateError {
    Refused,
    CommittedIndeterminate,
}

impl From<()> for CreateError {
    fn from(_: ()) -> Self {
        Self::Refused
    }
}

struct OwnedMember {
    name: &'static str,
    file: File,
    identity: Identity,
    snapshot: Snapshot,
    bytes: Vec<u8>,
}

impl OwnedMember {
    fn verify(&self, directory: &Dir, rooted: Option<&RootedLocation>) -> Result<()> {
        let fresh = if rooted.is_some() {
            rooted::open_leaf(directory, self.name)?
        } else {
            open_file(directory, self.name)?
        };
        for file in [&self.file, &fresh] {
            if let Some(rooted) = rooted {
                rooted.verify_file(file)?;
            }
            let metadata = private_file(file)?;
            if Identity::of(&metadata) != self.identity || Snapshot::of(&metadata) != self.snapshot
            {
                return Err(());
            }
        }
        let mut file = self.file.try_clone().map_err(|_| ())?;
        file.seek(SeekFrom::Start(0)).map_err(|_| ())?;
        let mut bytes = Vec::new();
        file.take(self.bytes.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        if bytes != self.bytes {
            return Err(());
        }
        Ok(())
    }
}

struct Preparation {
    location: Location,
    name: OsString,
    directory: Dir,
    identity: Identity,
    members: Vec<OwnedMember>,
    committed: bool,
}

impl Preparation {
    fn verify(&self) -> Result<()> {
        self.location
            .verify_named_directory(&self.name, &self.directory)?;
        if Identity::of(&private_directory(&self.directory)?) != self.identity {
            return Err(());
        }
        for member in &self.members {
            member.verify(&self.directory, self.location.rooted.as_ref())?;
        }
        Ok(())
    }

    fn write_member(&mut self, name: &'static str, bytes: &[u8]) -> Result<()> {
        self.verify()?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = self.directory.open_with(name, &options).map_err(|_| ())?;
        if let Some(rooted) = &self.location.rooted {
            rooted.verify_file(&file)?;
        }
        let identity = Identity::of(&private_file(&file)?);
        let snapshot = Snapshot::of(&private_file(&file)?);
        self.members.push(OwnedMember {
            name,
            file,
            identity,
            snapshot,
            bytes: Vec::new(),
        });
        let member = self.members.last_mut().ok_or(())?;
        member.file.write_all(bytes).map_err(|_| ())?;
        member.snapshot = Snapshot::of(&private_file(&member.file)?);
        member.bytes = bytes.to_vec();
        member.file.sync_all().map_err(|_| ())?;
        self.verify()
    }

    fn cleanup(&self) -> Result<()> {
        self.verify()?;
        let mut expected: Vec<&str> = self.members.iter().map(|member| member.name).collect();
        exact_members(&self.directory, &expected)?;
        for member in &self.members {
            self.location
                .verify_named_directory(&self.name, &self.directory)?;
            exact_members(&self.directory, &expected)?;
            member.verify(&self.directory, self.location.rooted.as_ref())?;
            self.directory.remove_file(member.name).map_err(|_| ())?;
            expected.retain(|name| *name != member.name);
        }
        exact_members(&self.directory, &[])?;
        self.location
            .verify_named_directory(&self.name, &self.directory)?;
        self.location
            .parent()
            .remove_dir(&self.name)
            .map_err(|_| ())
    }
}

impl Drop for Preparation {
    fn drop(&mut self) {
        if !self.committed {
            // Unknown members or changed ownership/identities preserve the
            // preparation for inspection. Cleanup never recursively descends.
            let _ = self.cleanup();
        }
    }
}

fn sync_directory(directory: &Dir) -> Result<()> {
    readable_directory(directory)?.sync_all().map_err(|_| ())
}

pub(crate) struct PreparedPackage {
    preparation: Preparation,
    key: HeldFile,
    manifest: HeldFile,
    encoded_key: Vec<u8>,
    encoded_manifest: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackagePrepareStage {
    KeySynced,
    ManifestSynced,
}

pub(crate) fn prepare(
    location: Location,
    comment: &str,
    encoded: &[u8],
) -> std::result::Result<PreparedPackage, CreateError> {
    prepare_observed(location, comment, encoded, |_| Ok(()))
}

pub(crate) fn prepare_observed(
    location: Location,
    comment: &str,
    encoded: &[u8],
    mut observe: impl FnMut(PackagePrepareStage) -> Result<()>,
) -> std::result::Result<PreparedPackage, CreateError> {
    let public_key = header_public_key(encoded).map_err(|_| ())?;
    let identifiers = comment.strip_prefix("vivac.device-key.v1:").ok_or(())?;
    let (device_id, credential_id) = identifiers.split_once(':').ok_or(())?;
    if !crate::creation_ciphertext::valid_identifier(device_id)
        || !crate::creation_ciphertext::valid_identifier(credential_id)
        || (location.rooted.is_some() && location.name.as_bytes() != credential_id.as_bytes())
    {
        return Err(CreateError::Refused);
    }
    location.recheck()?;
    location.require_absent()?;
    let mut operation = [0u8; 16];
    getrandom::getrandom(&mut operation).map_err(|_| ())?;
    let name = OsString::from(format!("{PREPARING}{}", hex(&operation)));
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    location
        .parent()
        .create_dir_with(&name, &builder)
        .map_err(|_| ())?;
    let directory = if location.rooted.is_some() {
        rooted::open_directory(location.parent(), &name)?
    } else {
        location.parent().open_dir_nofollow(&name).map_err(|_| ())?
    };
    let identity = Identity::of(&private_directory(&directory)?);
    let mut preparation = Preparation {
        location,
        name,
        directory,
        identity,
        members: Vec::new(),
        committed: false,
    };
    preparation.verify()?;
    preparation.write_member(KEY, encoded)?;
    observe(PackagePrepareStage::KeySynced)?;
    let key = HeldFile::open(&preparation.directory, KEY, MAX_KEY)?;
    let actual_key = key.read(&preparation.directory)?;
    if actual_key != encoded {
        return Err(CreateError::Refused);
    }
    let manifest = Manifest {
        version: 1,
        profile: PROFILE.to_owned(),
        device_id: device_id.to_owned(),
        credential_id: credential_id.to_owned(),
        public_key,
        key_sha256: hex(&Sha256::digest(&actual_key)),
        key_size: key.snapshot.size,
    };
    manifest.validate().map_err(|_| ())?;
    let encoded_manifest = serde_json::to_vec(&manifest).map_err(|_| ())?;
    if encoded_manifest.len() as u64 > MAX_MANIFEST {
        return Err(CreateError::Refused);
    }
    preparation.write_member(MANIFEST, &encoded_manifest)?;
    observe(PackagePrepareStage::ManifestSynced)?;
    preparation.verify()?;
    check_directory(&preparation.directory)?;
    let manifest = HeldFile::open(&preparation.directory, MANIFEST, MAX_MANIFEST)?;
    sync_directory(&preparation.directory)?;
    let prepared = PreparedPackage {
        preparation,
        key,
        manifest,
        encoded_key: actual_key,
        encoded_manifest,
    };
    prepared.revalidate(&prepared.preparation.name)?;
    Ok(prepared)
}

impl PreparedPackage {
    pub(crate) fn into_creation_draft(self) -> std::result::Result<CreationDraft, CreateError> {
        CreationDraft::from_prepared(self).map_err(CreateError::from)
    }

    fn revalidate(&self, name: &std::ffi::OsStr) -> Result<()> {
        let preparation = &self.preparation;
        preparation
            .location
            .verify_named_directory(name, &preparation.directory)?;
        self.revalidate_contents()?;
        preparation
            .location
            .verify_named_directory(name, &preparation.directory)
    }

    fn revalidate_published(&self) -> Result<()> {
        let preparation = &self.preparation;
        if !preparation.committed {
            return Err(());
        }
        preparation
            .location
            .verify_published_directory(&preparation.location.name, &preparation.directory)?;
        self.revalidate_contents()?;
        preparation
            .location
            .verify_published_directory(&preparation.location.name, &preparation.directory)
    }

    fn revalidate_contents(&self) -> Result<()> {
        let preparation = &self.preparation;
        if Identity::of(&private_directory(&preparation.directory)?) != preparation.identity {
            return Err(());
        }
        exact_members(&preparation.directory, &[KEY, MANIFEST])?;
        for member in &preparation.members {
            member.verify(&preparation.directory, preparation.location.rooted.as_ref())?;
        }
        if self.key.read(&preparation.directory)? != self.encoded_key
            || self.manifest.read(&preparation.directory)? != self.encoded_manifest
        {
            return Err(());
        }
        exact_members(&preparation.directory, &[KEY, MANIFEST])
    }

    pub(crate) fn publish(self) -> std::result::Result<(), CreateError> {
        self.publish_checked(check_directory)
    }

    pub(crate) fn publish_retained(self) -> std::result::Result<Self, CreateError> {
        self.publish_retained_with(&mut |_| Ok(()))
    }

    pub(crate) fn publish_observed(
        mut self,
        checkpoint: &mut impl FnMut(PackagePublishStage) -> Result<()>,
    ) -> std::result::Result<(), CreateError> {
        self.publish_in_place(check_directory, checkpoint)
    }

    pub(crate) fn publish_retained_with(
        mut self,
        checkpoint: &mut impl FnMut(PackagePublishStage) -> Result<()>,
    ) -> std::result::Result<Self, CreateError> {
        self.publish_in_place(check_directory, checkpoint)?;
        Ok(self)
    }

    fn publish_checked(
        mut self,
        check: impl FnOnce(&Dir) -> Result<()>,
    ) -> std::result::Result<(), CreateError> {
        self.publish_in_place(check, &mut |_| Ok(()))
    }

    fn publish_in_place(
        &mut self,
        check: impl FnOnce(&Dir) -> Result<()>,
        checkpoint: &mut impl FnMut(PackagePublishStage) -> Result<()>,
    ) -> std::result::Result<(), CreateError> {
        self.revalidate(&self.preparation.name)?;
        let preparation = &mut self.preparation;
        preparation.location.require_absent()?;
        checkpoint(PackagePublishStage::BeforeRename)?;
        let source = CString::new(preparation.name.as_bytes()).map_err(|_| ())?;
        let destination = CString::new(preparation.location.name.as_bytes()).map_err(|_| ())?;
        // SAFETY: names and retained same-parent directory descriptor stay live;
        // RENAME_NOREPLACE is mandatory and has no replacing-rename fallback.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                preparation.location.parent().as_raw_fd(),
                source.as_ptr(),
                preparation.location.parent().as_raw_fd(),
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(CreateError::Refused);
        }
        preparation.committed = true;
        let after_commit = (|| -> Result<()> {
            checkpoint(PackagePublishStage::Renamed)?;
            sync_directory(self.preparation.location.parent())?;
            checkpoint(PackagePublishStage::ParentSynced)?;
            self.revalidate(&self.preparation.location.name)?;
            check(&self.preparation.directory)?;
            self.revalidate(&self.preparation.location.name)
        })();
        after_commit.map_err(|()| CreateError::CommittedIndeterminate)
    }
}

#[cfg(test)]
#[path = "linux_package_storage/preparation_tests.rs"]
mod preparation_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooted_filesystem_comparison_refuses_same_device_bind_mount() {
        let original = FilesystemIdentity {
            device: (8, 1),
            mount: 41,
        };
        let bind = FilesystemIdentity {
            mount: 42,
            ..original
        };
        assert!(!original.same_filesystem(bind));
        assert!(original.same_filesystem(original));
    }

    #[test]
    fn ordinary_creation_errors_do_not_claim_a_committed_package() {
        assert_eq!(CreateError::from(()), CreateError::Refused);
        assert_ne!(CreateError::Refused, CreateError::CommittedIndeterminate);
    }

    #[test]
    fn strict_manifest_rejects_duplicate_unknown_and_noncanonical_fields() {
        let valid = Manifest {
            version: 1,
            profile: PROFILE.to_owned(),
            device_id: "00000000000000000000000001".to_owned(),
            credential_id: "00000000000000000000000002".to_owned(),
            public_key: "a".repeat(64),
            key_sha256: "b".repeat(64),
            key_size: 500,
        };
        let encoded = serde_json::to_string(&valid).expect("public fixture serialization failed");
        let parsed: Manifest =
            serde_json::from_str(&encoded).expect("public fixture parsing failed");
        assert!(parsed.validate().is_ok());
        for extra in [",\"version\":1}", ",\"unknown\":false}"] {
            let changed = format!("{}{}", &encoded[..encoded.len() - 1], extra);
            assert!(serde_json::from_str::<Manifest>(&changed).is_err());
        }
        let mut parsed = parsed;
        parsed.public_key = "A".repeat(64);
        assert!(parsed.validate().is_err());
        parsed.public_key = "a".repeat(64);
        parsed.key_size = MAX_KEY + 1;
        assert!(parsed.validate().is_err());
    }
}
