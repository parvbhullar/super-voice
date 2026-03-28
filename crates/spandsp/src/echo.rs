/// Echo canceller wrapping `echo_can_state_t` from SpanDSP.
///
/// SpanDSP operates at 8 kHz (G.711 sample rate). The active-call pipeline
/// uses 16 kHz (INTERNAL_SAMPLERATE). Callers are responsible for downsampling
/// 16 kHz → 8 kHz before calling `process_audio`, and for upsampling results
/// back if needed. The `SpanDspEchoCancelProcessor` adapter in the root crate
/// handles this resampling automatically.
use anyhow::{Result, anyhow};
use spandsp_sys::{echo_can_free, echo_can_init, echo_can_state_t, echo_can_update};
use std::ptr;

/// Safe wrapper around SpanDSP's `echo_can_state_t`.
///
/// Performs acoustic echo cancellation on 8 kHz PCM audio frames.
pub struct EchoCanceller {
    state: *mut echo_can_state_t,
}

impl EchoCanceller {
    /// Create a new echo canceller.
    ///
    /// `tail_len` is the echo tail length in samples (e.g. 128 for ~16 ms at 8 kHz).
    pub fn new(tail_len: i32) -> Result<Self> {
        // SAFETY: echo_can_init returns NULL on allocation failure.
        let state = unsafe { echo_can_init(tail_len, 0) };
        if state.is_null() {
            return Err(anyhow!("echo_can_init returned NULL"));
        }
        Ok(Self { state })
    }

    /// Process a pair of transmit/receive sample buffers in-place.
    ///
    /// `tx_samples` — near-end (microphone) samples to clean.
    /// `rx_samples` — far-end (speaker/reference) samples; modified in-place.
    ///
    /// Both slices must have equal length.
    pub fn process_audio(
        &mut self,
        tx_samples: &[i16],
        rx_samples: &mut [i16],
    ) -> Result<()> {
        if tx_samples.len() != rx_samples.len() {
            return Err(anyhow!(
                "tx and rx sample buffers must have equal length: {} vs {}",
                tx_samples.len(),
                rx_samples.len()
            ));
        }
        for (tx, rx) in tx_samples.iter().zip(rx_samples.iter_mut()) {
            // SAFETY: echo_can_update is safe per SpanDSP API; processes one sample at a time.
            *rx = unsafe { echo_can_update(self.state, *tx, *rx) };
        }
        Ok(())
    }

    /// Static factory function for StreamEngine registration.
    pub fn create() -> Result<Box<EchoCanceller>> {
        // 128 samples ≈ 16 ms echo tail at 8 kHz — typical telephony setting.
        Ok(Box::new(EchoCanceller::new(128)?))
    }
}

// SAFETY: EchoCanceller is used per-call in single-threaded contexts.
unsafe impl Send for EchoCanceller {}

impl Drop for EchoCanceller {
    fn drop(&mut self) {
        if !self.state.is_null() {
            // SAFETY: state was allocated by echo_can_init and not yet freed.
            unsafe { echo_can_free(self.state) };
            self.state = ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify EchoCanceller can be created and destroyed without panicking.
    #[test]
    fn create_and_drop() {
        let ec = EchoCanceller::new(128);
        assert!(ec.is_ok(), "EchoCanceller::new() must succeed");
    }

    /// Mismatched buffer lengths must return an error, not panic.
    #[test]
    fn mismatched_buffers_returns_error() {
        let mut ec = EchoCanceller::new(128).unwrap();
        let tx = vec![0i16; 160];
        let mut rx = vec![0i16; 80];
        assert!(ec.process_audio(&tx, &mut rx).is_err());
    }

    /// Processing a silent frame must succeed without crashing.
    #[test]
    fn process_silent_frame() {
        let mut ec = EchoCanceller::new(128).unwrap();
        let tx = vec![0i16; 160];
        let mut rx = vec![0i16; 160];
        assert!(ec.process_audio(&tx, &mut rx).is_ok());
    }
}
