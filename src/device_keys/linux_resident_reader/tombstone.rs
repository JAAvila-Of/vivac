//! The tombstone a retirement leaves at `admissions/<credential_id>.retiring`
//! after its commit rename: the admission record it kept, and whatever is
//! left of the package, validated against that record.
//!
//! A crash inside an identity-checked removal can leave the object under its
//! aside name, `.removing-<32 hex>`, in the package or tombstone directory.
//! Such an aside is accepted only when it matches the member it was: a
//! package member by the length and digest the record names, the record by
//! its own canonical bytes. Anything else is refused and left in place.

use super::*;

const KEY: &CStr = c"key.openssh";
const MANIFEST: &CStr = c"manifest.json";
const RECORD: &CStr = c"record.json";
const ASIDE_PREFIX: &[u8] = b".removing-";

/// Whether `name` is the aside name an identity-checked removal uses.
fn aside(name: &CStr) -> bool {
    name.to_bytes()
        .strip_prefix(ASIDE_PREFIX)
        .is_some_and(|rest| {
            rest.len() == 32
                && rest
                    .iter()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
enum Member {
    Key,
    Manifest,
}

/// Which package member a name holds: its own name, or an aside whose bytes
/// have the length and digest the record names for exactly one member.
fn classify(name: &CStr, bytes: &[u8], record: &Record) -> io::Result<Member> {
    let matches = |length: u64, digest: &str| {
        bytes.len() as u64 == length && package::hex(&Sha256::digest(bytes)) == digest
    };
    if name == KEY {
        Ok(Member::Key)
    } else if name == MANIFEST {
        Ok(Member::Manifest)
    } else if !aside(name) {
        Err(invalid())
    } else if matches(record.ciphertext_len, &record.ciphertext_sha256) {
        Ok(Member::Key)
    } else if matches(record.manifest_len, &record.manifest_sha256) {
        Ok(Member::Manifest)
    } else {
        Err(invalid())
    }
}

/// Where the tombstone's record is: under its name, under an aside, or gone.
#[derive(Clone, Debug, Eq, PartialEq)]
enum RecordPlace {
    Named,
    Aside(CString),
    Absent,
}

fn record_place(names: &[CString]) -> io::Result<RecordPlace> {
    match names {
        [] => Ok(RecordPlace::Absent),
        [name] if name.as_c_str() == RECORD => Ok(RecordPlace::Named),
        [name] if aside(name) => Ok(RecordPlace::Aside(name.clone())),
        _ => Err(invalid()),
    }
}

/// An aside in the tombstone is the record moved aside: its bytes are a
/// canonical record of this installation and credential.
fn record_aside(bytes: &[u8], installation_id: &str, credential_id: &str) -> io::Result<()> {
    let record = Record::parse(bytes)?;
    require(record.installation_id == installation_id && record.credential_id == credential_id)
}

/// What is left of a package directory once retirement may have removed
/// some of its members.
struct PartialPackage {
    directory: Directory,
    names: Vec<CString>,
    /// Any aside first, then `key.openssh`, then `manifest.json`: removal order.
    files: Vec<(CString, Member, FileSnapshot)>,
}

impl PartialPackage {
    fn capture(
        packages: &Directory,
        credential: &CStr,
        ids: (u32, u32),
        record: &Record,
    ) -> io::Result<Self> {
        let directory = capture_package(packages, credential, ids)?;
        let names = names(directory.fd.as_raw_fd(), 4)?;
        let mut files = Vec::new();
        for name in &names {
            let limit = if name.as_c_str() == MANIFEST {
                2048
            } else {
                65536
            };
            let file = FileSnapshot::capture(&directory, name, ids, limit)?;
            let member = classify(name, &file.bytes, record)?;
            files.push((name.clone(), member, file));
        }
        let mut members: Vec<_> = files.iter().map(|(_, member, _)| *member).collect();
        members.sort();
        members.dedup();
        require(members.len() == files.len())?;
        files.sort_by_key(|(name, member, _)| (!aside(name), *member));
        Ok(Self {
            directory,
            names,
            files,
        })
    }

    fn bytes(&self, kind: Member) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(_, member, _)| *member == kind)
            .map(|(_, _, file)| file.bytes.as_slice())
    }

    fn validate_retained(&self, ids: (u32, u32)) -> io::Result<()> {
        let fd = self.directory.fd.as_raw_fd();
        self.directory.validate(fd, ids)?;
        require(names(fd, 4)? == self.names)?;
        for (_, _, file) in &self.files {
            file.validate(file.fd.as_raw_fd(), ids)?;
        }
        self.directory.validate(fd, ids)
    }

    fn validate_named(&self, named: &OwnedFd, ids: (u32, u32)) -> io::Result<()> {
        let fd = named.as_raw_fd();
        self.directory.validate(fd, ids)?;
        require(names(fd, 4)? == self.names)?;
        for (name, _, file) in &self.files {
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
    names: Vec<CString>,
    record_file: Option<(CString, FileSnapshot)>,
    record: Option<Record>,
    package: Option<PartialPackage>,
}

impl CandidateTombstone {
    /// `None` when no entry holds the tombstone name. A tombstone that is
    /// present and fails any check is an error, never absence. A tombstone
    /// without its record, which only the last step leaves, must have no
    /// package left.
    pub fn open_fixed_namespace(credential_id: &str) -> io::Result<Option<Self>> {
        require(identifier(credential_id))?;
        let credential = CString::new(credential_id).map_err(|_| invalid())?;
        let name = CString::new(format!("{credential_id}.retiring")).map_err(|_| invalid())?;
        let root = CandidateRoot::open_fixed_namespace()?;
        let ids = stable_ids()?;
        require((root.binding().owner_uid, root.binding().owner_gid) == ids)?;
        let installation = root.binding().installation_id.clone();
        let admissions = Directory::capture(root.admissions_descriptor()?, ids, false)?;
        if entry_absent(admissions.fd.as_raw_fd(), &name)? {
            return Ok(None);
        }
        let tombstone =
            Directory::capture(open_at(admissions.fd.as_raw_fd(), &name, true)?, ids, true)?;
        admissions.identity.same_filesystem(&tombstone.identity)?;
        let names = names(tombstone.fd.as_raw_fd(), 4)?;
        let (record_file, record) = match record_place(&names)? {
            RecordPlace::Named => {
                let file = FileSnapshot::capture(&tombstone, RECORD, ids, 2048)?;
                let record = Record::parse(&file.bytes)?;
                (Some((RECORD.to_owned(), file)), Some(record))
            }
            RecordPlace::Aside(aside) => {
                let file = FileSnapshot::capture(&tombstone, &aside, ids, 2048)?;
                record_aside(&file.bytes, &installation, credential_id)?;
                (Some((aside, file)), None)
            }
            RecordPlace::Absent => (None, None),
        };
        let packages = Directory::capture(root.packages_descriptor()?, ids, false)?;
        admissions.identity.same_filesystem(&packages.identity)?;
        let package = if entry_absent(packages.fd.as_raw_fd(), &credential)? {
            None
        } else {
            let record = record.as_ref().ok_or_else(invalid)?;
            Some(PartialPackage::capture(
                &packages,
                &credential,
                ids,
                record,
            )?)
        };
        if let Some(record) = &record {
            compare_remaining(
                record,
                &installation,
                credential_id,
                package
                    .as_ref()
                    .and_then(|package| package.bytes(Member::Key)),
                package
                    .as_ref()
                    .and_then(|package| package.bytes(Member::Manifest)),
            )?;
        }
        let candidate = Self {
            root,
            ids,
            credential,
            name,
            admissions,
            packages,
            tombstone,
            names,
            record_file,
            record,
            package,
        };
        candidate.validate()?;
        Ok(Some(candidate))
    }

    /// The admission record the tombstone kept, or `None` once the last
    /// step has moved or removed it.
    pub fn record(&self) -> Option<&Record> {
        self.record.as_ref()
    }

    /// Whether any entry holds the admission name beside the tombstone.
    pub fn admission_present(&self) -> io::Result<bool> {
        self.validate()?;
        let present = !entry_absent(self.admissions.fd.as_raw_fd(), &self.credential)?;
        self.validate()?;
        Ok(present)
    }

    /// Whether any of the package is left.
    pub fn package_present(&self) -> bool {
        self.package.is_some()
    }

    /// The retained tombstone and package objects, for retirement.
    pub fn retirement_objects(&self) -> io::Result<RetirementObjects> {
        self.validate()?;
        let mut admission_members = Vec::new();
        if let Some((name, file)) = &self.record_file {
            admission_members.push((name.clone(), duplicate(&file.fd)?));
        }
        let package = match &self.package {
            None => None,
            Some(package) => {
                let mut members = Vec::new();
                for (name, _, file) in &package.files {
                    members.push((name.clone(), duplicate(&file.fd)?));
                }
                Some((duplicate_retained(&package.directory)?, members))
            }
        };
        let objects = RetirementObjects {
            admissions: duplicate_retained(&self.admissions)?,
            admission: duplicate_retained(&self.tombstone)?,
            admission_members,
            packages: duplicate_retained(&self.packages)?,
            package,
        };
        self.validate()?;
        Ok(objects)
    }

    pub fn has_opening_directory(&self) -> io::Result<bool> {
        self.validate()?;
        let present = self.root.has_opening_directory()?;
        self.validate()?;
        Ok(present)
    }

    pub fn has_endpoint_directory(&self) -> io::Result<bool> {
        self.validate()?;
        let present = self.root.endpoint_directory()?.is_some();
        self.validate()?;
        Ok(present)
    }

    pub fn has_session_directory(&self) -> io::Result<bool> {
        self.validate()?;
        let present = self.root.session_directory()?.is_some();
        self.validate()?;
        Ok(present)
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
        require(names(self.tombstone.fd.as_raw_fd(), 4)? == self.names)?;
        if let Some((_, file)) = &self.record_file {
            file.validate(file.fd.as_raw_fd(), self.ids)?;
        }
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
        require(names(tombstone.as_raw_fd(), 4)? == self.names)?;
        if let Some((name, file)) = &self.record_file {
            let current = open_at(tombstone.as_raw_fd(), name, false)?;
            file.validate(current.as_raw_fd(), self.ids)?;
        }
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
    use super::super::tests::{
        fixture_record, INSTALLATION, KEY as KEY_BYTES, MANIFEST as MANIFEST_BYTES,
    };
    use super::*;

    const ASIDE: &CStr = c".removing-0123456789abcdef0123456789abcdef";

    #[test]
    fn a_tombstone_accepts_only_the_package_members_its_record_names() {
        let record = fixture_record();
        let credential = record.credential_id.clone();
        for (key, manifest) in [
            (None, None),
            (Some(KEY_BYTES), None),
            (None, Some(MANIFEST_BYTES)),
            (Some(KEY_BYTES), Some(MANIFEST_BYTES)),
        ] {
            assert!(
                compare_remaining(&record, INSTALLATION, &credential, key, manifest).is_ok(),
                "{:?}",
                (key.is_some(), manifest.is_some())
            );
        }
        let mut key = KEY_BYTES.to_vec();
        key[0] ^= 1;
        let mut manifest = MANIFEST_BYTES.to_vec();
        manifest.push(b' ');
        for (key, manifest) in [
            (Some(&key[..]), None),
            (Some(&KEY_BYTES[..KEY_BYTES.len() - 1]), None),
            (None, Some(&manifest[..])),
            (Some(&key[..]), Some(MANIFEST_BYTES)),
            (Some(KEY_BYTES), Some(&manifest[..])),
            // Another package's manifest for this key.
            (Some(KEY_BYTES), Some(KEY_BYTES)),
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

    #[test]
    fn only_the_removal_aside_name_is_an_aside() {
        assert!(aside(ASIDE));
        for name in [
            c".removing-",
            c".removing-0123456789abcdef0123456789abcde",
            c".removing-0123456789abcdef0123456789abcdef0",
            c".removing-0123456789ABCDEF0123456789abcdef",
            c".removing-0123456789abcdef0123456789abcdeg",
            c"removing-0123456789abcdef0123456789abcdef0",
            c".pending-opening-0123456789abcdef0123456789abcdef",
            c"key.openssh",
        ] {
            assert!(!aside(name), "{name:?}");
        }
    }

    #[test]
    fn a_package_aside_is_accepted_only_as_the_member_it_matches() {
        let record = fixture_record();
        assert_eq!(classify(KEY, b"any", &record).unwrap(), Member::Key);
        assert_eq!(
            classify(MANIFEST, b"any", &record).unwrap(),
            Member::Manifest
        );
        assert_eq!(classify(ASIDE, KEY_BYTES, &record).unwrap(), Member::Key);
        assert_eq!(
            classify(ASIDE, MANIFEST_BYTES, &record).unwrap(),
            Member::Manifest
        );
        let mut altered = KEY_BYTES.to_vec();
        altered[0] ^= 1;
        for (name, bytes) in [
            (ASIDE, &altered[..]),
            (ASIDE, &KEY_BYTES[..KEY_BYTES.len() - 1]),
            (ASIDE, b"other bytes"),
            (c"other", KEY_BYTES),
            (c".removing-short", KEY_BYTES),
        ] {
            assert!(classify(name, bytes, &record).is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_tombstone_holds_its_record_an_aside_of_it_or_nothing() {
        let named = |names: &[&CStr]| -> Vec<CString> {
            names.iter().map(|name| (*name).to_owned()).collect()
        };
        assert_eq!(record_place(&[]).unwrap(), RecordPlace::Absent);
        assert_eq!(record_place(&named(&[RECORD])).unwrap(), RecordPlace::Named);
        assert_eq!(
            record_place(&named(&[ASIDE])).unwrap(),
            RecordPlace::Aside(ASIDE.to_owned())
        );
        for names in [
            named(&[RECORD, ASIDE]),
            named(&[c"other"]),
            named(&[KEY]),
            named(&[ASIDE, c".removing-fedcba9876543210fedcba9876543210"]),
        ] {
            assert!(record_place(&names).is_err(), "{names:?}");
        }
        let record = fixture_record();
        let canonical = record.canonical_bytes().unwrap();
        record_aside(&canonical, INSTALLATION, &record.credential_id).unwrap();
        let mut padded = canonical.clone();
        padded.push(b'\n');
        for (bytes, installation, credential) in [
            (&padded[..], INSTALLATION, record.credential_id.as_str()),
            (KEY_BYTES, INSTALLATION, record.credential_id.as_str()),
            (
                &canonical[..],
                "00000000000000000000000003",
                record.credential_id.as_str(),
            ),
            (&canonical[..], INSTALLATION, "00000000000000000000000003"),
        ] {
            assert!(record_aside(bytes, installation, credential).is_err());
        }
    }
}
