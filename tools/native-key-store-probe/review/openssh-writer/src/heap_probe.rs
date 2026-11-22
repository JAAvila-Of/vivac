use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const BUDGET_BYTES: usize = 2 * 1024 * 1024;
const COMMAND: &str = "--heap-package-check";

struct CountingState {
    current: AtomicUsize,
    peak: AtomicUsize,
    active: AtomicBool,
    valid: AtomicBool,
}

impl CountingState {
    const fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            active: AtomicBool::new(false),
            valid: AtomicBool::new(true),
        }
    }

    fn invalidate(&self) {
        self.valid.store(false, Ordering::Relaxed);
    }

    fn record_peak(&self, value: usize) {
        if self.active.load(Ordering::Relaxed) {
            self.peak.fetch_max(value, Ordering::Relaxed);
        }
    }

    fn allocated(&self, size: usize) {
        match self
            .current
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(size)
            }) {
            Ok(previous) => match previous.checked_add(size) {
                Some(current) => self.record_peak(current),
                None => self.invalidate(),
            },
            Err(_) => self.invalidate(),
        }
    }

    fn released(&self, size: usize) {
        if self
            .current
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_sub(size)
            })
            .is_err()
        {
            self.invalidate();
        }
    }

    fn prepare_reallocation(&self, new_size: usize) {
        if self.active.load(Ordering::Relaxed) {
            match self.current.load(Ordering::Relaxed).checked_add(new_size) {
                Some(potential) => self.record_peak(potential),
                None => self.invalidate(),
            }
        }
    }

    fn reallocated(&self, old_size: usize, new_size: usize, succeeded: bool) {
        if !succeeded {
            return;
        }
        match self
            .current
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_sub(old_size)?.checked_add(new_size)
            }) {
            Ok(previous) => match previous
                .checked_sub(old_size)
                .and_then(|value| value.checked_add(new_size))
            {
                Some(current) => self.record_peak(current),
                None => self.invalidate(),
            },
            Err(_) => self.invalidate(),
        }
    }

    fn begin(&self) -> Result<Scope<'_>, ()> {
        if !self.valid.load(Ordering::Relaxed)
            || self
                .active
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            self.invalidate();
            return Err(());
        }
        // The command checks that only one thread exists before entering here.
        let baseline = self.current.load(Ordering::Relaxed);
        self.peak.store(baseline, Ordering::Relaxed);
        Ok(Scope {
            state: self,
            baseline,
            finished: false,
        })
    }

    fn measure<T>(&self, operation: impl FnOnce() -> T) -> Result<(T, Measurement), ()> {
        let scope = self.begin()?;
        let result = operation();
        Ok((result, scope.finish()?))
    }
}

struct Scope<'a> {
    state: &'a CountingState,
    baseline: usize,
    finished: bool,
}

struct Measurement {
    baseline_bytes: usize,
    peak_additional_bytes: usize,
    live_after_bytes: usize,
}

impl Scope<'_> {
    fn finish(mut self) -> Result<Measurement, ()> {
        self.finished = true;
        if !self.state.active.swap(false, Ordering::Relaxed) {
            self.state.invalidate();
        }
        let peak = self.state.peak.load(Ordering::Relaxed);
        let additional = peak.checked_sub(self.baseline);
        if additional.is_none() {
            self.state.invalidate();
        }
        if !self.state.valid.load(Ordering::Relaxed) {
            return Err(());
        }
        Ok(Measurement {
            baseline_bytes: self.baseline,
            peak_additional_bytes: additional.ok_or(())?,
            live_after_bytes: self.state.current.load(Ordering::Relaxed),
        })
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.state.invalidate();
            self.state.active.store(false, Ordering::Relaxed);
        }
    }
}

struct CountingAllocator {
    state: CountingState,
}

impl CountingAllocator {
    const fn new() -> Self {
        Self {
            state: CountingState::new(),
        }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator::new();

// The forwarded System calls preserve every layout and pointer contract. The
// counters use only atomics and never allocate, format, lock or panic.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller supplies the valid allocation layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            self.state.allocated(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: System preserves the caller's layout and zeroed-byte contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            self.state.allocated(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: GlobalAlloc's caller owns this pointer with the matching layout.
        unsafe { System.dealloc(pointer, layout) };
        self.state.released(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Account conservatively for the old and new requests coexisting, even
        // if System resizes in place or later reports failure.
        self.state.prepare_reallocation(new_size);
        // SAFETY: the caller supplies the original allocation and valid new size.
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        self.state
            .reallocated(layout.size(), new_size, !resized.is_null());
        resized
    }
}

fn command_path(arguments: &[OsString]) -> Result<&Path, ()> {
    if arguments.len() != 3 || arguments[1] != OsStr::new(COMMAND) {
        return Err(());
    }
    let path = Path::new(&arguments[2]);
    if !path.is_absolute() {
        return Err(());
    }
    Ok(path)
}

fn single_threaded() -> Result<bool, ()> {
    let mut entries = std::fs::read_dir("/proc/self/task").map_err(|_| ())?;
    Ok(matches!(entries.next(), Some(Ok(_))) && entries.next().is_none())
}

fn run(arguments: &[OsString]) -> Result<i32, ()> {
    let path = command_path(arguments)?;
    let declaration = std::env::var_os("VIVAC_OPENSSH_DISPOSABLE");
    if declaration.as_deref() != Some(OsStr::new("1")) || !single_threaded()? {
        return Err(());
    }
    let (checked, measurement) = ALLOCATOR.state.measure(|| crate::package::check(path))?;
    // Keep the external harness inputs alive across the measurement boundary.
    std::hint::black_box(arguments);
    std::hint::black_box(&declaration);
    let within_budget = measurement.peak_additional_bytes <= BUDGET_BYTES;
    let report = serde_json::json!({
        "version": 1,
        "measurement": "additional-live-rust-requested-heap",
        "accepted": checked.is_ok(),
        "baseline_bytes": measurement.baseline_bytes,
        "peak_additional_bytes": measurement.peak_additional_bytes,
        "live_after_bytes": measurement.live_after_bytes,
        "budget_bytes": BUDGET_BYTES,
        "within_budget": within_budget,
        "allocator_valid": true,
        "realloc_conservative": true,
    });
    writeln!(std::io::stdout().lock(), "{report}").map_err(|_| ())?;
    Ok(if within_budget { 0 } else { 2 })
}

pub(crate) fn maybe_run() -> Option<i32> {
    let arguments: Vec<_> = std::env::args_os().collect();
    if arguments.get(1).map(OsString::as_os_str) != Some(OsStr::new(COMMAND)) {
        return None;
    }
    Some(run(&arguments).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_baseline_and_conservative_resize_track_live_requests() {
        let state = CountingState::new();
        state.allocated(100);
        let scope = state.begin().expect("measurement starts");
        state.allocated(40);
        state.prepare_reallocation(80);
        state.reallocated(40, 80, true);
        state.prepare_reallocation(20);
        state.reallocated(80, 20, true);
        state.released(20);
        let measurement = scope.finish().expect("valid counters");
        assert_eq!(measurement.baseline_bytes, 100);
        assert_eq!(measurement.peak_additional_bytes, 120);
        assert_eq!(measurement.live_after_bytes, 100);
    }

    #[test]
    fn failed_reallocation_keeps_the_original_request_live() {
        let state = CountingState::new();
        state.allocated(64);
        let scope = state.begin().expect("measurement starts");
        state.prepare_reallocation(128);
        state.reallocated(64, 128, false);
        let measurement = scope.finish().expect("valid counters");
        assert_eq!(measurement.peak_additional_bytes, 128);
        assert_eq!(measurement.live_after_bytes, 64);
    }

    #[test]
    fn invalid_arithmetic_cannot_produce_an_accepted_measurement() {
        let overflowing = CountingState::new();
        overflowing.current.store(usize::MAX, Ordering::Relaxed);
        overflowing.allocated(1);
        assert!(overflowing.begin().is_err());
        assert_eq!(overflowing.current.load(Ordering::Relaxed), usize::MAX);

        let underflowing = CountingState::new();
        let scope = underflowing.begin().expect("measurement starts");
        underflowing.released(1);
        assert!(scope.finish().is_err());
        assert_eq!(underflowing.current.load(Ordering::Relaxed), 0);

        let invalid_resize = CountingState::new();
        let scope = invalid_resize.begin().expect("measurement starts");
        invalid_resize.reallocated(1, 32, true);
        assert!(scope.finish().is_err());
    }

    #[test]
    fn nested_and_abandoned_scopes_are_invalid() {
        let nested = CountingState::new();
        let scope = nested.begin().expect("outer measurement starts");
        assert!(nested.begin().is_err());
        assert!(scope.finish().is_err());
        assert!(!nested.active.load(Ordering::Relaxed));

        let abandoned = CountingState::new();
        drop(abandoned.begin().expect("measurement starts"));
        assert!(abandoned.begin().is_err());
    }

    #[test]
    fn system_forwarding_preserves_zeroed_bytes_and_resize_contents() {
        let allocator = CountingAllocator::new();
        let first = Layout::from_size_align(64, 8).expect("valid layout");
        let grown = Layout::from_size_align(128, 8).expect("valid layout");
        let shrunk = Layout::from_size_align(32, 8).expect("valid layout");
        let scope = allocator.state.begin().expect("measurement starts");
        // SAFETY: all layouts are nonzero and each returned pointer is checked;
        // later calls use its current allocation layout and release it once.
        unsafe {
            let pointer = allocator.alloc_zeroed(first);
            assert!(!pointer.is_null());
            let bytes = std::slice::from_raw_parts_mut(pointer, first.size());
            assert!(bytes.iter().all(|byte| *byte == 0));
            bytes.fill(7);
            std::hint::black_box(&bytes);
            let pointer = allocator.realloc(pointer, first, grown.size());
            assert!(!pointer.is_null());
            assert!(std::slice::from_raw_parts(pointer, first.size())
                .iter()
                .all(|byte| *byte == 7));
            let pointer = allocator.realloc(pointer, grown, shrunk.size());
            assert!(!pointer.is_null());
            assert!(std::slice::from_raw_parts(pointer, shrunk.size())
                .iter()
                .all(|byte| *byte == 7));
            allocator.dealloc(pointer, shrunk);
        }
        let measurement = scope.finish().expect("valid forwarded counters");
        assert_eq!(measurement.peak_additional_bytes, 192);
        assert_eq!(measurement.live_after_bytes, 0);
    }

    #[test]
    fn system_allocation_is_counted_and_released() {
        let allocator = CountingAllocator::new();
        let layout = Layout::from_size_align(48, 16).expect("valid layout");
        let scope = allocator.state.begin().expect("measurement starts");
        // SAFETY: layout is valid, the non-null result is not read uninitialized,
        // and the same allocator receives the pointer and matching layout once.
        unsafe {
            let pointer = allocator.alloc(layout);
            assert!(!pointer.is_null());
            std::hint::black_box(pointer);
            allocator.dealloc(pointer, layout);
        }
        let measurement = scope.finish().expect("valid forwarded counters");
        assert_eq!(measurement.peak_additional_bytes, 48);
        assert_eq!(measurement.live_after_bytes, 0);
    }

    #[test]
    fn command_requires_one_absolute_path() {
        let arguments = |items: &[&str]| {
            items
                .iter()
                .map(|item| OsString::from(*item))
                .collect::<Vec<_>>()
        };
        assert!(command_path(&arguments(&["probe", COMMAND, "/tmp/package"])).is_ok());
        for invalid in [
            arguments(&["probe", COMMAND]),
            arguments(&["probe", COMMAND, "relative"]),
            arguments(&["probe", COMMAND, "/tmp/package", "extra"]),
            arguments(&["probe", "--package-check", "/tmp/package"]),
        ] {
            assert!(command_path(&invalid).is_err());
        }
    }
}
