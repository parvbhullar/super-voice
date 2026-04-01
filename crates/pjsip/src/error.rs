// crates/pjsip/src/error.rs
//! Error types wrapping pj_status_t.

use std::fmt;

/// Wrapper around pjsip's `pj_status_t` error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PjStatus(pub i32);

impl PjStatus {
    pub const SUCCESS: Self = Self(0); // PJ_SUCCESS

    /// Returns true if status indicates success (PJ_SUCCESS == 0).
    pub fn is_ok(self) -> bool {
        self.0 == 0
    }

    /// Convert to a human-readable error string via pj_strerror.
    pub fn message(self) -> String {
        let mut buf = [0u8; 256];
        let pj_str = unsafe {
            pjsip_sys::pj_strerror(
                self.0,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
            )
        };
        // pj_strerror returns a pj_str_t; extract via slen + ptr.
        if pj_str.slen > 0 && !pj_str.ptr.is_null() {
            let slice = unsafe {
                std::slice::from_raw_parts(pj_str.ptr as *const u8, pj_str.slen as usize)
            };
            String::from_utf8_lossy(slice).into_owned()
        } else {
            format!("pjsip error {}", self.0)
        }
    }
}

impl fmt::Display for PjStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_ok() {
            write!(f, "PJ_SUCCESS")
        } else {
            write!(f, "{}", self.message())
        }
    }
}

impl std::error::Error for PjStatus {}

/// Convert a pj_status_t to Result.
pub fn check_status(status: i32) -> Result<(), PjStatus> {
    if status == 0 {
        Ok(())
    } else {
        Err(PjStatus(status))
    }
}
