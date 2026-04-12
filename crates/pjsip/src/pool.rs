// crates/pjsip/src/pool.rs
//! Safe wrapper around pj_pool_t and pj_caching_pool.

use std::ffi::CString;
use std::ptr;

/// Global caching pool factory — must be initialized once at startup.
///
/// IMPORTANT: `pj_caching_pool_init` writes self-referential pointers inside
/// the struct (e.g. the `factory.pool_list` sentinel links to itself). The
/// struct must therefore **never move** after initialization. We heap-allocate
/// via `Box` to guarantee a stable address.
pub struct CachingPool {
    inner: Box<pjsip_sys::pj_caching_pool>,
}

impl CachingPool {
    /// Initialize the caching pool factory.
    ///
    /// Call this once at application startup, after `pj_init()`.
    pub fn new() -> Self {
        // Allocate on the heap first so the address is stable before init.
        let mut cp = Box::new(unsafe { std::mem::zeroed::<pjsip_sys::pj_caching_pool>() });
        unsafe {
            pjsip_sys::pj_caching_pool_init(cp.as_mut(), ptr::null(), 0);
        }
        Self { inner: cp }
    }

    /// Get a pointer to the pool factory (needed by pjsip_endpt_create).
    pub fn factory_ptr(&mut self) -> *mut pjsip_sys::pj_pool_factory {
        &mut self.inner.factory as *mut pjsip_sys::pj_pool_factory
    }

    /// Create a named pool with the given initial and increment sizes.
    pub fn create_pool(&mut self, name: &str, initial: usize, increment: usize) -> Pool {
        let name_cstr = CString::new(name).unwrap_or_default();
        let pool = unsafe {
            pjsip_sys::pj_pool_create(
                self.factory_ptr(),
                name_cstr.as_ptr(),
                initial,
                increment,
                None, // no callback — use default abort behavior
            )
        };
        Pool { ptr: pool }
    }
}

impl Drop for CachingPool {
    fn drop(&mut self) {
        unsafe { pjsip_sys::pj_caching_pool_destroy(self.inner.as_mut()) };
    }
}

/// Safe wrapper around a pj_pool_t.
pub struct Pool {
    pub(crate) ptr: *mut pjsip_sys::pj_pool_t,
}

impl Pool {
    /// Get the raw pointer to the underlying pool.
    pub fn as_ptr(&self) -> *mut pjsip_sys::pj_pool_t {
        self.ptr
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { pjsip_sys::pj_pool_release(self.ptr) };
        }
    }
}

// SAFETY: Pool is only used on the pjsip thread.
unsafe impl Send for Pool {}
