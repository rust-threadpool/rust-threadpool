use super::{auto_config, Builder, ChannelType, ThreadPool};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel};
use std::sync::{Arc, Barrier};
use std::thread::{self, sleep};
use std::time::Duration;

const TEST_TASKS: usize = 4;

fn builder() -> Builder {
    let mut b = super::builder();
    if std::env::var("TEST_CROSSBEAM").is_ok() {
        //assert!(false);
        b = b.with(ChannelType::Crossbeam);
    }
    b
}

#[test]
fn test_set_num_workers_increasing() {
    let new_thread_amount = TEST_TASKS + 8;
    let pool = builder().num_workers(TEST_TASKS).build();
    for _ in 0..TEST_TASKS {
        pool.execute(move || sleep(Duration::from_secs(23)));
    }
    sleep(Duration::from_secs(1));
    assert_eq!(pool.active_count(), TEST_TASKS);

    pool.set_num_workers(new_thread_amount);

    for _ in 0..(new_thread_amount - TEST_TASKS) {
        pool.execute(move || sleep(Duration::from_secs(23)));
    }
    sleep(Duration::from_secs(1));
    assert_eq!(pool.active_count(), new_thread_amount);

    pool.join();
}

#[test]
fn test_set_num_workers_decreasing() {
    let new_thread_amount = 2;
    let pool = builder().num_workers(TEST_TASKS).build();
    for _ in 0..TEST_TASKS {
        pool.execute(move || {
            assert_eq!(1, 1);
        });
    }
    pool.set_num_workers(new_thread_amount);
    for _ in 0..new_thread_amount {
        pool.execute(move || sleep(Duration::from_secs(23)));
    }
    sleep(Duration::from_secs(1));
    assert_eq!(pool.active_count(), new_thread_amount);

    pool.join();
}

#[test]
fn test_active_count() {
    let pool = builder().num_workers(TEST_TASKS).build();

    for _ in 0..2 * TEST_TASKS {
        pool.execute(move || loop {
            sleep(Duration::from_secs(10))
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
    let pool = builder().num_workers(TEST_TASKS).build();

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
    let _pool = builder().num_workers(0).build();
}
#[test]
#[should_panic]
fn test_zero_tasks_panic_runtime() {
    let pool = builder().num_workers(1).build();
    pool.set_num_workers(0);
}

#[test]
fn test_recovery_from_subtask_panic() {
    let pool = builder().num_workers(TEST_TASKS).build();

    // Panic all the existing threads.
    for _ in 0..TEST_TASKS {
        pool.execute(move || panic!("Ignore this panic, it must!"));
    }
    pool.join();

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
    let pool = builder().num_workers(TEST_TASKS).build();
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

    let pool = builder().num_workers(TEST_TASKS).build();
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

            let _ = tx.send(1);
        });
    }

    b0.wait();
    assert_eq!(pool.active_count(), TEST_TASKS);
    b1.wait();

    assert_eq!(rx.iter().take(test_tasks).fold(0, |a, b| a + b), test_tasks);
    pool.join();

    let atomic_active_count = pool.active_count();
    assert!(
        atomic_active_count == 0,
        "atomic_active_count: {}",
        atomic_active_count
    );
}

#[test]
fn test_shrink() {
    let test_tasks_begin = TEST_TASKS + 2;

    let pool = builder().num_workers(test_tasks_begin).build();
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
    pool.set_num_workers(TEST_TASKS);

    assert_eq!(pool.active_count(), test_tasks_begin);
    b1.wait();

    b2.wait();
    assert_eq!(pool.active_count(), TEST_TASKS);
    b3.wait();
}

#[test]
fn test_name() {
    let name = "test";
    let pool = builder().worker_name(name).num_workers(4).build();
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
    pool.set_num_workers(3);
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

    for worker_name in rx.iter().take(4) {
        assert_eq!(name, worker_name);
    }
}

#[test]
fn test_debug_print() {
    let pool = builder().num_workers(4).build();
    let debug = format!("{:?}", pool);
    assert_eq!(
        debug,
        "ThreadPool { name: None, queued_count: 0, active_count: 0, max_count: 4 }"
    );

    let pool = builder().worker_name("hello").num_workers(4).build();
    let debug = format!("{:?}", pool);
    assert_eq!(
        debug,
        "ThreadPool { name: Some(\"hello\"), queued_count: 0, active_count: 0, max_count: 4 }"
    );

    let pool = builder().num_workers(4).build();
    pool.execute(move || sleep(Duration::from_secs(5)));
    sleep(Duration::from_secs(1));
    let debug = format!("{:?}", pool);
    assert_eq!(
        debug,
        "ThreadPool { name: None, queued_count: 0, active_count: 1, max_count: 4 }"
    );
}

#[test]
fn test_repeate_join() {
    let pool = builder()
        .worker_name("repeate join test")
        .num_workers(8)
        .build();
    let test_count = Arc::new(AtomicUsize::new(0));

    for _ in 0..42 {
        let test_count = test_count.clone();
        pool.execute(move || {
            sleep(Duration::from_secs(2));
            test_count.fetch_add(1, Ordering::Release);
        });
    }

    println!("{:?}", pool);
    pool.join();
    assert_eq!(42, test_count.load(Ordering::Acquire));

    for _ in 0..42 {
        let test_count = test_count.clone();
        pool.execute(move || {
            sleep(Duration::from_secs(2));
            test_count.fetch_add(1, Ordering::Relaxed);
        });
    }
    pool.join();
    assert_eq!(84, test_count.load(Ordering::Relaxed));
}

#[test]
fn test_multi_join() {
    use std::sync::mpsc::TryRecvError::*;

    // Toggle the following lines to debug the deadlock
    fn error(_s: String) {
        //use ::std::io::Write;
        //let stderr = ::std::io::stderr();
        //let mut stderr = stderr.lock();
        //stderr.write(&_s.as_bytes()).is_ok();
    }

    let pool0 = builder()
        .worker_name("multi join pool0")
        .num_workers(4)
        .build();
    let pool1 = builder()
        .worker_name("multi join pool1")
        .num_workers(4)
        .build();
    let (tx, rx) = channel();

    for i in 0..8 {
        let pool1 = pool1.clone();
        let pool0_ = pool0.clone();
        let tx = tx.clone();
        pool0.execute(move || {
            pool1.execute(move || {
                error(format!("p1: {} -=- {:?}\n", i, pool0_));
                pool0_.join();
                error(format!("p1: send({})\n", i));
                tx.send(i).expect("send i from pool1 -> main");
            });
            error(format!("p0: {}\n", i));
        });
    }
    drop(tx);

    assert_eq!(rx.try_recv(), Err(Empty));
    error(format!("{:?}\n{:?}\n", pool0, pool1));
    pool0.join();
    error(format!("pool0.join() complete =-= {:?}", pool1));
    pool1.join();
    error("pool1.join() complete\n".into());
    assert_eq!(
        rx.iter().fold(0, |acc, i| acc + i),
        0 + 1 + 2 + 3 + 4 + 5 + 6 + 7
    );
}

#[test]
fn test_empty_pool() {
    // Joining an empty pool must return imminently
    let pool = auto_config();

    pool.join();

    assert!(true);
}

#[test]
fn test_no_fun_or_joy() {
    // What happens when you keep adding jobs after a join

    fn sleepy_function() {
        sleep(Duration::from_secs(6));
    }

    //let pool = ThreadPool::with_name("no fun or joy".into(), 8);
    let pool = builder()
        .worker_name("no fun or joy")
        .num_workers(8)
        .build();

    pool.execute(sleepy_function);

    let p_t = pool.clone();
    thread::spawn(move || {
        (0..23).map(|_| p_t.execute(sleepy_function)).count();
    });

    pool.join();
}

#[test]
fn test_clone() {
    let pool = builder()
        .worker_name("clone example")
        .num_workers(2)
        .build();

    // This batch of jobs will occupy the pool for some time
    for _ in 0..6 {
        pool.execute(move || {
            sleep(Duration::from_secs(2));
        });
    }

    // The following jobs will be inserted into the pool in a random fashion
    let t0 = {
        let pool = pool.clone();
        thread::spawn(move || {
            // wait for the first batch of tasks to finish
            pool.join();

            let (tx, rx) = channel();
            for i in 0..42 {
                let tx = tx.clone();
                pool.execute(move || {
                    tx.send(i).expect("channel will be waiting");
                });
            }
            drop(tx);
            rx.iter()
                .fold(0, |accumulator, element| accumulator + element)
        })
    };
    let t1 = {
        let pool = pool.clone();
        thread::spawn(move || {
            // wait for the first batch of tasks to finish
            pool.join();

            let (tx, rx) = channel();
            for i in 1..12 {
                let tx = tx.clone();
                pool.execute(move || {
                    tx.send(i).expect("channel will be waiting");
                });
            }
            drop(tx);
            rx.iter()
                .fold(1, |accumulator, element| accumulator * element)
        })
    };

    assert_eq!(
        861,
        t0.join()
            .expect("thread 0 will return after calculating additions",)
    );
    assert_eq!(
        39916800,
        t1.join()
            .expect("thread 1 will return after calculating multiplications",)
    );
}

#[test]
fn test_sync_shared_data() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<super::ThreadPoolSharedData>();
}

#[test]
fn test_send_shared_data() {
    fn assert_send<T: Send>() {}
    assert_send::<super::ThreadPoolSharedData>();
}

#[test]
fn test_send() {
    fn assert_send<T: Send>() {}
    assert_send::<ThreadPool>();
}

#[test]
fn test_cloned_eq() {
    let a = auto_config();

    assert_eq!(a, a.clone());
}

#[test]
/// The scenario is joining threads should not be stuck once their wave
/// of joins has completed. So once one thread joining on a pool has
/// succeded other threads joining on the same pool must get out even if
/// the thread is used for other jobs while the first group is finishing
/// their join
///
/// In this example this means the waiting threads will exit the join in
/// groups of four because the waiter pool has four workers.
fn test_join_wavesurfer() {
    let n_cycles = 4;
    let n_workers = 4;
    let (tx, rx) = channel();
    let builder = builder()
        .num_workers(n_workers)
        .worker_name("join wavesurfer");
    let p_waiter = builder.clone().build();
    let p_clock = builder.build();

    let barrier = Arc::new(Barrier::new(3));
    let wave_clock = Arc::new(AtomicUsize::new(0));
    let clock_thread = {
        let barrier = barrier.clone();
        let wave_clock = wave_clock.clone();
        thread::spawn(move || {
            barrier.wait();
            for wave_num in 0..n_cycles {
                wave_clock.store(wave_num, Ordering::SeqCst);
                sleep(Duration::from_secs(1));
            }
        })
    };

    {
        let barrier = barrier.clone();
        p_clock.execute(move || {
            barrier.wait();
            // this sleep is for stabilisation on weaker platforms
            sleep(Duration::from_millis(100));
        });
    }

    // prepare three waves of jobs
    for i in 0..3 * n_workers {
        let p_clock = p_clock.clone();
        let tx = tx.clone();
        let wave_clock = wave_clock.clone();
        p_waiter.execute(move || {
            let now = wave_clock.load(Ordering::SeqCst);
            p_clock.join();
            // submit jobs for the second wave
            p_clock.execute(|| sleep(Duration::from_secs(1)));
            let clock = wave_clock.load(Ordering::SeqCst);
            tx.send((now, clock, i)).unwrap();
        });
    }
    println!("all scheduled at {}", wave_clock.load(Ordering::SeqCst));
    barrier.wait();

    p_clock.join();
    //p_waiter.join();

    drop(tx);
    let mut hist = vec![0; n_cycles];
    let mut data = vec![];
    for (now, after, i) in rx.iter() {
        let mut dur = after - now;
        if dur >= n_cycles - 1 {
            dur = n_cycles - 1;
        }
        hist[dur] += 1;

        data.push((now, after, i));
    }
    for (i, n) in hist.iter().enumerate() {
        println!(
            "\t{}: {} {}",
            i,
            n,
            &*(0..*n).fold("".to_owned(), |s, _| s + "*")
        );
    }
    assert!(data.iter().all(|&(cycle, stop, i)| if i < n_workers {
        cycle == stop
    } else {
        cycle < stop
    }));

    clock_thread.join().unwrap();
}

#[test]
fn bound_flow() {
    let pool = builder().queue_size(4).num_workers(2).build();

    for _ in 0..6 {
        pool.execute(sleepy_function);
    }
    sleep(Duration::from_secs(1));
    assert_eq!(2, pool.active_count(), "{}", pool.active_count());
    assert_eq!(4, pool.queued_count());

    pool.join();

    fn sleepy_function() {
        sleep(Duration::from_secs(3));
    }
}
