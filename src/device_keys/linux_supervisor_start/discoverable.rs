use super::{active_io, refused, short_deadline};
use crate::linux_current_admission::CurrentAdmission;
use crate::linux_custody_root::optional_collection::ENDPOINTS;
use crate::linux_endpoint_runtime::EndpointRuntime;
use crate::linux_ipc::{packet_pair, registration};
use crate::linux_session::NativeSession;
use crate::ubuntu_provider::UbuntuProvider;
use std::io;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Instant;

const MAX_RESPONSES: usize = 4;

pub(super) fn serve(
    controller: &OwnedFd,
    credential: &str,
    session: &NativeSession,
    provider: &UbuntuProvider,
) -> io::Result<()> {
    provider.revalidate()?;
    session.endpoint_witness()?;
    let admission = CurrentAdmission::open_fixed_namespace(credential)?;
    admission.with_endpoint_location(|name, directory| {
        let runtime = EndpointRuntime::from_named_directory(directory, name)?;
        let listener = runtime.bind_listener(short_deadline())?;
        let (runtime_ref, listener_ref) = (&runtime, &listener);
        let check_location = || {
            admission.with_endpoint_location(|_, current| {
                runtime_ref.check_listener(listener_ref, &current)
            })
        };
        let result = (|| {
            check_location()?;
            let (stop, watch) = packet_pair()?;
            let (ready, incoming) = mpsc::sync_channel(1);
            let gate = session.gate();
            let busy = AtomicBool::new(false);
            let stopping = AtomicBool::new(false);
            let (busy_ref, stopping_ref, gate_ref) = (&busy, &stopping, &*gate);
            std::thread::scope(|scope| {
                let watcher = std::thread::Builder::new().spawn_scoped(scope, move || {
                    active_io::coordinate(active_io::Coordinator {
                        controller,
                        stop: &watch,
                        runtime: runtime_ref,
                        listener: listener_ref,
                        gate: gate_ref,
                        ready,
                        busy: busy_ref,
                        stopping: stopping_ref,
                        // An idle wake reads metadata only; every connection
                        // still runs the full location check below.
                        location: &|| runtime_ref.check_bound(listener_ref, ENDPOINTS.name),
                    })
                })?;
                let serving = (|| {
                    registration::send_session_child_step(
                        controller,
                        registration::EndpointSessionStep::Active,
                        short_deadline(),
                    )?;
                    let mut responses: Vec<registration::EndpointProofConnection> = Vec::new();
                    loop {
                        responses
                            .retain(registration::EndpointProofConnection::response_peer_is_live);
                        let next = if let Some(deadline) =
                            responses.iter().map(|item| item.deadline()).min()
                        {
                            match incoming
                                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                            {
                                Ok(request) => Some(request),
                                Err(RecvTimeoutError::Timeout) => continue,
                                Err(RecvTimeoutError::Disconnected) => None,
                            }
                        } else {
                            incoming.recv().ok()
                        };
                        let Some((socket, deadline)) = next else {
                            break;
                        };
                        if stopping.load(Ordering::Acquire) {
                            break;
                        }
                        responses
                            .retain(registration::EndpointProofConnection::response_peer_is_live);
                        if responses.len() == MAX_RESPONSES {
                            busy.store(false, Ordering::Release);
                            continue;
                        }
                        provider.revalidate()?;
                        check_location()?;
                        if let Ok(mut connection) =
                            registration::EndpointProofConnection::new(socket, deadline)
                        {
                            // Connection failures stay local as long as the
                            // provider and the session remain valid below.
                            let result = session.serve_endpoint_proof_checked(
                                &mut connection,
                                CurrentAdmission::open_fixed_namespace(credential)?,
                                || {
                                    provider.revalidate()?;
                                    check_location()?;
                                    if stopping.load(Ordering::Acquire) {
                                        return Err(refused());
                                    }
                                    Ok(())
                                },
                            );
                            if result.is_ok() {
                                responses.push(connection);
                                // The keeper's idle limit counts from the last served proof.
                                registration::send_session_child_step(
                                    controller,
                                    registration::EndpointSessionStep::Served,
                                    short_deadline(),
                                )?;
                            }
                        }
                        provider.revalidate()?;
                        check_location()?;
                        if let Err(error) = session.endpoint_witness() {
                            if !stopping.load(Ordering::Acquire) {
                                return Err(error);
                            }
                        }
                        busy.store(false, Ordering::Release);
                    }
                    Ok(())
                })();
                drop(stop);
                let control = watcher
                    .join()
                    .map_err(|_| io::Error::other("Endpoint control thread failed."))?;
                serving.and(control)
            })
        })();
        result.and(runtime.close_listener(listener))
    })
}
