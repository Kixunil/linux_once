# Linux Once

A Linux-optimized drop-in replacement for `std::sync::Once`

This crate implements the same thing as `std::sync::Once` except it internally uses Linux `futex`
instead of `CondVar`. This leads to ridiculously simple code (compared to `std`) with no
`unsafe` and theoretically a bit better performance. (Sadly, in practice the performance is
roughly same.)

On non-Linux systems this crate just reexports `Once` from `std` so that you can
unconditionally import `Once` from this crate and it'll work just fine.

This crate can reach 1.0 very soon. Things to resolve before then:

* wait for stabilization of force call?

## Why this should have better performance, yet it doesn't?

`Once` in std is also implemented using atomics but waiters use `thread::park` for waiting.
This happens to also be implemented using futex on Linux but with one difference: each waiting
thread has its own futex. The Thread handles are sotred in an atomic linked list (really clever
stuff) and the list is iterated when closure finishes execution waking the threads on-by-one,
issuing syscall for each thread.

This crate uses a single futex and a single syscall to wake up all waiters.

Here's where performance difference should be: one syscall should be less expensive than
multiple syscalls.

So why is this not faster then?

Frankly, I have no idea. I guess there might be some crazy optimization that makes `futex`
syscalls magically less expensive or maybe syscalls are nowhere near as expensive as I
originaly thought. These are my speculations. If you happen to have more information, please
let me know.

## License

MITNFA
