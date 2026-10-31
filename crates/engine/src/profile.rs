//! Developer-only, synchronous walk instrumentation. Never persisted in scans.
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct SyscallTiming {
    pub calls: u64,
    pub elapsed: Duration,
}
#[derive(Clone, Debug, Default)]
pub struct WalkProfile {
    pub bulk_enumeration_metadata: SyscallTiming,
    pub readdir: SyscallTiming,
    pub fstatat: SyscallTiming,
    pub forced_fallbacks: u64,
    /// Overlapping helper syscall wall time: never add to exclusive walk phases.
    pub prefetch_worker: SyscallTiming,
    pub prefetch_records: u64,
    pub prefetch_nonempty_batches: u64,
}
struct Active {
    profile: WalkProfile,
    force_fallback: bool,
    prefetch: bool,
    worker: Option<crate::prefetch::Worker>,
}
thread_local! { static ACTIVE: RefCell<Option<Active>> = const {RefCell::new(None)}; }
#[derive(Clone, Copy)]
pub(crate) enum Call {
    Bulk,
    ReadDir,
    StatAt,
}

// No Instant is created on an ordinary unprofiled syscall.
pub(crate) fn timed<T>(kind: Call, operation: impl FnOnce() -> T) -> T {
    let incoming_errno = unsafe { *libc::__error() };
    let enabled = ACTIVE.with(|a| a.borrow().is_some());
    if !enabled {
        unsafe {
            *libc::__error() = incoming_errno;
        }
        return operation();
    }
    let start = Instant::now();
    unsafe {
        *libc::__error() = incoming_errno;
    }
    let result = operation();
    // Callers inspect errno after return (including readdir EOF == zero).
    // Clock/TLS instrumentation must not change that observable syscall result.
    let errno = unsafe { *libc::__error() };
    let elapsed = start.elapsed();
    ACTIVE.with(|a| {
        if let Some(active) = a.borrow_mut().as_mut() {
            let timing = match kind {
                Call::Bulk => &mut active.profile.bulk_enumeration_metadata,
                Call::ReadDir => &mut active.profile.readdir,
                Call::StatAt => &mut active.profile.fstatat,
            };
            timing.calls += 1;
            timing.elapsed += elapsed;
        }
    });
    unsafe {
        *libc::__error() = errno;
    }
    result
}
pub(crate) fn force_fallback() -> bool {
    ACTIVE.with(|a| {
        let mut a = a.borrow_mut();
        if let Some(active) = a.as_mut().filter(|a| a.force_fallback) {
            active.profile.forced_fallbacks += 1;
            true
        } else {
            false
        }
    })
}
pub(crate) fn measure<T>(force_fallback: bool, operation: impl FnOnce() -> T) -> (T, WalkProfile) {
    measure_prefetch(force_fallback, false, operation)
}
pub(crate) fn measure_prefetch<T>(force_fallback: bool, prefetch: bool, operation: impl FnOnce() -> T) -> (T, WalkProfile) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.with(|a| *a.borrow_mut() = None);
        }
    }
    ACTIVE.with(|a| {
        assert!(a.borrow().is_none(), "nested walk profiling");
        *a.borrow_mut() = Some(Active {
            profile: WalkProfile::default(),
            force_fallback,
            prefetch,
            worker: None,
        });
    });
    let _reset = Reset;
    let result = operation();
    let profile = ACTIVE.with(|a| a.borrow_mut().take().unwrap().profile);
    (result, profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn syscall_errno_and_readdir_eof_survive_profiling() {
        for expected in [0, libc::EACCES, libc::ENOTSUP] {
            measure(false, || {
                unsafe {
                    *libc::__error() = expected;
                }
                let result = timed(Call::ReadDir, || {
                    assert_eq!(unsafe { *libc::__error() }, expected);
                    // Like readdir EOF, leave caller errno untouched.
                    -1
                });
                assert_eq!(result, -1);
                assert_eq!(unsafe { *libc::__error() }, expected);
            });
        }
    }
    #[test]
    fn profiling_is_opt_in_and_resets_after_unwind() {
        assert!(!force_fallback());
        let (_, p) = measure(false, || {
            timed(Call::StatAt, || ());
            assert!(!force_fallback());
        });
        assert_eq!(p.fstatat.calls, 1);
        assert_eq!(p.forced_fallbacks, 0);
        let _ = std::panic::catch_unwind(|| measure(true, || panic!("fixture")));
        assert!(!force_fallback());
        let (_, p) = measure(true, || assert!(force_fallback()));
        assert_eq!(p.forced_fallbacks, 1);
    }
}

// Only an explicit profiled experiment can allocate/spawn this helper.
pub(crate) fn prefetch(fd: i32, buffer: &mut Vec<u8>) -> Option<crate::prefetch::Pending> {
    ACTIVE.with(|a| {
        let mut active = a.borrow_mut();
        let active = active.as_mut().filter(|a| a.prefetch && !a.force_fallback)?;
        if active.worker.is_none() { active.worker = crate::prefetch::Worker::new().ok(); }
        active.worker.as_ref()?.submit(fd, buffer)
    })
}
pub(crate) fn worker_finished(elapsed: Duration, records: usize) {
    ACTIVE.with(|a| {
        if let Some(active) = a.borrow_mut().as_mut() {
            active.profile.prefetch_worker.calls += 1;
            active.profile.prefetch_worker.elapsed += elapsed;
            active.profile.prefetch_records += records as u64;
            active.profile.prefetch_nonempty_batches += u64::from(records > 0);
        }
    });
}
