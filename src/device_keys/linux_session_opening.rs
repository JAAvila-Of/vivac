//! `open-linux`: checks what it can without consuming anything, starts the
//! session unit, authenticates its keeper, hands it the unlock secret read from
//! the trusted terminal and returns the terminal once the keeper reports the
//! session open. The session then lives in the unit, not in this process.

use crate::attended_terminal::AttendedUnlockTerminal;
use crate::linux_current_admission::CurrentAdmission;
use crate::linux_session_control::{
    ClientFrame, ControlConnection, KeeperFrame, KeeperOutcome, CONTROL_SOCKET,
};
use crate::linux_session_record::{
    observe_evidence, ReopeningEvidence, SessionRecord, SessionRecords,
};
use crate::linux_supervisor_start::cancellation::{cancellation_pending, Cancellation};
use crate::linux_user_manager::{
    Account, KeeperInvocation, ManagerRefusal, SessionLimits, SessionUnit, UserManager,
};
use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// The connection to the keeper, and each send, fit in this budget.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// The keeper asks for the secret, or reports the session open, within this
/// budget of the previous step.
const KEEPER_TIMEOUT: Duration = Duration::from_secs(60);
/// After `Cancel`, the keeper ends the session cgroup and publishes within
/// its 25-second close budget; this leaves room for the report.
const OUTCOME_TIMEOUT: Duration = Duration::from_secs(40);
/// How often a wait for the keeper looks for a pending cancellation.
const CANCELLATION_INTERVAL: Duration = Duration::from_millis(20);

/// A remedy a refusal names on stderr, whether or not the receipt is JSON.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RefusalHint {
    EnableLinger { account: String },
    CloseFirst,
}

impl RefusalHint {
    pub(crate) fn remedy(&self, credential: &str) -> String {
        match self {
            Self::EnableLinger { account } => {
                format!("Run loginctl enable-linger {account} as an administrator, then retry.")
            }
            Self::CloseFirst => format!("Run vivac device-key close-linux {credential} first."),
        }
    }

    /// Names the account only when its name prints as itself.
    fn enable_linger(account: &Account) -> Option<Self> {
        let name = account.name.to_str().ok()?;
        (!name.is_empty() && name.bytes().all(|byte| byte.is_ascii_graphic())).then(|| {
            Self::EnableLinger {
                account: name.to_owned(),
            }
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OpeningOutcome {
    Open,
    Reopened,
    Refused(Option<RefusalHint>),
    OpeningUncertain,
    Interrupted,
    PublicationUncertain,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Preflight {
    Open,
    Reopen,
    Refuse(Option<RefusalHint>),
}

/// What the stored state allows before anything is consumed. A permit opens,
/// and the keeper retires a recorded session on the way. Without one, only
/// the kernel's evidence that the recorded session is gone reopens; a record
/// without that evidence names `close-linux`, which produces it.
pub(super) fn preflight(
    record: Option<&SessionRecord>,
    evidence: Option<ReopeningEvidence>,
    permit_present: bool,
) -> Preflight {
    match (record, evidence, permit_present) {
        (None, _, true) | (Some(_), Some(_), true) => Preflight::Open,
        (None, _, false) => Preflight::Refuse(None),
        (Some(_), Some(_), false) => Preflight::Reopen,
        (Some(_), None, _) => Preflight::Refuse(Some(RefusalHint::CloseFirst)),
    }
}

/// The status once `Begin` was sent: only what the keeper reported. Without
/// a report, the unit could have consumed the permit.
pub(super) fn outcome_after_begin(frame: Option<KeeperFrame>) -> OpeningOutcome {
    match frame {
        Some(KeeperFrame::Opened { reopened: false }) => OpeningOutcome::Open,
        Some(KeeperFrame::Opened { reopened: true }) => OpeningOutcome::Reopened,
        Some(KeeperFrame::Outcome(KeeperOutcome::Refused)) => OpeningOutcome::Refused(None),
        Some(KeeperFrame::Outcome(KeeperOutcome::Interrupted)) => OpeningOutcome::Interrupted,
        Some(KeeperFrame::Outcome(KeeperOutcome::PublicationUncertain)) => {
            OpeningOutcome::PublicationUncertain
        }
        None
        | Some(
            KeeperFrame::SecretRequest
            | KeeperFrame::Closing
            | KeeperFrame::Outcome(KeeperOutcome::Closed),
        ) => OpeningOutcome::OpeningUncertain,
    }
}

fn refused() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}

/// Opens the session for `credential` with `limits`, already validated.
pub(crate) fn open(credential: &str, limits: SessionLimits) -> OpeningOutcome {
    let Ok(_cancellation) = Cancellation::block() else {
        return OpeningOutcome::Refused(None);
    };
    let Ok(terminal) = AttendedUnlockTerminal::open() else {
        return OpeningOutcome::Refused(None);
    };
    if terminal.validate_foreground_hold().is_err() {
        return OpeningOutcome::Refused(None);
    }
    let manager = match check(credential) {
        Ok(manager) => manager,
        Err(hint) => return OpeningOutcome::Refused(hint),
    };
    let (Ok(unit), Ok(invocation)) = (
        SessionUnit::new(credential),
        KeeperInvocation::current(limits),
    ) else {
        return OpeningOutcome::Refused(None);
    };
    match start(credential, &manager, &unit, &invocation) {
        Ok(connection) => after_begin(&connection, terminal),
        Err(_) => {
            // The unit may be running even when its start failed or timed
            // out. Its keeper has not seen `Begin`, so stopping it consumes
            // nothing.
            let _ = manager.stop_unit(&unit);
            OpeningOutcome::Refused(None)
        }
    }
}

/// Every check that consumes nothing: the admission, the session collection,
/// the user manager, and the permit or the evidence for a reopening.
fn check(credential: &str) -> Result<UserManager, Option<RefusalHint>> {
    let admission = CurrentAdmission::open_fixed_namespace(credential).map_err(|_| None)?;
    if !admission.has_session_directory().map_err(|_| None)? {
        return Err(None);
    }
    let manager = UserManager::open().map_err(|refusal| match refusal {
        ManagerRefusal::LingerDisabled(account) => RefusalHint::enable_linger(&account),
        ManagerRefusal::Unsupported => None,
    })?;
    // A record that fails validation, or belongs to another admission,
    // refuses here and is left in place.
    let stored = SessionRecords::new(&admission).read().map_err(|_| None)?;
    let evidence = match &stored {
        Some(stored) => observe_evidence(&stored.record).map_err(|_| None)?,
        None => None,
    };
    let permit_present = !admission.opening_permit_absent().map_err(|_| None)?;
    if permit_present {
        admission.observe_opening_permit().map_err(|_| None)?;
    }
    match preflight(
        stored.as_ref().map(|stored| &stored.record),
        evidence,
        permit_present,
    ) {
        Preflight::Open | Preflight::Reopen => Ok(manager),
        Preflight::Refuse(hint) => Err(hint),
    }
}

fn control_socket(manager: &UserManager, unit: &SessionUnit) -> PathBuf {
    manager
        .runtime_directory()
        .join(unit.runtime_name())
        .join(OsStr::from_bytes(CONTROL_SOCKET.to_bytes()))
}

/// Starts the unit, finds this attempt's record, authenticates the keeper
/// against the unit cgroup it recorded, and sends `Begin`. The keeper reports
/// ready only after its record is durable and its socket bound.
fn start(
    credential: &str,
    manager: &UserManager,
    unit: &SessionUnit,
    invocation: &KeeperInvocation,
) -> io::Result<ControlConnection> {
    if cancellation_pending()? {
        return Err(refused());
    }
    manager.start_session_unit(unit, invocation)?;
    let admission = CurrentAdmission::open_fixed_namespace(credential)?;
    let stored = SessionRecords::new(&admission)
        .read()?
        .ok_or_else(refused)?;
    if stored.record.generation != unit.generation() || stored.record.unit != unit.name() {
        return Err(refused());
    }
    let connection = ControlConnection::connect_keeper(
        &control_socket(manager, unit),
        &stored.record.unit_cgroup,
        Instant::now() + REQUEST_TIMEOUT,
    )?;
    if cancellation_pending()? {
        return Err(refused());
    }
    // A failed send queued nothing: the keeper never saw `Begin`.
    connection.send_client(&ClientFrame::Begin, Instant::now() + REQUEST_TIMEOUT)?;
    Ok(connection)
}

enum Wait {
    Frame(KeeperFrame),
    Pending,
    Failed,
}

/// One short wait for the keeper, so that a cancellation is seen soon.
fn next_frame(connection: &ControlConnection, deadline: Instant) -> Wait {
    let now = Instant::now();
    if now >= deadline {
        return Wait::Failed;
    }
    match connection.receive_keeper((now + CANCELLATION_INTERVAL).min(deadline)) {
        Ok(frame) => Wait::Frame(frame),
        Err(error) if error.kind() == io::ErrorKind::TimedOut => Wait::Pending,
        Err(_) => Wait::Failed,
    }
}

/// From `Begin` to the keeper's report. The terminal is used once, for the
/// secret the keeper asks for; any failure here asks the keeper to cancel.
fn after_begin(connection: &ControlConnection, terminal: AttendedUnlockTerminal) -> OpeningOutcome {
    let mut terminal = Some(terminal);
    let mut deadline = Instant::now() + KEEPER_TIMEOUT;
    loop {
        if cancellation_pending().unwrap_or(true) {
            return cancel(connection);
        }
        match next_frame(connection, deadline) {
            Wait::Pending => {}
            Wait::Failed => return cancel(connection),
            Wait::Frame(KeeperFrame::SecretRequest) => {
                let Some(terminal) = terminal.take() else {
                    return cancel(connection);
                };
                if deliver_secret(connection, terminal).is_err() {
                    return cancel(connection);
                }
                deadline = Instant::now() + KEEPER_TIMEOUT;
            }
            Wait::Frame(frame) => return outcome_after_begin(Some(frame)),
        }
    }
}

/// Reads the secret from the terminal, still the foreground one, and sends
/// it. The secret is written straight into the zeroizing frame.
fn deliver_secret(
    connection: &ControlConnection,
    terminal: AttendedUnlockTerminal,
) -> io::Result<()> {
    terminal.validate_foreground_hold()?;
    let input = terminal.read().map_err(|()| refused())?;
    let mut frame = ClientFrame::Secret(Zeroizing::new([0; 64]));
    if let ClientFrame::Secret(secret) = &mut frame {
        if input.as_bytes().len() != secret.len() {
            return Err(refused());
        }
        secret.copy_from_slice(input.as_bytes());
    }
    drop(input);
    connection.send_client(&frame, Instant::now() + REQUEST_TIMEOUT)
}

/// Asks the keeper to cancel and reports what it answers. An `Opened` sent
/// before the keeper saw `Cancel` stands: the keeper ignores a later cancel.
fn cancel(connection: &ControlConnection) -> OpeningOutcome {
    let _ = connection.send_client(&ClientFrame::Cancel, Instant::now() + REQUEST_TIMEOUT);
    let deadline = Instant::now() + OUTCOME_TIMEOUT;
    loop {
        match connection.receive_keeper(deadline) {
            Ok(KeeperFrame::SecretRequest) => continue,
            Ok(frame) => return outcome_after_begin(Some(frame)),
            Err(_) => return outcome_after_begin(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_session_control::KeeperOutcome;

    fn record() -> SessionRecord {
        SessionRecord {
            version: 1,
            credential_id: "0123456789abcdefghjkmnpqrs".into(),
            generation: "0123456789abcdefghjkmnpqrt".into(),
            unit: "vivac-custody-0123456789abcdefghjkmnpqrs-0123456789abcdefghjkmnpqrt.service"
                .into(),
            unit_cgroup: "/user.slice/user-1000.slice/user@1000.service/app.slice/vivac-custody-0123456789abcdefghjkmnpqrs-0123456789abcdefghjkmnpqrt.service".into(),
            unit_cgroup_inode: 7,
            session_cgroup_inode: 8,
            boot_id: "00000000-0000-0000-0000-000000000000".into(),
            admission_sha256: "0".repeat(64),
        }
    }

    #[test]
    fn opening_preflight_selects_open_reopen_or_refusal() {
        use Preflight::{Open, Refuse, Reopen};
        let record = record();
        for evidence in [None, Some(ReopeningEvidence::NewBoot)] {
            assert_eq!(preflight(None, evidence, true), Open);
            assert_eq!(preflight(None, evidence, false), Refuse(None));
        }
        for evidence in [
            ReopeningEvidence::NewBoot,
            ReopeningEvidence::CgroupGone,
            ReopeningEvidence::CgroupEmpty,
        ] {
            assert_eq!(preflight(Some(&record), Some(evidence), false), Reopen);
            assert_eq!(preflight(Some(&record), Some(evidence), true), Open);
        }
        for permit_present in [false, true] {
            assert_eq!(
                preflight(Some(&record), None, permit_present),
                Refuse(Some(RefusalHint::CloseFirst))
            );
        }
    }

    #[test]
    fn outcomes_after_begin_never_claim_more_than_the_keeper_reported() {
        use OpeningOutcome::*;
        for (frame, expected) in [
            (None, OpeningUncertain),
            (
                Some(KeeperFrame::Outcome(KeeperOutcome::Refused)),
                Refused(None),
            ),
            (
                Some(KeeperFrame::Outcome(KeeperOutcome::Interrupted)),
                Interrupted,
            ),
            (
                Some(KeeperFrame::Outcome(KeeperOutcome::PublicationUncertain)),
                PublicationUncertain,
            ),
            (Some(KeeperFrame::Opened { reopened: true }), Reopened),
            (Some(KeeperFrame::Opened { reopened: false }), Open),
            // A close's frames, or a second secret request, are not an
            // outcome of this opening.
            (
                Some(KeeperFrame::Outcome(KeeperOutcome::Closed)),
                OpeningUncertain,
            ),
            (Some(KeeperFrame::Closing), OpeningUncertain),
            (Some(KeeperFrame::SecretRequest), OpeningUncertain),
        ] {
            assert_eq!(outcome_after_begin(frame), expected, "{frame:?}");
        }
    }

    #[test]
    fn each_refusal_remedy_names_the_command_to_run() {
        assert_eq!(
            RefusalHint::EnableLinger {
                account: "custodian".into()
            }
            .remedy("0123456789abcdefghjkmnpqrs"),
            "Run loginctl enable-linger custodian as an administrator, then retry."
        );
        assert_eq!(
            RefusalHint::CloseFirst.remedy("0123456789abcdefghjkmnpqrs"),
            "Run vivac device-key close-linux 0123456789abcdefghjkmnpqrs first."
        );
        let account = |name: &[u8]| Account {
            uid: 1000,
            name: std::ffi::CString::new(name).unwrap(),
        };
        assert_eq!(
            RefusalHint::enable_linger(&account(b"custodian")),
            Some(RefusalHint::EnableLinger {
                account: "custodian".into()
            })
        );
        for name in [&b""[..], b"two words", b"bell\x07", b"\xff"] {
            assert_eq!(RefusalHint::enable_linger(&account(name)), None, "{name:?}");
        }
    }
}
