// crates/pjsip/src/lib.rs
//! Safe Rust wrapper for pjproject SIP library.
//!
//! # Architecture
//!
//! A dedicated OS thread runs the pjsip endpoint event loop. Two
//! `tokio::sync::mpsc` channels bridge it to Tokio async:
//!
//! - **event channel**: pjsip callbacks -> Tokio async consumer (per-call)
//! - **command channel**: Tokio -> pjsip thread dispatch
//!
//! Each INVITE session gets a per-call event channel via the CALL_REGISTRY,
//! giving every call its own isolated event channel.

pub mod bridge;
pub mod command;
pub mod endpoint;
pub mod error;
pub mod event;
pub mod pool;
pub mod session;

pub use bridge::PjBridge;
pub use command::{PjCommand, PjCredential};
pub use endpoint::{PjEndpoint, PjEndpointConfig};
pub use error::{PjStatus, check_status};
pub use event::{PjCallEvent, PjCallEventReceiver, PjCallEventSender};
pub use pool::{CachingPool, Pool};
pub use session::PjInvSession;
