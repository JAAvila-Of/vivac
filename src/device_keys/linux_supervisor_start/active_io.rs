use super::PROOF_TIMEOUT;
use crate::linux_endpoint_runtime::{EndpointListener, EndpointRuntime};
use crate::linux_gate::{Gate, State};
use crate::linux_ipc::registration::await_session_keeper_close;
use crate::linux_ipc::ExpectedPeer;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MAX_PENDING: usize = 4;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const FAILED_EVENTS: i16 = libc::POLLHUP | libc::POLLERR | libc::POLLNVAL | libc::POLLRDHUP;

pub(super) struct Coordinator<'a> {
    pub controller: &'a OwnedFd,
    pub stop: &'a OwnedFd,
    pub runtime: &'a EndpointRuntime,
    pub listener: &'a EndpointListener,
    pub gate: &'a Gate,
    pub ready: mpsc::SyncSender<(OwnedFd, Instant)>,
    pub busy: &'a AtomicBool,
    pub stopping: &'a AtomicBool,
}

struct Pending {
    socket: OwnedFd,
    deadline: Instant,
}

fn refused() -> io::Error {
    io::ErrorKind::PermissionDenied.into()
}

pub(super) fn coordinate(coordinator: Coordinator<'_>) -> io::Result<()> {
    let result = coordinator.run();
    coordinator.stopping.store(true, Ordering::Release);
    let closed = coordinator
        .gate
        .stop_endpoint_admission(Duration::from_secs(1))
        .map_err(|_| refused());
    result.and(closed)
}

fn watched(fd: RawFd) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLRDHUP,
        revents: 0,
    }
}

impl Coordinator<'_> {
    fn run(&self) -> io::Result<()> {
        let mut pending: Vec<Pending> = Vec::with_capacity(MAX_PENDING);
        loop {
            if self.stopping.load(Ordering::Acquire)
                || self.gate.snapshot().map_err(|_| refused())?.state != State::Ready
            {
                return Err(refused());
            }
            let now = Instant::now();
            pending.retain(|connection| connection.deadline > now);
            let mut descriptors = Vec::with_capacity(3 + MAX_PENDING);
            descriptors.extend([
                watched(self.controller.as_raw_fd()),
                watched(self.stop.as_raw_fd()),
                watched(self.listener.as_raw_fd()),
            ]);
            descriptors.extend(
                pending
                    .iter()
                    .map(|connection| watched(connection.socket.as_raw_fd())),
            );
            let timeout = pending
                .iter()
                .map(|connection| {
                    connection
                        .deadline
                        .saturating_duration_since(now)
                        .as_millis()
                        .saturating_add(1)
                        .min(i32::MAX as u128) as i32
                })
                .min()
                .unwrap_or(-1);
            // SAFETY: all descriptors are initialized and writable for the declared length.
            let count =
                unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, timeout) };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }

            // Stop and control take precedence over every client event in this batch.
            if descriptors[1].revents != 0 || descriptors[0].revents & FAILED_EVENTS != 0 {
                return Err(refused());
            }
            if descriptors[0].revents & libc::POLLIN != 0 {
                return await_session_keeper_close(self.controller);
            }
            if descriptors[2].revents & FAILED_EVENTS != 0 {
                return Err(refused());
            }

            let now = Instant::now();
            for index in (0..pending.len()).rev() {
                let event = descriptors[index + 3].revents;
                if pending[index].deadline <= now || event & FAILED_EVENTS != 0 {
                    pending.swap_remove(index);
                } else if event & libc::POLLIN != 0 {
                    let connection = pending.swap_remove(index);
                    self.dispatch(connection.socket)?;
                }
            }
            if descriptors[2].revents & libc::POLLIN != 0 {
                self.accept(&mut pending)?;
            }
        }
    }

    fn accept(&self, pending: &mut Vec<Pending>) -> io::Result<()> {
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let socket = match self.runtime.accept(self.listener, deadline) {
            Ok(Some(socket)) => socket,
            Ok(None) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(error) => return Err(error),
        };
        if pending.len() < MAX_PENDING && ExpectedPeer::connected_identity(&socket).is_ok() {
            pending.push(Pending { socket, deadline });
        }
        Ok(())
    }

    fn dispatch(&self, socket: OwnedFd) -> io::Result<()> {
        if self.stopping.load(Ordering::Acquire)
            || self
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Ok(());
        }
        match self
            .ready
            .try_send((socket, Instant::now() + PROOF_TIMEOUT))
        {
            Ok(()) => Ok(()),
            Err(error) => {
                self.busy.store(false, Ordering::Release);
                match error {
                    mpsc::TrySendError::Full(_) => Ok(()),
                    mpsc::TrySendError::Disconnected(_) => Err(refused()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_gate::{CloseResource, Error, State};
    use crate::linux_ipc::packet_pair;
    use std::fs::File;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    struct EmptyResource;

    impl CloseResource for EmptyResource {
        fn stop(&self, _: Instant) -> Result<(), Error> {
            Ok(())
        }

        fn release(&self, _: Instant) -> Result<(), Error> {
            Ok(())
        }
    }

    #[test]
    fn local_stop_closes_admission_before_accepting_a_waiting_client() {
        let path = std::env::temp_dir().join(format!(
            "vivac-active-io-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = EndpointRuntime::from_directory(File::open(&path).unwrap().into()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let listener = runtime.bind_listener(deadline).unwrap();
        let client = runtime.connect_once(deadline).unwrap();
        let (controller, _keeper) = packet_pair().unwrap();
        let (stop, trigger) = packet_pair().unwrap();
        drop(trigger);
        let gate = Gate::new(Arc::new(EmptyResource));
        let (ready, receiver) = mpsc::sync_channel(1);
        let busy = AtomicBool::new(false);
        let stopping = AtomicBool::new(false);
        let result = coordinate(Coordinator {
            controller: &controller,
            stop: &stop,
            runtime: &runtime,
            listener: &listener,
            gate: &gate,
            ready,
            busy: &busy,
            stopping: &stopping,
        });
        assert!(result.is_err());
        assert!(stopping.load(Ordering::Acquire));
        assert!(!busy.load(Ordering::Acquire));
        assert!(receiver.try_recv().is_err());
        let snapshot = gate.snapshot().unwrap();
        assert_eq!(snapshot.state, State::Closing);
        assert!(!snapshot.close_acknowledged);
        gate.lock(Duration::from_secs(1)).unwrap();
        drop((client, listener, runtime));
        std::fs::remove_file(path.join("endpoint.sock")).unwrap();
        std::fs::remove_dir(path).unwrap();
    }
}
