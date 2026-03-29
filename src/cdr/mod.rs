//! CDR (Call Detail Record) module.
//!
//! Provides types and Redis queue for carrier-grade call records.

pub mod queue;
pub mod types;

pub use queue::CdrQueue;
pub use types::{CarrierCdr, CdrLeg, CdrStatus, CdrTiming};
