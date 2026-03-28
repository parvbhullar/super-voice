/// Packet loss concealment processor wrapping `plc_state_t` from SpanDSP.
///
/// SpanDSP operates at 8 kHz (G.711 sample rate). The active-call pipeline
/// uses 16 kHz (INTERNAL_SAMPLERATE). Callers are responsible for downsampling
/// 16 kHz → 8 kHz before calling the process methods, and for upsampling
/// results back if needed. The `SpanDspPlcProcessor` adapter in the root crate
/// handles this resampling automatically.
use anyhow::{Result, anyhow};
use libc::c_int;
use spandsp_sys::{plc_free, plc_fillin, plc_init, plc_rx, plc_state_t};
use std::ptr;

/// Safe wrapper around SpanDSP's `plc_state_t`.
///
/// Provides packet loss concealment for 8 kHz PCM audio.
pub struct PlcProcessor {
    state: *mut plc_state_t,
}

impl PlcProcessor {
    /// Create a new PLC processor.
    pub fn new() -> Result<Self> {
        // SAFETY: plc_init returns NULL on allocation failure.
        let state = unsafe { plc_init(ptr::null_mut()) };
        if state.is_null() {
            return Err(anyhow!("plc_init returned NULL"));
        }
        Ok(Self { state })
    }

    /// Process a received (good) audio frame, feeding samples into the PLC predictor.
    pub fn process_good_frame(&mut self, samples: &mut [i16]) -> Result<()> {
        // SAFETY: state is valid, samples pointer and length are valid.
        let ret = unsafe {
            plc_rx(self.state, samples.as_mut_ptr(), samples.len() as c_int)
        };
        if ret < 0 {
            return Err(anyhow!("plc_rx returned error: {ret}"));
        }
        Ok(())
    }

    /// Fill in a missing (lost) audio frame using PLC concealment.
    ///
    /// Overwrites `samples` with synthesised audio approximating the lost packet.
    pub fn fill_missing_frame(&mut self, samples: &mut [i16]) -> Result<()> {
        // SAFETY: state is valid, samples pointer and length are valid.
        let ret = unsafe {
            plc_fillin(self.state, samples.as_mut_ptr(), samples.len() as c_int)
        };
        if ret < 0 {
            return Err(anyhow!("plc_fillin returned error: {ret}"));
        }
        Ok(())
    }

    /// Static factory function for StreamEngine registration.
    pub fn create() -> Result<Box<PlcProcessor>> {
        Ok(Box::new(PlcProcessor::new()?))
    }
}

// SAFETY: PlcProcessor is used per-call in single-threaded contexts.
unsafe impl Send for PlcProcessor {}

impl Drop for PlcProcessor {
    fn drop(&mut self) {
        if !self.state.is_null() {
            // SAFETY: state was allocated by plc_init and not yet freed.
            unsafe { plc_free(self.state) };
            self.state = ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify PlcProcessor can be created and destroyed without panicking.
    #[test]
    fn create_and_drop() {
        let plc = PlcProcessor::new();
        assert!(plc.is_ok(), "PlcProcessor::new() must succeed");
    }

    /// Processing a good frame must succeed without crashing.
    #[test]
    fn process_good_frame_succeeds() {
        let mut plc = PlcProcessor::new().unwrap();
        let mut samples = vec![1000i16; 160];
        assert!(plc.process_good_frame(&mut samples).is_ok());
    }

    /// Filling a missing frame must produce non-panicking output.
    #[test]
    fn fill_missing_frame_succeeds() {
        let mut plc = PlcProcessor::new().unwrap();
        // First feed a good frame so the predictor has data.
        let mut good = vec![1000i16; 160];
        plc.process_good_frame(&mut good).unwrap();
        // Now simulate a lost frame.
        let mut lost = vec![0i16; 160];
        assert!(plc.fill_missing_frame(&mut lost).is_ok());
    }
}
