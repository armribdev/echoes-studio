//! Pre-spawned worker pool for parallel graph execution (audit R1).
//!
//! Built once when a `GraphScheduler` with `num_workers > 0` is constructed;
//! lives for the scheduler's lifetime; never allocates on dispatch.
//!
//! ## Protocol
//!
//! 1. The audio thread calls `WorkerPool::dispatch(indices, ctx, step_fn)`.
//! 2. The dispatcher fills the shared `Job` slot, resets the work counter,
//!    and `unpark`s every worker thread.
//! 3. Workers atomically claim indices via `next_idx.fetch_add` and invoke
//!    `step_fn(worker_id, step_idx, ctx)` for each.
//! 4. The audio thread also joins the work-stealing loop as worker
//!    `id = num_workers` so we don't sit idle while the pool drains.
//! 5. Once the counter has drained, the dispatcher spin-waits for every
//!    worker to bump `workers_done` (Acquire / Release pair).
//!
//! ## Real-time safety
//!
//! - No allocations on dispatch or in the worker loop.
//! - Workers `park()` between dispatches — no busy-wait while idle.  A spurious
//!   `park()` return (no job published) re-parks instead of panicking.
//! - Each worker, once at start-up, raises its thread priority and enables
//!   FTZ/DAZ — matching the audio callback thread.  A normal-priority thread
//!   can no longer preempt a worker mid-block (the audio thread spin-waits on
//!   them), and denormals are flushed identically on every thread that runs
//!   DSP, so parallel output stays consistent with sequential.
//! - The audio thread spin-waits on completion: workers run DSP that should
//!   never block; an OS preemption of a worker briefly stalls the callback
//!   but never deadlocks.
//! - Shutdown: drop sets `shutdown = true`, unparks each worker, joins.

use std::cell::UnsafeCell;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};

use crate::audio_graph::denormal::enable_flush_to_zero;

/// Per-step function pointer invoked by both the audio thread and worker
/// threads.  `worker_id` runs from `0 .. num_workers` for pool threads and
/// equals `num_workers` for the dispatching (audio) thread.
pub type StepFn<Ctx> = fn(worker_id: usize, step_idx: usize, ctx: Ctx);

/// Description of one level of work.  Lives in `PoolShared::job` between
/// dispatch and barrier; both pointers stay valid until every worker has
/// bumped `workers_done`.
struct Job<Ctx: Copy + 'static> {
    indices: *const usize,
    len: usize,
    ctx: Ctx,
    step_fn: StepFn<Ctx>,
}

impl<Ctx: Copy + 'static> Clone for Job<Ctx> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<Ctx: Copy + 'static> Copy for Job<Ctx> {}

// SAFETY: `indices` points into a slice the dispatcher pins on the stack for
// the entire dispatch; `Ctx` is `Copy + Send + Sync` by the pool bound.
unsafe impl<Ctx: Copy + Send + Sync + 'static> Send for Job<Ctx> {}
unsafe impl<Ctx: Copy + Send + Sync + 'static> Sync for Job<Ctx> {}

struct PoolShared<Ctx: Copy + Send + Sync + 'static> {
    /// Job slot — `Some(_)` between dispatch and barrier; `None` after.
    /// Written by the dispatcher with workers parked; read by workers after
    /// `unpark` (which provides the necessary happens-before).
    job: UnsafeCell<Option<Job<Ctx>>>,
    /// Next index to claim from `job.indices[0 .. job.len]`.
    next_idx: AtomicUsize,
    /// Number of workers that have finished the current job.  Reaches
    /// `num_workers` when the level is done.
    workers_done: AtomicUsize,
    /// Final shutdown flag; set in `Drop`, then every worker is unparked.
    shutdown: AtomicBool,
}

// SAFETY: the `UnsafeCell` is only mutated by the dispatcher while every
// worker is parked (and therefore not reading it).  Workers read it only
// after the `unpark` that follows the mutation.
unsafe impl<Ctx: Copy + Send + Sync + 'static> Sync for PoolShared<Ctx> {}

pub struct WorkerPool<Ctx: Copy + Send + Sync + 'static> {
    workers: Vec<JoinHandle<()>>,
    shared: Arc<PoolShared<Ctx>>,
}

impl<Ctx: Copy + Send + Sync + 'static> WorkerPool<Ctx> {
    /// Spawn `num_workers` worker threads.  Each parks immediately and waits
    /// for the first dispatch.
    pub fn new(num_workers: usize) -> Self {
        let shared = Arc::new(PoolShared {
            job: UnsafeCell::new(None),
            next_idx: AtomicUsize::new(0),
            workers_done: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
        });
        let mut workers = Vec::with_capacity(num_workers);
        for id in 0..num_workers {
            let shared_clone = Arc::clone(&shared);
            let handle = thread::Builder::new()
                .name(format!("audio-graph-worker-{id}"))
                .spawn(move || worker_loop(id, shared_clone))
                .expect("failed to spawn audio-graph worker");
            workers.push(handle);
        }
        WorkerPool { workers, shared }
    }

    /// Number of worker threads in the pool (does not include the dispatcher).
    pub fn num_workers(&self) -> usize {
        self.workers.len()
    }

    /// Dispatch `indices` for parallel processing.  Blocks the calling thread
    /// (spin-wait) until every index has been processed.
    ///
    /// The dispatcher participates as worker `num_workers`, so even with an
    /// empty pool every step still runs.
    pub fn dispatch(&self, indices: &[usize], ctx: Ctx, step_fn: StepFn<Ctx>) {
        let n = indices.len();
        if n == 0 {
            return;
        }

        if self.workers.is_empty() {
            for &step_idx in indices {
                step_fn(0, step_idx, ctx);
            }
            return;
        }

        // Set up the job slot.  Safety: every worker is parked, so we are the
        // sole accessor of `shared.job` until we unpark below.
        unsafe {
            *self.shared.job.get() = Some(Job {
                indices: indices.as_ptr(),
                len: n,
                ctx,
                step_fn,
            });
        }
        self.shared.next_idx.store(0, Ordering::Relaxed);
        // Release the workers_done reset *before* unparking so the worker's
        // first read sees zero.
        self.shared.workers_done.store(0, Ordering::Release);

        // Wake every worker.  `unpark` is a release operation relative to the
        // worker's `park`, so the writes above are visible inside the loop.
        for w in &self.workers {
            w.thread().unpark();
        }

        // Audio thread joins the work-stealing loop as worker `num_workers`.
        let audio_id = self.workers.len();
        loop {
            let idx = self.shared.next_idx.fetch_add(1, Ordering::Relaxed);
            if idx >= n {
                break;
            }
            let step_idx = unsafe { *indices.as_ptr().add(idx) };
            step_fn(audio_id, step_idx, ctx);
        }

        // Wait for every worker to finish.  Acquire matches each worker's
        // Release on `workers_done.fetch_add`.
        let target = self.workers.len();
        while self.shared.workers_done.load(Ordering::Acquire) < target {
            std::hint::spin_loop();
        }

        // Clear the job slot.  Safe: workers are now parked again.
        unsafe {
            *self.shared.job.get() = None;
        }
    }
}

impl<Ctx: Copy + Send + Sync + 'static> Drop for WorkerPool<Ctx> {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        for w in &self.workers {
            w.thread().unpark();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

/// Raise the calling worker's scheduling priority so a normal-priority thread
/// cannot preempt it mid-block.  The audio thread spin-waits on the workers at
/// the level barrier; a descheduled worker would stall the whole callback
/// (priority inversion).  Best-effort: a failure leaves the inherited priority
/// (the previous behaviour) and never panics.
#[cfg(windows)]
fn raise_worker_thread_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
    };
    // SAFETY: `GetCurrentThread` returns a pseudo-handle valid for this thread;
    // `SetThreadPriority` has no preconditions and we deliberately ignore the
    // result (priority elevation is an optimisation, not a correctness gate).
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

#[cfg(not(windows))]
fn raise_worker_thread_priority() {}

fn worker_loop<Ctx: Copy + Send + Sync + 'static>(id: usize, shared: Arc<PoolShared<Ctx>>) {
    // Per-thread audio-DSP setup, done once: flush denormals to zero (MXCSR is
    // per-thread) and lift this worker above normal-priority threads.  Both
    // mirror the audio callback thread so worker-run DSP behaves identically.
    enable_flush_to_zero();
    raise_worker_thread_priority();

    loop {
        thread::park();

        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }

        // Snapshot the job.  Safety: the dispatcher placed it before unparking
        // us, and won't mutate it again until every worker bumps `workers_done`.
        // `park()` may also return spuriously (no job published) — re-park
        // rather than panic, otherwise a dead worker would stall every later
        // dispatch on the completion barrier.
        let job = match unsafe { *shared.job.get() } {
            Some(job) => job,
            None => continue,
        };

        loop {
            let idx = shared.next_idx.fetch_add(1, Ordering::Relaxed);
            if idx >= job.len {
                break;
            }
            let step_idx = unsafe { *job.indices.add(idx) };
            (job.step_fn)(id, step_idx, job.ctx);
        }

        // Acknowledge completion.  Release pairs with the dispatcher's
        // Acquire on `workers_done`.
        shared.workers_done.fetch_add(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    // Ctx for the test: pointer to a fixed atomic counter array.
    #[derive(Clone, Copy)]
    struct TestCtx {
        counters: *const AtomicU32,
        len: usize,
    }
    unsafe impl Send for TestCtx {}
    unsafe impl Sync for TestCtx {}

    fn bump(_wid: usize, step_idx: usize, ctx: TestCtx) {
        assert!(step_idx < ctx.len);
        unsafe {
            (*ctx.counters.add(step_idx)).fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn dispatch_runs_every_index_exactly_once() {
        let counters: Vec<AtomicU32> = (0..64).map(|_| AtomicU32::new(0)).collect();
        let ctx = TestCtx {
            counters: counters.as_ptr(),
            len: counters.len(),
        };

        let pool = WorkerPool::<TestCtx>::new(3);
        let indices: Vec<usize> = (0..counters.len()).collect();
        pool.dispatch(&indices, ctx, bump);

        for c in &counters {
            assert_eq!(c.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn dispatch_with_zero_workers_still_runs() {
        let counters: Vec<AtomicU32> = (0..8).map(|_| AtomicU32::new(0)).collect();
        let ctx = TestCtx {
            counters: counters.as_ptr(),
            len: counters.len(),
        };
        let pool = WorkerPool::<TestCtx>::new(0);
        let indices: Vec<usize> = (0..counters.len()).collect();
        pool.dispatch(&indices, ctx, bump);
        for c in &counters {
            assert_eq!(c.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn multiple_dispatches_are_idempotent() {
        let counters: Vec<AtomicU32> = (0..16).map(|_| AtomicU32::new(0)).collect();
        let ctx = TestCtx {
            counters: counters.as_ptr(),
            len: counters.len(),
        };
        let pool = WorkerPool::<TestCtx>::new(2);
        let indices: Vec<usize> = (0..counters.len()).collect();
        for _ in 0..10 {
            pool.dispatch(&indices, ctx, bump);
        }
        for c in &counters {
            assert_eq!(c.load(Ordering::Relaxed), 10);
        }
    }
}
