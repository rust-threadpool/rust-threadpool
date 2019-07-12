# Design Threadpool 2.0

## Requirements and Goals

* Compatible with rust 1.13
* Unify wording of internal symbols and functions
* Access to the pool handle from within the task
* Optional support for crossbeam-channel?
    * Maybe integrated into libstd anyways
* Support for batches
    * Rename threads with each batch
    * Abort batches
* Respect Hyperthreading and NUMA borders
    * For security or performance reasons
* Optional: add a scope function

## Timetable

1. Publish this draft and request feedback from the community
2. Implement alpha and request testers
3. Maybe adjust and release beta
4. Publish release candidate to crates.io
5. Publish 2.0

## API examples

These examples are up for discussion.

### Simpler construction

Remove the need to import the `Builder` and `ThreadPool` structs unless to pass the objects around functions.
Thereby cleaning up the code a bit.

```rust
// for rust 2015
extern crate threadpool;

let default_pool = threadpool::new().unwrap();
let custom_pool = threadpool::builder()
                    .set_thread_label("my worker")
                    .build().unwrap();
```

### Schedule new tasks on the go

```rust
let pool = threadpool::new().unwrap();
let (sender, receiver) = channel();
for o in 0..5 {
    let sender = sender.clone();
    pool.execute(move |pool| {
        sender.send(Outer(o)).unwrap();
        for i in 0..5 {
            pool.execute(|_| {
                sender.send(Inner(o, i)).unwrap();
            });
        }
    });
}

// This will block until all Outer(o) are submitted, some Inner(o, i) will still be on the way as they are scheduled in the next generation
pool.join();

// Complete result of the tasks
let result = receiver.iter().collect::<Vec<_>>();

enum Marker {
    Outer(usize), Inner(usize, usize),
}
```

### Set affinity with builder

1. Respect the affinity set by the user via processor mask (taskset) or cgroup
2. Allow the user to disable the hyper threads (dual core with HT has layout full cores with ids 0 and 2 whereas the cores 1 and 3 are reduced cores) to run the workers on full cores only.

Not using the hyper threads can be useful when running untrusted code in a vm residing inside that thread.

```rust
/// Unless specified by the user using any mode except `AllNodes` will limit the number of workers to the size of the NUMA group or half of it when combined with `allow_hyperthreads(false)`.
enum NumaOptions {
    /// Do not respect the borders.
    /// n-threads = autodetect
    AllNodes,
    /// Check current node on construction and stay on it
    SameNode,
    /// Spawn Workers in one specific node
    SpecificNode(usize),
    /// Spread the workers over the specified NUMA nodes
    SpreadNodes(Vec<usize>),
}

let pool = threadpool::builder()
            // existing functionality
            .set_stack_size(4096)
            // dis-/allows the use of the hyper thread cores: default = true
            .allow_hyperthreads(false)
            // Could lead to unresolveable requirements at runtime
            .respect_numa_nodes(SameNode)
            .build()
            .except("Runtime construction error");
```

### Opt for crossbeam::channel instead of std::sync::mpsc::channel

Enable this functionality with the crate feature flag `crossbeam`.

An open question is whether or not it has to be enabled with one of these lines as well:

```rust
let pool = threadpool::builder().build::<threadpool::Crossbeam>().unwrap();
let pool: ThreadPool<threadpool::Crossbeam> = threadpool::new().unwrap();
```

### Shutdown operation


### Using Batches and cancelling batches

The question is how to cancel certain batches?

#### Name based batches

This method would work with the name of the tasks.

```rust
enum TaskLabels<F> {
    /// Reuse the last label
    UnlabeledTask(F),
    /// Rename the worker for this task only
    SingleLabeledTask(String, F),
    /// Relabel all tasks
    NewLabel(String, F),
}

let pool = threadpool::builder()
            .set_thread_label("Idle Workers")
            .build().unwrap();

// This will translate into `UnlabeledTask`
// The thread will run as "Idle Workers"
pool.execute(|_| some_operations());

// The thread will run as "Special Label" and afterwards return to normal
pool.execute(SingleLabeledTask("Special Label", |_| some_operations()));

// The thread will run as "Next Label"
pool.execute(NewLabel("Next Label", |_| some_operations()));
// The thread running this thread will relabel itself to "Next Label" as soon as it starts the task
pool.execute(|_| some_operations());
```

#### Generation based batches

This solution seems more elegant as it allows the user to reuse the names while cancelling specific parts.

Question: should the execute function return the generation?

```rust
#[derive(Debug, PartialEq)]
/// This GenerationId may in very rare cases wrap around
struct Generation(usize);

let pool = threadpool::new().unwrap();

// Assuming the same TaskLabels as before
let generation_0 = pool.execute(|_| some_operations());
// Returns the same generation
let generation_0 = pool.execute(|_| some_operations());
// Returns the same generation
let generation_0 = pool.get_current_generation();

// A new generation of tasks
let generation_1 = pool.next_generation();
// Returns the same generation
let generation_1 = pool.get_current_generation();
// Returns the same generation
let generation_1 = pool.execute(|_| some_operations());

// A new generation of tasks
let generation_2 = pool.next_generation();
// Returns the same generation
let generation_2 = pool.get_current_generation();
let generation_2 = pool.execute(|_| some_operations());

// Cancel one generation without affecting the others.
// May have no effect if the tasks of that generation are already finished.
pool.cancel_generation(generation_1);
```

### Scope function

Inside the scope we can not create new generations.
Aborting previous generations is fine.

Question: Should the running tasks of `Generation(1)` be aborted on panic?

```rust
let pool = threadpool::new().unwrap();
// generation is now 0 and normal tasks may be submitted

let mut list = vec![1, 2];

// this will create the new generation 1
let (generation_2, result) = pool.scope(|bound_pool_handle| {
    list.push(3);

    // inside we can only allow non-mutable access
    let list_view = &list[..];

    bound_pool_handle.execute(|bound_pool_handle| {
        for _ in 0..5 {
            bound_pool_handle.execute(|_| {
                some_operations(list_view);
            });
        }
    });

    Some(42)
});
// now the generation is 2

assert_eq!(Some(42), result);
assert_eq!(vec![1, 2, 3], list);
```

## Upgrade Guide

Once everything settles write a small Upgrade Guide for users.
