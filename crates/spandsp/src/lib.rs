//! Safe Rust wrappers for the SpanDSP carrier-grade DSP library.
//!
//! SpanDSP processes audio at 8 kHz (G.711 sample rate). The active-call
//! pipeline operates at 16 kHz (INTERNAL_SAMPLERATE). Adapter types in the
//! root `active-call` crate handle the 16 kHz ↔ 8 kHz resampling when wrapping
//! these processors in the `Processor` trait.
//!
//! # Processors
//!
//! - [`DtmfDetector`] — DTMF digit detection
//! - [`EchoCanceller`] — Acoustic echo cancellation with configurable tail length
//! - [`ToneDetector`] — Tone detection (Busy, Ringback, SIT) via Goertzel algorithm
//! - [`PlcProcessor`] — Packet loss concealment
//! - [`FaxEngine`] — T.38 terminal-mode fax engine (gateway mode deferred to v2)

pub mod dtmf;
pub mod echo;
pub mod fax;
pub mod plc;
pub mod tone;

pub use dtmf::DtmfDetector;
pub use echo::EchoCanceller;
pub use fax::{FaxEngine, FaxEvent, FaxTone};
pub use plc::PlcProcessor;
pub use tone::{ToneDetector, ToneType};
