//! Real-time-path benchmarks (filled in Task 38).

use criterion::{criterion_group, criterion_main, Criterion};

fn placeholder(c: &mut Criterion) {
    c.bench_function("noop", |b| b.iter(|| 1 + 1));
}

criterion_group!(benches, placeholder);
criterion_main!(benches);
