//! The retirement steps after a confirmed start. Every object is reached
//! through a descriptor retained before the commit, and every removal checks
//! that the named object is still the retained one; nothing is removed
//! recursively. The commit rename is the only step whose failure leaves the
//! credential as it was.

use crate::linux_reservation::StoreReservation;
use crate::linux_resident_reader::RetirementObjects;
use crate::linux_retained_removal::{
    entry_identity, remove_retained_entry, rename_no_replace, sync_directory, EntryIdentity,
};
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::OwnedFd;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RetirementStep {
    Commit,
    Permit,
    Endpoint,
    Record,
    Package,
    Tombstone,
}

/// Which stored objects a retirement found before it changed anything.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Presence {
    pub admission: bool,
    pub permit: bool,
    pub endpoint: bool,
    pub record: bool,
    pub package: bool,
    pub tombstone: bool,
}

/// The steps after the commit, in order, each with whether its object is
/// present.
fn removals(present: Presence) -> [(RetirementStep, bool); 5] {
    [
        (RetirementStep::Permit, present.permit),
        (RetirementStep::Endpoint, present.endpoint),
        (RetirementStep::Record, present.record),
        (RetirementStep::Package, present.package),
        (RetirementStep::Tombstone, present.tombstone),
    ]
}

/// From an admission, every step. From a tombstone alone, every step from
/// the first object still present. Anything else plans nothing, and the
/// caller refuses.
pub(super) fn remaining_steps(present: Presence) -> Vec<RetirementStep> {
    match (present.admission, present.tombstone) {
        (true, false) => {
            let mut steps = vec![RetirementStep::Commit];
            steps.extend(removals(present).map(|(step, _)| step));
            steps
        }
        (false, true) => removals(present)
            .into_iter()
            .skip_while(|(_, present)| !present)
            .map(|(step, _)| step)
            .collect(),
        _ => Vec::new(),
    }
}

/// The steps a run from the tombstone skips because their object is gone.
/// An interrupted run may have removed the object without its directory
/// being synchronized, so each of these directories is synchronized again.
pub(super) fn skipped_steps(present: Presence) -> Vec<RetirementStep> {
    if present.admission || !present.tombstone {
        return Vec::new();
    }
    removals(present)
        .into_iter()
        .take_while(|(_, present)| !present)
        .map(|(step, _)| step)
        .collect()
}

/// A tombstone without its record is what a crash inside the last step
/// leaves, after every other object is gone. With any other object left it
/// is not a state retirement produces.
pub(super) fn bare_tombstone_allows(present: Presence) -> bool {
    !(present.admission || present.permit || present.endpoint || present.record || present.package)
}

/// The nonblocking exclusive lock on the admission directory, which after
/// the commit rename is the tombstone, lets one retirement of a
/// credential run at a time; the package reservation is the credential reservation.
/// Either one held elsewhere refuses without waiting.
pub(super) struct Locks {
    _admission: StoreReservation,
    _package: Option<StoreReservation>,
}

pub(super) fn hold(admission: &OwnedFd, package: Option<&OwnedFd>) -> io::Result<Locks> {
    let admission = StoreReservation::acquire_directory(admission)?;
    let package = package
        .map(StoreReservation::acquire_directory)
        .transpose()?;
    Ok(Locks {
        _admission: admission,
        _package: package,
    })
}

impl Locks {
    pub(super) fn validate(&self) -> io::Result<()> {
        self._admission.validate_owned()?;
        if let Some(package) = &self._package {
            package.validate_owned()?;
        }
        Ok(())
    }
}

/// One name in a retained collection and the object found under it, if any.
pub(super) struct Entry {
    pub directory: OwnedFd,
    pub name: CString,
    pub identity: Option<EntryIdentity>,
}

struct Member {
    name: CString,
    identity: EntryIdentity,
}

/// A retained directory, its identity and its members in removal order.
struct Container {
    directory: OwnedFd,
    identity: EntryIdentity,
    members: Vec<Member>,
}

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(invalid())
    }
}

impl Container {
    /// The retained directory must be the one `parent` names, and each
    /// retained member the one the directory names.
    fn capture(
        parent: &OwnedFd,
        name: &CStr,
        directory: OwnedFd,
        members: Vec<(CString, OwnedFd)>,
    ) -> io::Result<Self> {
        let identity = EntryIdentity::from_descriptor(&directory)?;
        require(entry_identity(parent, name)? == Some(identity))?;
        let mut retained = Vec::new();
        for (name, file) in members {
            let identity = EntryIdentity::from_descriptor(&file)?;
            require(entry_identity(&directory, &name)? == Some(identity))?;
            retained.push(Member { name, identity });
        }
        Ok(Self {
            directory,
            identity,
            members: retained,
        })
    }

    fn check_named(&self, parent: &OwnedFd, name: &CStr) -> io::Result<()> {
        require(EntryIdentity::from_descriptor(&self.directory)? == self.identity)?;
        require(entry_identity(parent, name)? == Some(self.identity))?;
        for member in &self.members {
            require(entry_identity(&self.directory, &member.name)? == Some(member.identity))?;
        }
        Ok(())
    }

    /// Removes each retained member, then the directory itself, which any
    /// other member keeps from being removed.
    fn remove(&self, parent: &OwnedFd, name: &CStr) -> io::Result<()> {
        for member in &self.members {
            remove_retained_entry(&self.directory, &member.name, &member.identity)?;
        }
        remove_retained_entry(parent, name, &self.identity)
    }
}

/// Everything a retirement removes, retained before the commit.
pub(super) struct Plan {
    credential: CString,
    tombstone_name: CString,
    from_admission: bool,
    admissions: OwnedFd,
    admission: Container,
    permit: Entry,
    endpoint: Entry,
    record: Entry,
    packages: OwnedFd,
    package: Option<Container>,
}

/// How a run of steps ended without completing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Failure {
    /// The commit rename did not happen; nothing changed.
    Unchanged,
    /// Some state remains or its removal is not known to be durable.
    Incomplete,
}

impl Plan {
    pub(super) fn new(
        credential: &str,
        from_admission: bool,
        objects: RetirementObjects,
        [permit, endpoint, record]: [Entry; 3],
    ) -> io::Result<Self> {
        let credential_name = CString::new(credential).map_err(|_| invalid())?;
        let tombstone_name =
            CString::new(format!("{credential}.retiring")).map_err(|_| invalid())?;
        let RetirementObjects {
            admissions,
            admission,
            admission_members,
            packages,
            package,
        } = objects;
        let admission_name = if from_admission {
            &credential_name
        } else {
            &tombstone_name
        };
        let admission =
            Container::capture(&admissions, admission_name, admission, admission_members)?;
        let package = package
            .map(|(directory, members)| {
                Container::capture(&packages, &credential_name, directory, members)
            })
            .transpose()?;
        for entry in [&permit, &endpoint, &record] {
            require(entry.name == credential_name)?;
        }
        let plan = Self {
            credential: credential_name,
            tombstone_name,
            from_admission,
            admissions,
            admission,
            permit,
            endpoint,
            record,
            packages,
            package,
        };
        plan.check_named()?;
        Ok(plan)
    }

    pub(super) fn presence(&self) -> Presence {
        Presence {
            admission: self.from_admission,
            permit: self.permit.identity.is_some(),
            endpoint: self.endpoint.identity.is_some(),
            record: self.record.identity.is_some(),
            package: self.package.is_some(),
            tombstone: !self.from_admission,
        }
    }

    fn admission_name(&self) -> &CStr {
        if self.from_admission {
            &self.credential
        } else {
            &self.tombstone_name
        }
    }

    /// Every name still holds the retained object, or still holds nothing,
    /// and the name the admission is not under holds nothing.
    pub(super) fn check_named(&self) -> io::Result<()> {
        self.admission
            .check_named(&self.admissions, self.admission_name())?;
        let other = if self.from_admission {
            &self.tombstone_name
        } else {
            &self.credential
        };
        require(entry_identity(&self.admissions, other)?.is_none())?;
        for entry in [&self.permit, &self.endpoint, &self.record] {
            require(entry_identity(&entry.directory, &entry.name)? == entry.identity)?;
        }
        match &self.package {
            Some(package) => package.check_named(&self.packages, &self.credential),
            None => require(entry_identity(&self.packages, &self.credential)?.is_none()),
        }
    }

    /// Runs `steps`. A run from the tombstone first synchronizes the admission
    /// directory and the directory of each step in `skipped`.
    pub(super) fn run(
        &self,
        steps: &[RetirementStep],
        skipped: &[RetirementStep],
    ) -> Result<(), Failure> {
        let mut steps = steps;
        if let [RetirementStep::Commit, rest @ ..] = steps {
            rename_no_replace(&self.admissions, &self.credential, &self.tombstone_name)
                .map_err(|_| Failure::Unchanged)?;
            steps = rest;
            self.committed().map_err(|_| Failure::Incomplete)?;
        } else {
            self.synchronize_again(skipped)
                .map_err(|_| Failure::Incomplete)?;
        }
        for step in steps {
            self.step(*step).map_err(|_| Failure::Incomplete)?;
        }
        Ok(())
    }

    /// The commit rename moved the retained admission, and nothing else is
    /// under its old name; no removal starts before this is durable.
    fn committed(&self) -> io::Result<()> {
        sync_directory(&self.admissions)?;
        require(
            EntryIdentity::from_descriptor(&self.admission.directory)? == self.admission.identity,
        )?;
        require(
            entry_identity(&self.admissions, &self.tombstone_name)?
                == Some(self.admission.identity),
        )?;
        require(entry_identity(&self.admissions, &self.credential)?.is_none())
    }

    fn synchronize_again(&self, skipped: &[RetirementStep]) -> io::Result<()> {
        sync_directory(&self.admissions)?;
        for step in skipped {
            let directory = match step {
                RetirementStep::Permit => &self.permit.directory,
                RetirementStep::Endpoint => &self.endpoint.directory,
                RetirementStep::Record => &self.record.directory,
                RetirementStep::Package => &self.packages,
                RetirementStep::Commit | RetirementStep::Tombstone => return Err(invalid()),
            };
            sync_directory(directory)?;
        }
        Ok(())
    }

    fn step(&self, step: RetirementStep) -> io::Result<()> {
        match step {
            RetirementStep::Commit => Err(invalid()),
            RetirementStep::Permit => remove_entry(&self.permit),
            RetirementStep::Endpoint => remove_entry(&self.endpoint),
            RetirementStep::Record => remove_entry(&self.record),
            RetirementStep::Package => {
                if let Some(package) = &self.package {
                    package.remove(&self.packages, &self.credential)?;
                }
                sync_directory(&self.packages)?;
                require(entry_identity(&self.packages, &self.credential)?.is_none())
            }
            RetirementStep::Tombstone => {
                self.admission
                    .remove(&self.admissions, &self.tombstone_name)?;
                sync_directory(&self.admissions)?;
                require(entry_identity(&self.admissions, &self.tombstone_name)?.is_none())?;
                require(entry_identity(&self.admissions, &self.credential)?.is_none())
            }
        }
    }
}

/// Removes the retained object if there was one, synchronizes the
/// collection, and requires the name to be empty afterwards.
fn remove_entry(entry: &Entry) -> io::Result<()> {
    if let Some(identity) = &entry.identity {
        remove_retained_entry(&entry.directory, &entry.name, identity)?;
    }
    sync_directory(&entry.directory)?;
    require(entry_identity(&entry.directory, &entry.name)?.is_none())
}

#[cfg(test)]
mod tests {
    use super::RetirementStep::{Commit, Endpoint, Package, Permit, Record, Tombstone};
    use super::*;
    use crate::linux_retained_removal::tests::{scratch, Scratch};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    const CREDENTIAL: &str = "0123456789abcdefghjkmnpqrs";
    const ALL: Presence = Presence {
        admission: true,
        permit: true,
        endpoint: true,
        record: true,
        package: true,
        tombstone: false,
    };

    #[test]
    fn retirement_resumes_from_the_first_present_object() {
        assert_eq!(
            remaining_steps(ALL),
            [Commit, Permit, Endpoint, Record, Package, Tombstone]
        );
        let interrupted = Presence {
            admission: false,
            tombstone: true,
            permit: false,
            endpoint: false,
            record: true,
            package: true,
        };
        assert_eq!(remaining_steps(interrupted), [Record, Package, Tombstone]);
        assert_eq!(skipped_steps(interrupted), [Permit, Endpoint]);
        // From an admission every step runs, whatever is absent.
        let bare = Presence {
            admission: true,
            package: true,
            ..Presence::default()
        };
        assert_eq!(
            remaining_steps(bare),
            [Commit, Permit, Endpoint, Record, Package, Tombstone]
        );
        assert!(skipped_steps(bare).is_empty());
        // A permit left with the rest gone continues at the permit.
        let permit = Presence {
            tombstone: true,
            permit: true,
            ..Presence::default()
        };
        assert_eq!(
            remaining_steps(permit),
            [Permit, Endpoint, Record, Package, Tombstone]
        );
        assert!(skipped_steps(permit).is_empty());
        // Only the tombstone left, with or without its record.
        let last = Presence {
            tombstone: true,
            ..Presence::default()
        };
        assert_eq!(remaining_steps(last), [Tombstone]);
        assert_eq!(skipped_steps(last), [Permit, Endpoint, Record, Package]);
    }

    #[test]
    fn retirement_never_plans_without_admission_or_tombstone() {
        for both in [false, true] {
            let present = Presence {
                admission: both,
                tombstone: both,
                ..ALL
            };
            assert!(remaining_steps(present).is_empty(), "{present:?}");
            assert!(skipped_steps(present).is_empty(), "{present:?}");
        }
    }

    #[test]
    fn a_tombstone_without_its_record_resumes_only_when_everything_else_is_gone() {
        let last = Presence {
            tombstone: true,
            ..Presence::default()
        };
        assert!(bare_tombstone_allows(last));
        for present in [
            Presence {
                permit: true,
                ..last
            },
            Presence {
                endpoint: true,
                ..last
            },
            Presence {
                record: true,
                ..last
            },
            Presence {
                package: true,
                ..last
            },
            Presence {
                admission: true,
                ..last
            },
        ] {
            assert!(!bare_tombstone_allows(present), "{present:?}");
        }
    }

    #[test]
    fn a_second_retirement_refuses_while_the_first_holds_the_admission_lock() {
        let (root, _fd) = scratch();
        let admission = root.0.join(CREDENTIAL);
        let package = root.0.join("package");
        for path in [&admission, &package] {
            std::fs::create_dir(path).unwrap();
        }
        let open = |path: &Path| -> OwnedFd { std::fs::File::open(path).unwrap().into() };
        let first = hold(&open(&admission), Some(&open(&package))).unwrap();
        for second in [
            hold(&open(&admission), Some(&open(&package))),
            hold(&open(&admission), None),
        ] {
            let error = second.err().expect("the admission lock is held");
            assert_eq!(error.raw_os_error(), Some(libc::EWOULDBLOCK));
        }
        // After the commit rename the tombstone is the same directory.
        let tombstone = root.0.join(format!("{CREDENTIAL}.retiring"));
        std::fs::rename(&admission, &tombstone).unwrap();
        let error = hold(&open(&tombstone), None).err().expect("still held");
        assert_eq!(error.raw_os_error(), Some(libc::EWOULDBLOCK));
        first.validate().unwrap();
        drop(first);
        let again = hold(&open(&tombstone), Some(&open(&package))).unwrap();
        // A held package reservation refuses too, and releases the admission
        // lock it took.
        let other = root.0.join("other");
        std::fs::create_dir(&other).unwrap();
        let error = hold(&open(&other), Some(&open(&package)))
            .err()
            .expect("the package reservation is held");
        assert_eq!(error.raw_os_error(), Some(libc::EWOULDBLOCK));
        drop(hold(&open(&other), None).unwrap());
        drop(again);
    }

    /// A synthetic state directory with every object retirement removes.
    struct Tree {
        root: Scratch,
        _socket: std::os::unix::net::UnixListener,
    }

    fn directory(path: &Path) {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn tree() -> Tree {
        let (root, _fd) = scratch();
        for name in [
            "admissions",
            "packages",
            "openings-v1",
            "endpoints-v1",
            "sessions-v1",
        ] {
            directory(&root.0.join(name));
        }
        directory(&root.0.join("admissions").join(CREDENTIAL));
        std::fs::write(
            root.0
                .join("admissions")
                .join(CREDENTIAL)
                .join("record.json"),
            b"r",
        )
        .unwrap();
        directory(&root.0.join("packages").join(CREDENTIAL));
        for name in ["key.openssh", "manifest.json"] {
            std::fs::write(root.0.join("packages").join(CREDENTIAL).join(name), b"p").unwrap();
        }
        std::fs::write(root.0.join("openings-v1").join(CREDENTIAL), b"o").unwrap();
        std::fs::write(root.0.join("sessions-v1").join(CREDENTIAL), b"s").unwrap();
        let socket =
            std::os::unix::net::UnixListener::bind(root.0.join("endpoints-v1").join(CREDENTIAL))
                .unwrap();
        Tree {
            root,
            _socket: socket,
        }
    }

    impl Tree {
        fn path(&self, parts: &[&str]) -> std::path::PathBuf {
            parts
                .iter()
                .fold(self.root.0.clone(), |path, part| path.join(part))
        }

        fn open(&self, parts: &[&str]) -> OwnedFd {
            std::fs::File::open(self.path(parts)).unwrap().into()
        }

        fn names(&self, parts: &[&str]) -> Vec<String> {
            let mut names: Vec<_> = std::fs::read_dir(self.path(parts))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }

        fn entry(&self, collection: &str) -> Entry {
            let directory = self.open(&[collection]);
            let name = CString::new(CREDENTIAL).unwrap();
            let identity = entry_identity(&directory, &name).unwrap();
            Entry {
                directory,
                name,
                identity,
            }
        }

        /// What a reader would retain, from the admission or the tombstone.
        fn plan(
            &self,
            from_admission: bool,
            members: &[&str],
            package: Option<&[&str]>,
        ) -> io::Result<Plan> {
            let admission = if from_admission {
                CREDENTIAL.to_owned()
            } else {
                format!("{CREDENTIAL}.retiring")
            };
            let objects = RetirementObjects {
                admissions: self.open(&["admissions"]),
                admission: self.open(&["admissions", &admission]),
                admission_members: members
                    .iter()
                    .map(|name| {
                        (
                            CString::new(*name).unwrap(),
                            self.open(&["admissions", &admission, name]),
                        )
                    })
                    .collect(),
                packages: self.open(&["packages"]),
                package: package.map(|members| {
                    (
                        self.open(&["packages", CREDENTIAL]),
                        members
                            .iter()
                            .map(|name| {
                                (
                                    CString::new(*name).unwrap(),
                                    self.open(&["packages", CREDENTIAL, name]),
                                )
                            })
                            .collect(),
                    )
                }),
            };
            Plan::new(
                CREDENTIAL,
                from_admission,
                objects,
                [
                    self.entry("openings-v1"),
                    self.entry("endpoints-v1"),
                    self.entry("sessions-v1"),
                ],
            )
        }

        fn empty(&self) -> bool {
            [
                "admissions",
                "packages",
                "openings-v1",
                "endpoints-v1",
                "sessions-v1",
            ]
            .iter()
            .all(|name| self.names(&[name]).is_empty())
        }
    }

    const PACKAGE: &[&str] = &["key.openssh", "manifest.json"];

    #[test]
    fn a_confirmed_retirement_removes_every_retained_object() {
        let tree = tree();
        let plan = tree.plan(true, &["record.json"], Some(PACKAGE)).unwrap();
        assert_eq!(plan.presence(), ALL);
        plan.run(&remaining_steps(plan.presence()), &[]).unwrap();
        assert!(tree.empty());
    }

    #[test]
    fn an_occupied_tombstone_name_leaves_the_credential_unchanged() {
        let tree = tree();
        let plan = tree.plan(true, &["record.json"], Some(PACKAGE)).unwrap();
        directory(&tree.path(&["admissions", &format!("{CREDENTIAL}.retiring")]));
        assert!(plan.check_named().is_err());
        assert_eq!(
            plan.run(&remaining_steps(plan.presence()), &[]),
            Err(Failure::Unchanged)
        );
        assert_eq!(tree.names(&["admissions", CREDENTIAL]), ["record.json"]);
        assert_eq!(tree.names(&["packages", CREDENTIAL]), PACKAGE);
    }

    #[test]
    fn a_replaced_object_stops_the_retirement_and_stays_in_place() {
        let tree = tree();
        let plan = tree.plan(true, &["record.json"], Some(PACKAGE)).unwrap();
        let session = tree.path(&["sessions-v1", CREDENTIAL]);
        let replacement = tree.path(&["sessions-v1", "replacement"]);
        std::fs::write(&replacement, b"replacement").unwrap();
        std::fs::rename(&replacement, &session).unwrap();
        assert_eq!(
            plan.run(&remaining_steps(plan.presence()), &[]),
            Err(Failure::Incomplete)
        );
        assert_eq!(std::fs::read(&session).unwrap(), b"replacement");
        // The steps before it ran; the ones after it did not.
        assert_eq!(tree.names(&["openings-v1"]), Vec::<String>::new());
        assert_eq!(tree.names(&["endpoints-v1"]), Vec::<String>::new());
        assert_eq!(tree.names(&["packages", CREDENTIAL]), PACKAGE);
        assert_eq!(
            tree.names(&["admissions"]),
            [format!("{CREDENTIAL}.retiring")]
        );
        assert_eq!(
            tree.names(&["admissions", &format!("{CREDENTIAL}.retiring")]),
            ["record.json"]
        );
    }

    #[test]
    fn an_interrupted_retirement_completes_from_the_tombstone() {
        let tree = tree();
        let tombstone = format!("{CREDENTIAL}.retiring");
        std::fs::rename(
            tree.path(&["admissions", CREDENTIAL]),
            tree.path(&["admissions", &tombstone]),
        )
        .unwrap();
        for collection in ["openings-v1", "endpoints-v1"] {
            std::fs::remove_file(tree.path(&[collection, CREDENTIAL])).unwrap();
        }
        // A crash inside the key removal left it under its aside name.
        let aside = ".removing-0123456789abcdef0123456789abcdef";
        std::fs::rename(
            tree.path(&["packages", CREDENTIAL, "key.openssh"]),
            tree.path(&["packages", CREDENTIAL, aside]),
        )
        .unwrap();
        let plan = tree
            .plan(false, &["record.json"], Some(&[aside, "manifest.json"]))
            .unwrap();
        let present = plan.presence();
        assert_eq!(remaining_steps(present), [Record, Package, Tombstone]);
        plan.run(&remaining_steps(present), &skipped_steps(present))
            .unwrap();
        assert!(tree.empty());
    }

    #[test]
    fn an_empty_tombstone_is_the_last_step() {
        let tree = tree();
        let tombstone = format!("{CREDENTIAL}.retiring");
        std::fs::remove_file(tree.path(&["admissions", CREDENTIAL, "record.json"])).unwrap();
        std::fs::rename(
            tree.path(&["admissions", CREDENTIAL]),
            tree.path(&["admissions", &tombstone]),
        )
        .unwrap();
        for collection in ["openings-v1", "endpoints-v1", "sessions-v1"] {
            std::fs::remove_file(tree.path(&[collection, CREDENTIAL])).unwrap();
        }
        for name in PACKAGE {
            std::fs::remove_file(tree.path(&["packages", CREDENTIAL, name])).unwrap();
        }
        std::fs::remove_dir(tree.path(&["packages", CREDENTIAL])).unwrap();
        let plan = tree.plan(false, &[], None).unwrap();
        let present = plan.presence();
        assert!(bare_tombstone_allows(present));
        assert_eq!(remaining_steps(present), [Tombstone]);
        plan.run(&remaining_steps(present), &skipped_steps(present))
            .unwrap();
        assert!(tree.empty());
    }

    #[test]
    fn an_unexpected_package_member_keeps_the_package_in_place() {
        let tree = tree();
        let plan = tree.plan(true, &["record.json"], Some(PACKAGE)).unwrap();
        std::fs::write(tree.path(&["packages", CREDENTIAL, "unexpected"]), b"u").unwrap();
        assert_eq!(
            plan.run(&remaining_steps(plan.presence()), &[]),
            Err(Failure::Incomplete)
        );
        assert_eq!(tree.names(&["packages"]), [CREDENTIAL]);
        assert_eq!(tree.names(&["packages", CREDENTIAL]), ["unexpected"]);
    }
}
