//! `retire-linux`: removes a credential's stored custody state from this
//! account. Before anything changes it checks, in the contract's order, a
//! trusted foreground terminal, the joint admission or the tombstone of an
//! earlier attempt, that no session is open, the credential reservation and
//! the objects under the permit, endpoint and session record names, and then
//! asks the operator to type the credential ID. Any refusal changes nothing.

use crate::attended_terminal::{AttendedUnlockTerminal, Terminal};
use crate::linux_creation_record::Record;
use crate::linux_current_admission::CurrentAdmission;
use crate::linux_reservation::StoreReservation;
use crate::linux_resident_reader::tombstone::CandidateTombstone;
use crate::linux_retained_removal::{entry_identity, EntryIdentity};
use crate::linux_session_opening::RefusalHint;
use crate::linux_session_record::{
    observe_evidence, read_checked, ReopeningEvidence, SessionRecord, SessionRecords,
};
use crate::linux_supervisor_start::cancellation::Cancellation;
use sha2::{Digest, Sha256};
use std::ffi::CStr;
use std::io;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

/// The operator types the credential ID within this budget of the prompt.
const CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) const RETIREMENT_NOTICE: &str = "Credential {credential} on device {device} will be retired. Its encrypted key will be deleted and its unlock secret will open nothing afterwards. The team must revoke this credential separately.\nType the credential ID to confirm: ";

#[allow(
    dead_code,
    reason = "The retirement steps print it once the last one succeeds."
)]
pub(super) const RETIREMENT_REMINDER: &str = "Revoke credential {credential} in the team.\n";

#[allow(
    dead_code,
    reason = "Only the retirement steps, which follow the confirmation, report these."
)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RetireOutcome {
    Retired,
    Refused(Option<RefusalHint>),
    RetirementIncomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StartKind {
    Admission,
    Tombstone,
}

/// Where a retirement starts: the current joint admission, or the tombstone
/// an interrupted retirement left after its commit.
pub(super) enum RetirementStart {
    Admission(CurrentAdmission),
    Tombstone(CandidateTombstone),
}

/// Both an admission and a tombstone for one ID is a state no retirement
/// produces, and neither is touched; with neither there is nothing to retire.
pub(super) fn select_start(admission: bool, tombstone: bool) -> Option<StartKind> {
    match (admission, tombstone) {
        (true, false) => Some(StartKind::Admission),
        (false, true) => Some(StartKind::Tombstone),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EntryName {
    Permit,
    Endpoint,
    Record,
}

/// No entry, or the object retirement would remove: a private single-link
/// regular file for the permit and the session record, a single-link socket
/// for the endpoint, owned by `ids` either way.
pub(super) fn entry_allowed(
    name: EntryName,
    identity: Option<&EntryIdentity>,
    ids: (u32, u32),
) -> bool {
    let Some(identity) = identity else {
        return true;
    };
    let kind = identity.mode & libc::S_IFMT;
    (identity.uid, identity.gid) == ids
        && identity.links == 1
        && match name {
            EntryName::Permit | EntryName::Record => {
                kind == libc::S_IFREG && identity.mode & 0o7777 == 0o600
            }
            EntryName::Endpoint => kind == libc::S_IFSOCK,
        }
}

/// Every collection a later step synchronizes must exist; a missing one is
/// refused, not read as an absent entry. The endpoint collection and the
/// session collection each have a provisioning command, named in the remedy. Without
/// the opening collection the installation is broken and nothing names a fix.
pub(super) fn collections_allowed(
    openings: bool,
    endpoints: bool,
    sessions: bool,
) -> Result<(), Option<RefusalHint>> {
    if !openings {
        Err(None)
    } else if !endpoints {
        Err(Some(RefusalHint::ProvisionEndpoints))
    } else if !sessions {
        Err(Some(RefusalHint::ProvisionSessions))
    } else {
        Ok(())
    }
}

/// No session record, or the kernel's evidence that the recorded session is
/// gone. Otherwise the session is open: from an admission, `close-linux`
/// ends it; from a tombstone nothing can open or close it, so no remedy is
/// named.
pub(super) fn session_allows(
    start: StartKind,
    recorded: bool,
    evidence: Option<ReopeningEvidence>,
) -> Result<(), Option<RefusalHint>> {
    match (recorded, evidence, start) {
        (false, _, _) | (true, Some(_), _) => Ok(()),
        (true, None, StartKind::Admission) => Err(Some(RefusalHint::CloseFirst)),
        (true, None, StartKind::Tombstone) => Err(None),
    }
}

/// The typed line is the credential ID and its newline, nothing else.
pub(super) fn confirmation_matches(line: &[u8], credential: &str) -> bool {
    line.strip_suffix(b"\n") == Some(credential.as_bytes())
}

fn notice(credential: &str, device: &str) -> String {
    RETIREMENT_NOTICE
        .replace("{credential}", credential)
        .replace("{device}", device)
}

fn process_ids() -> (u32, u32) {
    // SAFETY: these calls only query this process's credentials.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

fn inspect(name: &CStr, directory: OwnedFd) -> io::Result<Option<EntryIdentity>> {
    entry_identity(&directory, name)
}

impl RetirementStart {
    /// A tombstone alone starts from it; otherwise only a joint admission
    /// that opens whole does. Any failure to read either is a refusal.
    fn open(credential: &str) -> Option<Self> {
        let tombstone = CandidateTombstone::open_fixed_namespace(credential).ok()?;
        let (admission, admission_present) = match &tombstone {
            Some(tombstone) => (None, tombstone.admission_present().ok()?),
            None => {
                let admission = CurrentAdmission::open_fixed_namespace(credential).ok();
                let present = admission.is_some();
                (admission, present)
            }
        };
        match select_start(admission_present, tombstone.is_some())? {
            StartKind::Admission => admission.map(Self::Admission),
            StartKind::Tombstone => tombstone.map(Self::Tombstone),
        }
    }

    fn kind(&self) -> StartKind {
        match self {
            Self::Admission(_) => StartKind::Admission,
            Self::Tombstone(_) => StartKind::Tombstone,
        }
    }

    /// Whether the opening, endpoint and session collection each exist.
    fn collections(&self) -> io::Result<(bool, bool, bool)> {
        Ok(match self {
            Self::Admission(admission) => (
                admission.has_opening_directory()?,
                admission.has_endpoint_directory()?,
                admission.has_session_directory()?,
            ),
            Self::Tombstone(tombstone) => (
                tombstone.has_opening_directory()?,
                tombstone.has_endpoint_directory()?,
                tombstone.has_session_directory()?,
            ),
        })
    }

    fn record(&self) -> io::Result<Record> {
        match self {
            Self::Admission(admission) => Record::parse(&admission.canonical_record()?),
            Self::Tombstone(tombstone) => Ok(tombstone.record().clone()),
        }
    }

    /// The session record, which must belong to this admission.
    fn session_record(&self) -> io::Result<Option<SessionRecord>> {
        let stored = match self {
            Self::Admission(admission) => SessionRecords::new(admission).read()?,
            Self::Tombstone(tombstone) => {
                let digest: [u8; 32] = Sha256::digest(tombstone.record().canonical_bytes()?).into();
                tombstone.with_session_location(|name, directory| {
                    let credential = name.to_str().map_err(|_| io::ErrorKind::InvalidData)?;
                    read_checked(&directory, name, process_ids(), credential, &digest)
                })?
            }
        };
        Ok(stored.map(|stored| stored.record))
    }

    /// The exclusive credential reservation on the package directory, which
    /// the admission or tombstone already checked for owner, mode and
    /// identity. A tombstone whose package is gone has nothing left to
    /// reserve.
    fn reserve(&self) -> io::Result<Option<StoreReservation>> {
        match self {
            Self::Admission(admission) => admission.reserve_package_directory().map(Some),
            Self::Tombstone(tombstone) => {
                let reservation = tombstone
                    .package_directory()?
                    .map(|package| StoreReservation::acquire_directory(&package))
                    .transpose()?;
                tombstone.validate()?;
                Ok(reservation)
            }
        }
    }

    /// The objects under the permit, endpoint and session record names.
    fn entries(&self) -> io::Result<[(EntryName, Option<EntryIdentity>); 3]> {
        let (permit, endpoint, record) = match self {
            Self::Admission(admission) => (
                admission.with_opening_location(inspect)?,
                admission.with_endpoint_location(inspect)?,
                admission.with_session_location(inspect)?,
            ),
            Self::Tombstone(tombstone) => (
                tombstone.with_opening_location(inspect)?,
                tombstone.with_endpoint_location(inspect)?,
                tombstone.with_session_location(inspect)?,
            ),
        };
        Ok([
            (EntryName::Permit, permit),
            (EntryName::Endpoint, endpoint),
            (EntryName::Record, record),
        ])
    }
}

/// Retires `credential`, already validated as a canonical ID.
pub(crate) fn retire(credential: &str) -> RetireOutcome {
    let refused = RetireOutcome::Refused(None);
    let Ok(_cancellation) = Cancellation::block() else {
        return refused;
    };
    let Ok(terminal) = AttendedUnlockTerminal::open() else {
        return refused;
    };
    if terminal.validate_foreground_hold().is_err() {
        return refused;
    }
    let Some(start) = RetirementStart::open(credential) else {
        return refused;
    };
    let Ok((openings, endpoints, sessions)) = start.collections() else {
        return refused;
    };
    if let Err(hint) = collections_allowed(openings, endpoints, sessions) {
        return RetireOutcome::Refused(hint);
    }
    let Ok(record) = start.session_record() else {
        return refused;
    };
    let evidence = match record.as_ref().map(observe_evidence).transpose() {
        Ok(evidence) => evidence.flatten(),
        Err(_) => return refused,
    };
    if let Err(hint) = session_allows(start.kind(), record.is_some(), evidence) {
        return RetireOutcome::Refused(hint);
    }
    // Held from here through the confirmation, and released on every
    // refusal by dropping it.
    let Ok(_reservation) = start.reserve() else {
        return refused;
    };
    let ids = process_ids();
    let Ok(entries) = start.entries() else {
        return refused;
    };
    if !entries
        .iter()
        .all(|(name, identity)| entry_allowed(*name, identity.as_ref(), ids))
    {
        return refused;
    }
    let Ok(record) = start.record() else {
        return refused;
    };
    if !confirm(terminal, credential, &record.device_id) {
        return refused;
    }
    // The retirement steps are not part of this command yet, so a confirmed
    // retirement still changes nothing.
    refused
}

/// Shows the notice on the trusted terminal, still the foreground one, and
/// reads the typed line within the confirmation budget, echoed: the ID is not
/// a secret and the operator sees what they confirm. End of input, a signal,
/// a lost terminal or the deadline refuse.
fn confirm(terminal: AttendedUnlockTerminal, credential: &str, device: &str) -> bool {
    if terminal.validate_foreground_hold().is_err() {
        return false;
    }
    let deadline = Instant::now() + CONFIRMATION_TIMEOUT;
    let Ok(mut terminal) = Terminal::from_retained(terminal, true) else {
        return false;
    };
    let typed = terminal.read_line(notice(credential, device).as_bytes(), deadline);
    let restored = terminal.finish().is_ok();
    // `read_line` accepts only a line that ended in its single newline, and
    // returns it without that newline.
    restored
        && Instant::now() < deadline
        && typed.is_ok_and(|typed| confirmation_matches(&[&typed[..], b"\n"].concat(), credential))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDENTIAL: &str = "0123456789abcdefghjkmnpqrs";
    const DEVICE: &str = "0123456789abcdefghjkmnpqrt";

    #[test]
    fn retirement_confirmation_requires_the_exact_credential_line() {
        assert!(confirmation_matches(
            format!("{CREDENTIAL}\n").as_bytes(),
            CREDENTIAL
        ));
        for line in [
            CREDENTIAL.to_owned(),
            format!("{CREDENTIAL} \n"),
            format!(" {CREDENTIAL}\n"),
            format!("{CREDENTIAL}\n\n"),
            format!("{CREDENTIAL}\r\n"),
            format!("{}\n", CREDENTIAL.to_ascii_uppercase()),
            format!("{DEVICE}\n"),
            "\n".to_owned(),
            String::new(),
        ] {
            assert!(
                !confirmation_matches(line.as_bytes(), CREDENTIAL),
                "{line:?}"
            );
        }
    }

    #[test]
    fn a_missing_collection_refuses_and_names_the_command_to_provision_it() {
        assert_eq!(collections_allowed(true, true, true), Ok(()));
        // Without the opening collection the installation is broken: no remedy,
        // whatever else is missing.
        for (endpoints, sessions) in [(true, true), (false, true), (true, false), (false, false)] {
            assert_eq!(collections_allowed(false, endpoints, sessions), Err(None));
        }
        for sessions in [true, false] {
            assert_eq!(
                collections_allowed(true, false, sessions),
                Err(Some(RefusalHint::ProvisionEndpoints))
            );
        }
        assert_eq!(
            collections_allowed(true, true, false),
            Err(Some(RefusalHint::ProvisionSessions))
        );
        assert_eq!(
            RefusalHint::ProvisionEndpoints.remedy(CREDENTIAL),
            "Run vivac device-key provision-linux-endpoints, then retry."
        );
        assert_eq!(
            RefusalHint::ProvisionSessions.remedy(CREDENTIAL),
            "Run vivac device-key provision-linux-sessions, then retry."
        );
    }

    #[test]
    fn an_open_session_names_close_linux_only_from_an_admission() {
        use ReopeningEvidence::{CgroupEmpty, CgroupGone, NewBoot};
        for start in [StartKind::Admission, StartKind::Tombstone] {
            assert_eq!(session_allows(start, false, None), Ok(()));
            for evidence in [NewBoot, CgroupGone, CgroupEmpty] {
                assert_eq!(session_allows(start, true, Some(evidence)), Ok(()));
            }
        }
        assert_eq!(
            session_allows(StartKind::Admission, true, None),
            Err(Some(RefusalHint::CloseFirst))
        );
        // A tombstone has no admission for `close-linux` to open.
        assert_eq!(session_allows(StartKind::Tombstone, true, None), Err(None));
    }

    #[test]
    fn retirement_refuses_when_both_admission_and_tombstone_exist() {
        assert_eq!(select_start(true, true), None);
        assert_eq!(select_start(false, false), None);
        assert_eq!(select_start(true, false), Some(StartKind::Admission));
        assert_eq!(select_start(false, true), Some(StartKind::Tombstone));
    }

    fn entry(mode: u32, links: u64, owner: (u32, u32)) -> EntryIdentity {
        EntryIdentity {
            device: 1,
            inode: 2,
            mode,
            uid: owner.0,
            gid: owner.1,
            links,
        }
    }

    #[test]
    fn retirement_entry_policy_refuses_unexpected_objects() {
        use EntryName::{Endpoint, Permit, Record};
        let ids = (1000, 1000);
        let file = entry(libc::S_IFREG | 0o600, 1, ids);
        let socket = entry(libc::S_IFSOCK | 0o755, 1, ids);
        for name in [Permit, Endpoint, Record] {
            assert!(entry_allowed(name, None, ids), "{name:?} absent");
        }
        assert!(entry_allowed(Permit, Some(&file), ids));
        assert!(entry_allowed(Record, Some(&file), ids));
        assert!(entry_allowed(Endpoint, Some(&socket), ids));
        assert!(!entry_allowed(Endpoint, Some(&file), ids));
        for name in [Permit, Record] {
            assert!(!entry_allowed(name, Some(&socket), ids), "{name:?} socket");
        }
        for unexpected in [
            entry(libc::S_IFDIR | 0o700, 1, ids),
            entry(libc::S_IFLNK | 0o777, 1, ids),
            entry(libc::S_IFREG | 0o644, 1, ids),
            entry(libc::S_IFREG | 0o600, 2, ids),
            entry(libc::S_IFREG | 0o600, 1, (1001, 1000)),
            entry(libc::S_IFREG | 0o600, 1, (1000, 1001)),
        ] {
            for name in [Permit, Record] {
                assert!(
                    !entry_allowed(name, Some(&unexpected), ids),
                    "{name:?} {unexpected:?}"
                );
            }
        }
        for unexpected in [
            entry(libc::S_IFDIR | 0o700, 1, ids),
            entry(libc::S_IFLNK | 0o777, 1, ids),
            entry(libc::S_IFSOCK | 0o755, 2, ids),
            entry(libc::S_IFSOCK | 0o755, 1, (1001, 1000)),
            entry(libc::S_IFSOCK | 0o755, 1, (1000, 1001)),
        ] {
            assert!(
                !entry_allowed(Endpoint, Some(&unexpected), ids),
                "{unexpected:?}"
            );
        }
    }

    #[test]
    fn retirement_notice_names_both_ids_and_team_revocation() {
        assert_eq!(
            notice(CREDENTIAL, DEVICE),
            "Credential 0123456789abcdefghjkmnpqrs on device 0123456789abcdefghjkmnpqrt will be \
             retired. Its encrypted key will be deleted and its unlock secret will open nothing \
             afterwards. The team must revoke this credential separately.\n\
             Type the credential ID to confirm: "
        );
        assert_eq!(
            RETIREMENT_REMINDER.replace("{credential}", CREDENTIAL),
            "Revoke credential 0123456789abcdefghjkmnpqrs in the team.\n"
        );
    }
}
