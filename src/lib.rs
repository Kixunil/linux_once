//! A Linux-optimized drop-in replacement for `std::sync::Once`
//!
//! This crate implements the same thing as `std::sync::Once` except it internally uses Linux `futex`
//! instead of `CondVar`. This leads to ridiculously simple code (compared to `std`) with no
//! `unsafe` and a bit better performance.
//!
//! On non-Linux systems this crate just reexports `Once` from `std` so that you can
//! unconditionally import `Once` from this crate and it'll work just fine.
//!
//! This crate can reach 1.0 very soon. Things to resolve before then:
//!
//! * wait for stabilization of force call?

#![cfg_attr(all(test, feature = "bench"), feature(test))]

#[cfg(all(test, feature = "bench"))]
extern crate test;

#[cfg(test)]
mod tests;

#[cfg(target_os = "linux")]
pub use linux::Once;

#[cfg(not(target_os = "linux"))]
pub use std::sync::Once;

#[cfg(target_os = "linux")]
mod linux {
    use linux_futex::{Futex, Private};
    use core::sync::atomic::Ordering;

    /// A synchronization primitive which can be used to run a one-time global initialization. Useful
    /// for one-time initialization for FFI or related functionality. This type can only be constructed
    /// with [`Once::new()`].
    pub struct Once(Futex<Private>);

    /// The closure didn't run yet
    const INCOMPLETE: i32 = 0;
    /// The closure panicked
    const POISONED: i32 = 2;
    /// The closure finished without panicking
    const COMPLETE: i32 = 1;
    /// The closure is running and no thread is waiting yet
    ///
    /// Used to avoid expensive syscall
    const RUNNING_NO_WAIT: i32 = 3;
    /// The closure is running and at least on thread is waiting
    const RUNNING_WAITING: i32 = 4;

    impl Once {
        /// Creates a new `Once` value.
        pub const fn new() -> Self {
            Once(Futex::new(INCOMPLETE))
        }

        /// Performs an initialization routine once and only once. The given closure will be executed if
        /// this is the first time `call_once` has been called, and otherwise the routine will *not* be
        /// invoked.
        ///
        /// This method will block the calling thread if another initialization routine is currently
        /// running.
        ///
        /// When this function returns, it is guaranteed that some initialization has run and completed (it
        /// may not be the closure specified). It is also guaranteed that any memory writes performed by the
        /// executed closure can be reliably observed by other threads at this point (there is a
        /// happens-before relation between the closure and code executing after the return).
        ///
        /// If the given closure recursively invokes call_once on the same [`Once`] instance the exact
        /// behavior is not specified, allowed outcomes are a panic or a deadlock.
        ///
        /// Note specific to the Linux version: recursive calls currently cause deadlock. This
        /// information is only intended to help debugging and must **not** be relied on.
        pub fn call_once<F: FnOnce()>(&self, f: F) {
            // Fast path
            // std calls is_completed() at this point, we store the state instead to reuse later and
            // avoid repeating atomic operation
            let state = self.0.value.load(Ordering::Acquire);
            if state == COMPLETE {
                return;
            }

            let mut f = Some(f);
            self.internal_call_once(state, &mut || f.take().expect("closure called more than once")())
        }

        #[cold]
        fn internal_call_once(&self, mut state: i32, f: &mut dyn FnMut()) {
            // No need to over-complicate the checker as much as std does
            struct PanicChecker<'a> {
                futex: &'a Futex<Private>,
                value_to_write: i32,
            }

            impl<'a> Drop for PanicChecker<'a> {
                fn drop(&mut self) {
                    // Only make expensive syscall if there are threads waiting
                    if self.futex.value.swap(self.value_to_write, Ordering::AcqRel) == RUNNING_WAITING {
                        self.futex.wake(i32::max_value());
                    }
                }
            }

            loop {
                match state {
                    INCOMPLETE => {
                        // same thing std does
                        // except we use weak, which seems a bit better
                        if let Err(old) = self.0.value.compare_exchange_weak(INCOMPLETE, RUNNING_NO_WAIT, Ordering::Acquire, Ordering::Acquire) {
                            state = old;
                            continue;
                        }

                        {
                            // we do it a bit simpler
                            let mut panic_checker = PanicChecker { futex: &self.0, value_to_write: POISONED, };
                            f();
                            panic_checker.value_to_write = COMPLETE;
                        }
                        break;
                    },
                    COMPLETE => break,
                    POISONED => panic!("Once instance has previously been poisoned"),
                    // we have two versions of running to optimize a bit
                    running => {
                        // Signal that there's at least one thread waiting
                        if let Err(old) = self.0.value.compare_exchange(RUNNING_NO_WAIT, RUNNING_WAITING, Ordering::AcqRel, Ordering::Acquire) {
                            // reuse expensive load
                            state = old;
                        }

                        // TODO: is it worth spinning a bit?
                        //       Probably not because the operation is supposed to be expensive but
                        //       we don't know until we measure.

                        // actual waiting logic
                        while state >= RUNNING_NO_WAIT {
                            // We need to check the value regardless, o we just ignore the error
                            let _ = self.0.wait(running);
                            state = self.0.value.load(Ordering::Acquire);
                        }
                        break;
                    },
                }
            }
        }

        /// Returns `true` if some [`call_once()`](Self::call_once) call has completed successfully. Specifically, is_completed
        /// will return false in the following situations:
        ///
        /// * [`call_once()`](Self::call_once) was not called at all,
        /// * [`call_once()`](Self::call_once) was called, but has not yet completed,
        /// * the [`Once`] instance is poisoned
        ///
        /// This function returning `false` does not mean that [`Once`] has not been executed. For example, it
        /// may have been executed in the time between when `is_completed` starts executing and when it returns,
        /// in which case the `false` return value would be stale (but still permissible).
        pub fn is_completed(&self) -> bool {
            self.0.value.load(Ordering::Acquire) == COMPLETE
        }
    }
}

#[cfg(test)]
mod our_tests {
    use super::Once;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering::Relaxed}};
    #[cfg(feature = "bench")]
    use test::Bencher;

    // Simulate 5 threads attempting to run `Once` at the same time
    #[cfg(feature = "bench")]
    const CONTENDED_THREADS: usize = 5;
    // Simulate expensive operation that takes 1ms to complete
    #[cfg(feature = "bench")]
    const CONTENDED_WAIT: u64 = 1_000_000;

    #[test]
    fn basic() {
        let mut ran = false;
        let once = Once::new();
        once.call_once(|| ran = true);
        assert!(ran);
        ran = false;
        once.call_once(|| ran = true);
        assert!(!ran);
    }

    #[test]
    fn multithreaded() {
        let once = Arc::new((Once::new(), AtomicUsize::new(0)));
        let once_cloned = Arc::clone(&once);

        let handle = std::thread::spawn(move || once_cloned.0.call_once(|| { once_cloned.1.fetch_add(1, Relaxed); }));
        once.0.call_once(|| { once.1.fetch_add(1, Relaxed); });
        handle.join().expect("failed to join thread");
        assert_eq!(once.1.load(Relaxed), 1);
    }

    #[bench]
    #[cfg(feature = "bench")]
    #[cfg_attr(miri, ignore)]
    fn measure_std_trivial(bencher: &mut Bencher) {
        bencher.iter(|| {
            let mut ran = false;
            let once = std::sync::Once::new();
            once.call_once(|| ran = true);
            assert!(ran);
        })
    }

    #[bench]
    #[cfg(feature = "bench")]
    #[cfg_attr(miri, ignore)]
    fn measure_linux_trivial(bencher: &mut Bencher) {
        bencher.iter(|| {
            let mut ran = false;
            let once = Once::new();
            once.call_once(|| ran = true);
            assert!(ran);
        })
    }

    #[bench]
    #[cfg(feature = "bench")]
    #[cfg_attr(miri, ignore)]
    fn measure_std_contended(bencher: &mut Bencher) {
        let barrier = Arc::new(std::sync::Barrier::new(CONTENDED_THREADS));
        bencher.iter(|| {
            let once = Arc::new(std::sync::Once::new());
            let threads = (0..CONTENDED_THREADS)
                .into_iter()
                .map(|_| {
                    let cloned_once = Arc::clone(&once);
                    let cloned_barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        cloned_barrier.wait();
                        cloned_once.call_once(|| std::thread::sleep(std::time::Duration::from_nanos(CONTENDED_WAIT)))
                    })
                })
                // required for true concurrency
                .collect::<Vec<_>>();

            threads
                .into_iter()
                .map(|thread| thread.join().map(drop))
                .collect::<Result<(), _>>()
                .expect("Failed to join");
        })
    }

    #[bench]
    #[cfg(feature = "bench")]
    #[cfg_attr(miri, ignore)]
    fn measure_linux_contended(bencher: &mut Bencher) {
        let barrier = Arc::new(std::sync::Barrier::new(CONTENDED_THREADS));
        bencher.iter(|| {
            let once = Arc::new(Once::new());
            let threads = (0..CONTENDED_THREADS)
                .into_iter()
                .map(|_| {
                    let cloned = Arc::clone(&once);
                    let cloned_barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        cloned_barrier.wait();
                        cloned.call_once(|| std::thread::sleep(std::time::Duration::from_nanos(CONTENDED_WAIT)))
                    })
                })
                // required for true concurrency
                .collect::<Vec<_>>();

            threads
                .into_iter()
                .map(|thread| thread.join().map(drop))
                .collect::<Result<(), _>>()
                .expect("Failed to join");
        })
    }
}
