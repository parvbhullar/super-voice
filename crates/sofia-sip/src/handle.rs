//! Safe wrapper around `nua_handle_t` — the per-dialog object in Sofia-SIP.
//!
//! `SofiaHandle` is reference-counted by Sofia-SIP itself:
//! - `Clone` increments the C reference count via `nua_handle_ref`.
//! - `Drop` decrements it via `nua_handle_unref`.
//!
//! `SofiaHandle` is `Send` because it is only ever *dereferenced* on the
//! dedicated Sofia thread (via commands).  Cloning/dropping merely changes
//! the reference count with atomic-like C semantics.

use sofia_sip_sys::{nua_handle_ref, nua_handle_t, nua_handle_unref};

/// Safe wrapper around a `*mut nua_handle_t`.
///
/// Reference-counted — `Clone` calls `nua_handle_ref`, `Drop` calls
/// `nua_handle_unref`.  The raw pointer is never exposed outside this
/// module except via [`SofiaHandle::as_ptr`].
#[derive(Debug)]
pub struct SofiaHandle {
    ptr: *mut nua_handle_t,
}

// SAFETY: The raw pointer is only dereferenced on the dedicated Sofia
// thread.  Sending the handle to another thread is safe because all
// accesses to the underlying C object go through the command channel
// which serialises them onto the Sofia thread.
unsafe impl Send for SofiaHandle {}

impl SofiaHandle {
    /// Takes ownership of `ptr`.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid, non-null `nua_handle_t` pointer.  The caller
    /// must ensure the handle's reference count is already incremented for
    /// this `SofiaHandle` (i.e. ownership is transferred, not borrowed).
    pub fn from_raw(ptr: *mut nua_handle_t) -> Self {
        debug_assert!(!ptr.is_null(), "SofiaHandle::from_raw called with null pointer");
        Self { ptr }
    }

    /// Returns the raw underlying pointer without transferring ownership.
    pub fn as_ptr(&self) -> *mut nua_handle_t {
        self.ptr
    }
}

impl Clone for SofiaHandle {
    fn clone(&self) -> Self {
        // SAFETY: self.ptr is valid for the lifetime of this handle.
        let new_ptr = unsafe { nua_handle_ref(self.ptr) };
        Self { ptr: new_ptr }
    }
}

impl Drop for SofiaHandle {
    fn drop(&mut self) {
        // SAFETY: self.ptr is valid and we hold one reference count.
        unsafe { nua_handle_unref(self.ptr) };
    }
}
