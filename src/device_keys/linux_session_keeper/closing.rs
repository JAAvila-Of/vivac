//! Every way out of a session that started: kill the session cgroup, wait for
//! the kernel to report it empty, and only then publish the opening permit
//! again. The record is removed only after a durable publication.

use super::{ready_now, refused, short_request, wait_readable, Children, Keeper, CLOSE_BUDGET};
use crate::linux_admission_storage::PublishError;
use crate::linux_ipc::registration::{
    receive_any_session_child_step, send_session_keeper_step, EndpointSessionStep,
};
use crate::linux_session_cgroup::EmptySessionCgroup;
use crate::linux_session_control::{ClientFrame, ControlConnection, KeeperFrame, KeeperOutcome};
use crate::linux_session_record::SessionRecord;
use crate::linux_supervisor_start::wait_for_success;
use std::io;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const CLOSED_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CloseReason {
    Request,
    ManagerStop,
    IdleExpiry,
    MaximumExpiry,
    SessionEnded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecordDisposition {
    Remove,
    Keep,
}

/// The session cgroup could not be shown empty: nothing is published, and
/// the record stays as evidence for the next opening.
pub(super) const EMPTY_UNKNOWN: (KeeperOutcome, RecordDisposition) =
    (KeeperOutcome::Interrupted, RecordDisposition::Keep);

/// After a failed opening. A permit that is published again, or one that was
/// never consumed, leaves the credential ready to open as before.
pub(super) fn failure_outcome(
    result: Result<(), PublishError>,
) -> (KeeperOutcome, RecordDisposition) {
    match result {
        Ok(()) => (KeeperOutcome::Refused, RecordDisposition::Remove),
        Err(PublishError::Unpublished(error)) if error.raw_os_error() == Some(libc::EEXIST) => {
            (KeeperOutcome::Refused, RecordDisposition::Remove)
        }
        Err(PublishError::Unpublished(_)) => (KeeperOutcome::Interrupted, RecordDisposition::Keep),
        Err(PublishError::PublishedDurabilityUnknown(_)) => {
            (KeeperOutcome::PublicationUncertain, RecordDisposition::Keep)
        }
    }
}

/// After an active session. Only a durable publication is a close.
pub(super) fn close_outcome(
    result: Result<(), PublishError>,
) -> (KeeperOutcome, RecordDisposition) {
    match result {
        Ok(()) => (KeeperOutcome::Closed, RecordDisposition::Remove),
        Err(PublishError::Unpublished(_)) => (KeeperOutcome::Interrupted, RecordDisposition::Keep),
        Err(PublishError::PublishedDurabilityUnknown(_)) => {
            (KeeperOutcome::PublicationUncertain, RecordDisposition::Keep)
        }
    }
}

/// The kernel's report must be about the session cgroup this keeper recorded.
pub(super) fn empty_session_matches(
    evidence: &EmptySessionCgroup,
    record: &SessionRecord,
) -> Result<(), PublishError> {
    if evidence.identity().inode != record.session_cgroup_inode {
        return Err(PublishError::Unpublished(io::Error::from(
            io::ErrorKind::InvalidData,
        )));
    }
    Ok(())
}

/// Whether the client has asked to cancel. Only an explicit `Cancel` counts:
/// a closing that lost its client still finishes.
fn cancel_requested(client: &ControlConnection) -> bool {
    ready_now(client.as_raw_fd()).is_ok_and(|revents| revents & libc::POLLIN != 0)
        && matches!(
            client.receive_client(short_request()),
            Ok(ClientFrame::Cancel)
        )
}

/// The last word before the exclusive rename: refused after the stop budget
/// or once the client has asked to cancel.
fn authorize_rename(deadline: Instant, client: Option<&ControlConnection>) -> io::Result<()> {
    if Instant::now() >= deadline {
        return Err(io::Error::from(io::ErrorKind::TimedOut));
    }
    if client.is_some_and(cancel_requested) {
        return Err(refused());
    }
    Ok(())
}

fn finish(
    keeper: &Keeper,
    children: &mut Children,
    client: Option<ControlConnection>,
    (outcome, disposition): (KeeperOutcome, RecordDisposition),
) -> KeeperOutcome {
    if let Some(root) = children.runtime.take() {
        // The unit's runtime directory goes with the unit in any case.
        let _ = root.remove_empty();
    }
    if disposition == RecordDisposition::Remove {
        // A durable permit stands even if this fails; the next opening retires
        // the record on evidence.
        let _ = keeper.remove_record();
    }
    if let Some(client) = client {
        let _ = client.send_keeper(KeeperFrame::Outcome(outcome), short_request());
    }
    outcome
}

/// Asks the supervisor for a clean close and receives the closure of the
/// active generation.
fn close_supervisor(children: &mut Children, deadline: Instant) -> io::Result<()> {
    use EndpointSessionStep::*;
    let Children {
        owner,
        supervisor,
        adopted,
        ..
    } = children;
    let owner = owner.as_mut().ok_or_else(refused)?;
    let (receipt, channel) = supervisor.as_ref().ok_or_else(refused)?;
    let adopted = adopted.as_mut().ok_or_else(refused)?;
    let short = || short_request().min(deadline);
    send_session_keeper_step(channel, owner, receipt.token, Close, short())?;
    // A proof served just before Close was sent is reported ahead of Closed.
    let closed_deadline = (Instant::now() + CLOSED_TIMEOUT).min(deadline);
    loop {
        wait_readable(channel.as_raw_fd(), closed_deadline)?;
        match receive_any_session_child_step(channel, owner, receipt.token, closed_deadline)? {
            Served => continue,
            Closed => break,
            _ => return Err(refused()),
        }
    }
    adopted.probe_receive_active_closure(short())?;
    adopted.confirm_active_closure(short())?;
    let _ = wait_for_success(owner, *receipt, short());
    Ok(())
}

/// Closes an active session for `reason`; `client` is the `close-linux`
/// that asked, if any, and receives the outcome.
pub(super) fn close_session(
    reason: CloseReason,
    client: Option<ControlConnection>,
    keeper: &Keeper,
    children: &mut Children,
) -> KeeperOutcome {
    let deadline = Instant::now() + CLOSE_BUDGET;
    let closure =
        reason != CloseReason::SessionEnded && close_supervisor(children, deadline).is_ok();
    let empty = match keeper.session.kill_and_wait_empty(deadline) {
        Ok(empty) => empty,
        Err(_) => return finish(keeper, children, client, EMPTY_UNKNOWN),
    };
    let result = empty_session_matches(&empty, &keeper.record.record).and_then(|()| {
        let adopted = children
            .adopted
            .as_mut()
            .ok_or_else(|| PublishError::Unpublished(refused()))?;
        let authorize = || authorize_rename(deadline, client.as_ref());
        if closure {
            adopted.publish_clean_close(authorize)
        } else {
            adopted.publish_after_empty_session(&empty, authorize)
        }
    });
    finish(keeper, children, client, close_outcome(result))
}

/// Ends an opening that failed after `Begin`, a cancellation included.
pub(super) fn fail_opening(
    keeper: &Keeper,
    children: &mut Children,
    client: Option<ControlConnection>,
) -> KeeperOutcome {
    let deadline = Instant::now() + CLOSE_BUDGET;
    // Closing each channel ends the children's waits, the one for the secret
    // included; the kill below ends whatever is left.
    drop(children.verifier.take());
    if let Some((receipt, channel)) = children.supervisor.take() {
        drop(channel);
        if let Some(owner) = &children.owner {
            let _ = owner.signal(receipt.token, libc::SIGHUP);
        }
    }
    let empty = match keeper.session.kill_and_wait_empty(deadline) {
        Ok(empty) => empty,
        Err(_) => return finish(keeper, children, client, EMPTY_UNKNOWN),
    };
    let authorize = || authorize_rename(deadline, None);
    let result = empty_session_matches(&empty, &keeper.record.record).and_then(|()| match children
        .adopted
        .as_mut()
    {
        Some(adopted) => adopted.publish_after_empty_session(&empty, authorize),
        None => keeper
            .admission
            .republish_after_empty_session(&empty, authorize),
    });
    finish(keeper, children, client, failure_outcome(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_admission_storage::PublishError;
    use crate::linux_session_cgroup::EmptySessionCgroup;
    use crate::linux_session_control::KeeperOutcome;
    use crate::linux_session_record::SessionRecord;

    fn os(code: i32) -> io::Error {
        io::Error::from_raw_os_error(code)
    }

    #[test]
    fn failed_opening_outcomes_keep_the_record_without_durable_publication() {
        use KeeperOutcome::*;
        use RecordDisposition::*;
        assert_eq!(failure_outcome(Ok(())), (Refused, Remove));
        assert_eq!(
            failure_outcome(Err(PublishError::Unpublished(os(libc::EEXIST)))),
            (Refused, Remove)
        );
        assert_eq!(
            failure_outcome(Err(PublishError::Unpublished(os(libc::EIO)))),
            (Interrupted, Keep)
        );
        assert_eq!(
            failure_outcome(Err(PublishError::PublishedDurabilityUnknown(os(
                libc::EEXIST
            )))),
            (PublicationUncertain, Keep)
        );
        assert_eq!(EMPTY_UNKNOWN, (Interrupted, Keep));
    }

    #[test]
    fn close_outcomes_report_only_a_durable_publication_as_closed() {
        use KeeperOutcome::*;
        use RecordDisposition::*;
        assert_eq!(close_outcome(Ok(())), (Closed, Remove));
        for error in [os(libc::EEXIST), os(libc::EIO)] {
            assert_eq!(
                close_outcome(Err(PublishError::Unpublished(error))),
                (Interrupted, Keep)
            );
        }
        assert_eq!(
            close_outcome(Err(PublishError::PublishedDurabilityUnknown(os(libc::EIO)))),
            (PublicationUncertain, Keep)
        );
    }

    #[test]
    fn publication_requires_the_recorded_session_cgroup() {
        let record = SessionRecord {
            version: 1,
            credential_id: "00000000000000000000000004".to_owned(),
            generation: "00000000000000000000000005".to_owned(),
            unit: "vivac-custody-00000000000000000000000004-00000000000000000000000005.service"
                .to_owned(),
            unit_cgroup: "/user.slice/user-1000.slice/user@1000.service/app.slice/vivac-custody-00000000000000000000000004-00000000000000000000000005.service".to_owned(),
            unit_cgroup_inode: 11,
            session_cgroup_inode: 12,
            boot_id: "01234567-89ab-cdef-0123-456789abcdef".to_owned(),
            admission_sha256: "ab".repeat(32),
        };
        empty_session_matches(&EmptySessionCgroup::for_test(12), &record).unwrap();
        for inode in [11, 13] {
            assert!(matches!(
                empty_session_matches(&EmptySessionCgroup::for_test(inode), &record),
                Err(PublishError::Unpublished(_))
            ));
        }
    }
}
