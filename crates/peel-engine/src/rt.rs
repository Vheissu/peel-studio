//! Realtime allocation checking.
//!
//! [`guard`] runs a closure with allocation checking when the `rt-check`
//! feature is enabled (debug builds only) and is a plain call otherwise.
//! Checking needs the global allocator to be `assert_no_alloc::AllocDisabler`;
//! see the crate docs for the two lines the final binary adds.

/// True when the crate was built with `rt-check` and `debug_assertions`, so
/// the guard is active (provided the binary installed the allocator).
pub const ENABLED: bool = cfg!(all(feature = "rt-check", debug_assertions));

/// Runs `f` with heap allocation and deallocation forbidden on this thread
/// (when checking is enabled). Used around the whole audio callback body.
#[inline]
pub fn guard<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(feature = "rt-check")]
    {
        assert_no_alloc::assert_no_alloc(f)
    }
    #[cfg(not(feature = "rt-check"))]
    {
        f()
    }
}
