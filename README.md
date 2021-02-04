# Linux Once

A Linux-optimized drop-in replacement for `std::sync::Once`

This crate implements the same thing as `std::sync::Once` except it internally uses Linux `futex`
instead of `CondVar`. This leads to ridiculously simple code (compared to `std`) with no
`unsafe` and a bit better performance.

On non-Linux systems this crate just reexports `Once` from `std` so that you can
unconditionally import `Once` from this crate and it'll work just fine.

This crate can reach 1.0 very soon. Things to resolve before then:

* wait for stabilization of force call?
