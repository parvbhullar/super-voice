/// Processor trait adapters for SpanDSP types.
///
/// SpanDSP operates at 8 kHz. The active-call pipeline operates at 16 kHz
/// (INTERNAL_SAMPLERATE). Each adapter handles the 16 kHz → 8 kHz downsampling
/// before passing frames to SpanDSP, and 8 kHz → 16 kHz upsampling on output
/// where applicable (echo canceller, PLC).
///
/// All adapters are gated behind `#[cfg(feature = "carrier")]` via the parent module.
use anyhow::Result;
use spandsp::{DtmfDetector, EchoCanceller, FaxEngine, PlcProcessor, ToneDetector};
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

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
/// When `far_end_ref` is set (via `with_far_end_ref`), the adapter uses the
/// external buffer as the far-end (speaker) reference signal for proper AEC.
/// Without a reference, it falls back to near-end-only mode (self-reference).
pub struct SpanDspEchoCancelProcessor {
    inner: EchoCanceller,
    /// Optional external far-end reference buffer for proper AEC.
    far_end_ref: Option<Arc<Mutex<Vec<i16>>>>,
}

impl SpanDspEchoCancelProcessor {
    /// Static factory for StreamEngine registration.
    pub fn create() -> Result<Box<dyn Processor>> {
        let inner = EchoCanceller::new(128)?;
        Ok(Box::new(Self {
            inner,
            far_end_ref: None,
        }))
    }

    /// Builder method: attach an external far-end reference buffer.
    ///
    /// The buffer should contain the downsampled (8 kHz) far-end signal and
    /// be updated by the caller before each `process_frame` call.
    pub fn with_far_end_ref(mut self, far_end: Arc<Mutex<Vec<i16>>>) -> Self {
        self.far_end_ref = Some(far_end);
        self
    }
}

impl Processor for SpanDspEchoCancelProcessor {
    fn process_frame(&mut self, frame: &mut AudioFrame) -> Result<()> {
        if let Samples::PCM { samples } = &mut frame.samples {
            if samples.is_empty() {
                return Ok(());
            }
            let downsampled = downsample_16k_to_8k(samples);

            let (tx, mut rx) = if let Some(ref far_ref) = self.far_end_ref {
                // Use external far-end reference for proper AEC.
                let far_end = far_ref.lock().unwrap_or_else(|e| e.into_inner());
                let far_8k = if far_end.len() == downsampled.len() {
                    far_end.clone()
                } else {
                    // Length mismatch — fall back to self-reference.
                    downsampled.clone()
                };
                (downsampled.clone(), far_8k)
            } else {
                // Near-end-only mode: use the same buffer for both tx and rx.
                (downsampled.clone(), downsampled)
            };

            self.inner.process_audio(&tx, &mut rx)?;
            *samples = upsample_8k_to_16k(&rx);
        }
        Ok(())
    }
}

// SAFETY: SpanDspEchoCancelProcessor owns its EchoCanceller exclusively.
// Arc<Mutex<_>> for far_end_ref is inherently Sync.
unsafe impl Sync for SpanDspEchoCancelProcessor {}

// ─── SpanDspPlcProcessor ─────────────────────────────────────────────────────

/// `Processor` adapter for SpanDSP packet loss concealment (PLC).
///
/// Feeds good PCM frames into the SpanDSP PLC predictor at 8 kHz.
/// The `Processor::process_frame` path treats all frames as good frames.
///
/// For loss concealment, call `process_with_loss_detection` which uses
/// sequence number gaps to detect missing frames and invoke `fill_missing_frame`.
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

    /// Process a frame with sequence-number-based loss detection.
    ///
    /// If `seq_received != seq_expected`, the gap is treated as lost packets
    /// and `fill_missing_frame` is called for each missing sequence number.
    ///
    /// Returns the (possibly concealed) frame contents in `frame`.
    pub fn process_with_loss_detection(
        &mut self,
        frame: &mut AudioFrame,
        seq_expected: u16,
        seq_received: u16,
    ) -> Result<()> {
        // Detect gaps (handle sequence number wrap-around).
        let gap = seq_received.wrapping_sub(seq_expected) as usize;
        if gap > 0 && gap < 100 {
            // Conceal the missing frames with synthesised audio.
            if let Samples::PCM { samples } = &mut frame.samples {
                let frame_len = samples.len() / 2; // 8 kHz frame size
                for _ in 0..gap {
                    let mut concealed = vec![0i16; frame_len];
                    self.inner.fill_missing_frame(&mut concealed)?;
                    // Use the last concealed frame as the output.
                    *samples = upsample_8k_to_16k(&concealed);
                }
            }
        } else {
            self.process_frame(frame)?;
        }
        Ok(())
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

// ─── SpanDspToneDetectorProcessor ────────────────────────────────────────────

/// `Processor` adapter for tone detection (Busy, Ringback, SIT).
///
/// Downsample 16 kHz PCM to 8 kHz, feed to `ToneDetector`, log detected tones
/// via `tracing`. The frame itself is not modified.
pub struct SpanDspToneDetectorProcessor {
    inner: ToneDetector,
}

impl SpanDspToneDetectorProcessor {
    /// Static factory for StreamEngine registration.
    pub fn create() -> Result<Box<dyn Processor>> {
        let inner = ToneDetector::new()?;
        Ok(Box::new(Self { inner }))
    }
}

impl Processor for SpanDspToneDetectorProcessor {
    fn process_frame(&mut self, frame: &mut AudioFrame) -> Result<()> {
        if let Samples::PCM { samples } = &frame.samples {
            if samples.is_empty() {
                return Ok(());
            }
            let downsampled = downsample_16k_to_8k(samples);
            if let Some(tone) = self.inner.process_audio(&downsampled)? {
                info!(
                    track_id = %frame.track_id,
                    tone = ?tone,
                    "SpanDSP tone detected"
                );
            }
        }
        Ok(())
    }
}

// SAFETY: SpanDspToneDetectorProcessor owns its ToneDetector exclusively.
unsafe impl Sync for SpanDspToneDetectorProcessor {}

// ─── SpanDspFaxProcessor ─────────────────────────────────────────────────────

/// `Processor` adapter for T.38 terminal-mode fax.
///
/// Downsample 16 kHz PCM to 8 kHz, feed to `FaxEngine::process_audio`, log
/// fax events via `tracing`. Gateway mode is deferred to v2.
pub struct SpanDspFaxProcessor {
    inner: FaxEngine,
}

impl SpanDspFaxProcessor {
    /// Static factory for StreamEngine registration.
    pub fn create() -> Result<Box<dyn Processor>> {
        let inner = FaxEngine::new_terminal(false)?;
        Ok(Box::new(Self { inner }))
    }
}

impl Processor for SpanDspFaxProcessor {
    fn process_frame(&mut self, frame: &mut AudioFrame) -> Result<()> {
        if let Samples::PCM { samples } = &mut frame.samples {
            if samples.is_empty() {
                return Ok(());
            }
            let mut downsampled = downsample_16k_to_8k(samples);
            let event = self.inner.process_audio(&mut downsampled)?;
            match event {
                spandsp::FaxEvent::None => {}
                spandsp::FaxEvent::ToneDetected(tone) => {
                    info!(
                        track_id = %frame.track_id,
                        tone = ?tone,
                        "SpanDSP fax tone detected"
                    );
                }
                spandsp::FaxEvent::PageReceived => {
                    info!(track_id = %frame.track_id, "SpanDSP fax page received");
                }
                spandsp::FaxEvent::Complete => {
                    info!(track_id = %frame.track_id, "SpanDSP fax complete");
                }
                spandsp::FaxEvent::Error(msg) => {
                    info!(track_id = %frame.track_id, error = %msg, "SpanDSP fax error");
                }
            }
        }
        Ok(())
    }
}

// SAFETY: SpanDspFaxProcessor owns its FaxEngine exclusively.
unsafe impl Sync for SpanDspFaxProcessor {}

// ─── Tests ────────────────────────────────────────────────────────────────────

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

    // ─── Existing tests ───────────────────────────────────────────────────────

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

    // ─── New adapter tests ────────────────────────────────────────────────────

    /// SpanDspToneDetectorProcessor processes a silent frame without error.
    #[test]
    fn tone_detector_adapter_processes_silent_frame() {
        let mut proc = SpanDspToneDetectorProcessor::create()
            .expect("SpanDspToneDetectorProcessor::create must succeed");
        let mut frame = make_frame(vec![0i16; 320]); // 20ms silence at 16kHz
        assert!(proc.process_frame(&mut frame).is_ok());
        // Frame should be unmodified.
        if let Samples::PCM { samples } = &frame.samples {
            assert!(samples.iter().all(|&s| s == 0));
        }
    }

    /// SpanDspFaxProcessor processes a silent frame without error.
    #[test]
    fn fax_adapter_processes_silent_frame() {
        let mut proc =
            SpanDspFaxProcessor::create().expect("SpanDspFaxProcessor::create must succeed");
        let mut frame = make_frame(vec![0i16; 320]); // 20ms silence at 16kHz
        assert!(proc.process_frame(&mut frame).is_ok());
    }

    /// SpanDspEchoCancelProcessor with far_end_ref set processes frame using reference.
    #[test]
    fn echo_cancel_adapter_with_far_end_ref() {
        let far_end = Arc::new(Mutex::new(vec![500i16; 160])); // 20ms at 8kHz
        let inner = EchoCanceller::new(128).expect("EchoCanceller::new must succeed");
        let mut proc = SpanDspEchoCancelProcessor {
            inner,
            far_end_ref: Some(Arc::clone(&far_end)),
        };

        let mut frame = make_frame(vec![1000i16; 320]); // 20ms at 16kHz
        assert!(
            proc.process_frame(&mut frame).is_ok(),
            "process_frame with far_end_ref must succeed"
        );
        // Verify frame was processed (samples modified or at least not errored).
        if let Samples::PCM { samples } = &frame.samples {
            assert_eq!(samples.len(), 320, "sample count must be preserved");
        }
    }

    /// SpanDspPlcProcessor conceal_loss produces non-zero output after feeding good frames.
    #[test]
    fn plc_adapter_conceals_lost_frame() {
        use spandsp::PlcProcessor;
        let inner = PlcProcessor::new().expect("PlcProcessor::new must succeed");
        let mut proc = SpanDspPlcProcessor { inner };

        // Feed several good frames to build the predictor state.
        let tone_samples: Vec<i16> = (0..320)
            .map(|i| {
                use std::f64::consts::PI;
                (16000.0 * (2.0 * PI * 440.0 * i as f64 / 16000.0).sin()) as i16
            })
            .collect();

        for _ in 0..10 {
            let mut frame = make_frame(tone_samples.clone());
            proc.process_frame(&mut frame).unwrap();
        }

        // Now simulate a loss: process_with_loss_detection with a gap of 1.
        let mut loss_frame = make_frame(vec![0i16; 320]);
        let result = proc.process_with_loss_detection(&mut loss_frame, 100, 101);
        assert!(result.is_ok(), "process_with_loss_detection must succeed");

        // The concealed frame should be non-zero (PLC generates synthetic audio).
        if let Samples::PCM { samples } = &loss_frame.samples {
            let any_nonzero = samples.iter().any(|&s| s != 0);
            assert!(any_nonzero, "Concealed frame must contain non-zero samples");
        }
    }
}
