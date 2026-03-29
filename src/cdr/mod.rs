//! CDR (Call Detail Record) module.
//!
//! Provides types, Redis queue, webhook delivery, disk fallback, and
//! background processor for carrier-grade call records.

pub mod disk_fallback;
pub mod processor;
pub mod queue;
pub mod store;
pub mod types;
pub mod webhook;

pub use processor::CdrProcessor;
pub use queue::CdrQueue;
pub use store::{CdrFilter, CdrPage, CdrStore};
pub use types::{CarrierCdr, CdrLeg, CdrStatus, CdrTiming};
