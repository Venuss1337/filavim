//! Test-only per-thread allocation accounting. Ordinary release builds do not
//! replace the allocator or execute these counters.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {
    // Windows' std macro expansion triggers this lint even with a const block.
    #[allow(clippy::missing_const_for_thread_local)]
    static COUNTS: Cell<(bool, u64, u64)> = const { Cell::new((false, 0, 0)) };
}
struct Counting;
fn count(size: usize) {
    let _ = COUNTS.try_with(|c| {
        let (enabled, n, bytes) = c.get();
        if enabled {
            c.set((true, n + 1, bytes + size as u64));
        }
    });
}
// SAFETY: every operation forwards unchanged to the system allocator; counters
// contain no heap allocation and are local to the measured test thread.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            count(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(p, layout, size) };
        if !p.is_null() {
            count(size);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            count(layout.size());
        }
        p
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
pub fn allocations(f: impl FnOnce()) -> (u64, u64) {
    struct Stop;
    impl Drop for Stop {
        fn drop(&mut self) {
            COUNTS.with(|c| {
                let (_, n, b) = c.get();
                c.set((false, n, b));
            });
        }
    }
    COUNTS.with(|c| c.set((true, 0, 0)));
    let stop = Stop;
    f();
    drop(stop);
    COUNTS.with(|c| {
        let (_, n, b) = c.get();
        (n, b)
    })
}
