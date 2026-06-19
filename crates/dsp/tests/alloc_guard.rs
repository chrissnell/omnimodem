//! Real-time-path allocation guard (design §"No allocation on the streaming hot
//! path"). A counting global allocator wraps the system allocator; we assert
//! that the per-sample filter loop performs **zero** heap allocations once its
//! buffers are warm. On a real-time modem a per-sample allocation is a latency
//! bug, so this is a hard test, not advisory.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn count_allocs<F: FnOnce()>(f: F) -> usize {
    let before = ALLOCS.load(Ordering::Relaxed);
    f();
    ALLOCS.load(Ordering::Relaxed) - before
}

#[test]
fn fir_push_loop_is_allocation_free() {
    use omnimodem_dsp::frontend::fir::{design_lowpass, Fir};

    let mut fir = Fir::new(design_lowpass(63, 3_000.0, 48_000.0));
    let block: Vec<f32> = (0..8192).map(|n| (n as f32 * 0.01).sin()).collect();

    // Warm the history buffer (already sized at construction; this just primes).
    let _ = fir.push(0.0);

    let allocs = count_allocs(|| {
        let mut acc = 0.0f32;
        for &x in &block {
            acc += fir.push(x);
        }
        // Keep the optimizer honest.
        assert!(acc.is_finite());
    });

    assert_eq!(allocs, 0, "Fir::push must not allocate on the streaming hot path");
}
