//! `close-linux`: asks the session keeper for a clean close over its control
//! socket. When no keeper answers, it stops the recorded unit, confirms that
//! the unit's cgroup is gone or empty, and reports what the stored permit and
//! record then show. No terminal is needed, and the fallback never removes the
//! session record.

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

/// What the reply of an authenticated keeper makes of the close. Without an
/// outcome, whether from a hang-up, a failed send or silence, the keeper may
/// have published, so only a reported outcome is more than uncertain.
pub(super) fn close_outcome(reply: Option<KeeperFrame>) -> CloseOutcome {
    match reply {
        Some(KeeperFrame::Outcome(KeeperOutcome::Closed)) => CloseOutcome::Closed,
        Some(KeeperFrame::Outcome(KeeperOutcome::Refused)) => CloseOutcome::Refused,
        Some(KeeperFrame::Outcome(KeeperOutcome::Interrupted)) => CloseOutcome::Interrupted,
        Some(KeeperFrame::Outcome(KeeperOutcome::PublicationUncertain)) => {
            CloseOutcome::PublicationUncertain
        }
        None
        | Some(KeeperFrame::SecretRequest | KeeperFrame::Opened { .. } | KeeperFrame::Closing) => {
            CloseOutcome::CloseUncertain
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FallbackStep {
    Stop,
    Refuse,
}

/// What to do when no keeper could be authenticated. The unit is stopped only
/// then, and never once a signal asked to cancel: stopping it would be the
/// opposite.
pub(super) fn fallback_step(cancellation_pending: bool) -> FallbackStep {
    if cancellation_pending {
        FallbackStep::Refuse
    } else {
        FallbackStep::Stop
    }
}

/// What the stored state shows once the unit has ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StoredAfterStop {
    /// A permit that validates as this credential's opening permit.
    pub(super) permit: bool,
    /// A session record of this admission.
    pub(super) record: bool,
}

/// After the unit was stopped. Only a unit whose cgroup is gone or empty has
/// ended; the stored state then says what its keeper left. A keeper that was
/// alive but could not be reached may have closed cleanly when the stop
/// reached it, and a record can outlive a durable publication when its removal
/// failed, so the permit is never assumed absent.
pub(super) fn fallback_outcome(
    unit: CgroupObservation,
    stored: impl FnOnce() -> io::Result<StoredAfterStop>,
) -> CloseOutcome {
    match unit {
        CgroupObservation::Absent | CgroupObservation::Empty => match stored() {
            // The keeper removes its record only after a durable, validated
            // publication.
            Ok(StoredAfterStop {
                permit: true,
                record: false,
            }) => CloseOutcome::Closed,
            // The rename happened, and this attempt cannot show it durable;
            // the next opening retires the record on the kernel evidence.
            Ok(StoredAfterStop {
                permit: true,
                record: true,
            }) => CloseOutcome::PublicationUncertain,
            Ok(StoredAfterStop { permit: false, .. }) => CloseOutcome::Interrupted,
            Err(_) => CloseOutcome::CloseUncertain,
        },
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
    let (Ok(manager), Ok(unit)) = (UserManager::open_running(), record.unit()) else {
        return CloseOutcome::Refused;
    };
    // Nothing was asked of the keeper yet, so a cancellation closes nothing.
    if cancellation_pending().unwrap_or(true) {
        return CloseOutcome::Refused;
    }
    if let Some(outcome) = ask_keeper(&manager, &unit, &record) {
        return outcome;
    }
    // No keeper could be authenticated. The unit may still be running; stopping
    // it ends every process in it, and the record stays as the evidence the
    // next opening needs.
    if fallback_step(cancellation_pending().unwrap_or(true)) == FallbackStep::Refuse {
        return CloseOutcome::Refused;
    }
    let _ = manager.stop_unit(&unit);
    let identity = CgroupIdentity {
        inode: record.unit_cgroup_inode,
    };
    match observe(&record.unit_cgroup, identity) {
        Ok(observation) => fallback_outcome(observation, || stored_after_stop(credential)),
        Err(_) => CloseOutcome::CloseUncertain,
    }
}

/// Reads the permit, then the record, through a fresh admission. The keeper
/// removes its record only after a durable, validated publication, so the
/// permit is read first: a record that goes after it was read cannot turn an
/// unproven publication into `closed`.
fn stored_after_stop(credential: &str) -> io::Result<StoredAfterStop> {
    let admission = CurrentAdmission::open_fixed_namespace(credential)?;
    let permit = !admission.opening_permit_absent()?;
    if permit {
        admission.observe_opening_permit()?;
    }
    let record = SessionRecords::new(&admission).read()?.is_some();
    Ok(StoredAfterStop { permit, record })
}

fn control_socket(manager: &UserManager, unit: &SessionUnit) -> PathBuf {
    manager
        .runtime_directory()
        .join(unit.runtime_name())
        .join(OsStr::from_bytes(CONTROL_SOCKET.to_bytes()))
}

/// Sends `Close` and waits for the keeper's outcome. A signal sends `Cancel`
/// once, and the wait goes on: the keeper's answer says whether the cancel
/// arrived before its permit rename was authorized. Once the keeper is
/// authenticated the result is always its outcome or `CloseUncertain`; `None`
/// means no keeper was reached and sends the caller to the fallback.
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
    if connection
        .send_client(&ClientFrame::Close, Instant::now() + REQUEST_TIMEOUT)
        .is_err()
    {
        return Some(close_outcome(None));
    }
    let deadline = Instant::now() + OUTCOME_TIMEOUT;
    let mut cancelled = false;
    loop {
        if !cancelled && cancellation_pending().unwrap_or(true) {
            cancelled = true;
            let _ = connection.send_client(&ClientFrame::Cancel, Instant::now() + REQUEST_TIMEOUT);
        }
        let now = Instant::now();
        if now >= deadline {
            return Some(close_outcome(None));
        }
        match connection.receive_keeper((now + CANCELLATION_INTERVAL).min(deadline)) {
            Ok(frame @ KeeperFrame::Outcome(_)) => return Some(close_outcome(Some(frame))),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(_) => return Some(close_outcome(None)),
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
            assert_eq!(close_outcome(Some(KeeperFrame::Outcome(outcome))), expected);
        }
    }

    #[test]
    fn an_authenticated_keeper_without_an_outcome_is_uncertain() {
        // A hang-up, a failed send or silence, after or without `Closing`.
        assert_eq!(close_outcome(None), CloseOutcome::CloseUncertain);
        for frame in [
            KeeperFrame::Closing,
            KeeperFrame::SecretRequest,
            KeeperFrame::Opened { reopened: false },
        ] {
            assert_eq!(close_outcome(Some(frame)), CloseOutcome::CloseUncertain);
        }
    }

    #[test]
    fn the_unit_is_stopped_only_when_no_keeper_was_reached_and_nothing_was_cancelled() {
        assert_eq!(fallback_step(false), FallbackStep::Stop);
        assert_eq!(fallback_step(true), FallbackStep::Refuse);
    }

    #[test]
    fn fallback_reads_the_stored_state_once_the_unit_has_ended() {
        use CloseOutcome::*;
        let stored = |permit, record| move || Ok(StoredAfterStop { permit, record });
        for unit in [CgroupObservation::Absent, CgroupObservation::Empty] {
            // A keeper that closed cleanly removed its record after a durable,
            // validated publication.
            assert_eq!(
                fallback_outcome(unit, stored(true, false)),
                Closed,
                "{unit:?}"
            );
            // The rename happened; this attempt cannot show it durable.
            assert_eq!(
                fallback_outcome(unit, stored(true, true)),
                PublicationUncertain,
                "{unit:?}"
            );
            for record in [false, true] {
                assert_eq!(
                    fallback_outcome(unit, stored(false, record)),
                    Interrupted,
                    "{unit:?}"
                );
            }
            assert_eq!(
                fallback_outcome(unit, || Err(io::Error::from(io::ErrorKind::InvalidData))),
                CloseUncertain,
                "{unit:?}"
            );
        }
    }

    #[test]
    fn fallback_reads_nothing_while_the_unit_may_still_run() {
        for unit in [CgroupObservation::Populated, CgroupObservation::Replaced] {
            let read = std::cell::Cell::new(false);
            let outcome = fallback_outcome(unit, || {
                read.set(true);
                Ok(StoredAfterStop {
                    permit: true,
                    record: false,
                })
            });
            assert_eq!(outcome, CloseOutcome::CloseUncertain, "{unit:?}");
            assert!(!read.get(), "{unit:?}");
        }
    }
}
