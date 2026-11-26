//! `retire-linux`: removes a credential's stored custody state from this
//! account. Before anything changes it checks, in the contract's order, a
//! trusted foreground terminal, the joint admission or the tombstone of an
//! earlier attempt, that no session is open, the credential reservation and
//! the objects under the permit, endpoint and session record names, and then
//! asks the operator to type the credential ID. Any refusal changes nothing.

use crate::attended_terminal::{AttendedUnlockTerminal, Terminal};
use crate::linux_creation_record::Record;
use crate::linux_current_admission::CurrentAdmission;
use crate::linux_resident_reader::tombstone::CandidateTombstone;
use crate::linux_resident_reader::RetirementObjects;
use crate::linux_retained_removal::{entry_identity, EntryIdentity};
use crate::linux_session_opening::RefusalHint;
use crate::linux_session_record::{
    observe_evidence, read_checked, ReopeningEvidence, SessionRecords, StoredSessionRecord,
};
use crate::linux_supervisor_start::cancellation::{cancellation_pending, Cancellation};
use sha2::{Digest, Sha256};
use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

#[path = "linux_custody_retirement/steps.rs"]
mod steps;

use steps::{bare_tombstone_allows, hold, remaining_steps, skipped_steps, Entry, Failure, Plan};

/// The operator types the credential ID within this budget of the prompt.
const CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(60);

/// The reminder is written once, and a terminal that cannot take it within
/// this budget does not hold back the outcome.
const REMINDER_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) const RETIREMENT_NOTICE: &str = "Credential {credential} on device {device} will be retired. Its encrypted key will be deleted and its unlock secret will open nothing afterwards. The team must revoke this credential separately.\nType the credential ID to confirm: ";

/// Shown when only an empty tombstone is left, and its record with the
/// device ID is already gone.
pub(super) const BARE_TOMBSTONE_NOTICE: &str = "The retirement of credential {credential} is finishing. Its encrypted key is already deleted. The team must revoke this credential separately.\nType the credential ID to confirm: ";

pub(super) const RETIREMENT_REMINDER: &str = "Revoke credential {credential} in the team.\n";

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
/// regular file for the permit and the session record, a private
/// single-link socket for the endpoint, as the keeper binds it, owned by
/// `ids` either way.
pub(super) fn entry_allowed(
    name: EntryName,
    identity: Option<&EntryIdentity>,
    ids: (u32, u32),
) -> bool {
    let Some(identity) = identity else {
        return true;
    };
    let kind = match name {
        EntryName::Permit | EntryName::Record => libc::S_IFREG,
        EntryName::Endpoint => libc::S_IFSOCK,
    };
    (identity.uid, identity.gid) == ids
        && identity.links == 1
        && identity.mode & libc::S_IFMT == kind
        && identity.mode & 0o7777 == 0o600
}

/// The session record whose session was found closed must be the object
/// still under the record name once the reservation is held; it is the
/// object step 4 removes.
pub(super) fn session_bound(
    checked: Option<&EntryIdentity>,
    inspected: Option<&EntryIdentity>,
) -> bool {
    checked == inspected
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

/// The confirmation notice: the device comes from the record, and a
/// tombstone without one gets the notice of the last step.
fn notice(credential: &str, device: Option<&str>) -> String {
    match device {
        Some(device) => RETIREMENT_NOTICE
            .replace("{credential}", credential)
            .replace("{device}", device),
        None => BARE_TOMBSTONE_NOTICE.replace("{credential}", credential),
    }
}

fn process_ids() -> (u32, u32) {
    // SAFETY: these calls only query this process's credentials.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

/// The collection and the object under the credential's name in it.
fn inspect(name: &CStr, directory: OwnedFd) -> io::Result<Entry> {
    let identity = entry_identity(&directory, name)?;
    Ok(Entry {
        directory,
        name: name.to_owned(),
        identity,
    })
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

    fn validate(&self) -> io::Result<()> {
        match self {
            Self::Admission(admission) => admission.validate(),
            Self::Tombstone(tombstone) => tombstone.validate(),
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

    /// The admission record, or `None` for a tombstone that no longer has it.
    fn record(&self) -> io::Result<Option<Record>> {
        match self {
            Self::Admission(admission) => Record::parse(&admission.canonical_record()?).map(Some),
            Self::Tombstone(tombstone) => Ok(tombstone.record().cloned()),
        }
    }

    /// The session record, which must belong to this admission. Without the
    /// admission record nothing can be matched, so any entry is refused.
    fn session_record(&self) -> io::Result<Option<StoredSessionRecord>> {
        match self {
            Self::Admission(admission) => SessionRecords::new(admission).read(),
            Self::Tombstone(tombstone) => {
                let Some(record) = tombstone.record() else {
                    let entry = tombstone.with_session_location(inspect)?;
                    return match entry.identity {
                        None => Ok(None),
                        Some(_) => Err(io::ErrorKind::InvalidData.into()),
                    };
                };
                let digest: [u8; 32] = Sha256::digest(record.canonical_bytes()?).into();
                tombstone.with_session_location(|name, directory| {
                    let credential = name.to_str().map_err(|_| io::ErrorKind::InvalidData)?;
                    read_checked(&directory, name, process_ids(), credential, &digest)
                })
            }
        }
    }

    /// The retained admission or tombstone and package objects.
    fn objects(&self) -> io::Result<RetirementObjects> {
        match self {
            Self::Admission(admission) => admission.retirement_objects(),
            Self::Tombstone(tombstone) => tombstone.retirement_objects(),
        }
    }

    /// The objects under the permit, endpoint and session record names.
    fn entries(&self) -> io::Result<[Entry; 3]> {
        Ok(match self {
            Self::Admission(admission) => [
                admission.with_opening_location(inspect)?,
                admission.with_endpoint_location(inspect)?,
                admission.with_session_location(inspect)?,
            ],
            Self::Tombstone(tombstone) => [
                tombstone.with_opening_location(inspect)?,
                tombstone.with_endpoint_location(inspect)?,
                tombstone.with_session_location(inspect)?,
            ],
        })
    }
}

/// Retires `credential`, already validated as a canonical ID.
pub(crate) fn retire(credential: &str) -> RetireOutcome {
    let refused = RetireOutcome::Refused(None);
    // Held until the command returns: before the commit a pending signal
    // refuses, and after it signals stay blocked and are never checked.
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
    let Ok(stored) = start.session_record() else {
        return refused;
    };
    let evidence = match stored
        .as_ref()
        .map(|stored| observe_evidence(&stored.record))
        .transpose()
    {
        Ok(evidence) => evidence.flatten(),
        Err(_) => return refused,
    };
    if let Err(hint) = session_allows(start.kind(), stored.is_some(), evidence) {
        return RetireOutcome::Refused(hint);
    }
    // The admission lock and the credential reservation are held from here
    // through the last step, and released on every refusal by dropping them.
    let Ok(objects) = start.objects() else {
        return refused;
    };
    let package = objects.package.as_ref().map(|(directory, _)| directory);
    let Ok(locks) = hold(&objects.admission, package) else {
        return refused;
    };
    if start.validate().is_err() {
        return refused;
    }
    let ids = process_ids();
    let Ok(entries) = start.entries() else {
        return refused;
    };
    let allowed = [EntryName::Permit, EntryName::Endpoint, EntryName::Record]
        .iter()
        .zip(&entries)
        .all(|(name, entry)| entry_allowed(*name, entry.identity.as_ref(), ids));
    if !allowed
        || !session_bound(
            stored.as_ref().map(|stored| &stored.identity),
            entries[2].identity.as_ref(),
        )
    {
        return refused;
    }
    let Ok(record) = start.record() else {
        return refused;
    };
    let Ok(plan) = Plan::new(
        credential,
        start.kind() == StartKind::Admission,
        objects,
        entries,
    ) else {
        return refused;
    };
    let present = plan.presence();
    if record.is_none() && !bare_tombstone_allows(present) {
        return refused;
    }
    let steps = remaining_steps(present);
    if steps.is_empty() {
        return refused;
    }
    let Ok(reminder) = duplicate(&terminal) else {
        return refused;
    };
    let device = record.as_ref().map(|record| record.device_id.as_str());
    if !confirm(terminal, credential, device) {
        return refused;
    }
    // Validate the snapshots and the retained objects, then commit unless
    // cancellation arrived first.
    if start.validate().is_err()
        || locks.validate().is_err()
        || plan.check_named().is_err()
        || !matches!(cancellation_pending(), Ok(false))
    {
        return refused;
    }
    match plan.run(&steps, &skipped_steps(present)) {
        Err(Failure::Unchanged) => refused,
        Err(Failure::Incomplete) => RetireOutcome::RetirementIncomplete,
        Ok(()) => {
            drop(locks);
            write_reminder(&reminder, credential);
            RetireOutcome::Retired
        }
    }
}

fn duplicate(terminal: &AttendedUnlockTerminal) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates the live terminal descriptor atomically.
    let raw = unsafe { libc::fcntl(terminal.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fcntl returned a fresh owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// Writes the team revocation reminder to the terminal once. The state is
/// already gone, so a terminal that was lost or does not take it changes
/// nothing about the outcome.
fn write_reminder(terminal: &OwnedFd, credential: &str) {
    let text = RETIREMENT_REMINDER.replace("{credential}", credential);
    let bytes = text.as_bytes();
    let deadline = Instant::now() + REMINDER_TIMEOUT;
    let mut position = 0;
    while position < bytes.len() {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        let mut descriptor = libc::pollfd {
            fd: terminal.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let timeout = remaining.as_millis().clamp(1, 2000) as i32;
        // SAFETY: descriptor is one writable pollfd for this live terminal.
        if unsafe { libc::poll(&mut descriptor, 1, timeout) } <= 0
            || descriptor.revents & libc::POLLOUT == 0
        {
            return;
        }
        // SAFETY: the remaining slice stays live for the write.
        let count = unsafe {
            libc::write(
                terminal.as_raw_fd(),
                bytes[position..].as_ptr().cast(),
                bytes.len() - position,
            )
        };
        if count > 0 {
            position += count as usize;
        } else if count == 0 || io::Error::last_os_error().kind() != io::ErrorKind::WouldBlock {
            return;
        }
    }
}

/// Shows the notice on the trusted terminal, still the foreground one, and
/// reads the typed line within the confirmation budget, echoed: the ID is not
/// a secret and the operator sees what they confirm. End of input, a signal,
/// a lost terminal or the deadline refuse.
fn confirm(terminal: AttendedUnlockTerminal, credential: &str, device: Option<&str>) -> bool {
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
        let socket = entry(libc::S_IFSOCK | 0o600, 1, ids);
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
            entry(libc::S_IFSOCK | 0o600, 2, ids),
            entry(libc::S_IFSOCK | 0o600, 1, (1001, 1000)),
            entry(libc::S_IFSOCK | 0o600, 1, (1000, 1001)),
            // Not the mode the keeper binds it with.
            entry(libc::S_IFSOCK | 0o755, 1, ids),
            entry(libc::S_IFSOCK | 0o700, 1, ids),
            entry(libc::S_IFSOCK | 0o666, 1, ids),
            entry(libc::S_IFSOCK | 0o4600, 1, ids),
        ] {
            assert!(
                !entry_allowed(Endpoint, Some(&unexpected), ids),
                "{unexpected:?}"
            );
        }
    }

    #[test]
    fn the_session_record_removed_is_the_one_found_closed() {
        let ids = (1000, 1000);
        let record = entry(libc::S_IFREG | 0o600, 1, ids);
        let other = EntryIdentity { inode: 3, ..record };
        assert!(session_bound(None, None));
        assert!(session_bound(Some(&record), Some(&record)));
        assert!(!session_bound(Some(&record), Some(&other)));
        assert!(!session_bound(Some(&record), None));
        assert!(!session_bound(None, Some(&record)));
    }

    #[test]
    fn retirement_notice_names_both_ids_and_team_revocation() {
        assert_eq!(
            notice(CREDENTIAL, Some(DEVICE)),
            "Credential 0123456789abcdefghjkmnpqrs on device 0123456789abcdefghjkmnpqrt will be \
             retired. Its encrypted key will be deleted and its unlock secret will open nothing \
             afterwards. The team must revoke this credential separately.\n\
             Type the credential ID to confirm: "
        );
        assert_eq!(
            notice(CREDENTIAL, None),
            "The retirement of credential 0123456789abcdefghjkmnpqrs is finishing. Its encrypted \
             key is already deleted. The team must revoke this credential separately.\n\
             Type the credential ID to confirm: "
        );
        assert_eq!(
            RETIREMENT_REMINDER.replace("{credential}", CREDENTIAL),
            "Revoke credential 0123456789abcdefghjkmnpqrs in the team.\n"
        );
    }
}
