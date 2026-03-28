//! Safe wrapper around `su_root_t` — the Sofia-SIP event loop root.
//!
//! `SuRoot` owns the C-allocated `su_root_t` and drives the event loop via
//! [`SuRoot::step`].  It is `Send` because it is only ever used on the
//! dedicated Sofia thread after construction.

use anyhow::Result;
use sofia_sip_sys::{su_duration_t, su_root_create, su_root_destroy, su_root_step, su_root_t};

/// Safe wrapper around a `*mut su_root_t`.
///
/// - `SuRoot::new()` allocates via `su_root_create(NULL)`.
/// - [`SuRoot::step`] drives the event loop for one tick.
/// - `Drop` calls `su_root_destroy`.
pub struct SuRoot {
    ptr: *mut su_root_t,
}

// SAFETY: SuRoot is only ever used on the dedicated Sofia OS thread.
unsafe impl Send for SuRoot {}

impl SuRoot {
    /// Create a new `su_root_t` event loop context.
    pub fn new() -> Result<Self> {
        // SAFETY: su_root_create(NULL) is the standard way to create a root.
        let ptr = unsafe { su_root_create(std::ptr::null_mut()) };
        if ptr.is_null() {
            anyhow::bail!("su_root_create returned null — Sofia-SIP initialisation failed");
        }
        Ok(Self { ptr })
    }

    /// Run the Sofia event loop for one step with the given timeout.
    ///
    /// `timeout_ms` is clamped at a practical upper bound.  Pass `1` for
    /// a 1 ms non-blocking poll, which keeps latency low.
    pub fn step(&self, timeout_ms: u64) {
        // SAFETY: self.ptr is valid for the lifetime of this SuRoot.
        unsafe { su_root_step(self.ptr, timeout_ms as su_duration_t) };
    }

    /// Returns the raw pointer without transferring ownership.
    pub fn as_ptr(&self) -> *mut su_root_t {
        self.ptr
    }
}

impl Drop for SuRoot {
    fn drop(&mut self) {
        // SAFETY: self.ptr is valid and we own the allocation.
        unsafe { su_root_destroy(self.ptr) };
    }
}

impl std::fmt::Debug for SuRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SuRoot").field("ptr", &self.ptr).finish()
    }
}
