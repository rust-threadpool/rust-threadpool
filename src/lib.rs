// Copyright 2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Abstraction of a thread pool for basic parallelism.

#![cfg_attr(feature = "scoped-pool", feature(scoped))]

#[cfg(feature = "scoped-pool")]
use std::mem;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
#[cfg(feature = "scoped-pool")]
use std::thread::JoinGuard;

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
    jobs: &'a Arc<Mutex<Receiver<Thunk<'static>>>>,
    active: bool
}

impl<'a> Sentinel<'a> {
    fn new(jobs: &'a Arc<Mutex<Receiver<Thunk<'static>>>>) -> Sentinel<'a> {
        Sentinel {
            jobs: jobs,
            active: true
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
            spawn_in_pool(self.jobs.clone())
        }
    }
}

/// A thread pool used to execute functions in parallel.
///
/// Spawns `n` worker threads and replenishes the pool if any worker threads
/// panic.
///
/// # Example
///
/// ```rust
/// use threadpool::ThreadPool;
/// use std::sync::mpsc::channel;
///
/// let pool = ThreadPool::new(4);
///
/// let (tx, rx) = channel();
/// for i in 0..8 {
///     let tx = tx.clone();
///     pool.execute(move|| {
///         tx.send(i).unwrap();
///     });
/// }
///
/// assert_eq!(rx.iter().take(8).fold(0, |a, b| a + b), 28);
/// ```
pub struct ThreadPool {
    // How the threadpool communicates with subthreads.
    //
    // This is the only such Sender, so when it is dropped all subthreads will
    // quit.
    jobs: Sender<Thunk<'static>>
}

impl ThreadPool {
    /// Spawns a new thread pool with `threads` threads.
    ///
    /// # Panics
    ///
    /// This function will panic if `threads` is 0.
    pub fn new(threads: usize) -> ThreadPool {
        assert!(threads >= 1);

        let (tx, rx) = channel::<Thunk<'static>>();
        let rx = Arc::new(Mutex::new(rx));

        // Threadpool threads
        for _ in 0..threads {
            spawn_in_pool(rx.clone());
        }

        ThreadPool { jobs: tx }
    }

    /// Executes the function `job` on a thread in the pool.
    pub fn execute<F>(&self, job: F)
        where F : FnOnce() + Send + 'static
    {
        self.jobs.send(Box::new(move || job())).unwrap();
    }
}

fn spawn_in_pool(jobs: Arc<Mutex<Receiver<Thunk<'static>>>>) {
    thread::spawn(move || {
        // Will spawn a new thread on panic unless it is cancelled.
        let sentinel = Sentinel::new(&jobs);

        loop {
            let message = {
                // Only lock jobs for the time it takes
                // to get a job, not run it.
                let lock = jobs.lock().unwrap();
                lock.recv()
            };

            match message {
                Ok(job) => job.call_box(),

                // The Threadpool was dropped.
                Err(..) => break
            }
        }

        sentinel.cancel();
    });
}

/// A scoped thread pool used to execute functions in parallel.
///
/// `ScopedPool` is different from `ThreadPool` in that:
///
/// * When dropped, it propagates panics that occur in the worker threads.
/// * It doesn't require the `'static` bound on the functions that are executed.
/// * Worker threads are joined when the `ScopedPool` is dropped.
///
/// # Example
///
/// ```rust
/// use threadpool::ScopedPool;
///
/// let mut numbers: &mut [u32] = &mut [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
///
/// // We need an extra scope to shorten the lifetime of the pool
/// {
///     let pool = ScopedPool::new(4);
///     for x in &mut numbers[..] {
///         pool.execute(move|| {
///             *x += 1;
///         });
///     }
/// }
///
/// assert_eq!(numbers, [2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
/// ```
#[cfg(feature = "scoped-pool")]
pub struct ScopedPool<'pool> {
    sender: Option<Sender<Thunk<'pool>>>,
    _guards: Vec<JoinGuard<'pool, ()>>
}

#[cfg(feature = "scoped-pool")]
impl<'pool> ScopedPool<'pool> {
    /// Spawns a new thread pool with `threads` threads.
    ///
    /// # Panics
    ///
    /// This function will panic if `threads` is 0.
    pub fn new(threads: u32) -> ScopedPool<'pool> {
        assert!(threads >= 1);

        let (sender, receiver) = channel();
        let receiver = Arc::new(Mutex::new(receiver));

        let mut guards = Vec::with_capacity(threads as usize);
        for _ in 0..threads {
            guards.push(spawn_scoped_in_pool(receiver.clone()));
        }

        ScopedPool { sender: Some(sender), _guards: guards }
    }

    /// Executes the function `job` on a thread in the pool.
    pub fn execute<F>(&self, job: F)
        where F: FnOnce() + Send + 'pool
    {
        self.sender.as_ref().unwrap().send(Box::new(job)).unwrap();
    }
}

#[cfg(feature = "scoped-pool")]
impl<'a> Drop for ScopedPool<'a> {
    fn drop(&mut self) {
        // We need to ensure that the sender is dropped before the JoinGuards
        // Otherwise the threads will be joined and wait forever in the loop
        mem::replace(&mut self.sender, None);
    }
}

#[cfg(feature = "scoped-pool")]
fn spawn_scoped_in_pool<'a>(jobs: Arc<Mutex<Receiver<Thunk<'a>>>>) -> JoinGuard<'a, ()>
{
    thread::scoped(move || {
        loop {
            let message = {
                // Only lock jobs for the time it takes
                // to get a job, not run it.
                let lock = jobs.lock().unwrap();
                lock.recv()
            };

            match message {
                Ok(job) => job.call_box(),

                // The pool was dropped.
                Err(..) => break
            }
        }
    })
}

#[cfg(test)]
mod test {
    use super::ThreadPool;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Barrier};

    const TEST_TASKS: usize = 4;

    #[test]
    fn test_works() {
        let pool = ThreadPool::new(TEST_TASKS);

        let (tx, rx) = channel();
        for _ in 0..TEST_TASKS {
            let tx = tx.clone();
            pool.execute(move|| {
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
            pool.execute(move|| -> () { panic!() });
        }

        // Ensure new threads were spawned to compensate.
        let (tx, rx) = channel();
        for _ in 0..TEST_TASKS {
            let tx = tx.clone();
            pool.execute(move|| {
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
            pool.execute(move|| {
                waiter.wait();
                panic!();
            });
        }

        drop(pool);

        // Kick off the failure.
        waiter.wait();
    }
}

#[cfg(all(test, feature = "scoped-pool"))]
mod test_scoped {
    use super::ScopedPool;
    use std::sync::mpsc::channel;

    const TEST_TASKS: u32 = 4;

    #[test]
    fn test_works_1() {
        let pool = ScopedPool::new(TEST_TASKS);

        let (tx, rx) = channel();
        for _ in 0..TEST_TASKS {
            let tx = tx.clone();
            pool.execute(move|| {
                tx.send(1).unwrap();
            });
        }

        assert_eq!(rx.iter().take(TEST_TASKS as usize).fold(0, |a, b| a + b), TEST_TASKS);
    }

    #[test]
    fn test_works_2() {
        let mut numbers: &mut [u32] = &mut [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        {
            let pool = ScopedPool::new(TEST_TASKS);
            for x in numbers.iter_mut() {
                pool.execute(move || {
                    *x += 1;
                });
            }
        }
        assert_eq!(numbers, [2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    }

    #[test]
    #[should_panic]
    fn test_zero_tasks_panic() {
        ScopedPool::new(0);
    }

    #[test]
    #[should_panic]
    fn test_panic_propagation() {
        let pool = ScopedPool::new(TEST_TASKS);

        // Panic all the existing threads.
        for _ in 0..TEST_TASKS {
            pool.execute(move|| -> () { panic!() });
        }
    }
}
