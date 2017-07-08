// Copyright 2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A thread pool used to execute functions in parallel.
//!
//! Spawns a specified number of worker threads and replenishes the pool if any worker threads
//! panic.
//!
//! # Examples
//!
//! ## Synchronized with a channel
//!
//! Every thread sends one message over the channel, which then is collected with the `take()`.
//!
//! ```
//! use threadpool::ThreadPool;
//! use std::sync::mpsc::channel;
//!
//! let n_workers = 4;
//! let n_jobs = 8;
//! let pool = ThreadPool::new(n_workers);
//!
//! let (tx, rx) = channel();
//! for _ in 0..n_jobs {
//!     let tx = tx.clone();
//!     pool.execute(move|| {
//!         tx.send(1).unwrap();
//!     });
//! }
//!
//! assert_eq!(rx.iter().take(n_jobs).fold(0, |a, b| a + b), 8);
//! ```
//!
//! ## Synchronized with a barrier
//!
//! Keep in mind, if a barrier synchronizes more jobs than you have workers in the pool,
//! you will end up with a [deadlock](https://en.wikipedia.org/wiki/Deadlock)
//! at the barrier which is [not considered unsafe]
//! (http://doc.rust-lang.org/reference.html#behavior-not-considered-unsafe).
//!
//! ```
//! use threadpool::ThreadPool;
//! use std::sync::{Arc, Barrier};
//! use std::sync::atomic::{AtomicUsize, Ordering};
//!
//! // create at least as many workers as jobs or you will deadlock yourself
//! let n_workers = 42;
//! let n_jobs = 23;
//! let pool = ThreadPool::new(n_workers);
//! let an_atomic = Arc::new(AtomicUsize::new(0));
//!
//! assert!(n_jobs <= n_workers, "too many jobs, will deadlock");
//!
//! // create a barrier that wait all jobs plus the starter thread
//! let barrier = Arc::new(Barrier::new(n_jobs + 1));
//! for _ in 0..n_jobs {
//!     let barrier = barrier.clone();
//!     let an_atomic = an_atomic.clone();
//!
//!     pool.execute(move|| {
//!         // do the heavy work
//!         an_atomic.fetch_add(1, Ordering::Relaxed);
//!
//!         // then wait for the other threads
//!         barrier.wait();
//!     });
//! }
//!
//! // wait for the threads to finish the work
//! barrier.wait();
//! assert_eq!(an_atomic.load(Ordering::SeqCst), 23);
//! ```

use std::fmt;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{Builder, panicking};

trait FnBox {
    fn call_box(self: Box<Self>);
}

impl<F: FnOnce()> FnBox for F {
    fn call_box(self: Box<F>) {
        (*self)()
    }
}

type Thunk<'a> = Box<FnBox + Send + 'a>;

struct Sentinel<'a> {
    name: Option<String>,
    jobs: &'a Arc<Mutex<Receiver<Thunk<'static>>>>,
    shared_data: &'a Arc<ThreadPoolSharedData>,
    active: bool,
}

impl<'a> Sentinel<'a> {
    fn new(name: Option<String>,
           jobs: &'a Arc<Mutex<Receiver<Thunk<'static>>>>,
           shared_data: &'a Arc<ThreadPoolSharedData>)
           -> Sentinel<'a> {
        Sentinel {
            name: name,
            jobs: jobs,
            shared_data: shared_data,
            active: true,
        }
    }

    // Cancel and destroy this sentinel.
    fn cancel(mut self) {
        self.active = false;
    }
}

impl<'a> Drop for Sentinel<'a> {
    fn drop(&mut self) {
        if self.active {
            self.shared_data.active_count.fetch_sub(1, Ordering::SeqCst);
            if panicking() {
                self.shared_data.panic_count.fetch_add(1, Ordering::SeqCst);
            }
            spawn_in_pool(self.name.clone(),
                          self.jobs.clone(),
                          self.shared_data.clone())
        }
    }
}

struct ThreadPoolSharedData {
    active_count: AtomicUsize,
    max_thread_count: AtomicUsize,
    panic_count: AtomicUsize,
}

/// Abstraction of a thread pool for basic parallelism.
#[derive(Clone)]
pub struct ThreadPool {
    // How the threadpool communicates with subthreads.
    //
    // This is the only such Sender, so when it is dropped all subthreads will
    // quit.
    name: Option<String>,
    jobs: Sender<Thunk<'static>>,
    job_receiver: Arc<Mutex<Receiver<Thunk<'static>>>>,
    shared_data: Arc<ThreadPoolSharedData>,
}

impl ThreadPool {
    /// Spawns a new thread pool with `num_threads` threads.
    ///
    /// # Panics
    ///
    /// This function will panic if `num_threads` is 0.
    pub fn new(num_threads: usize) -> ThreadPool {
        ThreadPool::new_pool(None, num_threads)
    }

    /// Spawns a new thread pool with `num_threads` threads. Each thread will have the
    /// [name][thread name] `name`.
    ///
    /// # Panics
    ///
    /// This function will panic if `num_threads` is 0.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::sync::mpsc::sync_channel;
    /// use std::thread;
    /// use threadpool::ThreadPool;
    ///
    /// let (tx, rx) = sync_channel(0);
    /// let mut pool = ThreadPool::new_with_name("worker".into(), 2);
    /// for _ in 0..2 {
    ///     let tx = tx.clone();
    ///     pool.execute(move || {
    ///         let name = thread::current().name().unwrap().to_owned();
    ///         tx.send(name).unwrap();
    ///     });
    /// }
    ///
    /// for thread_name in rx.iter().take(2) {
    ///     assert_eq!("worker", thread_name);
    /// }
    /// ```
    ///
    /// [thread name]: https://doc.rust-lang.org/std/thread/struct.Thread.html#method.name
    pub fn new_with_name(name: String, num_threads: usize) -> ThreadPool {
        ThreadPool::new_pool(Some(name), num_threads)
    }

    #[inline]
    fn new_pool(name: Option<String>, num_threads: usize) -> ThreadPool {
        assert!(num_threads >= 1);

        let (tx, rx) = channel::<Thunk<'static>>();
        let rx = Arc::new(Mutex::new(rx));

        let shared_data = Arc::new(ThreadPoolSharedData {
            active_count: AtomicUsize::new(0),
            max_thread_count: AtomicUsize::new(num_threads),
            panic_count: AtomicUsize::new(0),
        });

        // Threadpool threads
        for _ in 0..num_threads {
            spawn_in_pool(name.clone(),
                          rx.clone(),
                          shared_data.clone());
        }

        ThreadPool {
            name: name,
            jobs: tx,
            job_receiver: rx,
            shared_data: shared_data,
        }
    }

    /// Executes the function `job` on a thread in the pool.
    pub fn execute<F>(&self, job: F)
        where F: FnOnce() + Send + 'static
    {
        self.jobs.send(Box::new(job)).unwrap();
    }

    /// Returns the number of currently active threads.
    ///
    /// # Examples
    ///
    /// ```
    /// use threadpool::ThreadPool;
    /// use std::time::Duration;
    /// use std::thread::sleep;
    ///
    /// let num_threads = 10;
    /// let pool = ThreadPool::new(num_threads);
    /// for _ in 0..num_threads {
    ///     pool.execute(move || {
    ///         sleep(Duration::from_secs(5));
    ///     });
    /// }
    /// sleep(Duration::from_secs(1));
    /// assert_eq!(pool.active_count(), num_threads);
    /// ```
    pub fn active_count(&self) -> usize {
        self.shared_data.active_count.load(Ordering::SeqCst)
    }

    /// Returns the number of created threads
    pub fn max_count(&self) -> usize {
        self.shared_data.max_thread_count.load(Ordering::Relaxed)
    }

    /// Returns the number of panicked threads over the lifetime of the pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use threadpool::ThreadPool;
    /// use std::time::Duration;
    /// use std::thread::sleep;
    ///
    /// let num_threads = 10;
    /// let pool = ThreadPool::new(num_threads);
    /// for _ in 0..num_threads {
    ///     pool.execute(move || {
    ///         panic!()
    ///     });
    /// }
    /// sleep(Duration::from_secs(1));
    /// assert_eq!(pool.panic_count(), num_threads);
    /// ```
    pub fn panic_count(&self) -> usize {
        self.shared_data.panic_count.load(Ordering::Relaxed)
    }

    /// **Deprecated: Use `ThreadPool::set_num_threads`**
    #[deprecated(since = "1.3.0", note = "use ThreadPool::set_num_threads")]
    pub fn set_threads(&mut self, num_threads: usize) {
        self.set_num_threads(num_threads)
    }

    /// Sets the number of worker-threads to use as `num_threads`.
    /// Can be used to change the threadpool size during runtime.
    /// Will not abort already running or waiting threads.
    ///
    /// # Panics
    ///
    /// This function will panic if `num_threads` is 0.
    pub fn set_num_threads(&mut self, num_threads: usize) {
        assert!(num_threads >= 1);
        let prev_num_threads = self.shared_data.max_thread_count.swap(num_threads, Ordering::Release);
        if let Some(num_spawn) = num_threads.checked_sub(prev_num_threads) {
            // Spawn new threads
            for _ in 0..num_spawn {
                spawn_in_pool(self.name.clone(),
                              self.job_receiver.clone(),
                              self.shared_data.clone());
            }
        }
    }
}


impl fmt::Debug for ThreadPool {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "ThreadPool {{ name: {:?}, active_count: {}, max_count: {} }}",
               self.name,
               self.active_count(),
               self.max_count())
    }
}


fn spawn_in_pool(name: Option<String>,
                 jobs: Arc<Mutex<Receiver<Thunk<'static>>>>,
                 shared_data: Arc<ThreadPoolSharedData>) {
    let mut builder = Builder::new();
    if let Some(ref name) = name {
        builder = builder.name(name.clone());
    }
    builder.spawn(move || {
            // Will spawn a new thread on panic unless it is cancelled.
            let sentinel = Sentinel::new(name,
                                         &jobs,
                                         &shared_data);

            loop {
                // Shutdown this thread if the pool has become smaller
                let thread_counter_val = shared_data.active_count.load(Ordering::Acquire);
                let max_thread_count_val = shared_data.max_thread_count.load(Ordering::Relaxed);
                if thread_counter_val >= max_thread_count_val {
                    break;
                }
                let message = {
                    // Only lock jobs for the time it takes
                    // to get a job, not run it.
                    let lock = jobs.lock().unwrap();
                    lock.recv()
                };

                let job = match message {
                    Ok(job) => job,
                    // The ThreadPool was dropped.
                    Err(..) => break,
                };
                // Do not allow IR around the job execution
                shared_data.active_count.fetch_add(1, Ordering::SeqCst);

                job.call_box();

                shared_data.active_count.fetch_sub(1, Ordering::SeqCst);
            }

            sentinel.cancel();
        })
        .unwrap();
}

#[cfg(test)]
mod test {
    use super::ThreadPool;
    use std::sync::mpsc::{sync_channel, channel};
    use std::sync::{Arc, Barrier};
    use std::thread::{self, sleep};
    use std::time::Duration;

    const TEST_TASKS: usize = 4;

    #[test]
    fn test_set_num_threads_increasing() {
        let new_thread_amount = TEST_TASKS + 8;
        let mut pool = ThreadPool::new(TEST_TASKS);
        for _ in 0..TEST_TASKS {
            pool.execute(move || {
                loop {
                    sleep(Duration::from_secs(10))
                }
            });
        }
        pool.set_num_threads(new_thread_amount);
        for _ in 0..(new_thread_amount - TEST_TASKS) {
            pool.execute(move || {
                loop {
                    sleep(Duration::from_secs(10))
                }
            });
        }
        sleep(Duration::from_secs(1));
        assert_eq!(pool.active_count(), new_thread_amount);
    }

    #[test]
    fn test_set_num_threads_decreasing() {
        let new_thread_amount = 2;
        let mut pool = ThreadPool::new(TEST_TASKS);
        for _ in 0..TEST_TASKS {
            pool.execute(move || {
                1 + 1;
            });
        }
        pool.set_num_threads(new_thread_amount);
        for _ in 0..new_thread_amount {
            pool.execute(move || {
                loop {
                    sleep(Duration::from_secs(10))
                }
            });
        }
        sleep(Duration::from_secs(1));
        assert_eq!(pool.active_count(), new_thread_amount);
    }

    #[test]
    fn test_active_count() {
        let pool = ThreadPool::new(TEST_TASKS);
        for _ in 0..TEST_TASKS {
            pool.execute(move || {
                loop {
                    sleep(Duration::from_secs(10))
                }
            });
        }
        sleep(Duration::from_secs(1));
        let active_count = pool.active_count();
        assert_eq!(active_count, TEST_TASKS);
        let initialized_count = pool.max_count();
        assert_eq!(initialized_count, TEST_TASKS);
    }

    #[test]
    fn test_works() {
        let pool = ThreadPool::new(TEST_TASKS);

        let (tx, rx) = channel();
        for _ in 0..TEST_TASKS {
            let tx = tx.clone();
            pool.execute(move || {
                tx.send(1).unwrap();
            });
        }

        assert_eq!(rx.iter().take(TEST_TASKS).fold(0, |a, b| a + b), TEST_TASKS);
    }

    #[test]
    #[should_panic]
    fn test_zero_tasks_panic() {
        ThreadPool::new(0);
    }

    #[test]
    fn test_recovery_from_subtask_panic() {
        let pool = ThreadPool::new(TEST_TASKS);

        // Panic all the existing threads.
        for _ in 0..TEST_TASKS {
            pool.execute(move || { panic!() });
        }
        sleep(Duration::from_secs(1));

        assert_eq!(pool.panic_count(), TEST_TASKS);

        // Ensure new threads were spawned to compensate.
        let (tx, rx) = channel();
        for _ in 0..TEST_TASKS {
            let tx = tx.clone();
            pool.execute(move || {
                tx.send(1).unwrap();
            });
        }

        assert_eq!(rx.iter().take(TEST_TASKS).fold(0, |a, b| a + b), TEST_TASKS);
    }

    #[test]
    fn test_should_not_panic_on_drop_if_subtasks_panic_after_drop() {

        let pool = ThreadPool::new(TEST_TASKS);
        let waiter = Arc::new(Barrier::new(TEST_TASKS + 1));

        // Panic all the existing threads in a bit.
        for _ in 0..TEST_TASKS {
            let waiter = waiter.clone();
            pool.execute(move || {
                waiter.wait();
                panic!("Ignore this panic, it should!");
            });
        }

        drop(pool);

        // Kick off the failure.
        waiter.wait();
    }

    #[test]
    fn test_massive_task_creation() {
        let test_tasks = 4_200_000;

        let pool = ThreadPool::new(TEST_TASKS);
        let b0 = Arc::new(Barrier::new(TEST_TASKS + 1));
        let b1 = Arc::new(Barrier::new(TEST_TASKS + 1));

        let (tx, rx) = channel();

        for i in 0..test_tasks {
            let tx = tx.clone();
            let (b0, b1) = (b0.clone(), b1.clone());

            pool.execute(move || {

                // Wait until the pool has been filled once.
                if i < TEST_TASKS {
                    b0.wait();
                    // wait so the pool can be measured
                    b1.wait();
                }

                tx.send(1).is_ok();
            });
        }

        b0.wait();
        assert_eq!(pool.active_count(), TEST_TASKS);
        b1.wait();

        assert_eq!(rx.iter().take(test_tasks).fold(0, |a, b| a + b), test_tasks);
        // `iter().take(test_tasks).fold` may be faster than the last thread finishing itself, so
        // values of 0 or 1 are ok.
        let atomic_active_count = pool.active_count();
        assert!(atomic_active_count <= 1, "atomic_active_count: {}", atomic_active_count);
    }

    #[test]
    fn test_shrink() {
        let test_tasks_begin = TEST_TASKS + 2;

        let mut pool = ThreadPool::new(test_tasks_begin);
        let b0 = Arc::new(Barrier::new(test_tasks_begin + 1));
        let b1 = Arc::new(Barrier::new(test_tasks_begin + 1));

        for _ in 0..test_tasks_begin {
            let (b0, b1) = (b0.clone(), b1.clone());
            pool.execute(move || {
                b0.wait();
                b1.wait();
            });
        }

        let b2 = Arc::new(Barrier::new(TEST_TASKS + 1));
        let b3 = Arc::new(Barrier::new(TEST_TASKS + 1));

        for _ in 0..TEST_TASKS {
            let (b2, b3) = (b2.clone(), b3.clone());
            pool.execute(move || {
                b2.wait();
                b3.wait();
            });
        }

        b0.wait();
        pool.set_num_threads(TEST_TASKS);

        assert_eq!(pool.active_count(), test_tasks_begin);
        b1.wait();


        b2.wait();
        assert_eq!(pool.active_count(), TEST_TASKS);
        b3.wait();
    }

    #[test]
    fn test_name() {
        let name = "test";
        let mut pool = ThreadPool::new_with_name(name.to_owned(), 2);
        let (tx, rx) = sync_channel(0);

        // initial thread should share the name "test"
        for _ in 0..2 {
            let tx = tx.clone();
            pool.execute(move || {
                let name = thread::current().name().unwrap().to_owned();
                tx.send(name).unwrap();
            });
        }

        // new spawn thread should share the name "test" too.
        pool.set_num_threads(3);
        let tx_clone = tx.clone();
        pool.execute(move || {
            let name = thread::current().name().unwrap().to_owned();
            tx_clone.send(name).unwrap();
            panic!();
        });

        // recover thread should share the name "test" too.
        pool.execute(move || {
            let name = thread::current().name().unwrap().to_owned();
            tx.send(name).unwrap();
        });

        for thread_name in rx.iter().take(4) {
            assert_eq!(name, thread_name);
        }
    }

    #[test]
    fn test_debug() {
        let pool = ThreadPool::new(4);
        let debug = format!("{:?}", pool);
        assert_eq!(debug, "ThreadPool { name: None, active_count: 0, max_count: 4 }");

        let pool = ThreadPool::new_with_name("hello".into(), 4);
        let debug = format!("{:?}", pool);
        assert_eq!(debug, "ThreadPool { name: Some(\"hello\"), active_count: 0, max_count: 4 }");

        let pool = ThreadPool::new(4);
        pool.execute(move || {
            sleep(Duration::from_secs(5))
        });
        sleep(Duration::from_secs(1));
        let debug = format!("{:?}", pool);
        assert_eq!(debug, "ThreadPool { name: None, active_count: 1, max_count: 4 }");
    }
}
