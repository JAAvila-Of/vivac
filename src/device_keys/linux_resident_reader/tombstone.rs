//! The tombstone a retirement leaves at `admissions/<credential_id>.retiring`
//! after its commit rename: the admission record it kept, and whatever is
//! left of the package, validated against that record.

use super::*;

const PACKAGE_MEMBERS: [&CStr; 2] = [c"key.openssh", c"manifest.json"];

/// What is left of a package directory once retirement may have removed
/// some of its members.
struct PartialPackage {
    directory: Directory,
    members: Vec<bool>,
    key_file: Option<FileSnapshot>,
    manifest_file: Option<FileSnapshot>,
}

impl PartialPackage {
    fn capture(packages: &Directory, credential: &CStr, ids: (u32, u32)) -> io::Result<Self> {
        let directory = capture_package(packages, credential, ids)?;
        let members = members(directory.fd.as_raw_fd(), &PACKAGE_MEMBERS)?;
        let key_file = members[0]
            .then(|| FileSnapshot::capture(&directory, PACKAGE_MEMBERS[0], ids, 65536))
            .transpose()?;
        let manifest_file = members[1]
            .then(|| FileSnapshot::capture(&directory, PACKAGE_MEMBERS[1], ids, 2048))
            .transpose()?;
        Ok(Self {
            directory,
            members,
            key_file,
            manifest_file,
        })
    }

    fn files(&self) -> impl Iterator<Item = (&'static CStr, &FileSnapshot)> {
        PACKAGE_MEMBERS
            .into_iter()
            .zip([&self.key_file, &self.manifest_file])
            .filter_map(|(name, file)| file.as_ref().map(|file| (name, file)))
    }

    fn validate_retained(&self, ids: (u32, u32)) -> io::Result<()> {
        let fd = self.directory.fd.as_raw_fd();
        self.directory.validate(fd, ids)?;
        require(members(fd, &PACKAGE_MEMBERS)? == self.members)?;
        for (_, file) in self.files() {
            file.validate(file.fd.as_raw_fd(), ids)?;
        }
        self.directory.validate(fd, ids)
    }

    fn validate_named(&self, named: &OwnedFd, ids: (u32, u32)) -> io::Result<()> {
        let fd = named.as_raw_fd();
        self.directory.validate(fd, ids)?;
        require(members(fd, &PACKAGE_MEMBERS)? == self.members)?;
        for (name, file) in self.files() {
            let current = open_at(fd, name, false)?;
            file.validate(current.as_raw_fd(), ids)?;
        }
        self.directory.validate(fd, ids)
    }
}

/// Whether what remains of a package belongs to `record`: each member left
/// has the length and digest the record names, and when both are left they
/// form the package the record admitted.
fn compare_remaining(
    record: &Record,
    installation_id: &str,
    credential_id: &str,
    key: Option<&[u8]>,
    manifest: Option<&[u8]>,
) -> io::Result<()> {
    require(record.installation_id == installation_id && record.credential_id == credential_id)?;
    if let Some(key) = key {
        require(
            key.len() as u64 == record.ciphertext_len
                && package::hex(&Sha256::digest(key)) == record.ciphertext_sha256,
        )?;
    }
    if let Some(manifest) = manifest {
        require(
            manifest.len() as u64 == record.manifest_len
                && package::hex(&Sha256::digest(manifest)) == record.manifest_sha256,
        )?;
    }
    if let (Some(key), Some(manifest)) = (key, manifest) {
        compare_package(record, installation_id, credential_id, key, manifest)?;
    }
    Ok(())
}

/// Read-only tombstone candidate. It admits nothing: no reader selects a
/// tombstone, and it grants no authority to open the credential.
pub struct CandidateTombstone {
    root: CandidateRoot,
    ids: (u32, u32),
    credential: CString,
    name: CString,
    admissions: Directory,
    packages: Directory,
    tombstone: Directory,
    record_file: FileSnapshot,
    record: Record,
    package: Option<PartialPackage>,
}

impl CandidateTombstone {
    /// `None` when no entry holds the tombstone name. A tombstone that is
    /// present and fails any check is an error, never absence.
    pub fn open_fixed_namespace(credential_id: &str) -> io::Result<Option<Self>> {
        require(identifier(credential_id))?;
        let credential = CString::new(credential_id).map_err(|_| invalid())?;
        let name = CString::new(format!("{credential_id}.retiring")).map_err(|_| invalid())?;
        let root = CandidateRoot::open_fixed_namespace()?;
        let ids = stable_ids()?;
        require((root.binding().owner_uid, root.binding().owner_gid) == ids)?;
        let admissions = Directory::capture(root.admissions_descriptor()?, ids, false)?;
        if entry_absent(admissions.fd.as_raw_fd(), &name)? {
            return Ok(None);
        }
        let tombstone =
            Directory::capture(open_at(admissions.fd.as_raw_fd(), &name, true)?, ids, true)?;
        admissions.identity.same_filesystem(&tombstone.identity)?;
        shape(tombstone.fd.as_raw_fd(), &[c"record.json"])?;
        let record_file = FileSnapshot::capture(&tombstone, c"record.json", ids, 2048)?;
        let record = Record::parse(&record_file.bytes)?;
        let packages = Directory::capture(root.packages_descriptor()?, ids, false)?;
        admissions.identity.same_filesystem(&packages.identity)?;
        let package = if entry_absent(packages.fd.as_raw_fd(), &credential)? {
            None
        } else {
            Some(PartialPackage::capture(&packages, &credential, ids)?)
        };
        compare_remaining(
            &record,
            &root.binding().installation_id,
            credential_id,
            package
                .as_ref()
                .and_then(|package| package.key_file.as_ref())
                .map(|file| file.bytes.as_slice()),
            package
                .as_ref()
                .and_then(|package| package.manifest_file.as_ref())
                .map(|file| file.bytes.as_slice()),
        )?;
        let candidate = Self {
            root,
            ids,
            credential,
            name,
            admissions,
            packages,
            tombstone,
            record_file,
            record,
            package,
        };
        candidate.validate()?;
        Ok(Some(candidate))
    }

    /// The admission record the tombstone kept.
    pub fn record(&self) -> &Record {
        &self.record
    }

    /// Whether any entry holds the admission name beside the tombstone.
    pub fn admission_present(&self) -> io::Result<bool> {
        self.validate()?;
        let present = !entry_absent(self.admissions.fd.as_raw_fd(), &self.credential)?;
        self.validate()?;
        Ok(present)
    }

    /// A fresh descriptor for the package directory, which anchors the
    /// credential reservation, or `None` once it is gone.
    pub fn package_directory(&self) -> io::Result<Option<OwnedFd>> {
        self.validate()?;
        let directory = self
            .package
            .as_ref()
            .map(|package| duplicate_retained(&package.directory))
            .transpose()?;
        self.validate()?;
        Ok(directory)
    }

    pub fn with_opening_location<T>(
        &self,
        operation: impl FnOnce(&CStr, OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self.root.with_opening_directory(|directory| {
            operation(&self.credential, directory.try_clone()?)
        });
        self.validate()?;
        result
    }

    pub fn with_endpoint_location<T>(
        &self,
        operation: impl FnOnce(&CStr, OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self
            .root
            .with_endpoint_directory(|directory| operation(&self.credential, directory));
        self.validate()?;
        result
    }

    pub fn with_session_location<T>(
        &self,
        operation: impl FnOnce(&CStr, OwnedFd) -> io::Result<T>,
    ) -> io::Result<T> {
        self.validate()?;
        let result = self
            .root
            .with_session_directory(|directory| operation(&self.credential, directory));
        self.validate()?;
        result
    }

    pub fn validate(&self) -> io::Result<()> {
        require(stable_ids()? == self.ids)?;
        self.root.validate()?;
        for _ in 0..2 {
            self.validate_retained()?;
            self.validate_named()?;
        }
        self.root.validate()?;
        require(stable_ids()? == self.ids)
    }

    fn validate_retained(&self) -> io::Result<()> {
        for directory in [&self.admissions, &self.packages, &self.tombstone] {
            directory.validate(directory.fd.as_raw_fd(), self.ids)?;
        }
        shape(self.tombstone.fd.as_raw_fd(), &[c"record.json"])?;
        self.record_file
            .validate(self.record_file.fd.as_raw_fd(), self.ids)?;
        if let Some(package) = &self.package {
            package.validate_retained(self.ids)?;
        }
        self.tombstone
            .validate(self.tombstone.fd.as_raw_fd(), self.ids)
    }

    fn validate_named(&self) -> io::Result<()> {
        let admissions = self.root.admissions_descriptor()?;
        let packages = self.root.packages_descriptor()?;
        self.admissions.validate(admissions.as_raw_fd(), self.ids)?;
        self.packages.validate(packages.as_raw_fd(), self.ids)?;
        let tombstone = open_at(admissions.as_raw_fd(), &self.name, true)?;
        self.tombstone.validate(tombstone.as_raw_fd(), self.ids)?;
        shape(tombstone.as_raw_fd(), &[c"record.json"])?;
        let record = open_at(tombstone.as_raw_fd(), c"record.json", false)?;
        self.record_file.validate(record.as_raw_fd(), self.ids)?;
        match &self.package {
            None => require(entry_absent(packages.as_raw_fd(), &self.credential)?)?,
            Some(package) => {
                let named = open_at(packages.as_raw_fd(), &self.credential, true)?;
                package.validate_named(&named, self.ids)?;
            }
        }
        self.tombstone.validate(tombstone.as_raw_fd(), self.ids)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture_record, INSTALLATION, KEY, MANIFEST};
    use super::*;

    #[test]
    fn a_tombstone_accepts_only_the_package_members_its_record_names() {
        let record = fixture_record();
        let credential = record.credential_id.clone();
        for (key, manifest) in [
            (None, None),
            (Some(KEY), None),
            (None, Some(MANIFEST)),
            (Some(KEY), Some(MANIFEST)),
        ] {
            assert!(
                compare_remaining(&record, INSTALLATION, &credential, key, manifest).is_ok(),
                "{:?}",
                (key.is_some(), manifest.is_some())
            );
        }
        let mut key = KEY.to_vec();
        key[0] ^= 1;
        let mut manifest = MANIFEST.to_vec();
        manifest.push(b' ');
        for (key, manifest) in [
            (Some(&key[..]), None),
            (Some(&KEY[..KEY.len() - 1]), None),
            (None, Some(&manifest[..])),
            (Some(&key[..]), Some(MANIFEST)),
            (Some(KEY), Some(&manifest[..])),
            // Another package's manifest for this key.
            (Some(KEY), Some(KEY)),
        ] {
            assert!(
                compare_remaining(&record, INSTALLATION, &credential, key, manifest).is_err(),
                "{:?}",
                (key.map(<[u8]>::len), manifest.map(<[u8]>::len))
            );
        }
        for (installation, other) in [
            ("00000000000000000000000003", credential.as_str()),
            (INSTALLATION, "00000000000000000000000003"),
        ] {
            assert!(compare_remaining(&record, installation, other, None, None).is_err());
        }
    }
}
