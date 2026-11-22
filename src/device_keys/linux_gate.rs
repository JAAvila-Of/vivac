use std::io;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Closed,
    Busy,
    Cancelled,
    LeaseInvalidated,
    Backend,
    Uncertain,
    Timeout,
    Poisoned,
    Reentrant,
}

pub type Proof = [u8; 64];

pub trait CloseResource: Send + Sync {
    fn stop(&self, deadline: Instant) -> Result<(), Error>;
    fn release(&self, deadline: Instant) -> Result<(), Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Ready,
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestOutcome {
    Published,
    Cancelled,
    LeaseInvalidated,
    Failed,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub state: State,
    pub epoch: u64,
    pub id: u64,
    pub active: bool,
    pub native_requests_sent: u64,
    pub native_replies_received: u64,
    pub proofs_verified: u64,
    pub proofs_published: u64,
    pub lock_callers: u64,
    pub lock_acknowledgments: u64,
    pub busy_refused: u64,
    pub closed_refused: u64,
    pub workers_joined: u64,
    pub close_acknowledged: bool,
    pub resources_closed_after_drain: bool,
    pub drain_waiting: bool,
    pub outcome: Option<RequestOutcome>,
}

#[derive(Default)]
struct Events {
    sent: bool,
    reply: bool,
    verified: bool,
    invalid: bool,
}

struct Data {
    snapshot: Snapshot,
    worker: Option<JoinHandle<()>>,
    worker_id: Option<ThreadId>,
    proof: Option<Proof>,
    events: Events,
    uncertain: bool,
    custody: Option<Arc<Gate>>,
}

impl Data {
    fn uncertain(&mut self) {
        if self.snapshot.state == State::Ready {
            self.snapshot.epoch = self.snapshot.epoch.saturating_add(1);
        }
        self.snapshot.state = State::Closing;
        self.snapshot.outcome = Some(RequestOutcome::Uncertain);
        self.uncertain = true;
    }

    fn increment(&mut self, field: fn(&mut Snapshot) -> &mut u64) -> Result<(), Error> {
        let counter = field(&mut self.snapshot);
        match counter.checked_add(1) {
            Some(value) => {
                *counter = value;
                Ok(())
            }
            None => {
                self.uncertain();
                Err(Error::Uncertain)
            }
        }
    }

    fn close_admission(&mut self) -> Result<(), Error> {
        if self.snapshot.state == State::Ready {
            self.snapshot.state = State::Closing;
            self.increment(|state| &mut state.epoch)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct JoinState {
    completed: Option<(u64, bool)>,
}

#[derive(Default)]
struct Cleanup {
    stopped: bool,
    released: bool,
    uncertain: bool,
}

pub struct Gate {
    data: Mutex<Data>,
    joining: Mutex<JoinState>,
    cleanup: Mutex<Cleanup>,
    resource: Arc<dyn CloseResource>,
}

pub struct Request {
    gate: Arc<Gate>,
    epoch: u64,
    id: u64,
}

fn deadline(timeout: Duration) -> Result<Instant, Error> {
    Instant::now().checked_add(timeout).ok_or(Error::Timeout)
}

fn pause(until: Instant) -> Result<(), Error> {
    let remaining = until
        .checked_duration_since(Instant::now())
        .ok_or(Error::Timeout)?;
    if remaining.is_zero() {
        return Err(Error::Timeout);
    }
    thread::sleep(remaining.min(Duration::from_millis(1)));
    Ok(())
}

fn acquire<T>(mutex: &Mutex<T>, until: Instant) -> Result<MutexGuard<'_, T>, Error> {
    loop {
        if Instant::now() >= until {
            return Err(Error::Timeout);
        }
        match mutex.try_lock() {
            Ok(guard) => {
                if Instant::now() >= until {
                    return Err(Error::Timeout);
                }
                return Ok(guard);
            }
            Err(TryLockError::Poisoned(_)) => return Err(Error::Poisoned),
            Err(TryLockError::WouldBlock) => pause(until)?,
        }
    }
}

impl Gate {
    pub fn new(resource: Arc<dyn CloseResource>) -> Arc<Self> {
        let gate = Arc::new(Self {
            data: Mutex::new(Data {
                snapshot: Snapshot {
                    state: State::Ready,
                    epoch: 0,
                    id: 0,
                    active: false,
                    native_requests_sent: 0,
                    native_replies_received: 0,
                    proofs_verified: 0,
                    proofs_published: 0,
                    lock_callers: 0,
                    lock_acknowledgments: 0,
                    busy_refused: 0,
                    closed_refused: 0,
                    workers_joined: 0,
                    close_acknowledged: false,
                    resources_closed_after_drain: false,
                    drain_waiting: false,
                    outcome: None,
                },
                worker: None,
                worker_id: None,
                proof: None,
                events: Events::default(),
                uncertain: false,
                custody: None,
            }),
            joining: Mutex::new(JoinState::default()),
            cleanup: Mutex::new(Cleanup::default()),
            resource,
        });
        // The cycle is broken only after observed drain and resource release.
        gate.data.lock().expect("new gate mutex").custody = Some(gate.clone());
        gate
    }

    pub fn start(
        self: &Arc<Self>,
        work: impl FnOnce(Request) -> Result<Proof, Error> + Send + 'static,
    ) -> Result<u64, Error> {
        self.start_endpoint(move |request| work(request).map(Some))
    }

    pub(crate) fn start_endpoint(
        self: &Arc<Self>,
        work: impl FnOnce(Request) -> Result<Option<Proof>, Error> + Send + 'static,
    ) -> Result<u64, Error> {
        let mut data = self.data.lock().map_err(|_| Error::Poisoned)?;
        if data.snapshot.state != State::Ready {
            data.increment(|state| &mut state.closed_refused)?;
            return Err(Error::Closed);
        }
        if data.snapshot.active || data.proof.is_some() {
            data.increment(|state| &mut state.busy_refused)?;
            return Err(Error::Busy);
        }
        data.increment(|state| &mut state.id)?;
        let id = data.snapshot.id;
        let epoch = data.snapshot.epoch;
        data.events = Events::default();
        data.snapshot.outcome = None;
        data.snapshot.active = true;
        let gate = self.clone();
        match thread::Builder::new().spawn(move || {
            let result = work(Request {
                gate: gate.clone(),
                epoch,
                id,
            });
            gate.finish(id, epoch, result);
        }) {
            Ok(worker) => {
                data.worker_id = Some(worker.thread().id());
                data.worker = Some(worker);
                Ok(id)
            }
            Err(_) => {
                data.snapshot.active = false;
                data.snapshot.outcome = Some(RequestOutcome::Failed);
                Err(Error::Backend)
            }
        }
    }

    fn finish(&self, id: u64, epoch: u64, result: Result<Option<Proof>, Error>) {
        let Ok(mut data) = self.data.lock() else {
            return;
        };
        if data.snapshot.id != id || !data.snapshot.active {
            data.uncertain();
            return;
        }
        if data.uncertain {
            data.snapshot.outcome = Some(RequestOutcome::Uncertain);
            return;
        }
        let outcome = match result {
            Err(Error::Uncertain | Error::Poisoned) => {
                data.uncertain();
                return;
            }
            Err(Error::LeaseInvalidated) => {
                if data.close_admission().is_err() {
                    return;
                }
                RequestOutcome::LeaseInvalidated
            }
            _ if data.snapshot.state != State::Ready || data.snapshot.epoch != epoch => {
                RequestOutcome::Cancelled
            }
            Ok(Some(proof)) if data.events.verified && !data.events.invalid => {
                if data.increment(|state| &mut state.proofs_published).is_err() {
                    return;
                }
                data.proof = Some(proof);
                RequestOutcome::Published
            }
            Ok(None)
                if !data.events.invalid
                    && (!data.events.sent || data.events.reply && data.events.verified) =>
            {
                data.snapshot.outcome = Some(RequestOutcome::Cancelled);
                return;
            }
            Err(Error::Cancelled) => RequestOutcome::Cancelled,
            _ => RequestOutcome::Failed,
        };
        if matches!(outcome, RequestOutcome::Failed | RequestOutcome::Cancelled)
            && data.close_admission().is_err()
        {
            return;
        }
        data.snapshot.outcome = Some(outcome);
    }

    fn join_until(&self, until: Instant) -> Result<(), Error> {
        let mut joining = acquire(&self.joining, until)?;
        loop {
            let mut data = acquire(&self.data, until)?;
            if let Some((id, succeeded)) = joining.completed {
                if data.snapshot.id != id || !data.snapshot.active {
                    data.uncertain();
                    return Err(Error::Uncertain);
                }
                data.snapshot.active = false;
                data.snapshot.drain_waiting = false;
                data.worker_id = None;
                joining.completed = None;
                data.increment(|state| &mut state.workers_joined)?;
                if !succeeded || data.snapshot.outcome.is_none() {
                    data.uncertain();
                }
                return Ok(());
            }
            if !data.snapshot.active {
                return Ok(());
            }
            let Some(worker) = data.worker.as_ref() else {
                data.uncertain();
                return Err(Error::Uncertain);
            };
            if worker.is_finished() {
                let id = data.snapshot.id;
                let worker = data.worker.take().expect("finished worker exists");
                drop(data);
                let succeeded = worker.join().is_ok();
                // Preserve a completed join across a deadline before state can be reacquired.
                joining.completed = Some((id, succeeded));
            } else {
                drop(data);
                pause(until)?;
            }
        }
    }

    pub fn wait_request(&self, id: u64, timeout: Duration) -> Result<RequestOutcome, Error> {
        let until = deadline(timeout)?;
        {
            let data = acquire(&self.data, until)?;
            if id == 0 || id != data.snapshot.id {
                return Err(Error::Cancelled);
            }
            if data.worker_id == Some(thread::current().id()) {
                return Err(Error::Reentrant);
            }
        }
        self.join_until(until)?;
        let data = acquire(&self.data, until)?;
        if id != data.snapshot.id {
            return Err(Error::Cancelled);
        }
        data.snapshot.outcome.ok_or(Error::Uncertain)
    }

    pub fn take_proof(&self, id: u64) -> Result<Option<Proof>, Error> {
        let mut data = self.data.lock().map_err(|_| Error::Poisoned)?;
        if id == 0 || id != data.snapshot.id {
            return Err(Error::Cancelled);
        }
        Ok(data.proof.take())
    }

    pub(crate) fn take_ready_proof(&self, id: u64, epoch: u64) -> Result<Option<Proof>, Error> {
        let mut data = self.data.lock().map_err(|_| Error::Poisoned)?;
        if id == 0
            || id != data.snapshot.id
            || epoch != data.snapshot.epoch
            || data.snapshot.state != State::Ready
            || data.snapshot.active
            || data.snapshot.outcome != Some(RequestOutcome::Published)
        {
            return Err(Error::Cancelled);
        }
        Ok(data.proof.take())
    }

    /// Orders one nonblocking endpoint send against admission closure.
    pub(crate) fn publish_endpoint_frame(
        &self,
        id: u64,
        epoch: u64,
        send: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let data = self.data.lock().map_err(|_| io::ErrorKind::InvalidData)?;
        if id == 0
            || data.snapshot.id != id
            || data.snapshot.epoch != epoch
            || data.snapshot.state != State::Ready
            || data.snapshot.active
            || data.snapshot.outcome != Some(RequestOutcome::Published)
            || data.proof.is_some()
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let result = send();
        drop(data);
        result
    }

    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        Ok(self.data.lock().map_err(|_| Error::Poisoned)?.snapshot)
    }

    /// Stops endpoint admission without draining or acknowledging native cleanup.
    pub fn stop_endpoint_admission(&self, timeout: Duration) -> Result<(), Error> {
        let until = deadline(timeout)?;
        let mut data = acquire(&self.data, until)?;
        if data.worker_id == Some(thread::current().id()) {
            return Err(Error::Reentrant);
        }
        data.close_admission()
    }

    pub fn lock(&self, timeout: Duration) -> Result<(), Error> {
        let until = deadline(timeout)?;
        {
            let mut data = acquire(&self.data, until)?;
            if data.worker_id == Some(thread::current().id()) {
                return Err(Error::Reentrant);
            }
            data.increment(|state| &mut state.lock_callers)?;
            data.close_admission()?;
        }
        let mut cleanup = acquire(&self.cleanup, until)?;
        {
            let mut data = acquire(&self.data, until)?;
            if data.snapshot.state == State::Closed {
                let acknowledgments = data.snapshot.lock_acknowledgments;
                data.increment(|state| &mut state.lock_acknowledgments)?;
                if Instant::now() >= until {
                    data.snapshot.lock_acknowledgments = acknowledgments;
                    return Err(Error::Timeout);
                }
                return Ok(());
            }
        }
        if cleanup.uncertain {
            let mut data = acquire(&self.data, until)?;
            data.uncertain();
            return Err(Error::Uncertain);
        }
        if !cleanup.stopped {
            if Instant::now() >= until {
                return Err(Error::Timeout);
            }
            self.resource_result(self.resource.stop(until), &mut cleanup)?;
            cleanup.stopped = true;
        }
        {
            let mut data = acquire(&self.data, until)?;
            data.snapshot.drain_waiting = data.snapshot.active;
        }
        self.join_until(until)?;
        {
            let data = acquire(&self.data, until)?;
            if data.uncertain {
                return Err(Error::Uncertain);
            }
        }
        if !cleanup.released {
            if Instant::now() >= until {
                return Err(Error::Timeout);
            }
            self.resource_result(self.resource.release(until), &mut cleanup)?;
            cleanup.released = true;
        }
        let custody = {
            let mut data = acquire(&self.data, until)?;
            if data.uncertain || data.snapshot.active {
                return Err(Error::Uncertain);
            }
            let previous = data.snapshot;
            data.increment(|state| &mut state.lock_acknowledgments)?;
            data.snapshot.resources_closed_after_drain = true;
            data.snapshot.close_acknowledged = true;
            data.snapshot.state = State::Closed;
            if Instant::now() >= until {
                data.snapshot = previous;
                return Err(Error::Timeout);
            }
            data.custody.take()
        };
        drop(cleanup);
        drop(custody);
        Ok(())
    }

    fn resource_result(
        &self,
        result: Result<(), Error>,
        cleanup: &mut Cleanup,
    ) -> Result<(), Error> {
        if matches!(result, Err(Error::Uncertain | Error::Poisoned)) {
            cleanup.uncertain = true;
            // A callback may already have exhausted its deadline. Never wait here.
            if let Ok(mut data) = self.data.try_lock() {
                data.uncertain();
            }
            return Err(Error::Uncertain);
        }
        result
    }
}

impl Request {
    pub fn check(&self) -> Result<(), Error> {
        let data = self.gate.data.lock().map_err(|_| Error::Poisoned)?;
        if data.snapshot.id != self.id
            || !data.snapshot.active
            || data.snapshot.state != State::Ready
            || data.snapshot.epoch != self.epoch
        {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    fn event(&self, step: u8) -> Result<(), Error> {
        let mut data = self.gate.data.lock().map_err(|_| Error::Poisoned)?;
        if data.snapshot.id != self.id || !data.snapshot.active {
            return Err(Error::Cancelled);
        }
        let valid = !data.events.invalid
            && match step {
                0 => !data.events.sent,
                1 => data.events.sent && !data.events.reply,
                2 => data.events.reply && !data.events.verified,
                _ => false,
            };
        if !valid {
            data.events.invalid = true;
            return Err(Error::Backend);
        }
        match step {
            0 => {
                data.increment(|state| &mut state.native_requests_sent)?;
                data.events.sent = true;
            }
            1 => {
                data.increment(|state| &mut state.native_replies_received)?;
                data.events.reply = true;
            }
            _ => {
                data.increment(|state| &mut state.proofs_verified)?;
                data.events.verified = true;
            }
        }
        Ok(())
    }

    pub fn native_sent(&self) -> Result<(), Error> {
        self.event(0)
    }
    pub fn native_reply(&self) -> Result<(), Error> {
        self.event(1)
    }
    pub fn verified(&self) -> Result<(), Error> {
        self.event(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;

    #[derive(Default)]
    struct Resource {
        stops: AtomicU64,
        releases: AtomicU64,
    }

    impl CloseResource for Resource {
        fn stop(&self, _: Instant) -> Result<(), Error> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn release(&self, _: Instant) -> Result<(), Error> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !predicate() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    fn proof(request: Request, value: u8) -> Result<Proof, Error> {
        request.native_sent()?;
        request.native_reply()?;
        request.verified()?;
        Ok([value; 64])
    }

    #[test]
    fn endpoint_discard_preserves_ready_only_before_send_or_after_verified_reply() {
        for completed in [false, true] {
            let resource = Arc::new(Resource::default());
            let gate = Gate::new(resource.clone());
            let epoch = gate.snapshot().unwrap().epoch;
            let id = gate
                .start_endpoint(move |request| {
                    if completed {
                        request.native_sent()?;
                        request.native_reply()?;
                        request.verified()?;
                    }
                    Ok(None)
                })
                .unwrap();
            assert_eq!(
                gate.wait_request(id, Duration::from_secs(2)),
                Ok(RequestOutcome::Cancelled)
            );
            let state = gate.snapshot().unwrap();
            assert_eq!(state.state, State::Ready);
            assert_eq!(state.epoch, epoch);
            assert!(!state.active && !state.close_acknowledged);
            assert_eq!(state.proofs_published, 0);
            assert_eq!(gate.take_proof(id), Ok(None));
            assert!(gate
                .publish_endpoint_frame(id, epoch, || panic!("discarded"))
                .is_err());
            assert_eq!(resource.stops.load(Ordering::SeqCst), 0);
            let next = gate
                .start_endpoint(|request| proof(request, 4).map(Some))
                .unwrap();
            assert_eq!(
                gate.wait_request(next, Duration::from_secs(2)),
                Ok(RequestOutcome::Published)
            );
            assert_eq!(gate.take_ready_proof(next, epoch), Ok(Some([4; 64])));
            gate.lock(Duration::from_secs(2)).unwrap();
        }
    }

    #[test]
    fn endpoint_discard_requires_a_verified_drain_after_native_send() {
        for replied in [false, true] {
            let gate = Gate::new(Arc::new(Resource::default()));
            let id = gate
                .start_endpoint(move |request| {
                    request.native_sent()?;
                    if replied {
                        request.native_reply()?;
                    }
                    Ok(None)
                })
                .unwrap();
            assert_eq!(
                gate.wait_request(id, Duration::from_secs(2)),
                Ok(RequestOutcome::Failed)
            );
            assert_eq!(gate.snapshot().unwrap().state, State::Closing);
            assert_eq!(gate.take_proof(id), Ok(None));
            gate.lock(Duration::from_secs(2)).unwrap();
        }
    }

    #[test]
    fn endpoint_discard_cannot_hide_an_invalid_native_event() {
        let gate = Gate::new(Arc::new(Resource::default()));
        let id = gate
            .start_endpoint(|request| {
                request.native_sent()?;
                request.native_reply()?;
                request.verified()?;
                assert_eq!(request.verified(), Err(Error::Backend));
                Ok(None)
            })
            .unwrap();
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::Failed)
        );
        assert_eq!(gate.snapshot().unwrap().state, State::Closing);
        assert_eq!(gate.take_proof(id), Ok(None));
        gate.lock(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn stopping_endpoint_admission_prevents_a_prepared_proof_send_without_cleanup() {
        let resource = Arc::new(Resource::default());
        let gate = Gate::new(resource.clone());
        let epoch = gate.snapshot().unwrap().epoch;
        let id = gate
            .start_endpoint(|request| proof(request, 2).map(Some))
            .unwrap();
        gate.wait_request(id, Duration::from_secs(2)).unwrap();
        assert_eq!(gate.take_ready_proof(id, epoch), Ok(Some([2; 64])));
        let observer = gate.clone();
        let closer = thread::spawn(move || {
            observer
                .stop_endpoint_admission(Duration::from_secs(2))
                .unwrap();
        });
        closer.join().unwrap();
        assert!(gate
            .publish_endpoint_frame(id, epoch, || panic!("closed"))
            .is_err());
        let state = gate.snapshot().unwrap();
        assert_eq!(state.state, State::Closing);
        assert_eq!(state.epoch, epoch + 1);
        assert_eq!(state.lock_callers, 0);
        assert_eq!(state.lock_acknowledgments, 0);
        assert!(!state.close_acknowledged && !state.resources_closed_after_drain);
        assert_eq!(resource.stops.load(Ordering::SeqCst), 0);
        assert_eq!(resource.releases.load(Ordering::SeqCst), 0);
        gate.lock(Duration::from_secs(2)).unwrap();
        assert_eq!(resource.releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stopping_endpoint_admission_invalidates_an_active_worker_before_discard() {
        let resource = Arc::new(Resource::default());
        let gate = Gate::new(resource.clone());
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker_gate = gate.clone();
        let id = gate
            .start_endpoint(move |request| {
                assert_eq!(
                    worker_gate.stop_endpoint_admission(Duration::from_secs(1)),
                    Err(Error::Reentrant)
                );
                request.native_sent()?;
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(2)).unwrap();
                request.native_reply()?;
                request.verified()?;
                Ok(None)
            })
            .unwrap();
        entry.recv_timeout(Duration::from_secs(2)).unwrap();
        gate.stop_endpoint_admission(Duration::from_secs(2))
            .unwrap();
        let state = gate.snapshot().unwrap();
        assert_eq!(state.state, State::Closing);
        assert!(state.active);
        assert_eq!(resource.stops.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::Cancelled)
        );
        assert_eq!(gate.take_proof(id), Ok(None));
        assert_eq!(gate.start_endpoint(|_| Ok(None)), Err(Error::Closed));
        gate.lock(Duration::from_secs(2)).unwrap();
        assert_eq!(resource.releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn closing_cancels_a_result_produced_after_the_epoch_changed() {
        let resource = Arc::new(Resource::default());
        let gate = Gate::new(resource.clone());
        let epoch = gate.snapshot().unwrap().epoch;
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let id = gate
            .start(move |_| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                Ok([7; 64])
            })
            .unwrap();
        entry.recv_timeout(Duration::from_secs(2)).unwrap();
        let closer = gate.clone();
        let close = std::thread::spawn(move || closer.lock(Duration::from_secs(2)));
        until(|| gate.snapshot().unwrap().state == State::Closing);
        release.send(()).unwrap();
        assert_eq!(close.join().unwrap(), Ok(()));
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(1)),
            Ok(RequestOutcome::Cancelled)
        );
        assert_eq!(gate.take_proof(id), Ok(None));
        assert_eq!(gate.take_ready_proof(id, epoch), Err(Error::Cancelled));
        assert_eq!(gate.snapshot().unwrap().proofs_published, 0);
        assert_eq!(resource.releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ready_proof_requires_the_exact_epoch_and_observed_join() {
        let gate = Gate::new(Arc::new(Resource::default()));
        let epoch = gate.snapshot().unwrap().epoch;
        let id = gate.start(|request| proof(request, 4)).unwrap();
        until(|| gate.snapshot().unwrap().outcome == Some(RequestOutcome::Published));
        assert_eq!(gate.take_ready_proof(id, epoch), Err(Error::Cancelled));
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::Published)
        );
        assert_eq!(gate.take_ready_proof(0, epoch), Err(Error::Cancelled));
        assert_eq!(gate.take_ready_proof(id + 1, epoch), Err(Error::Cancelled));
        assert_eq!(gate.take_ready_proof(id, epoch + 1), Err(Error::Cancelled));
        assert!(gate
            .publish_endpoint_frame(id, epoch, || panic!("unsent"))
            .is_err());
        assert_eq!(gate.take_ready_proof(id, epoch), Ok(Some([4; 64])));
        assert_eq!(gate.take_ready_proof(id, epoch), Ok(None));
        let sent = std::cell::Cell::new(false);
        gate.publish_endpoint_frame(id, epoch, || {
            sent.set(true);
            Ok(())
        })
        .unwrap();
        assert!(sent.get());
        gate.lock(Duration::from_secs(2)).unwrap();
        assert!(gate
            .publish_endpoint_frame(id, epoch, || panic!("closed"))
            .is_err());
    }

    #[test]
    fn ready_proof_refuses_closed_epoch_while_the_legacy_slot_survives() {
        let gate = Gate::new(Arc::new(Resource::default()));
        let epoch = gate.snapshot().unwrap().epoch;
        let id = gate.start(|request| proof(request, 8)).unwrap();
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::Published)
        );
        gate.lock(Duration::from_secs(2)).unwrap();
        assert_eq!(gate.take_ready_proof(id, epoch), Err(Error::Cancelled));
        let closed_epoch = gate.snapshot().unwrap().epoch;
        assert_eq!(
            gate.take_ready_proof(id, closed_epoch),
            Err(Error::Cancelled)
        );
        assert_eq!(gate.take_proof(id), Ok(Some([8; 64])));
    }

    #[test]
    fn a_published_slot_stays_busy_until_taken_and_survives_lock() {
        let gate = Gate::new(Arc::new(Resource::default()));
        let id = gate.start(|request| proof(request, 9)).unwrap();
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::Published)
        );
        assert_eq!(gate.start(|_| Ok([0; 64])), Err(Error::Busy));
        assert_eq!(gate.lock(Duration::from_secs(2)), Ok(()));
        assert_eq!(gate.take_proof(id), Ok(Some([9; 64])));
        assert_eq!(gate.take_proof(id), Ok(None));
        assert_eq!(gate.start(|_| Ok([0; 64])), Err(Error::Closed));
    }

    #[test]
    fn completion_does_not_clear_active_before_the_join_is_observed() {
        let gate = Gate::new(Arc::new(Resource::default()));
        let id = gate.start(|request| proof(request, 1)).unwrap();
        until(|| gate.snapshot().unwrap().outcome.is_some());
        assert!(gate.snapshot().unwrap().active);
        assert_eq!(gate.take_proof(id), Ok(Some([1; 64])));
        assert_eq!(gate.start(|_| Ok([2; 64])), Err(Error::Busy));
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::Published)
        );
        assert!(!gate.snapshot().unwrap().active);
        assert_eq!(gate.snapshot().unwrap().workers_joined, 1);
        gate.lock(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn parallel_lock_callers_share_cleanup_and_each_receive_an_ack() {
        let resource = Arc::new(Resource::default());
        let gate = Gate::new(resource.clone());
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut closers = Vec::new();
        for _ in 0..2 {
            let gate = gate.clone();
            let barrier = barrier.clone();
            closers.push(std::thread::spawn(move || {
                barrier.wait();
                gate.lock(Duration::from_secs(2))
            }));
        }
        barrier.wait();
        for closer in closers {
            assert_eq!(closer.join().unwrap(), Ok(()));
        }
        let state = gate.snapshot().unwrap();
        assert_eq!(state.lock_callers, 2);
        assert_eq!(state.lock_acknowledgments, 2);
        assert_eq!(resource.stops.load(Ordering::SeqCst), 1);
        assert_eq!(resource.releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn timeout_retains_the_worker_and_resources_until_a_successful_retry() {
        let resource = Arc::new(Resource::default());
        let gate = Gate::new(resource.clone());
        let (release, released) = mpsc::channel();
        let id = gate
            .start(move |_| {
                released.recv().unwrap();
                Ok([3; 64])
            })
            .unwrap();
        assert_eq!(gate.lock(Duration::from_millis(20)), Err(Error::Timeout));
        let state = gate.snapshot().unwrap();
        assert!(state.active && state.drain_waiting && !state.close_acknowledged);
        assert_eq!(state.state, State::Closing);
        assert_eq!(resource.releases.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
        gate.lock(Duration::from_secs(2)).unwrap();
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(1)),
            Ok(RequestOutcome::Cancelled)
        );
        assert_eq!(resource.releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn worker_panic_or_uncertainty_permanently_prevents_release_and_ack() {
        for panic in [false, true] {
            let resource = Arc::new(Resource::default());
            let gate = Gate::new(resource.clone());
            let id = gate
                .start(move |_| {
                    assert!(!panic, "controlled worker panic");
                    Err(Error::Uncertain)
                })
                .unwrap();
            assert_eq!(
                gate.wait_request(id, Duration::from_secs(2)),
                Ok(RequestOutcome::Uncertain)
            );
            assert_eq!(gate.lock(Duration::from_secs(1)), Err(Error::Uncertain));
            let state = gate.snapshot().unwrap();
            assert!(!state.active && !state.close_acknowledged);
            assert_eq!(state.state, State::Closing);
            assert_eq!(resource.releases.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn a_worker_cannot_lock_its_own_gate_or_close_admission() {
        let gate = Gate::new(Arc::new(Resource::default()));
        let worker_gate = gate.clone();
        let id = gate
            .start(move |request| {
                assert_eq!(
                    worker_gate.lock(Duration::from_secs(1)),
                    Err(Error::Reentrant)
                );
                let state = worker_gate.snapshot()?;
                assert_eq!(state.state, State::Ready);
                assert_eq!(state.lock_callers, 0);
                proof(request, 4)
            })
            .unwrap();
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::Published)
        );
        gate.lock(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn missing_or_repeated_verification_events_cannot_publish_a_proof() {
        for duplicate in [false, true] {
            let gate = Gate::new(Arc::new(Resource::default()));
            let id = gate
                .start(move |request| {
                    if duplicate {
                        request.native_sent()?;
                        request.native_reply()?;
                        request.verified()?;
                        assert_eq!(request.verified(), Err(Error::Backend));
                    }
                    Ok([5; 64])
                })
                .unwrap();
            assert_eq!(
                gate.wait_request(id, Duration::from_secs(2)),
                Ok(RequestOutcome::Failed)
            );
            assert_eq!(gate.take_proof(id), Ok(None));
            assert_eq!(gate.snapshot().unwrap().proofs_published, 0);
            assert_eq!(gate.snapshot().unwrap().state, State::Closing);
            assert_eq!(gate.start(|request| proof(request, 1)), Err(Error::Closed));
            gate.lock(Duration::from_secs(2)).unwrap();
        }
    }

    #[test]
    fn backend_errors_and_unsolicited_cancellation_close_admission_until_lock() {
        for error in [Error::Backend, Error::Timeout, Error::Cancelled] {
            let resource = Arc::new(Resource::default());
            let gate = Gate::new(resource.clone());
            let id = gate.start(move |_| Err(error)).unwrap();
            let outcome = if error == Error::Cancelled {
                RequestOutcome::Cancelled
            } else {
                RequestOutcome::Failed
            };
            assert_eq!(gate.wait_request(id, Duration::from_secs(2)), Ok(outcome));
            let state = gate.snapshot().unwrap();
            assert_eq!(state.state, State::Closing);
            assert!(!state.close_acknowledged && !state.resources_closed_after_drain);
            assert_eq!(resource.releases.load(Ordering::SeqCst), 0);
            assert_eq!(gate.start(|request| proof(request, 1)), Err(Error::Closed));
            gate.lock(Duration::from_secs(2)).unwrap();
            assert_eq!(gate.snapshot().unwrap().state, State::Closed);
            assert_eq!(resource.releases.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn lease_invalidation_closes_admission_but_requires_an_explicit_lock() {
        let resource = Arc::new(Resource::default());
        let gate = Gate::new(resource.clone());
        let id = gate.start(|_| Err(Error::LeaseInvalidated)).unwrap();
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::LeaseInvalidated)
        );
        assert_eq!(gate.snapshot().unwrap().state, State::Closing);
        assert_eq!(resource.stops.load(Ordering::SeqCst), 0);
        assert_eq!(gate.start(|request| proof(request, 1)), Err(Error::Closed));
        gate.lock(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn callback_uncertainty_cannot_be_turned_into_an_ack_by_retrying() {
        struct UncertainResource(AtomicU64);
        impl CloseResource for UncertainResource {
            fn stop(&self, _: Instant) -> Result<(), Error> {
                Err(Error::Uncertain)
            }
            fn release(&self, _: Instant) -> Result<(), Error> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let resource = Arc::new(UncertainResource(AtomicU64::new(0)));
        let gate = Gate::new(resource.clone());
        for _ in 0..2 {
            assert_eq!(gate.lock(Duration::from_secs(1)), Err(Error::Uncertain));
        }
        assert_eq!(resource.0.load(Ordering::SeqCst), 0);
        assert_eq!(gate.snapshot().unwrap().lock_acknowledgments, 0);
    }

    #[test]
    fn a_successful_release_after_the_deadline_does_not_produce_a_late_ack() {
        struct LateResource(AtomicU64);
        impl CloseResource for LateResource {
            fn stop(&self, _: Instant) -> Result<(), Error> {
                Ok(())
            }
            fn release(&self, until: Instant) -> Result<(), Error> {
                self.0.fetch_add(1, Ordering::SeqCst);
                while Instant::now() <= until {
                    std::thread::yield_now();
                }
                Ok(())
            }
        }
        let resource = Arc::new(LateResource(AtomicU64::new(0)));
        let gate = Gate::new(resource.clone());
        assert_eq!(gate.lock(Duration::from_millis(50)), Err(Error::Timeout));
        let state = gate.snapshot().unwrap();
        assert_eq!(state.state, State::Closing);
        assert!(!state.close_acknowledged);
        assert!(!state.resources_closed_after_drain);
        assert_eq!(resource.0.load(Ordering::SeqCst), 1);
        gate.lock(Duration::from_secs(1)).unwrap();
        assert_eq!(resource.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn counter_overflow_fails_closed_without_releasing_resources() {
        let resource = Arc::new(Resource::default());
        let gate = Gate::new(resource.clone());
        gate.data.lock().unwrap().snapshot.id = u64::MAX;
        assert_eq!(
            gate.start(|request| proof(request, 1)),
            Err(Error::Uncertain)
        );
        assert_eq!(gate.snapshot().unwrap().state, State::Closing);
        assert_eq!(gate.lock(Duration::from_secs(1)), Err(Error::Uncertain));
        assert_eq!(resource.releases.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dropping_callers_retains_custody_until_a_joined_close() {
        let gate = Gate::new(Arc::new(Resource::default()));
        let retained = Arc::downgrade(&gate);
        let (release, released) = mpsc::channel();
        let id = gate
            .start(move |request| {
                released.recv().unwrap();
                proof(request, 6)
            })
            .unwrap();
        drop(gate);
        assert!(retained.upgrade().is_some());
        release.send(()).unwrap();
        let gate = retained.upgrade().unwrap();
        assert_eq!(
            gate.wait_request(id, Duration::from_secs(2)),
            Ok(RequestOutcome::Published)
        );
        gate.lock(Duration::from_secs(2)).unwrap();
        drop(gate);
        assert!(retained.upgrade().is_none());
    }

    #[test]
    fn retryable_backend_errors_leave_closing_and_require_new_observed_success() {
        struct RetryResource {
            fail_stop: bool,
            stops: AtomicU64,
            releases: AtomicU64,
        }
        impl CloseResource for RetryResource {
            fn stop(&self, _: Instant) -> Result<(), Error> {
                let attempt = self.stops.fetch_add(1, Ordering::SeqCst);
                if self.fail_stop && attempt == 0 {
                    Err(Error::Backend)
                } else {
                    Ok(())
                }
            }
            fn release(&self, _: Instant) -> Result<(), Error> {
                let attempt = self.releases.fetch_add(1, Ordering::SeqCst);
                if !self.fail_stop && attempt == 0 {
                    Err(Error::Backend)
                } else {
                    Ok(())
                }
            }
        }
        for fail_stop in [false, true] {
            let resource = Arc::new(RetryResource {
                fail_stop,
                stops: AtomicU64::new(0),
                releases: AtomicU64::new(0),
            });
            let gate = Gate::new(resource.clone());
            assert_eq!(gate.lock(Duration::from_secs(1)), Err(Error::Backend));
            let state = gate.snapshot().unwrap();
            assert_eq!(state.state, State::Closing);
            assert!(!state.close_acknowledged && !state.resources_closed_after_drain);
            gate.lock(Duration::from_secs(1)).unwrap();
            assert_eq!(gate.snapshot().unwrap().lock_acknowledgments, 1);
            assert_eq!(
                resource.stops.load(Ordering::SeqCst),
                if fail_stop { 2 } else { 1 }
            );
            assert_eq!(
                resource.releases.load(Ordering::SeqCst),
                if fail_stop { 1 } else { 2 }
            );
        }
    }
}
