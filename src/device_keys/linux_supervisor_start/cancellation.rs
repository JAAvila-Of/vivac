use std::io;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::rc::Rc;

pub(super) const SIGNALS: [libc::c_int; 7] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
];

pub(super) fn signal_set() -> io::Result<libc::sigset_t> {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: sigemptyset initializes the complete signal set.
    if unsafe { libc::sigemptyset(set.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful sigemptyset initialized the complete value.
    let mut set = unsafe { set.assume_init() };
    for signal in SIGNALS {
        // SAFETY: set is initialized and every listed signal is valid.
        if unsafe { libc::sigaddset(&mut set, signal) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(set)
}

pub(super) fn cancellation_pending() -> io::Result<bool> {
    let mut pending = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: sigpending initializes the set before membership is queried.
    if unsafe { libc::sigpending(pending.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for signal in SIGNALS {
        // SAFETY: successful sigpending initialized the complete set.
        match unsafe { libc::sigismember(pending.as_ptr(), signal) } {
            0 => {}
            1 => return Ok(true),
            _ => return Err(io::Error::last_os_error()),
        }
    }
    Ok(false)
}

pub(super) struct Cancellation {
    set: libc::sigset_t,
    original: libc::sigset_t,
    dispositions: Vec<(libc::c_int, libc::sigaction)>,
    _thread: PhantomData<Rc<()>>,
}

impl Cancellation {
    pub(super) fn block() -> io::Result<Self> {
        let set = signal_set()?;
        let mut original = MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: set is initialized and original can hold the previous mask.
        let code = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, original.as_mut_ptr()) };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        let mut cancellation = Self {
            set,
            // SAFETY: successful pthread_sigmask initialized the previous mask.
            original: unsafe { original.assume_init() },
            dispositions: Vec::new(),
            _thread: PhantomData,
        };
        // SAFETY: zero initializes the scalar fields and the signal set is
        // initialized explicitly before this disposition is installed.
        let mut default = unsafe { std::mem::zeroed::<libc::sigaction>() };
        default.sa_sigaction = libc::SIG_DFL;
        // SAFETY: default contains a complete writable signal set.
        if unsafe { libc::sigemptyset(&mut default.sa_mask) } != 0 {
            return Err(io::Error::last_os_error());
        }
        for signal in SIGNALS {
            let mut previous = MaybeUninit::<libc::sigaction>::uninit();
            // SAFETY: previous can hold the queried disposition.
            if unsafe { libc::sigaction(signal, std::ptr::null(), previous.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful sigaction initialized the complete disposition.
            let previous = unsafe { previous.assume_init() };
            if previous.sa_sigaction == libc::SIG_IGN {
                // The signals are blocked before inherited ignored dispositions
                // are adopted, so cancellation remains observable as pending.
                // SAFETY: default is initialized and signal is valid.
                if unsafe { libc::sigaction(signal, &default, std::ptr::null_mut()) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                cancellation.dispositions.push((signal, previous));
            }
        }
        Ok(cancellation)
    }
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        let immediate = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        loop {
            // Cancellation has already invalidated the attempt. Consume its
            // pending signals before restoring the mask so the final receipt
            // can still be delivered. SpawnOwner resets masks in its children.
            // SAFETY: set and timeout are initialized; no signal info is requested.
            let signal = unsafe { libc::sigtimedwait(&self.set, std::ptr::null_mut(), &immediate) };
            if signal >= 0 {
                continue;
            }
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        for (signal, previous) in &self.dispositions {
            // SAFETY: the saved disposition is initialized and cancellation
            // signals remain blocked until every inherited action is restored.
            let _ = unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
        }
        // SAFETY: this thread-affine guard restores its saved initialized mask.
        let _ = unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.original, std::ptr::null_mut())
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_cancellation_is_adopted_and_restored() {
        const CHILD: &str = "VIVAC_TEST_IGNORED_CANCELLATION";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "linux_supervisor_start::cancellation::tests::ignored_cancellation_is_adopted_and_restored",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stdout)
            );
            return;
        }

        // This child runs only this test, so process-wide dispositions are isolated.
        let mut original_mask = MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: the null set queries the mask into complete writable storage.
        assert_eq!(
            unsafe {
                libc::pthread_sigmask(
                    libc::SIG_SETMASK,
                    std::ptr::null(),
                    original_mask.as_mut_ptr(),
                )
            },
            0
        );
        // SAFETY: successful pthread_sigmask initialized the complete mask.
        let original_mask = unsafe { original_mask.assume_init() };
        // SAFETY: zero initializes scalar fields; the signal set is set below.
        let mut ignored = unsafe { std::mem::zeroed::<libc::sigaction>() };
        ignored.sa_sigaction = libc::SIG_IGN;
        // SAFETY: ignored contains a complete writable signal set.
        assert_eq!(unsafe { libc::sigemptyset(&mut ignored.sa_mask) }, 0);
        for signal in SIGNALS {
            // SAFETY: ignored is initialized and every listed signal is valid.
            assert_eq!(
                unsafe { libc::sigaction(signal, &ignored, std::ptr::null_mut()) },
                0
            );
        }
        let cancellation = Cancellation::block().unwrap();
        for signal in SIGNALS {
            let mut current = MaybeUninit::<libc::sigaction>::uninit();
            // SAFETY: current can hold one complete disposition.
            assert_eq!(
                unsafe { libc::sigaction(signal, std::ptr::null(), current.as_mut_ptr()) },
                0
            );
            // SAFETY: successful sigaction initialized current.
            assert_eq!(unsafe { current.assume_init() }.sa_sigaction, libc::SIG_DFL);
            // SAFETY: pthread_self selects this thread and signal is blocked here.
            assert_eq!(
                unsafe { libc::pthread_kill(libc::pthread_self(), signal) },
                0
            );
        }
        assert!(cancellation_pending().unwrap());
        drop(cancellation);
        assert!(!cancellation_pending().unwrap());
        let mut restored_mask = MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: the null set queries the mask into complete writable storage.
        assert_eq!(
            unsafe {
                libc::pthread_sigmask(
                    libc::SIG_SETMASK,
                    std::ptr::null(),
                    restored_mask.as_mut_ptr(),
                )
            },
            0
        );
        for signal in SIGNALS {
            // SAFETY: both masks were initialized by successful pthread_sigmask.
            assert_eq!(
                unsafe { libc::sigismember(&original_mask, signal) },
                unsafe { libc::sigismember(restored_mask.as_ptr(), signal) }
            );
            let mut current = MaybeUninit::<libc::sigaction>::uninit();
            // SAFETY: current can hold one complete disposition.
            assert_eq!(
                unsafe { libc::sigaction(signal, std::ptr::null(), current.as_mut_ptr()) },
                0
            );
            // SAFETY: successful sigaction initialized current.
            assert_eq!(unsafe { current.assume_init() }.sa_sigaction, libc::SIG_IGN);
        }
    }
}
