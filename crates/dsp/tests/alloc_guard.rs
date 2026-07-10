//! Real-time-path allocation guard (design §"No allocation on the streaming hot
//! path"). A counting global allocator wraps the system allocator; we assert
//! that the per-sample filter loop performs **zero** heap allocations once its
//! buffers are warm. On a real-time modem a per-sample allocation is a latency
//! bug, so this is a hard test, not advisory.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

// Per-thread allocation counter. A process-global counter would also tally
// allocations made concurrently by libtest's harness threads (output capture,
// etc.), racing the measured loop and flaking the assertion; counting only the
// current thread attributes allocations to the code actually under test. The
// counter is `const`-initialized so first access never itself allocates (which
// would recurse through this allocator).
thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn count_allocs<F: FnOnce()>(f: F) -> usize {
    let before = ALLOCS.with(|c| c.get());
    f();
    ALLOCS.with(|c| c.get()) - before
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
