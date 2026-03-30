/// Tone detector using Goertzel-based frequency detection.
///
/// SpanDSP's `super_tone_rx` callback API does not fire reliably with
/// SpanDSP 0.0.6 (the descriptor-based tone detection requires the callback
/// mechanism which appears inactive in this version). This module provides
/// a pure-Rust Goertzel fallback that detects Busy, Ringback, and SIT tones
/// by measuring energy at specific frequencies.
///
/// SpanDSP operates at 8 kHz (G.711 sample rate). The active-call pipeline
/// uses 16 kHz (INTERNAL_SAMPLERATE). Callers are responsible for downsampling
/// 16 kHz → 8 kHz before calling `process_audio`.
use anyhow::Result;

/// Tone type recognised by the detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToneType {
    Busy,
    Ringback,
    Sit,
    Unknown,
}

// ─── Goertzel algorithm ───────────────────────────────────────────────────────

/// Compute the normalised power (0.0–1.0 relative to input amplitude) at
/// `target_freq` Hz in `samples` sampled at `sample_rate` Hz using the
/// Goertzel algorithm.
///
/// Returns the squared magnitude divided by the number of samples squared,
/// giving a value that is amplitude-normalised.
fn goertzel_power(samples: &[i16], target_freq: f64, sample_rate: f64) -> f64 {
    use std::f64::consts::PI;
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    let k = (0.5 + (n as f64 * target_freq / sample_rate)) as usize;
    let w = 2.0 * PI * k as f64 / n as f64;
    let coeff = 2.0 * w.cos();
    let mut s_prev = 0.0f64;
    let mut s_prev2 = 0.0f64;
    for &sample in samples {
        let s = sample as f64 + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    let power = s_prev2 * s_prev2 + s_prev * s_prev - coeff * s_prev * s_prev2;
    power / (n as f64 * n as f64)
}

// ─── Tone pattern definitions ─────────────────────────────────────────────────

/// Minimum Goertzel power threshold for a frequency to be considered "present".
/// Calibrated for 16000-amplitude test tones (about 50% of i16 full-scale).
const POWER_THRESHOLD: f64 = 100_000.0;

/// Detect whether the `samples` buffer contains the given dual-tone (f1, f2).
///
/// Both frequencies must exceed `POWER_THRESHOLD`.
fn detect_dual_tone(samples: &[i16], f1: f64, f2: f64, sample_rate: f64) -> bool {
    let p1 = goertzel_power(samples, f1, sample_rate);
    let p2 = goertzel_power(samples, f2, sample_rate);
    p1 >= POWER_THRESHOLD && p2 >= POWER_THRESHOLD
}

/// Detect whether the `samples` buffer contains a single-frequency tone.
fn detect_single_tone(samples: &[i16], freq: f64, sample_rate: f64) -> bool {
    goertzel_power(samples, freq, sample_rate) >= POWER_THRESHOLD
}

// ─── Cadence tracking ─────────────────────────────────────────────────────────

/// Minimum number of consecutive on-blocks to confirm a tone is present.
const MIN_ON_BLOCKS: u32 = 3;
/// Maximum silence blocks before resetting the on-block counter.
const MAX_SILENCE_BLOCKS: u32 = 30;

/// Per-tone cadence state.
#[derive(Default)]
struct CadenceState {
    /// Consecutive blocks where the tone was detected.
    on_blocks: u32,
    /// Consecutive blocks where the tone was absent.
    off_blocks: u32,
    /// Whether we have ever confirmed a complete on-phase.
    confirmed: bool,
}

impl CadenceState {
    /// Update with whether the tone is present in the current block.
    /// Returns `true` if the tone is now confirmed.
    fn update(&mut self, present: bool) -> bool {
        if present {
            self.on_blocks += 1;
            self.off_blocks = 0;
            if self.on_blocks >= MIN_ON_BLOCKS {
                self.confirmed = true;
            }
        } else {
            self.off_blocks += 1;
            if self.off_blocks > MAX_SILENCE_BLOCKS {
                self.on_blocks = 0;
                self.confirmed = false;
            }
        }
        self.confirmed
    }
}

// ─── ToneDetector ─────────────────────────────────────────────────────────────

/// Tone detector using Goertzel frequency analysis.
///
/// Internally maintains cadence state for each supported tone so that transient
/// energy spikes do not trigger false positives. Requires `MIN_ON_BLOCKS`
/// consecutive blocks (each 160 samples / 20 ms at 8 kHz) before confirming.
pub struct ToneDetector {
    busy: CadenceState,
    ringback: CadenceState,
    sit_phase: u8,
    sit_phase_blocks: u32,
}

impl ToneDetector {
    /// Create a new tone detector.
    pub fn new() -> Result<Self> {
        Ok(Self {
            busy: CadenceState::default(),
            ringback: CadenceState::default(),
            sit_phase: 0,
            sit_phase_blocks: 0,
        })
    }

    /// Process 8 kHz PCM samples and return any detected tone.
    ///
    /// Returns `Ok(Some(ToneType))` when a tone is confirmed, `Ok(None)` otherwise.
    pub fn process_audio(&mut self, samples: &[i16]) -> Result<Option<ToneType>> {
        if samples.is_empty() {
            return Ok(None);
        }
        const SAMPLE_RATE: f64 = 8000.0;

        // Check Busy tone (480 Hz + 620 Hz).
        let is_busy = detect_dual_tone(samples, 480.0, 620.0, SAMPLE_RATE);
        if self.busy.update(is_busy) {
            return Ok(Some(ToneType::Busy));
        }

        // Check Ringback tone (440 Hz + 480 Hz).
        let is_ringback = detect_dual_tone(samples, 440.0, 480.0, SAMPLE_RATE);
        if self.ringback.update(is_ringback) {
            return Ok(Some(ToneType::Ringback));
        }

        // Check SIT: three sequential single-frequency segments.
        // Phase 0: ~985 Hz; Phase 1: ~1428 Hz; Phase 2: ~1777 Hz.
        let sit_freqs = [985.0f64, 1428.0f64, 1777.0f64];
        let expected_freq = sit_freqs[self.sit_phase as usize];
        if detect_single_tone(samples, expected_freq, SAMPLE_RATE) {
            self.sit_phase_blocks += 1;
            if self.sit_phase_blocks >= MIN_ON_BLOCKS {
                if self.sit_phase < 2 {
                    self.sit_phase += 1;
                    self.sit_phase_blocks = 0;
                } else {
                    // All three SIT phases confirmed.
                    self.sit_phase = 0;
                    self.sit_phase_blocks = 0;
                    return Ok(Some(ToneType::Sit));
                }
            }
        } else {
            // Reset SIT detection if a phase times out.
            self.sit_phase = 0;
            self.sit_phase_blocks = 0;
        }

        Ok(None)
    }

    /// Static factory function for StreamEngine registration.
    pub fn create() -> Result<Box<ToneDetector>> {
        Ok(Box::new(ToneDetector::new()?))
    }
}

// SAFETY: ToneDetector contains no raw pointers; inherently Send+Sync.
unsafe impl Send for ToneDetector {}
unsafe impl Sync for ToneDetector {}

/// Generate a pure tone at `frequency` Hz for `num_samples` at `sample_rate` Hz.
#[cfg(test)]
pub fn generate_tone(frequency: f64, sample_rate: f64, num_samples: usize) -> Vec<i16> {
    use std::f64::consts::PI;
    (0..num_samples)
        .map(|i| {
            let t = i as f64 / sample_rate;
            (16000.0 * (2.0 * PI * frequency * t).sin()) as i16
        })
        .collect()
}

/// Mix two sample buffers (dual-tone synthesis).
#[cfg(test)]
pub fn mix_tones(a: &[i16], b: &[i16]) -> Vec<i16> {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            ((x as i32 + y as i32).clamp(i16::MIN as i32, i16::MAX as i32)) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify ToneDetector can be created and destroyed without panicking.
    #[test]
    fn create_and_drop() {
        let td = ToneDetector::new();
        assert!(td.is_ok(), "ToneDetector::new() must succeed");
    }

    /// Silence must return Ok(None) — no false positives.
    #[test]
    fn silence_returns_none() {
        let mut td = ToneDetector::new().unwrap();
        let silence = vec![0i16; 160];
        let result = td.process_audio(&silence);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    /// Empty slice must return Ok(None) without panicking.
    #[test]
    fn empty_slice_returns_none() {
        let mut td = ToneDetector::new().unwrap();
        let result = td.process_audio(&[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    /// Goertzel power for a clean tone should exceed the threshold.
    #[test]
    fn goertzel_detects_480hz() {
        let samples = generate_tone(480.0, 8000.0, 160);
        let power = goertzel_power(&samples, 480.0, 8000.0);
        assert!(
            power >= POWER_THRESHOLD,
            "Goertzel power {power:.0} should be >= {POWER_THRESHOLD}"
        );
    }

    /// Feeding a synthetic Busy tone (480 Hz + 620 Hz) for MIN_ON_BLOCKS
    /// consecutive frames (≥3 × 20ms = 60ms) should detect ToneType::Busy.
    #[test]
    fn detects_busy_tone() {
        let mut td = ToneDetector::new().unwrap();
        let f480 = generate_tone(480.0, 8000.0, 160);
        let f620 = generate_tone(620.0, 8000.0, 160);
        let busy = mix_tones(&f480, &f620);

        let mut detected: Option<ToneType> = None;
        for _ in 0..20 {
            if let Ok(Some(t)) = td.process_audio(&busy) {
                detected = Some(t);
                break;
            }
        }

        assert_eq!(
            detected,
            Some(ToneType::Busy),
            "Expected Busy tone detection within 20 frames of 480+620 Hz audio"
        );
    }

    /// Static factory must succeed.
    #[test]
    fn create_factory_succeeds() {
        let td = ToneDetector::create();
        assert!(td.is_ok(), "ToneDetector::create() must succeed");
    }
}
