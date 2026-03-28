/// Tone detector stub for SpanDSP `super_tone_rx_state_t`.
///
/// SpanDSP operates at 8 kHz (G.711 sample rate). The active-call pipeline
/// uses 16 kHz (INTERNAL_SAMPLERATE). Callers are responsible for downsampling
/// 16 kHz → 8 kHz before calling `process_audio`.
///
/// NOTE: The full `super_tone_rx_init` API requires a `super_tone_rx_descriptor_t`
/// structure describing the tone patterns to detect. That descriptor setup is
/// non-trivial and will be implemented in Phase 10. This stub compiles and returns
/// `Ok(None)` from `process_audio` as a placeholder.
///
/// TODO: Phase 10 — initialise `super_tone_rx_descriptor_t` for Busy, Ringback,
/// and SIT tones, then call `super_tone_rx()` in `process_audio`.
use anyhow::Result;

/// Tone type recognised by the detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToneType {
    Busy,
    Ringback,
    Sit,
    Unknown,
}

/// Stub tone detector.
///
/// Returns `Ok(None)` for all audio until Phase 10 descriptor setup is complete.
pub struct ToneDetector;

impl ToneDetector {
    /// Create a new tone detector stub.
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Process 8 kHz PCM samples.
    ///
    /// Returns `Ok(None)` until Phase 10 tone descriptor setup is complete.
    #[allow(unused_variables)]
    pub fn process_audio(&mut self, samples: &[i16]) -> Result<Option<ToneType>> {
        // TODO: Phase 10 — initialise super_tone_rx_descriptor_t and call
        // super_tone_rx() to map result to ToneType.
        Ok(None)
    }

    /// Static factory function for StreamEngine registration.
    pub fn create() -> Result<Box<ToneDetector>> {
        Ok(Box::new(ToneDetector::new()?))
    }
}

// SAFETY: ToneDetector has no raw pointers; it is inherently Send.
unsafe impl Send for ToneDetector {}

// Derive Sync since there is no interior mutability via raw pointers.
unsafe impl Sync for ToneDetector {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify ToneDetector can be created and destroyed without panicking.
    #[test]
    fn create_and_drop() {
        let td = ToneDetector::new();
        assert!(td.is_ok(), "ToneDetector::new() must succeed");
    }

    /// Stub must return Ok(None) for any audio input.
    #[test]
    fn process_returns_none() {
        let mut td = ToneDetector::new().unwrap();
        let silence = vec![0i16; 160];
        let result = td.process_audio(&silence);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }
}
