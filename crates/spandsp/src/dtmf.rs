/// DTMF digit detector wrapping `dtmf_rx_state_t` from SpanDSP.
///
/// SpanDSP operates at 8 kHz (G.711 sample rate). The active-call pipeline
/// uses 16 kHz (INTERNAL_SAMPLERATE). Callers are responsible for downsampling
/// 16 kHz → 8 kHz before calling `process_audio`, and for upsampling results
/// back if needed. The `SpanDspDtmfDetector` adapter in the root crate handles
/// this resampling automatically.
use anyhow::{Result, anyhow};
use libc::{c_char, c_int, c_void};
use spandsp_sys::{dtmf_rx_free, dtmf_rx_init, dtmf_rx_state_t};
use std::ptr;

/// Accumulated DTMF digits detected by the callback trampoline.
struct DetectedDigits {
    digits: Vec<char>,
}

/// C callback trampoline called by SpanDSP when DTMF digits are detected.
///
/// # Safety
/// `user_data` must be a valid `*mut DetectedDigits` pointer that outlives
/// all calls to this function.
unsafe extern "C" fn dtmf_callback(
    user_data: *mut c_void,
    digits: *const c_char,
    len: c_int,
) {
    if user_data.is_null() || digits.is_null() || len <= 0 {
        return;
    }
    // SAFETY: user_data is a valid *mut DetectedDigits that outlives this call.
    // digits is a valid pointer to len ASCII bytes as documented by SpanDSP.
    unsafe {
        let store = &mut *(user_data as *mut DetectedDigits);
        let slice = std::slice::from_raw_parts(digits as *const u8, len as usize);
        for &b in slice {
            if b.is_ascii() {
                store.digits.push(b as char);
            }
        }
    }
}

/// Safe wrapper around SpanDSP's `dtmf_rx_state_t`.
///
/// Detects DTMF digits in 8 kHz PCM audio frames.
pub struct DtmfDetector {
    state: *mut dtmf_rx_state_t,
    /// Heap-allocated digit store; pointer passed as `user_data` to SpanDSP callback.
    detected: Box<DetectedDigits>,
}

impl DtmfDetector {
    /// Create a new DTMF detector.
    pub fn new() -> Result<Self> {
        let mut detected = Box::new(DetectedDigits { digits: Vec::new() });
        let user_data = &mut *detected as *mut DetectedDigits as *mut c_void;

        // SAFETY: dtmf_rx_init returns NULL on allocation failure.
        let state = unsafe {
            dtmf_rx_init(
                ptr::null_mut(),
                Some(dtmf_callback),
                user_data,
            )
        };
        if state.is_null() {
            return Err(anyhow!("dtmf_rx_init returned NULL"));
        }
        Ok(Self { state, detected })
    }

    /// Process a block of 8 kHz PCM samples.
    ///
    /// Detected digits are accumulated internally and retrievable via [`get_digits`].
    pub fn process_audio(&mut self, samples: &[i16]) -> Result<()> {
        // SAFETY: state is valid, samples pointer/len are valid.
        let ret = unsafe {
            spandsp_sys::dtmf_rx(
                self.state,
                samples.as_ptr(),
                samples.len() as c_int,
            )
        };
        if ret < 0 {
            return Err(anyhow!("dtmf_rx returned error: {ret}"));
        }
        Ok(())
    }

    /// Drain and return all detected DTMF digits since the last call.
    pub fn get_digits(&mut self) -> Vec<char> {
        std::mem::take(&mut self.detected.digits)
    }

    /// Static factory function for StreamEngine registration.
    pub fn create() -> Result<Box<DtmfDetector>> {
        Ok(Box::new(DtmfDetector::new()?))
    }
}

// SAFETY: DtmfDetector is used per-call in single-threaded contexts.
// The raw pointer is not shared across threads.
unsafe impl Send for DtmfDetector {}

impl Drop for DtmfDetector {
    fn drop(&mut self) {
        if !self.state.is_null() {
            // SAFETY: state was allocated by dtmf_rx_init and not yet freed.
            unsafe { dtmf_rx_free(self.state) };
            self.state = ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify DtmfDetector can be created and destroyed without panicking.
    #[test]
    fn create_and_drop() {
        let detector = DtmfDetector::new();
        assert!(detector.is_ok(), "DtmfDetector::new() must succeed");
        // Implicit drop — no crash means Drop impl is safe.
    }

    /// Process a silent frame; no digits expected.
    #[test]
    fn process_silent_frame() {
        let mut detector = DtmfDetector::new().unwrap();
        let silence = vec![0i16; 160]; // 20 ms at 8 kHz
        assert!(detector.process_audio(&silence).is_ok());
        assert!(detector.get_digits().is_empty());
    }
}
