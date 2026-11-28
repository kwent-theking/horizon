//! Bounded, ordered CPU work for resource loading and frame preparation.
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    // A pack stage can call another parallel loader. Keep nested maps serial
    // rather than multiplying their worker budgets (and temporary memory).
    static IN_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct WorkerGuard(bool);
impl WorkerGuard {
    fn enter() -> Self {
        Self(IN_WORKER.replace(true))
    }
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        IN_WORKER.set(self.0);
    }
}

/// Thread cap for `par_map`: decoded data piles up per thread, and a 16 GB
/// machine has no room for one worker per core on a big CPU.
pub const MAX_PAR_THREADS: usize = 6;

fn thread_limit(available: usize, requested: Option<&str>) -> usize {
    let requested = requested.and_then(|value| value.parse::<usize>().ok());
    requested
        .unwrap_or(available)
        .clamp(1, MAX_PAR_THREADS)
        .min(available.max(1))
}

/// CPU budget for loading and frame preparation. Override for measurements
/// with `HORIZON_PAR_THREADS=1..6`; never exceed the CPU count or memory cap.
pub fn threads() -> usize {
    static THREADS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *THREADS.get_or_init(|| {
        thread_limit(
            std::thread::available_parallelism().map_or(1, |n| n.get()),
            std::env::var("HORIZON_PAR_THREADS").ok().as_deref(),
        )
    })
}

/// Small frame workloads stay on the caller; no worker or merge overhead.
pub fn frame_threads(threads: usize, len: usize, threshold: usize) -> usize {
    if len < threshold {
        1
    } else {
        threads.clamp(1, MAX_PAR_THREADS).min(len.max(1))
    }
}

/// `f` over `items` on up to `MAX_PAR_THREADS` threads, results in `items`' order.
pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    par_map_with(threads(), items, f)
}

/// `f` over `items` on at most `threads` threads, results in `items`' order.
/// Threads take the next item as they finish one, so a run of slow items
/// doesn't hold back the others.
pub fn par_map_with<T: Sync, R: Send>(
    threads: usize,
    items: &[T],
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let threads = if IN_WORKER.get() {
        1
    } else {
        threads.min(MAX_PAR_THREADS).min(items.len())
    };
    if threads <= 1 {
        let _guard = WorkerGuard::enter();
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut out: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let _guard = WorkerGuard::enter();
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    out.sort_unstable_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/// Independent owned jobs, mutated in disjoint slices without cloning their
/// state. The caller consumes the same slice in order after the join.
pub fn for_each_mut_with<T: Send>(threads: usize, items: &mut [T], f: impl Fn(&mut T) + Sync) {
    let threads = if IN_WORKER.get() {
        1
    } else {
        threads.min(MAX_PAR_THREADS).min(items.len())
    };
    if threads <= 1 {
        let _guard = WorkerGuard::enter();
        items.iter_mut().for_each(f);
        return;
    }
    let chunk_size = items.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let workers: Vec<_> = items
            .chunks_mut(chunk_size)
            .map(|chunk| {
                let f = &f;
                scope.spawn(move || {
                    let _guard = WorkerGuard::enter();
                    chunk.iter_mut().for_each(f);
                })
            })
            .collect();
        for worker in workers {
            worker
                .join()
                .unwrap_or_else(|error| std::panic::resume_unwind(error));
        }
    });
}

/// Consume one batch before decoding the next. Only `batch_size` inputs and
/// outputs are kept in addition to the caller's iterator and final tables.
/// Decoding may finish out of order; committing always follows input order.
pub fn map_batches_with<T: Sync, R: Send>(
    threads: usize,
    items: impl IntoIterator<Item = T>,
    batch_size: usize,
    f: impl Fn(&T) -> R + Sync,
    mut consume: impl FnMut(R),
) {
    let mut items = items.into_iter();
    loop {
        let batch: Vec<_> = items.by_ref().take(batch_size.max(1)).collect();
        if batch.is_empty() {
            break;
        }
        for result in par_map_with(threads, &batch, &f) {
            consume(result);
        }
    }
}

/// Fallible bounded decoding. Return the first error in source order, and
/// never read inputs from the next batch after a failed batch.
pub fn try_map_batches_with<T: Sync, R: Send, E: Send>(
    threads: usize,
    items: impl IntoIterator<Item = T>,
    batch_size: usize,
    f: impl Fn(&T) -> Result<R, E> + Sync,
    mut consume: impl FnMut(R),
) -> Result<(), E> {
    let mut items = items.into_iter();
    loop {
        let batch: Vec<_> = items.by_ref().take(batch_size.max(1)).collect();
        if batch.is_empty() {
            return Ok(());
        }
        for result in par_map_with(threads, &batch, &f) {
            consume(result?);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_PAR_THREADS, for_each_mut_with, frame_threads, map_batches_with, par_map, par_map_with,
        thread_limit, try_map_batches_with,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn with_keeps_order_and_respects_the_cap() {
        let items: Vec<u32> = (0..200).collect();
        let running = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let out = par_map_with(2, &items, |x| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_micros(200));
            running.fetch_sub(1, Ordering::SeqCst);
            x + 1
        });
        assert_eq!(out, items.iter().map(|x| x + 1).collect::<Vec<_>>());
        assert!(peak.load(Ordering::SeqCst) <= 2);
    }

    #[test]
    fn results_keep_the_items_order() {
        let items: Vec<u32> = (0..1000).collect();
        assert_eq!(
            par_map(&items, |x| x * 2),
            items.iter().map(|x| x * 2).collect::<Vec<_>>()
        );
        assert!(par_map(&[] as &[u32], |x| *x).is_empty());
    }

    #[test]
    fn limits_and_frame_thresholds() {
        assert_eq!(thread_limit(16, None), 6);
        assert_eq!(thread_limit(16, Some("999")), 6);
        assert_eq!(thread_limit(2, Some("6")), 2);
        assert_eq!(thread_limit(16, Some("0")), 1);
        assert_eq!(thread_limit(0, Some("invalid")), 1);
        assert_eq!(frame_threads(6, 15, 16), 1);
        assert_eq!(frame_threads(6, 16, 16), 6);
        assert_eq!(frame_threads(1, 256, 16), 1);
        assert_eq!(frame_threads(64, 256, 16), 6);
    }

    #[test]
    fn explicit_maps_also_cap_workers() {
        let caller = std::thread::current().id();
        let workers = par_map_with(64, &[(); 128], |_| std::thread::current().id());
        let mut seen = Vec::new();
        for id in workers {
            assert_ne!(id, caller);
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        assert!(seen.len() <= MAX_PAR_THREADS);
        assert_eq!(
            par_map_with(0, &[()], |_| std::thread::current().id()),
            [caller]
        );
    }

    #[test]
    fn nested_maps_reuse_the_outer_workers() {
        for threads in [1, 6] {
            par_map_with(threads, &[(); 12], |_| {
                let outer = std::thread::current().id();
                for inner in par_map_with(6, &[(); 12], |_| std::thread::current().id()) {
                    assert_eq!(inner, outer);
                }
            });
        }
    }

    #[test]
    fn batches_commit_in_order_before_reading_more_inputs() {
        let decoded = AtomicUsize::new(0);
        let committed = AtomicUsize::new(0);
        let mut out = Vec::new();
        let inputs = (0..37).inspect(|i| {
            if i % 8 == 0 {
                assert_eq!(committed.load(Ordering::SeqCst), *i);
            }
        });
        map_batches_with(
            6,
            inputs,
            8,
            |i| {
                decoded.fetch_add(1, Ordering::SeqCst);
                i * i
            },
            |value| {
                assert!(decoded.load(Ordering::SeqCst) - committed.load(Ordering::SeqCst) <= 8);
                out.push(value);
                committed.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert_eq!(out, (0..37).map(|i| i * i).collect::<Vec<_>>());
    }

    #[test]
    fn deterministic_across_worker_and_batch_counts() {
        let expected: Vec<_> = (0_u64..129)
            .map(|i| i.wrapping_mul(0x9e3779b97f4a7c15))
            .collect();
        for threads in [0, 1, 2, 4, 6, 12] {
            for size in [0, 1, 8, 64] {
                let mut out = Vec::new();
                map_batches_with(
                    threads,
                    0_u64..129,
                    size,
                    |i| i.wrapping_mul(0x9e3779b97f4a7c15),
                    |value| out.push(value),
                );
                assert_eq!(out, expected);
            }
        }
    }

    #[test]
    fn failures_commit_only_the_ordered_prefix_and_stop_prefetching() {
        let fetched = AtomicUsize::new(0);
        let mut out = Vec::new();
        let result = try_map_batches_with(
            6,
            (0..50).inspect(|_| {
                fetched.fetch_add(1, Ordering::SeqCst);
            }),
            8,
            |i| if *i == 3 || *i == 5 { Err(*i) } else { Ok(*i) },
            |value| out.push(value),
        );
        assert_eq!(result, Err(3));
        assert_eq!(out, [0, 1, 2]);
        assert_eq!(fetched.load(Ordering::SeqCst), 8);
    }

    #[test]
    fn owned_jobs_mutate_once_in_place_without_requiring_sync() {
        // RefCell is Send but not Sync: workers own disjoint mutable jobs.
        for threads in [0, 1, 2, 6, 64] {
            let mut jobs: Vec<_> = (0..129).map(std::cell::RefCell::new).collect();
            for_each_mut_with(threads, &mut jobs, |value| *value.get_mut() += 1);
            assert_eq!(
                jobs.into_iter()
                    .map(std::cell::RefCell::into_inner)
                    .collect::<Vec<_>>(),
                (1..130).collect::<Vec<_>>()
            );
        }
        for_each_mut_with(6, &mut [] as &mut [u32], |_| panic!("empty job"));
    }
}
