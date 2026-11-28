//! An exact count of the bytes the program's allocations hold right now.
//! Private bytes include whatever the allocator keeps for reuse, so they can't
//! say how much of the process is live data; this can.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// The system allocator with every allocation and release counted. Install it
/// with `#[global_allocator]` in the binary; the counters are two relaxed
/// atomic operations per call.
pub struct CountingAllocator;

fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    // Only a new high writes the shared peak: most calls just read it, so the
    // worker threads don't fight over its cache line on every allocation.
    if live > PEAK.load(Relaxed) {
        PEAK.fetch_max(live, Relaxed);
    }
}

fn sub(bytes: usize) {
    LIVE.fetch_sub(bytes, Relaxed);
}

// SAFETY: every call is forwarded unchanged to `System`, which upholds the
// `GlobalAlloc` contract; the counters never touch the memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout contract is passed through.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            add(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout contract is passed through.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            add(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` and `layout` come from this allocator's `alloc`.
        unsafe { System.dealloc(ptr, layout) };
        sub(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `ptr` and `layout` come from this allocator's `alloc`.
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        if !moved.is_null() {
            sub(layout.size());
            add(new_size);
        }
        moved
    }
}

/// Bytes allocated and not yet freed (zero when the counting allocator isn't installed).
pub fn live_heap() -> u64 {
    LIVE.load(Relaxed) as u64
}

/// The most `live_heap` has been since the start or the last `reset_heap_peak`.
pub fn heap_peak() -> u64 {
    PEAK.load(Relaxed) as u64
}

/// Start a new peak at the current live bytes, to measure one stage on its own.
pub fn reset_heap_peak() {
    PEAK.store(LIVE.load(Relaxed), Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test owns the counters: in the test binary the counting allocator
    /// isn't installed, so only these calls move them.
    #[test]
    fn counts_allocations_reallocations_frees_and_peaks() {
        let allocator = CountingAllocator;
        let layout = Layout::from_size_align(4096, 8).unwrap();
        // SAFETY: the layouts are non-zero and each pointer is freed with the layout it was last given.
        unsafe {
            let ptr = allocator.alloc(layout);
            assert!(!ptr.is_null());
            assert_eq!(live_heap(), 4096);
            let grown = allocator.realloc(ptr, layout, 8192);
            assert!(!grown.is_null());
            assert_eq!(live_heap(), 8192);
            allocator.dealloc(grown, Layout::from_size_align(8192, 8).unwrap());
        }
        assert_eq!(live_heap(), 0);
        assert_eq!(heap_peak(), 8192);
        reset_heap_peak();
        assert_eq!(heap_peak(), 0);
    }
}
