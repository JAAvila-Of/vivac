//! From the client's `Begin` to `Opened`: the supervisor and the verifier
//! start in the session cgroup, the supervisor opens with the authority the
//! keeper chose, and the unlock secret goes from the client to the supervisor.
//! A cancellation, a hang-up of the client or a stop request ends the opening.

use super::{refused, watched, Children, Keeper, HANGUP, REQUEST_TIMEOUT};
use crate::linux_current_admission::{CurrentAdmission, OpeningAuthority};
use crate::linux_endpoint_runtime::RuntimeRoot;
use crate::linux_ipc::registration::{
    authorize_endpoint_start, finish_endpoint_start, receive_session_child_step,
    send_session_keeper_step, send_session_secret, EndpointSessionStep, EndpointStartupRole,
    KeeperChallengeSender,
};
use crate::linux_session_control::{ClientFrame, ControlConnection, KeeperFrame};
use crate::linux_spawn::{Receipt, SpawnOwner};
use crate::linux_supervisor_start::{
    launch, short_deadline, wait_for_success, SHORT_STEP, STARTUP_TIMEOUT, SUPERVISOR, VERIFIER,
};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const REGISTER_TIMEOUT: Duration = Duration::from_secs(20);
const SECRET_TIMEOUT: Duration = Duration::from_secs(20);
const UNLOCK_TIMEOUT: Duration = Duration::from_secs(30);
const PROOF_TIMEOUT: Duration = Duration::from_secs(22);
/// The short steps between the unlock secret reaching the supervisor and
/// `Opened`: the secret send, `Prove`, the endpoint start, its success, the
/// activation, its report and `Opened` itself.
const SHORT_STEPS_AFTER_SECRET: u32 = 7;
/// The longest the keeper may take after the unlock secret before it reports
/// the opening, the sum of its own per-step budgets.
pub(super) const AFTER_SECRET_BUDGET: Duration = UNLOCK_TIMEOUT
    .saturating_add(PROOF_TIMEOUT)
    .saturating_add(SHORT_STEP.saturating_mul(SHORT_STEPS_AFTER_SECRET));

pub(super) enum OpeningEvent {
    Frame(ClientFrame),
    Cancelled,
}

/// A hang-up, an error condition or a failed read of the client's connection
/// is a cancellation: a client killed by a signal it cannot catch leaves
/// nothing else behind.
pub(super) fn opening_event(revents: i16, frame: io::Result<ClientFrame>) -> OpeningEvent {
    if revents & HANGUP != 0 {
        return OpeningEvent::Cancelled;
    }
    match frame {
        Ok(frame) => OpeningEvent::Frame(frame),
        Err(_) => OpeningEvent::Cancelled,
    }
}

pub(super) fn opening_step(authority: OpeningAuthority) -> EndpointSessionStep {
    match authority {
        OpeningAuthority::Permit => EndpointSessionStep::Start,
        OpeningAuthority::Reopened => EndpointSessionStep::Reopen,
    }
}

/// The supervisor must report back the authority the keeper sent it.
pub(super) fn require_reported_authority(
    sent: OpeningAuthority,
    reported: Option<OpeningAuthority>,
) -> io::Result<()> {
    if reported != Some(sent) {
        return Err(refused());
    }
    Ok(())
}

enum Ready {
    Child,
    Client(OpeningEvent),
}

/// Watches the client and the stop signals while the keeper waits on a child.
struct Watch<'a> {
    client: &'a ControlConnection,
    signals: &'a OwnedFd,
}

impl Watch<'_> {
    fn wait(&self, child: &OwnedFd, deadline: Instant) -> io::Result<Ready> {
        let mut descriptors = [
            watched(self.client.as_raw_fd()),
            watched(self.signals.as_raw_fd()),
            watched(child.as_raw_fd()),
        ];
        loop {
            super::poll_until(&mut descriptors, deadline)?;
            if descriptors[1].revents != 0 && super::stop_requested(self.signals)? {
                return Err(refused());
            }
            if descriptors[0].revents != 0 {
                return Ok(Ready::Client(self.event(descriptors[0].revents)));
            }
            if descriptors[2].revents != 0 {
                return Ok(Ready::Child);
            }
        }
    }

    fn event(&self, revents: i16) -> OpeningEvent {
        let frame = if revents & HANGUP == 0 {
            self.client.receive_client(Instant::now() + REQUEST_TIMEOUT)
        } else {
            Err(io::Error::from(io::ErrorKind::UnexpectedEof))
        };
        opening_event(revents, frame)
    }

    /// Refuses when the client has sent anything or hung up, or a stop is
    /// pending, without waiting.
    fn check(&self) -> io::Result<()> {
        if super::ready_now(self.client.as_raw_fd())? != 0
            || (super::ready_now(self.signals.as_raw_fd())? != 0
                && super::stop_requested(self.signals)?)
        {
            return Err(refused());
        }
        Ok(())
    }

    fn receive_step(
        &self,
        (child, socket): &(Receipt, OwnedFd),
        owner: &SpawnOwner,
        step: EndpointSessionStep,
        deadline: Instant,
    ) -> io::Result<()> {
        match self.wait(socket, deadline)? {
            Ready::Child => receive_session_child_step(socket, owner, child.token, step, deadline),
            Ready::Client(_) => Err(refused()),
        }
    }

    /// The client's `Secret`, while the supervisor stays silent. Anything
    /// else from either side ends the opening.
    fn secret(&self, supervisor: &OwnedFd, deadline: Instant) -> io::Result<Zeroizing<[u8; 64]>> {
        match self.wait(supervisor, deadline)? {
            Ready::Client(OpeningEvent::Frame(ClientFrame::Secret(secret))) => Ok(secret),
            _ => Err(refused()),
        }
    }
}

fn send_step(
    (child, socket): &(Receipt, OwnedFd),
    owner: &SpawnOwner,
    step: EndpointSessionStep,
) -> io::Result<()> {
    send_session_keeper_step(socket, owner, child.token, step, short_deadline())
}

/// Runs the opening. Whatever it started stays in `children`, so that a
/// failure can end it and publish only after the session cgroup is empty.
pub(super) fn open(
    keeper: &Keeper,
    client: &ControlConnection,
    children: &mut Children,
) -> io::Result<()> {
    use EndpointSessionStep::*;
    let watch = Watch {
        client,
        signals: &keeper.signals,
    };
    let credential = keeper.unit.credential();
    let startup_deadline = Instant::now() + STARTUP_TIMEOUT;
    let Children {
        owner,
        supervisor,
        verifier,
        runtime,
        adopted,
    } = children;
    let mut created = SpawnOwner::new_dedicated()?;
    created.place_children_in(keeper.session.procs()?)?;
    let owner = owner.insert(created);
    let root = runtime.insert(RuntimeRoot::create_in(&keeper.runtime)?);
    let session = Some((credential, keeper.runtime.as_path()));
    let supervisor = &*supervisor.insert(launch(owner, SUPERVISOR, session)?);
    let verifier = &*verifier.insert(launch(owner, VERIFIER, session)?);
    for (child, peer, role) in [
        (supervisor, verifier, EndpointStartupRole::Supervisor),
        (verifier, supervisor, EndpointStartupRole::Verifier),
    ] {
        authorize_endpoint_start(
            &child.1,
            owner,
            child.0.token,
            peer.0.token,
            root.directory(),
            role,
            startup_deadline,
        )?;
    }
    watch.receive_step(verifier, owner, Prepared, startup_deadline)?;
    watch.check()?;
    send_step(supervisor, owner, opening_step(keeper.authority))?;
    watch.receive_step(
        supervisor,
        owner,
        Register,
        Instant::now() + REGISTER_TIMEOUT,
    )?;
    let offer = adopted.insert(
        KeeperChallengeSender::for_owned_child(
            supervisor.1.try_clone()?,
            owner,
            supervisor.0.token,
            short_deadline(),
        )?
        .receive_launch_context(CurrentAdmission::open_fixed_namespace(credential)?)?
        .receive_offer()?
        .adopt()?,
    );
    offer.confirm()?;
    require_reported_authority(keeper.authority, offer.confirmed_opening_authority())?;
    client.send_keeper(KeeperFrame::SecretRequest, short_deadline())?;
    let secret = watch.secret(&supervisor.1, Instant::now() + SECRET_TIMEOUT)?;
    send_session_secret(
        &supervisor.1,
        owner,
        supervisor.0.token,
        &secret,
        short_deadline(),
    )?;
    drop(secret);
    watch.receive_step(supervisor, owner, Unlocked, Instant::now() + UNLOCK_TIMEOUT)?;
    send_step(verifier, owner, Prove)?;
    let proof_deadline = Instant::now() + PROOF_TIMEOUT;
    watch.receive_step(supervisor, owner, Proved, proof_deadline)?;
    watch.receive_step(verifier, owner, Proved, proof_deadline)?;
    finish_endpoint_start(&verifier.1, owner, verifier.0.token, short_deadline())?;
    wait_for_success(owner, verifier.0, short_deadline())?;
    watch.check()?;
    send_step(supervisor, owner, Activate)?;
    watch.receive_step(supervisor, owner, Active, short_deadline())?;
    // A cancellation that arrived during activation still refuses.
    watch.check()?;
    client.send_keeper(
        KeeperFrame::Opened {
            reopened: keeper.authority == OpeningAuthority::Reopened,
        },
        short_deadline(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_current_admission::OpeningAuthority;
    use crate::linux_ipc::registration::EndpointSessionStep;
    use crate::linux_session_control::ClientFrame;
    use zeroize::Zeroizing;

    fn eof() -> io::Result<ClientFrame> {
        Err(io::Error::from(io::ErrorKind::UnexpectedEof))
    }

    #[test]
    fn control_hangup_during_opening_is_a_cancellation() {
        for revents in [
            libc::POLLHUP,
            libc::POLLERR,
            libc::POLLNVAL,
            libc::POLLIN | libc::POLLHUP,
        ] {
            assert!(matches!(
                opening_event(revents, Ok(ClientFrame::Secret(Zeroizing::new([b'a'; 64])))),
                OpeningEvent::Cancelled
            ));
            assert!(matches!(
                opening_event(revents, eof()),
                OpeningEvent::Cancelled
            ));
        }
        assert!(matches!(
            opening_event(libc::POLLIN, eof()),
            OpeningEvent::Cancelled
        ));
        assert!(matches!(
            opening_event(libc::POLLIN, Ok(ClientFrame::Cancel)),
            OpeningEvent::Frame(ClientFrame::Cancel)
        ));
        assert!(matches!(
            opening_event(
                libc::POLLIN,
                Ok(ClientFrame::Secret(Zeroizing::new([b'a'; 64])))
            ),
            OpeningEvent::Frame(ClientFrame::Secret(_))
        ));
    }

    #[test]
    fn opening_requires_the_authority_the_keeper_sent() {
        use OpeningAuthority::{Permit, Reopened};
        assert_eq!(opening_step(Permit), EndpointSessionStep::Start);
        assert_eq!(opening_step(Reopened), EndpointSessionStep::Reopen);
        require_reported_authority(Permit, Some(Permit)).unwrap();
        require_reported_authority(Reopened, Some(Reopened)).unwrap();
        for (sent, reported) in [
            (Permit, Some(Reopened)),
            (Reopened, Some(Permit)),
            (Permit, None),
            (Reopened, None),
        ] {
            assert!(
                require_reported_authority(sent, reported).is_err(),
                "{sent:?} {reported:?}"
            );
        }
    }
}
