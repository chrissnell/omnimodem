//! Real-time-path benchmarks (design §"Real-time-path guards"): on a real-time
//! modem a throughput regression *is* a correctness bug, so the per-sample
//! filter loop, the STFT feed, and the ensemble feed are benchmarked. The
//! companion allocation guard (`tests/alloc_guard.rs`) asserts the streaming
//! sample loop is heap-allocation-free.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use omnimodem_dsp::ensemble::ParallelDemodulator;
use omnimodem_dsp::frontend::fir::{design_lowpass, Fir};
use omnimodem_dsp::frontend::stft::Stft;
use omnimodem_dsp::mode::{DemodShape, Demodulator, Duplex, ModeCaps};
use omnimodem_dsp::types::{Frame, Sample};

/// A frame-less streaming demod: the realistic hot path is "samples in, nothing
/// out" (frames are rare). Exercises the ensemble's per-sample fan-out.
struct SilentDemod;
impl Demodulator for SilentDemod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: 48_000,
            bandwidth_hz: 3_000.0,
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }
    fn feed(&mut self, _s: &[Sample]) -> Vec<Frame> {
        Vec::new()
    }
    fn reset(&mut self) {}
}

fn bench_fir(c: &mut Criterion) {
    let mut fir = Fir::new(design_lowpass(63, 3_000.0, 48_000.0));
    let block: Vec<f32> = (0..4096).map(|n| (n as f32 * 0.01).sin()).collect();
    c.bench_function("fir_push_4096", |b| {
        b.iter(|| {
            let mut acc = 0.0f32;
            for &x in &block {
                acc += fir.push(black_box(x));
            }
            black_box(acc)
        })
    });
}

fn bench_stft(c: &mut Criterion) {
    let block: Vec<f32> = (0..4096).map(|n| (n as f32 * 0.02).sin()).collect();
    c.bench_function("stft_feed_4096_n512_h128", |b| {
        b.iter(|| {
            let mut stft = Stft::new(512, 128);
            black_box(stft.feed(black_box(&block)).len())
        })
    });
}

fn bench_ensemble(c: &mut Criterion) {
    let block: Vec<f32> = (0..1024).map(|n| (n as f32 * 0.03).sin()).collect();
    c.bench_function("parallel_demod_feed_3x1024", |b| {
        b.iter_batched(
            || ParallelDemodulator::new(vec![SilentDemod, SilentDemod, SilentDemod], 144),
            |mut ens| black_box(ens.feed(black_box(&block)).len()),
            criterion::BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_fir, bench_stft, bench_ensemble);
criterion_main!(benches);
