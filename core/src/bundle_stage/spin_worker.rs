//! A dedicated thread that runs one borrowed job alongside the calling thread.
//!

use std::{
    hint::spin_loop,
    panic::{self, AssertUnwindSafe},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

type Job = Box<dyn FnOnce() + Send + 'static>;

const IDLE_PARK: Duration = Duration::from_millis(1);

struct Shared {
    job: Mutex<Option<Job>>,
    /// Set by the caller when a job is published; cleared by the worker once it has finished it.
    busy: AtomicBool,
    job_panicked: AtomicBool,
    /// While set, the worker spins waiting for work instead of parking.
    active: AtomicBool,
    exit: AtomicBool,
}

pub(crate) struct SpinWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl SpinWorker {
    pub(crate) fn new(name: &str) -> Self {
        let shared = Arc::new(Shared {
            job: Mutex::new(None),
            busy: AtomicBool::new(false),
            job_panicked: AtomicBool::new(false),
            active: AtomicBool::new(false),
            exit: AtomicBool::new(false),
        });
        let thread = {
            let shared = shared.clone();
            thread::Builder::new()
                .name(name.to_string())
                .spawn(move || Self::run(&shared))
                .unwrap()
        };
        Self {
            shared,
            thread: Some(thread),
        }
    }

    fn run(shared: &Shared) {
        while !shared.exit.load(Ordering::Relaxed) {
            if shared.busy.load(Ordering::Acquire) {
                if let Some(job) = shared.job.lock().unwrap().take() {
                    if panic::catch_unwind(AssertUnwindSafe(job)).is_err() {
                        shared.job_panicked.store(true, Ordering::Relaxed);
                    }
                }
                shared.busy.store(false, Ordering::Release);
            } else if shared.active.load(Ordering::Relaxed) {
                spin_loop();
            } else {
                thread::park_timeout(IDLE_PARK);
            }
        }
    }

    fn unpark(&self) {
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }

    pub(crate) fn set_active(&self, active: bool) {
        let was_active = self.shared.active.swap(active, Ordering::Relaxed);
        if active && !was_active {
            self.unpark();
        }
    }

    #[cfg_attr(not(feature = "build_validator"), allow(dead_code))]
    pub(crate) fn run_alongside<'a, R>(
        &self,
        job: impl FnOnce() + Send + 'a,
        main: impl FnOnce() -> R,
    ) -> R {
        let job: Box<dyn FnOnce() + Send + 'a> = Box::new(job);
        // SAFETY: `job` may borrow data that lives for `'a`. This function does not return, nor
        // unwind past `_wait`, until the worker has finished running (or dropped) `job`, so those
        // borrows remain valid for as long as the worker can use them.
        let job: Job = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + 'a>, Job>(job) };
        *self.shared.job.lock().unwrap() = Some(job);
        self.shared.busy.store(true, Ordering::Release);
        if !self.shared.active.load(Ordering::Relaxed) {
            self.unpark();
        }

        let _wait = WaitForJob(&self.shared);
        let result = main();
        drop(_wait);

        if self.shared.job_panicked.swap(false, Ordering::Relaxed) {
            panic!("spin worker job panicked");
        }
        result
    }
}

#[cfg_attr(not(feature = "build_validator"), allow(dead_code))]
struct WaitForJob<'a>(&'a Shared);

impl Drop for WaitForJob<'_> {
    fn drop(&mut self) {
        while self.0.busy.load(Ordering::Acquire) {
            spin_loop();
        }
    }
}

impl Drop for SpinWorker {
    fn drop(&mut self) {
        self.shared.exit.store(true, Ordering::Relaxed);
        self.unpark();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
