/// Minimal FaxEngine stub proving fax_state_t FFI bindings compile.
/// Full T.38 fax implementation deferred to Phase 10.
///
/// This stub exists solely to verify that the `fax_state_t` type from SpanDSP
/// is correctly resolved through the bindgen-generated bindings in `spandsp-sys`.
/// No audio processing methods are provided — this is a compile-time proof only.
use anyhow::{Result, anyhow};
use spandsp_sys::{fax_free, fax_init, fax_state_t};
use std::ptr;

/// Minimal FaxEngine stub proving fax_state_t FFI bindings compile.
/// Full T.38 fax implementation deferred to Phase 10.
pub struct FaxEngine {
    state: *mut fax_state_t,
}

impl FaxEngine {
    /// Create a minimal fax engine to prove FFI bindings link correctly.
    ///
    /// Passes `calling = 1` (originating side). No T.38 setup is performed.
    pub fn new() -> Result<Self> {
        // SAFETY: fax_init returns NULL on allocation failure.
        let state = unsafe { fax_init(ptr::null_mut(), 1) };
        if state.is_null() {
            return Err(anyhow!("fax_init returned NULL"));
        }
        Ok(Self { state })
    }
}

// SAFETY: FaxEngine is used per-call in single-threaded contexts.
unsafe impl Send for FaxEngine {}

impl Drop for FaxEngine {
    fn drop(&mut self) {
        if !self.state.is_null() {
            // SAFETY: state was allocated by fax_init and not yet freed.
            unsafe { fax_free(self.state) };
            self.state = ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time proof: fax_state_t type is accessible from spandsp_sys bindings.
    /// Note: In SpanDSP 0.0.6 the struct is opaque (zero-size placeholder); this
    /// test verifies the binding resolves at compile time rather than checking size.
    #[test]
    fn fax_state_t_binding_resolves() {
        // If this compiles, the fax_state_t binding is accessible.
        // The type is opaque in SpanDSP 0.0.6 (internal definition hidden).
        let _ = std::mem::size_of::<spandsp_sys::fax_state_t>();
    }

    /// Verify FaxEngine can be created and destroyed without panicking.
    #[test]
    fn create_and_drop() {
        let engine = FaxEngine::new();
        assert!(engine.is_ok(), "FaxEngine::new() must succeed");
    }
}
