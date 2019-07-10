# threadpool

A thread pool for running a number of jobs on a fixed set of worker threads.

[![Build Status](https://travis-ci.org/rust-threadpool/rust-threadpool.svg?branch=master)](https://travis-ci.org/rust-threadpool/rust-threadpool)
[![doc.rs](https://docs.rs/threadpool/badge.svg)](https://docs.rs/threadpool)

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
threadpool = "1.0"
```

and this to your crate root:

```rust
extern crate threadpool;
```

## Minimal requirements

This crate requires Rust >= 1.13.0

## Memory performance

Rust [1.32.0](https://blog.rust-lang.org/2019/01/17/Rust-1.32.0.html) has switched from jemalloc to the operating systems allocator.
While this enables more plattforms for some workloads this means some performance loss.

To regain the performance consider enableing the [jemallocator crate](https://crates.io/crates/jemallocator).

## Similar libraries

* [rayon (`rayon::ThreadPool`)](https://docs.rs/rayon/*/rayon/struct.ThreadPool.html)
* [rust-scoped-pool](http://github.com/reem/rust-scoped-pool)
* [scoped-threadpool-rs](https://github.com/Kimundi/scoped-threadpool-rs)
* [crossbeam](https://github.com/aturon/crossbeam)

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

## Development

To install rust version 1.13.0 with [rustup](https://rustup.rs) execute this command:
```
rustup install 1.13.0
```

To run the tests with 1.13.0 use this command:
```
cargo +1.13.0 test
```

If your build fails with this error:
```
warning: unused manifest key: package.categories
error: failed to parse lock file at: /home/vp/rust/threadpool/Cargo.lock
```

You can fix it by removing the lock file:
```
rm Cargo.lock
```
