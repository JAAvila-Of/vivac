use super::{require, EndpointSessionWitness};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

struct SlotGuard {
    slot: Arc<AtomicBool>,
}

impl SlotGuard {
    fn reserve(slot: &Arc<AtomicBool>) -> io::Result<Self> {
        slot.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| io::Error::from(io::ErrorKind::WouldBlock))?;
        Ok(Self { slot: slot.clone() })
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.slot.store(false, Ordering::Release);
    }
}

pub struct EndpointProofAttempt {
    slot: SlotGuard,
    witness: EndpointSessionWitness,
    deadline: Instant,
    consumed: bool,
    invalidated: AtomicBool,
}

impl EndpointProofAttempt {
    pub(super) fn issue(
        slot: &Arc<AtomicBool>,
        caller_deadline: Instant,
        capture: impl FnOnce() -> io::Result<EndpointSessionWitness>,
    ) -> io::Result<Self> {
        let slot = SlotGuard::reserve(slot)?;
        let emitted = Instant::now();
        let deadline = caller_deadline.min(
            emitted
                .checked_add(Duration::from_secs(30))
                .ok_or_else(invalid)?,
        );
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        let attempt = Self {
            slot,
            witness: capture()?,
            deadline,
            consumed: false,
            invalidated: AtomicBool::new(false),
        };
        attempt.check_live()?;
        Ok(attempt)
    }

    fn checked<T>(&self, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        if self.invalidated.load(Ordering::Acquire) {
            return Err(invalid());
        }
        match operation() {
            Err(error) => {
                self.invalidated.store(true, Ordering::Release);
                Err(error)
            }
            Ok(value) => {
                if self.invalidated.load(Ordering::Acquire) {
                    Err(invalid())
                } else {
                    Ok(value)
                }
            }
        }
    }

    pub fn check_live(&self) -> io::Result<()> {
        self.checked(|| {
            if Instant::now() >= self.deadline {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            self.witness.check_live()?;
            if Instant::now() >= self.deadline {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            Ok(())
        })
    }

    pub(super) fn deadline(&self) -> io::Result<Instant> {
        self.check_live()?;
        let deadline = self.deadline;
        self.check_live()?;
        Ok(deadline)
    }

    pub(super) fn binding_values(&self) -> io::Result<([u8; 32], [u64; 6])> {
        self.checked(|| {
            self.check_live()?;
            let generation = self.witness.generation()?;
            let store = self.witness.store_identity()?;
            self.check_live()?;
            Ok((generation, store))
        })
    }

    pub(super) fn validate(
        &self,
        slot: &Arc<AtomicBool>,
        validate: impl FnOnce(&EndpointSessionWitness) -> io::Result<()>,
    ) -> io::Result<()> {
        self.checked(|| {
            self.check_live()?;
            require(Arc::ptr_eq(&self.slot.slot, slot))?;
            validate(&self.witness)?;
            self.check_live()
        })
    }

    pub(super) fn consume(
        &mut self,
        slot: &Arc<AtomicBool>,
        validate: impl FnOnce(&EndpointSessionWitness) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.consumed {
            self.invalidated.store(true, Ordering::Release);
            return Err(invalid());
        }
        self.consumed = true;
        self.validate(slot, validate)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::linux_gate::{CloseResource, Error, Gate, State};
    use super::*;

    struct EmptyResource;
    impl CloseResource for EmptyResource {
        fn stop(&self, _: Instant) -> Result<(), Error> {
            Ok(())
        }
        fn release(&self, _: Instant) -> Result<(), Error> {
            Ok(())
        }
    }
    struct Fixture {
        gate: Arc<Gate>,
        lifetime: Arc<()>,
        slot: Arc<AtomicBool>,
    }
    impl Fixture {
        fn new() -> Self {
            Self {
                gate: Gate::new(Arc::new(EmptyResource)),
                lifetime: Arc::new(()),
                slot: Arc::new(AtomicBool::new(false)),
            }
        }
        fn capture(&self) -> io::Result<EndpointSessionWitness> {
            EndpointSessionWitness::capture(&self.gate, &self.lifetime, [7; 32], [1; 6])
        }
        fn issue(&self) -> EndpointProofAttempt {
            EndpointProofAttempt::issue(
                &self.slot,
                Instant::now() + Duration::from_secs(10),
                || self.capture(),
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.gate.lock(Duration::from_secs(1)).unwrap();
        }
    }

    #[test]
    fn endpoint_attempt_occupied_slot_refuses_before_capture_or_deadline_validation() {
        let fixture = Fixture::new();
        let attempt = fixture.issue();
        let called = std::cell::Cell::new(false);
        let occupied = EndpointProofAttempt::issue(&fixture.slot, Instant::now(), || {
            called.set(true);
            fixture.capture()
        });
        assert_eq!(occupied.err().unwrap().kind(), io::ErrorKind::WouldBlock);
        assert!(!called.get());
        assert_eq!(Arc::strong_count(&fixture.lifetime), 1);
        drop(attempt);
        assert!(!fixture.slot.load(Ordering::Acquire));
        assert!(fixture.issue().check_live().is_ok());
    }

    #[test]
    fn endpoint_attempt_failed_construction_releases_only_its_reservation() {
        let fixture = Fixture::new();
        let failure = EndpointProofAttempt::issue(
            &fixture.slot,
            Instant::now() + Duration::from_secs(10),
            || Err(io::Error::from_raw_os_error(libc::EXDEV)),
        );
        assert_eq!(failure.err().unwrap().raw_os_error(), Some(libc::EXDEV));
        assert!(!fixture.slot.load(Ordering::Acquire));
        let expired =
            EndpointProofAttempt::issue(&fixture.slot, Instant::now(), || fixture.capture());
        assert_eq!(expired.err().unwrap().kind(), io::ErrorKind::TimedOut);
        assert!(!fixture.slot.load(Ordering::Acquire));
    }

    #[test]
    fn endpoint_attempt_obeys_caller_deadline_and_thirty_second_cap() {
        let fixture = Fixture::new();
        let caller = Instant::now() + Duration::from_secs(5);
        let attempt =
            EndpointProofAttempt::issue(&fixture.slot, caller, || fixture.capture()).unwrap();
        assert_eq!(attempt.deadline, caller);
        drop(attempt);
        let before = Instant::now();
        let capped =
            EndpointProofAttempt::issue(&fixture.slot, before + Duration::from_secs(300), || {
                fixture.capture()
            })
            .unwrap();
        let after = Instant::now();
        assert!(capped.deadline >= before + Duration::from_secs(30));
        assert!(capped.deadline <= after + Duration::from_secs(30));
    }

    #[test]
    fn endpoint_attempt_expiration_latches_without_releasing_or_renewing_slot() {
        let fixture = Fixture::new();
        let mut attempt = fixture.issue();
        attempt.deadline = Instant::now();
        assert_eq!(
            attempt.check_live().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        attempt.deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(
            attempt.check_live().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(fixture.slot.load(Ordering::Acquire));
        assert_eq!(
            EndpointProofAttempt::issue(&fixture.slot, Instant::now(), || fixture.capture())
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        drop(attempt);
        assert!(!fixture.slot.load(Ordering::Acquire));
    }

    #[test]
    fn endpoint_attempt_consumes_once_and_keeps_slot_until_drop() {
        let fixture = Fixture::new();
        let mut attempt = fixture.issue();
        attempt
            .consume(&fixture.slot, |witness| witness.check_live())
            .unwrap();
        assert!(attempt.check_live().is_ok());
        assert!(fixture.slot.load(Ordering::Acquire));
        let called = std::cell::Cell::new(false);
        let reused = attempt.consume(&fixture.slot, |_| {
            called.set(true);
            Ok(())
        });
        assert_eq!(reused.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(!called.get());
        assert_eq!(
            attempt.check_live().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(fixture.slot.load(Ordering::Acquire));
        drop(attempt);
        assert!(!fixture.slot.load(Ordering::Acquire));
    }

    #[test]
    fn endpoint_attempt_wrong_slot_or_witness_invalidates_after_consuming() {
        for wrong_slot in [true, false] {
            let fixture = Fixture::new();
            let other = Fixture::new();
            let mut attempt = fixture.issue();
            let slot = if wrong_slot {
                &other.slot
            } else {
                &fixture.slot
            };
            assert!(attempt
                .consume(slot, |witness| {
                    witness.validate_binding(&other.gate, [7; 32], [1; 6])
                })
                .is_err());
            assert!(attempt.consumed);
            assert_eq!(
                attempt.check_live().unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert!(fixture.slot.load(Ordering::Acquire));
            assert!(!other.slot.load(Ordering::Acquire));
            drop(attempt);
            assert!(!fixture.slot.load(Ordering::Acquire));
        }
    }

    #[test]
    fn endpoint_attempt_existing_attempt_survives_active_work_but_new_issuance_refuses() {
        let fixture = Fixture::new();
        let attempt = fixture.issue();
        fixture
            .gate
            .start(|request| loop {
                request.check()?;
                std::thread::sleep(Duration::from_millis(1));
            })
            .unwrap();
        assert!(fixture.gate.snapshot().unwrap().active);
        assert!(attempt.check_live().is_ok());
        let other_slot = Arc::new(AtomicBool::new(false));
        assert!(EndpointProofAttempt::issue(
            &other_slot,
            Instant::now() + Duration::from_secs(10),
            || fixture.capture(),
        )
        .is_err());
        assert!(!other_slot.load(Ordering::Acquire));
        fixture.gate.lock(Duration::from_secs(1)).unwrap();
        assert!(attempt.check_live().is_err());
        assert!(fixture.slot.load(Ordering::Acquire));
    }

    #[test]
    fn endpoint_attempt_refuses_closure_epoch_change_or_dead_session_with_gate_alive() {
        for close in [true, false] {
            let mut fixture = Fixture::new();
            let attempt = fixture.issue();
            if close {
                let epoch = fixture.gate.snapshot().unwrap().epoch;
                fixture.gate.lock(Duration::from_secs(1)).unwrap();
                assert_ne!(fixture.gate.snapshot().unwrap().epoch, epoch);
            } else {
                drop(std::mem::replace(&mut fixture.lifetime, Arc::new(())));
                assert_eq!(fixture.gate.snapshot().unwrap().state, State::Ready);
            }
            assert!(attempt.check_live().is_err());
            assert_eq!(
                attempt.check_live().unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert!(fixture.slot.load(Ordering::Acquire));
        }
    }
}
