//! `close-linux`: asks the session keeper for a clean close over its control
//! socket. When the keeper is gone, it stops the recorded unit and confirms
//! that the unit's cgroup is gone or empty. No terminal is needed, and the
//! fallback never removes the session record.

use crate::linux_current_admission::CurrentAdmission;
use crate::linux_session_cgroup::{observe, CgroupIdentity, CgroupObservation};
use crate::linux_session_control::{
    ClientFrame, ControlConnection, KeeperFrame, KeeperOutcome, CONTROL_SOCKET,
};
use crate::linux_session_record::{SessionRecord, SessionRecords};
use crate::linux_supervisor_start::cancellation::{cancellation_pending, Cancellation};
use crate::linux_user_manager::{SessionUnit, UserManager};
use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The connection to the keeper, and each send, fit in this budget.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// The keeper closes within its 25-second budget; this leaves room for the
/// report.
const OUTCOME_TIMEOUT: Duration = Duration::from_secs(40);
/// How often a wait for the keeper looks for a pending cancellation.
const CANCELLATION_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CloseOutcome {
    Closed,
    Refused,
    Interrupted,
    PublicationUncertain,
    CloseUncertain,
}

/// What the keeper's reply makes of the close. `None` means no keeper
/// answered and none reported Closing, so the unit is stopped instead.
/// A keeper that reported Closing and then went silent may have published.
pub(super) fn close_outcome(
    reply: Option<KeeperFrame>,
    closing_acknowledged: bool,
) -> Option<CloseOutcome> {
    match reply {
        Some(KeeperFrame::Outcome(KeeperOutcome::Closed)) => Some(CloseOutcome::Closed),
        Some(KeeperFrame::Outcome(KeeperOutcome::Refused)) => Some(CloseOutcome::Refused),
        Some(KeeperFrame::Outcome(KeeperOutcome::Interrupted)) => Some(CloseOutcome::Interrupted),
        Some(KeeperFrame::Outcome(KeeperOutcome::PublicationUncertain)) => {
            Some(CloseOutcome::PublicationUncertain)
        }
        None
        | Some(KeeperFrame::SecretRequest | KeeperFrame::Opened { .. } | KeeperFrame::Closing) => {
            closing_acknowledged.then_some(CloseOutcome::CloseUncertain)
        }
    }
}

/// After the unit was stopped: no permit was published, and the next opening
/// reopens on this evidence only when the unit's cgroup is gone or empty.
pub(super) fn fallback_outcome(unit: CgroupObservation) -> CloseOutcome {
    match unit {
        CgroupObservation::Absent | CgroupObservation::Empty => CloseOutcome::Interrupted,
        CgroupObservation::Populated | CgroupObservation::Replaced => CloseOutcome::CloseUncertain,
    }
}

/// Closes the session of `credential`. Without a record there is nothing to
/// close, and a record that fails validation is left where it is.
pub(crate) fn close(credential: &str) -> CloseOutcome {
    let Ok(_cancellation) = Cancellation::block() else {
        return CloseOutcome::Refused;
    };
    let Ok(admission) = CurrentAdmission::open_fixed_namespace(credential) else {
        return CloseOutcome::Refused;
    };
    let Ok(Some(stored)) = SessionRecords::new(&admission).read() else {
        return CloseOutcome::Refused;
    };
    let record = stored.record;
    let (Ok(manager), Ok(unit)) = (UserManager::open(), record.unit()) else {
        return CloseOutcome::Refused;
    };
    // Nothing was asked of the keeper yet, so a cancellation closes nothing.
    if cancellation_pending().unwrap_or(true) {
        return CloseOutcome::Refused;
    }
    if let Some(outcome) = ask_keeper(&manager, &unit, &record) {
        return outcome;
    }
    // The unit may be running even when its keeper does not answer. Stopping
    // it ends every process in it; the record stays as the evidence the next
    // opening needs.
    let _ = manager.stop_unit(&unit);
    let identity = CgroupIdentity {
        inode: record.unit_cgroup_inode,
    };
    match observe(&record.unit_cgroup, identity) {
        Ok(observation) => fallback_outcome(observation),
        Err(_) => CloseOutcome::CloseUncertain,
    }
}

fn control_socket(manager: &UserManager, unit: &SessionUnit) -> PathBuf {
    manager
        .runtime_directory()
        .join(unit.runtime_name())
        .join(OsStr::from_bytes(CONTROL_SOCKET.to_bytes()))
}

/// Sends `Close` and waits for the keeper's outcome. A signal sends `Cancel`
/// once, and the wait goes on: the keeper's answer says whether the cancel
/// arrived before its permit rename was authorized. `None` sends the caller
/// to the fallback.
fn ask_keeper(
    manager: &UserManager,
    unit: &SessionUnit,
    record: &SessionRecord,
) -> Option<CloseOutcome> {
    let connection = ControlConnection::connect_keeper(
        &control_socket(manager, unit),
        &record.unit_cgroup,
        Instant::now() + REQUEST_TIMEOUT,
    )
    .ok()?;
    // A failed send queued nothing: the keeper never saw `Close`.
    connection
        .send_client(&ClientFrame::Close, Instant::now() + REQUEST_TIMEOUT)
        .ok()?;
    let deadline = Instant::now() + OUTCOME_TIMEOUT;
    let mut acknowledged = false;
    let mut cancelled = false;
    loop {
        if !cancelled && cancellation_pending().unwrap_or(true) {
            cancelled = true;
            let _ = connection.send_client(&ClientFrame::Cancel, Instant::now() + REQUEST_TIMEOUT);
        }
        let now = Instant::now();
        if now >= deadline {
            return close_outcome(None, acknowledged);
        }
        match connection.receive_keeper((now + CANCELLATION_INTERVAL).min(deadline)) {
            Ok(KeeperFrame::Closing) => acknowledged = true,
            Ok(frame @ KeeperFrame::Outcome(_)) => return close_outcome(Some(frame), acknowledged),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(_) => return close_outcome(None, acknowledged),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_session_cgroup::CgroupObservation;
    use crate::linux_session_control::{KeeperFrame, KeeperOutcome};

    #[test]
    fn close_outcomes_follow_the_keeper_reply() {
        for (outcome, expected) in [
            (KeeperOutcome::Closed, CloseOutcome::Closed),
            (KeeperOutcome::Refused, CloseOutcome::Refused),
            (KeeperOutcome::Interrupted, CloseOutcome::Interrupted),
            (
                KeeperOutcome::PublicationUncertain,
                CloseOutcome::PublicationUncertain,
            ),
        ] {
            for acknowledged in [false, true] {
                assert_eq!(
                    close_outcome(Some(KeeperFrame::Outcome(outcome)), acknowledged),
                    Some(expected)
                );
            }
        }
        assert_eq!(
            close_outcome(None, true),
            Some(CloseOutcome::CloseUncertain)
        );
        assert_eq!(close_outcome(None, false), None);
    }

    #[test]
    fn fallback_reports_interrupted_only_when_the_unit_is_gone_or_empty() {
        for (observation, expected) in [
            (CgroupObservation::Absent, CloseOutcome::Interrupted),
            (CgroupObservation::Empty, CloseOutcome::Interrupted),
            (CgroupObservation::Populated, CloseOutcome::CloseUncertain),
            (CgroupObservation::Replaced, CloseOutcome::CloseUncertain),
        ] {
            assert_eq!(fallback_outcome(observation), expected, "{observation:?}");
        }
    }
}
