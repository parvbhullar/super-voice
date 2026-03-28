/// Processor trait adapters for SpanDSP types.
///
/// SpanDSP operates at 8 kHz. The active-call pipeline operates at 16 kHz
/// (INTERNAL_SAMPLERATE). Each adapter handles the 16 kHz → 8 kHz downsampling
/// before passing frames to SpanDSP, and 8 kHz → 16 kHz upsampling on output
/// where applicable (echo canceller, PLC).
///
/// All adapters are gated behind `#[cfg(feature = "carrier")]` via the parent module.
use anyhow::Result;
use spandsp::{DtmfDetector, EchoCanceller, PlcProcessor};
use tracing::debug;

use crate::media::{AudioFrame, Samples, processor::Processor};

// ─── Helper: 16 kHz → 8 kHz (drop every other sample) ───────────────────────

fn downsample_16k_to_8k(samples: &[i16]) -> Vec<i16> {
    samples.iter().step_by(2).copied().collect()
}

// ─── Helper: 8 kHz → 16 kHz (linear interpolation) ──────────────────────────

fn upsample_8k_to_16k(samples: &[i16]) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(samples.len() * 2);
    for window in samples.windows(2) {
        out.push(window[0]);
        // Linear interpolation between consecutive samples.
        out.push(((window[0] as i32 + window[1] as i32) / 2) as i16);
    }
    // Final sample has no successor — duplicate it.
    if let Some(&last) = samples.last() {
        out.push(last);
        out.push(last);
    }
    out
}

// ─── SpanDspDtmfDetector ─────────────────────────────────────────────────────

/// `Processor` adapter for SpanDSP DTMF detection.
///
/// Downsample 16 kHz PCM to 8 kHz, detect DTMF digits via SpanDSP, and log
/// any detected digits via `tracing`. The frame itself is not modified.
pub struct SpanDspDtmfDetector {
    inner: DtmfDetector,
}

impl SpanDspDtmfDetector {
    /// Static factory for StreamEngine registration.
    pub fn create() -> Result<Box<dyn Processor>> {
        let inner = DtmfDetector::new()?;
        Ok(Box::new(Self { inner }))
    }
}

impl Processor for SpanDspDtmfDetector {
    fn process_frame(&mut self, frame: &mut AudioFrame) -> Result<()> {
        if let Samples::PCM { samples } = &frame.samples {
            if samples.is_empty() {
                return Ok(());
            }
            let downsampled = downsample_16k_to_8k(samples);
            self.inner.process_audio(&downsampled)?;
            let digits = self.inner.get_digits();
            if !digits.is_empty() {
                debug!(
                    track_id = %frame.track_id,
                    digits = ?digits,
                    "SpanDSP DTMF detected"
                );
            }
        }
        Ok(())
    }
}

// SAFETY: SpanDspDtmfDetector owns its DtmfDetector exclusively.
unsafe impl Sync for SpanDspDtmfDetector {}

// ─── SpanDspEchoCancelProcessor ──────────────────────────────────────────────

/// `Processor` adapter for SpanDSP echo cancellation.
///
/// Downsample 16 kHz PCM to 8 kHz, run echo cancellation, then upsample back
/// to 16 kHz, replacing the frame's samples in-place.
///
/// Note: echo cancellation is most effective when a separate far-end (reference)
/// signal is available. This adapter uses the incoming frame as both tx and rx
/// (near-end only mode), which suppresses residual echo but requires a proper
/// far-end reference for full AEC. Full AEC integration is planned for Phase 10.
pub struct SpanDspEchoCancelProcessor {
    inner: EchoCanceller,
}

impl SpanDspEchoCancelProcessor {
    /// Static factory for StreamEngine registration.
    pub fn create() -> Result<Box<dyn Processor>> {
        let inner = EchoCanceller::new(128)?;
        Ok(Box::new(Self { inner }))
    }
}

impl Processor for SpanDspEchoCancelProcessor {
    fn process_frame(&mut self, frame: &mut AudioFrame) -> Result<()> {
        if let Samples::PCM { samples } = &mut frame.samples {
            if samples.is_empty() {
                return Ok(());
            }
            let downsampled = downsample_16k_to_8k(samples);
            // Use the same buffer for both tx (near-end) and rx (reference).
            // This provides near-end processing only; full AEC needs a far-end feed.
            let tx = downsampled.clone();
            let mut rx = downsampled;
            self.inner.process_audio(&tx, &mut rx)?;
            *samples = upsample_8k_to_16k(&rx);
        }
        Ok(())
    }
}

// SAFETY: SpanDspEchoCancelProcessor owns its EchoCanceller exclusively.
unsafe impl Sync for SpanDspEchoCancelProcessor {}

// ─── SpanDspPlcProcessor ─────────────────────────────────────────────────────

/// `Processor` adapter for SpanDSP packet loss concealment (PLC).
///
/// Feeds good PCM frames into the SpanDSP PLC predictor at 8 kHz.
/// Lost frames can be concealed by calling the underlying `PlcProcessor`
/// directly. In this adapter, all received frames are treated as good frames.
///
/// Frame samples are downsampled to 8 kHz, fed to PLC, then upsampled back
/// to 16 kHz.
pub struct SpanDspPlcProcessor {
    inner: PlcProcessor,
}

impl SpanDspPlcProcessor {
    /// Static factory for StreamEngine registration.
    pub fn create() -> Result<Box<dyn Processor>> {
        let inner = PlcProcessor::new()?;
        Ok(Box::new(Self { inner }))
    }
}

impl Processor for SpanDspPlcProcessor {
    fn process_frame(&mut self, frame: &mut AudioFrame) -> Result<()> {
        if let Samples::PCM { samples } = &mut frame.samples {
            if samples.is_empty() {
                return Ok(());
            }
            let mut downsampled = downsample_16k_to_8k(samples);
            self.inner.process_good_frame(&mut downsampled)?;
            *samples = upsample_8k_to_16k(&downsampled);
        }
        Ok(())
    }
}

// SAFETY: SpanDspPlcProcessor owns its PlcProcessor exclusively.
unsafe impl Sync for SpanDspPlcProcessor {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{AudioFrame, Samples};

    fn make_frame(samples: Vec<i16>) -> AudioFrame {
        AudioFrame {
            track_id: "test".to_string(),
            samples: Samples::PCM { samples },
            timestamp: 0,
            sample_rate: 16000,
            channels: 1,
            src_packet: None,
        }
    }

    #[test]
    fn dtmf_adapter_processes_silent_frame() {
        let mut proc =
            SpanDspDtmfDetector::create().expect("SpanDspDtmfDetector::create must succeed");
        let mut frame = make_frame(vec![0i16; 320]); // 20ms at 16kHz
        assert!(proc.process_frame(&mut frame).is_ok());
    }

    #[test]
    fn echo_cancel_adapter_processes_frame() {
        let mut proc = SpanDspEchoCancelProcessor::create()
            .expect("SpanDspEchoCancelProcessor::create must succeed");
        let mut frame = make_frame(vec![1000i16; 320]);
        assert!(proc.process_frame(&mut frame).is_ok());
        // Frame samples should have been replaced by upsampled output.
        if let Samples::PCM { samples } = &frame.samples {
            assert_eq!(samples.len(), 320);
        }
    }

    #[test]
    fn plc_adapter_processes_good_frame() {
        let mut proc =
            SpanDspPlcProcessor::create().expect("SpanDspPlcProcessor::create must succeed");
        let mut frame = make_frame(vec![500i16; 320]);
        assert!(proc.process_frame(&mut frame).is_ok());
    }

    #[test]
    fn downsample_16k_to_8k_halves_length() {
        let samples: Vec<i16> = (0..320).map(|i| i as i16).collect();
        let down = downsample_16k_to_8k(&samples);
        assert_eq!(down.len(), 160);
        assert_eq!(down[0], 0);
        assert_eq!(down[1], 2);
    }

    #[test]
    fn upsample_8k_to_16k_doubles_length() {
        let samples: Vec<i16> = vec![0, 100, 200];
        let up = upsample_8k_to_16k(&samples);
        // 3 samples → 2 windows of 2 + final duplicate pair = 6 samples
        assert_eq!(up.len(), 6);
    }
}
