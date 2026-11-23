//! The active session: one poll loop over the stop signals, the control
//! socket, the supervisor, the native agent and a timer. The idle and the
//! maximum limit are both measured on the boot-time clock, so time spent
//! suspended counts.

use super::closing::CloseReason;
use super::{
    next_connection, refuse_request, refused, short_request, stop_requested, watched, Children,
    Keeper,
};
use crate::linux_ipc::registration::{receive_any_session_child_step, EndpointSessionStep};
use crate::linux_session_control::{ClientFrame, ControlConnection, KeeperFrame};
use crate::linux_supervisor_start::short_deadline;
use crate::linux_user_manager::SessionLimits;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

/// Times on `CLOCK_BOOTTIME`, measured from boot.
pub(super) struct SessionClock {
    activation: Duration,
    last_served: Duration,
    limits: SessionLimits,
}

impl SessionClock {
    pub(super) fn new(activation: Duration, limits: SessionLimits) -> Self {
        Self {
            activation,
            last_served: activation,
            limits,
        }
    }

    /// Records a served request; a report that arrives late never moves the
    /// idle limit back.
    pub(super) fn served(&mut self, now: Duration) {
        self.last_served = self.last_served.max(now);
    }

    fn maximum(&self) -> Duration {
        self.activation.saturating_add(self.limits.maximum)
    }

    fn idle(&self) -> Duration {
        self.last_served.saturating_add(self.limits.idle)
    }

    pub(super) fn deadline(&self) -> Duration {
        self.maximum().min(self.idle())
    }

    /// The limit that has run out at `now`, the maximum first.
    pub(super) fn expired(&self, now: Duration) -> Option<CloseReason> {
        if now >= self.maximum() {
            Some(CloseReason::MaximumExpiry)
        } else if now >= self.idle() {
            Some(CloseReason::IdleExpiry)
        } else {
            None
        }
    }
}

pub(super) fn boot_time() -> io::Result<Duration> {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: now is writable for one time value.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Duration::new(now.tv_sec as u64, now.tv_nsec as u32))
}

/// A timer descriptor on `CLOCK_BOOTTIME`, set to an absolute time.
struct ExpiryTimer(OwnedFd);

impl ExpiryTimer {
    fn new() -> io::Result<Self> {
        // SAFETY: the timer is created from scalar arguments as a fresh descriptor.
        let raw = unsafe {
            libc::timerfd_create(libc::CLOCK_BOOTTIME, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK)
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful call returned this descriptor, owned from here.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(raw) }))
    }

    fn arm(&self, at: Duration) -> io::Result<()> {
        // A zero value would turn the timer off, so the value is never zero.
        let value = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: at.as_secs().min(libc::time_t::MAX as u64) as libc::time_t,
                tv_nsec: at.subsec_nanos().max(1) as libc::c_long,
            },
        };
        // SAFETY: value is initialized and no previous value is requested.
        if unsafe {
            libc::timerfd_settime(
                self.0.as_raw_fd(),
                libc::TFD_TIMER_ABSTIME,
                &value,
                std::ptr::null_mut(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Consumes an expiration, if one is pending.
    fn acknowledge(&self) -> io::Result<()> {
        let mut count = 0u64;
        // SAFETY: count is writable for the eight bytes each read returns.
        let read = unsafe {
            libc::read(
                self.0.as_raw_fd(),
                (&raw mut count).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if read < 0 {
            let error = io::Error::last_os_error();
            if !matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) {
                return Err(error);
            }
        }
        Ok(())
    }
}

/// Serves until a reason to close. A failure of the loop itself ends the
/// session too: it closes like a session that ended on its own.
pub(super) fn serve(
    keeper: &Keeper,
    children: &mut Children,
) -> (CloseReason, Option<ControlConnection>) {
    serve_until_close(keeper, children).unwrap_or((CloseReason::SessionEnded, None))
}

fn serve_until_close(
    keeper: &Keeper,
    children: &Children,
) -> io::Result<(CloseReason, Option<ControlConnection>)> {
    let owner = children.owner.as_ref().ok_or_else(refused)?;
    let (supervisor, channel) = children.supervisor.as_ref().ok_or_else(refused)?;
    let agent = children
        .adopted
        .as_ref()
        .ok_or_else(refused)?
        .active_agent()?
        .try_clone_to_owned()?;
    let listener = keeper.listener()?;
    let mut clock = SessionClock::new(boot_time()?, keeper.limits);
    let timer = ExpiryTimer::new()?;
    timer.arm(clock.deadline())?;
    loop {
        let mut descriptors = [
            watched(keeper.signals.as_raw_fd()),
            watched(timer.0.as_raw_fd()),
            watched(channel.as_raw_fd()),
            watched(agent.as_raw_fd()),
            watched(listener.as_raw_fd()),
        ];
        // SAFETY: every entry is an initialized pollfd. The armed timer
        // bounds the wait, so no timeout is needed.
        let ready = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                -1,
            )
        };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if descriptors[0].revents != 0 && stop_requested(&keeper.signals)? {
            return Ok((CloseReason::ManagerStop, None));
        }
        if descriptors[1].revents != 0 {
            timer.acknowledge()?;
            if let Some(reason) = clock.expired(boot_time()?) {
                return Ok((reason, None));
            }
            timer.arm(clock.deadline())?;
        }
        if descriptors[2].revents != 0 {
            match receive_any_session_child_step(channel, owner, supervisor.token, short_deadline())
            {
                Ok(EndpointSessionStep::Served) => {
                    clock.served(boot_time()?);
                    timer.arm(clock.deadline())?;
                }
                _ => return Ok((CloseReason::SessionEnded, None)),
            }
        }
        if descriptors[3].revents != 0 {
            return Ok((CloseReason::SessionEnded, None));
        }
        if descriptors[4].revents != 0 {
            while let Some(connection) = next_connection(listener) {
                match connection.receive_client(short_request()) {
                    Ok(ClientFrame::Close) => {
                        let _ = connection.send_keeper(KeeperFrame::Closing, short_request());
                        return Ok((CloseReason::Request, Some(connection)));
                    }
                    Ok(_) => refuse_request(&connection, short_request()),
                    Err(_) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_user_manager::SessionLimits;
    use std::time::Duration;

    fn seconds(value: u64) -> Duration {
        Duration::from_secs(value)
    }

    const LIMITS: SessionLimits = SessionLimits {
        idle: Duration::from_secs(100),
        maximum: Duration::from_secs(1000),
    };

    #[test]
    fn idle_deadline_moves_with_each_served_request() {
        let mut clock = SessionClock::new(seconds(50), LIMITS);
        assert_eq!(clock.deadline(), seconds(150));
        assert_eq!(clock.expired(seconds(149)), None);
        clock.served(seconds(120));
        assert_eq!(clock.deadline(), seconds(220));
        // A report that arrives late never moves the deadline back.
        clock.served(seconds(110));
        assert_eq!(clock.deadline(), seconds(220));
        assert_eq!(clock.expired(seconds(219)), None);
        assert_eq!(clock.expired(seconds(220)), Some(CloseReason::IdleExpiry));
    }

    #[test]
    fn maximum_deadline_caps_idle_renewal() {
        let mut clock = SessionClock::new(seconds(50), LIMITS);
        clock.served(seconds(990));
        assert_eq!(clock.deadline(), seconds(1050));
        assert_eq!(clock.expired(seconds(1049)), None);
        assert_eq!(
            clock.expired(seconds(1050)),
            Some(CloseReason::MaximumExpiry)
        );
        let mut idle_first = SessionClock::new(seconds(50), LIMITS);
        idle_first.served(seconds(900));
        assert_eq!(idle_first.deadline(), seconds(1000));
        assert_eq!(
            idle_first.expired(seconds(1000)),
            Some(CloseReason::IdleExpiry)
        );
    }
}
