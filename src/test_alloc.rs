#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static CURRENT: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocated(bytes: usize) {
    let _ = CURRENT.try_with(|current| {
        let next = current.get().saturating_add(bytes);
        current.set(next);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(next)));
    });
}

fn released(bytes: usize) {
    let _ = CURRENT.try_with(|current| current.set(current.get().saturating_sub(bytes)));
}

// The implementation forwards every allocation contract to System unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // The caller supplies the valid layout required by GlobalAlloc.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // System preserves the caller's layout and zero-initialization contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // The pointer and matching layout come from the GlobalAlloc caller.
        unsafe { System.dealloc(pointer, layout) };
        released(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // System receives the original pointer, layout, and requested new size.
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            if new_size >= layout.size() {
                allocated(new_size - layout.size());
            } else {
                released(layout.size() - new_size);
            }
        }
        resized
    }
}

pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    let baseline = CURRENT.with(Cell::get);
    let previous_peak = PEAK.with(|peak| peak.replace(baseline));
    let result = operation();
    let measured = PEAK.with(|peak| {
        let measured = peak.get();
        peak.set(previous_peak.max(measured));
        measured.saturating_sub(baseline)
    });
    (result, measured)
}
