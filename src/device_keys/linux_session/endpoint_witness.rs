use super::super::linux_gate::{Gate, State};
use super::{refused, require};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

#[cfg(test)]
pub(super) fn new_generation() -> io::Result<[u8; 32]> {
    super::super::linux_spawn::new_session_generation()
}

pub struct EndpointSessionWitness {
    gate: Weak<Gate>,
    lifetime: Weak<()>,
    epoch: u64,
    pid: u32,
    generation: [u8; 32],
    store: [u64; 6],
    invalidated: AtomicBool,
}

impl EndpointSessionWitness {
    pub(super) fn capture(
        gate: &Arc<Gate>,
        lifetime: &Arc<()>,
        generation: [u8; 32],
        store: [u64; 6],
    ) -> io::Result<Self> {
        require(generation.iter().any(|byte| *byte != 0))?;
        let snapshot = gate.snapshot().map_err(|_| refused())?;
        require(snapshot.state == State::Ready && !snapshot.active)?;
        let witness = Self {
            gate: Arc::downgrade(gate),
            lifetime: Arc::downgrade(lifetime),
            epoch: snapshot.epoch,
            pid: std::process::id(),
            generation,
            store,
            invalidated: AtomicBool::new(false),
        };
        witness.check_live()?;
        Ok(witness)
    }

    pub(super) fn checked<T>(&self, check: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        if self.invalidated.load(Ordering::Acquire) {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        match check() {
            Err(error) => {
                self.invalidated.store(true, Ordering::Release);
                Err(error)
            }
            Ok(value) => {
                if self.invalidated.load(Ordering::Acquire) {
                    Err(io::Error::from(io::ErrorKind::InvalidData))
                } else {
                    Ok(value)
                }
            }
        }
    }

    pub fn check_live(&self) -> io::Result<()> {
        self.checked(|| {
            let _lifetime = self.lifetime.upgrade().ok_or_else(refused)?;
            require(self.pid == std::process::id())?;
            let gate = self.gate.upgrade().ok_or_else(refused)?;
            let snapshot = gate.snapshot().map_err(|_| refused())?;
            require(snapshot.state == State::Ready && snapshot.epoch == self.epoch)
        })
    }

    pub fn generation(&self) -> io::Result<[u8; 32]> {
        self.check_live()?;
        let generation = self.generation;
        self.check_live()?;
        Ok(generation)
    }

    pub fn store_identity(&self) -> io::Result<[u64; 6]> {
        self.check_live()?;
        let store = self.store;
        self.check_live()?;
        Ok(store)
    }

    pub(super) fn validate_binding(
        &self,
        gate: &Arc<Gate>,
        generation: [u8; 32],
        store: [u64; 6],
    ) -> io::Result<()> {
        self.checked(|| {
            self.check_live()?;
            let captured = self.gate.upgrade().ok_or_else(refused)?;
            require(Arc::ptr_eq(&captured, gate))?;
            require(self.generation == generation && self.store == store)?;
            self.check_live()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::linux_gate::{CloseResource, Error, RequestOutcome};
    use super::*;
    use std::time::{Duration, Instant};

    struct EmptyResource;
    impl CloseResource for EmptyResource {
        fn stop(&self, _: Instant) -> Result<(), Error> {
            Ok(())
        }
        fn release(&self, _: Instant) -> Result<(), Error> {
            Ok(())
        }
    }
    fn fixture() -> (Arc<Gate>, Arc<()>, EndpointSessionWitness) {
        let gate = Gate::new(Arc::new(EmptyResource));
        let lifetime = Arc::new(());
        let witness =
            EndpointSessionWitness::capture(&gate, &lifetime, [7; 32], [1, 2, 3, 4, 5, 6]).unwrap();
        (gate, lifetime, witness)
    }

    #[test]
    fn endpoint_session_witness_refuses_reuse_after_a_failed_binding() {
        let (gate, _lifetime, witness) = fixture();
        assert!(witness
            .validate_binding(&gate, [8; 32], [1, 2, 3, 4, 5, 6])
            .is_err());
        assert_eq!(
            witness.check_live().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(witness.generation().is_err());
        assert!(witness.store_identity().is_err());
        gate.lock(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn endpoint_session_witness_refuses_recheck_after_preserving_the_first_native_error() {
        let (gate, _lifetime, witness) = fixture();
        let first = witness.checked(|| Err::<(), _>(io::Error::from_raw_os_error(libc::EXDEV)));
        assert_eq!(first.unwrap_err().raw_os_error(), Some(libc::EXDEV));
        let called = std::cell::Cell::new(false);
        let second = witness.checked(|| {
            called.set(true);
            Ok(())
        });
        assert_eq!(second.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(!called.get());
        assert!(witness.generation().is_err());
        assert!(witness.store_identity().is_err());
        gate.lock(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn endpoint_session_witness_refuses_wrong_gate_generation_and_store() {
        for changed in 0..3 {
            let (gate, _lifetime, witness) = fixture();
            let other = Gate::new(Arc::new(EmptyResource));
            let actual = if changed == 0 { &other } else { &gate };
            let generation = if changed == 1 { [8; 32] } else { [7; 32] };
            let store = if changed == 2 {
                [9; 6]
            } else {
                [1, 2, 3, 4, 5, 6]
            };
            assert!(witness.validate_binding(actual, generation, store).is_err());
            gate.lock(Duration::from_secs(1)).unwrap();
            other.lock(Duration::from_secs(1)).unwrap();
        }
    }

    #[test]
    fn endpoint_session_witness_refuses_wrong_pid_epoch_closed_and_dropped_gate() {
        for changed in 0..4 {
            let (gate, _lifetime, mut witness) = fixture();
            assert!(witness.check_live().is_ok());
            match changed {
                0 => witness.pid = witness.pid.wrapping_add(1),
                1 => witness.epoch = witness.epoch.wrapping_add(1),
                _ => gate.lock(Duration::from_secs(1)).unwrap(),
            }
            if changed == 3 {
                drop(gate);
                assert!(witness.gate.upgrade().is_none());
            } else {
                assert!(witness.check_live().is_err());
                if changed < 2 {
                    gate.lock(Duration::from_secs(1)).unwrap();
                }
            }
            assert!(witness.check_live().is_err());
        }
    }

    #[test]
    fn endpoint_session_witness_refuses_new_capture_during_work_but_keeps_existing_witness_live() {
        let (gate, lifetime, witness) = fixture();
        gate.start(move |request| loop {
            request.check()?;
            std::thread::sleep(Duration::from_millis(1));
        })
        .unwrap();
        assert!(gate.snapshot().unwrap().active);
        assert!(witness.check_live().is_ok());
        assert_eq!(witness.generation().unwrap(), [7; 32]);
        assert_eq!(witness.store_identity().unwrap(), [1, 2, 3, 4, 5, 6]);
        assert!(EndpointSessionWitness::capture(&gate, &lifetime, [7; 32], [1; 6]).is_err());
        gate.lock(Duration::from_secs(1)).unwrap();
        assert_eq!(
            gate.snapshot().unwrap().outcome,
            Some(RequestOutcome::Cancelled)
        );
        assert!(witness.check_live().is_err());
        assert!(EndpointSessionWitness::capture(&gate, &lifetime, [7; 32], [1; 6]).is_err());
    }

    #[test]
    fn endpoint_session_witness_refuses_a_dead_session_while_its_gate_remains_ready() {
        let (gate, lifetime, witness) = fixture();
        assert_eq!(Arc::strong_count(&lifetime), 1);
        assert!(witness.check_live().is_ok());
        drop(lifetime);
        assert!(witness.gate.upgrade().is_some());
        assert_eq!(gate.snapshot().unwrap().state, State::Ready);
        assert!(witness.check_live().is_err());
        assert_eq!(
            witness.check_live().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            witness.generation().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            witness.store_identity().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        gate.lock(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn endpoint_session_witness_generations_are_nonzero_and_distinct() {
        let first = new_generation().unwrap();
        let second = new_generation().unwrap();
        assert_ne!(first, [0; 32]);
        assert_ne!(second, [0; 32]);
        assert_ne!(first, second);
    }
}
