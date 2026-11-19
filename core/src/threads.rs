//! One process-wide CPU limit for every parallel native reader.
//!
//! By default all logical CPUs are used (rayon's global pool, which also
//! honors `RAYON_NUM_THREADS`). `set_threads(n)` limits every later call to
//! `n` CPUs. Unlike `RAYON_NUM_THREADS`, it works after the global pool has
//! started, because limited work runs in a dedicated pool of that size.
//!
//! A limit counts every busy thread: a reader that overlaps a sequential scan
//! thread with parallel parsing gives the parsers `n - 1` workers, and runs
//! without overlap when `n` is 1.

use rayon::{ThreadPool, ThreadPoolBuilder};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 0 means "no limit".
static LIMIT: AtomicUsize = AtomicUsize::new(0);
static POOLS: Mutex<Vec<(usize, Arc<ThreadPool>)>> = Mutex::new(Vec::new());

/// The CPU budget for a parallel call: the limit, or the global pool size.
#[doc(hidden)]
pub fn budget() -> usize {
    match LIMIT.load(Ordering::Relaxed) {
        0 => rayon::current_num_threads().max(1),
        limit => limit,
    }
}

fn pool(threads: usize) -> Arc<ThreadPool> {
    let mut pools = POOLS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, pool)) = pools.iter().find(|(size, _)| *size == threads) {
        return Arc::clone(pool);
    }
    let pool = Arc::new(
        ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("open-eda-{index}"))
            .build()
            .expect("failed to start a thread pool"),
    );
    pools.push((threads, Arc::clone(&pool)));
    pool
}

/// Whether the current thread is a worker of one of our pools.
fn inside_pool() -> bool {
    let pools = POOLS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pools
        .iter()
        .any(|(_, pool)| pool.current_thread_index().is_some())
}

/// Run `work` (and every rayon call inside it) on `workers` threads.
///
/// Re-entrant: inside one of our pools `work` runs as is, so a nested call
/// keeps the outer budget and never starts a second pool.
fn run_on<R: Send>(workers: usize, work: impl FnOnce() -> R + Send) -> R {
    if inside_pool() {
        return work();
    }
    if LIMIT.load(Ordering::Relaxed) == 0 && workers == rayon::current_num_threads() {
        return work();
    }
    pool(workers.max(1)).install(work)
}

/// Run `work`, and every rayon call inside it, within the CPU budget.
#[doc(hidden)]
pub fn install<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    run_on(budget(), work)
}

/// Run parallel work while one extra thread (a sequential scanner) is also
/// busy: the parallel part gets one CPU fewer. Callers overlap only when
/// `may_overlap()` is true.
#[doc(hidden)]
pub fn install_beside_scanner<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    run_on(budget().saturating_sub(1).max(1), work)
}

/// `install_beside_scanner` when a scanner thread overlaps, else `install`.
#[doc(hidden)]
pub fn install_for<R: Send>(overlapped: bool, work: impl FnOnce() -> R + Send) -> R {
    if overlapped {
        install_beside_scanner(work)
    } else {
        install(work)
    }
}

/// Whether a reader may run a scanner thread beside its parallel workers.
/// Never inside one of our pools: the pool already uses the whole budget, so a
/// nested scanner would make `n + 1` busy threads.
#[doc(hidden)]
pub fn may_overlap() -> bool {
    budget() > 1 && !inside_pool()
}

/// The number of CPUs native readers will use.
pub fn get_threads() -> usize {
    budget()
}

/// Limit native readers to `threads` CPUs; `None` or 0 removes the limit.
/// Values above 4096 are rejected.
pub fn set_threads(threads: Option<usize>) -> Result<(), crate::Error> {
    let value = threads.unwrap_or(0);
    if value > 4096 {
        return Err(crate::Error::Message(format!(
            "threads must be between 1 and 4096, got {value}"
        )));
    }
    LIMIT.store(value, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tests write the process-wide `LIMIT`; run them one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn limit_controls_the_pool_size() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        LIMIT.store(3, Ordering::Relaxed);
        assert_eq!(budget(), 3);
        assert_eq!(install(rayon::current_num_threads), 3);
        assert_eq!(install_beside_scanner(rayon::current_num_threads), 2);
        assert!(may_overlap());
        // Inside one of our pools a nested reader must not add a scanner
        // thread beside the full pool.
        assert!(!install(may_overlap));
        LIMIT.store(1, Ordering::Relaxed);
        assert_eq!(install(rayon::current_num_threads), 1);
        assert_eq!(install_beside_scanner(rayon::current_num_threads), 1);
        assert!(!may_overlap());
        LIMIT.store(0, Ordering::Relaxed);
        assert_eq!(
            install(rayon::current_num_threads),
            rayon::current_num_threads()
        );
    }

    #[test]
    fn nested_install_keeps_the_outer_pool() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        LIMIT.store(3, Ordering::Relaxed);
        let nested = install_beside_scanner(|| install(rayon::current_num_threads));
        let nested_for = install_for(true, || install_for(false, rayon::current_num_threads));
        LIMIT.store(0, Ordering::Relaxed);
        assert_eq!(nested, 2);
        assert_eq!(nested_for, 2);
    }
}
