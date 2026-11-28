//! Bounded, ordered CPU preparation, shared with item loading.
pub use horizon_core::par::{
    MAX_PAR_THREADS, for_each_mut_with, frame_threads, map_batches_with, par_map, par_map_with,
    threads, try_map_batches_with,
};

/// Stage duration (not cumulative launch time), with its worker budget for
/// comparing otherwise identical runs at 1, 2, 4 and 6 threads.
pub fn time<R>(stage: &str, threads: usize, f: impl FnOnce() -> R) -> R {
    let started = std::time::Instant::now();
    let result = f();
    log::info!(
        "Horizon: stage={stage} threads={} elapsed_ms={}",
        threads.clamp(1, MAX_PAR_THREADS),
        started.elapsed().as_millis()
    );
    result
}

/// Decode bounded batches and commit each batch's results in source order.
pub fn map_batched<T: Sync, R: Send>(
    stage: &str,
    threads: usize,
    items: impl IntoIterator<Item = T>,
    batch_size: usize,
    f: impl Fn(&T) -> R + Sync,
    consume: impl FnMut(R),
) {
    time(stage, threads, || {
        map_batches_with(threads, items, batch_size, f, consume)
    });
}
