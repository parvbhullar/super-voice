//! Safe Rust wrapper for the Sofia-SIP library.
//!
//! # Overview
//!
//! This crate provides a memory-safe, async-friendly API over the raw
//! `sofia-sip-sys` FFI bindings.  The central design is a **dedicated OS
//! thread** running the Sofia event loop (`su_root_step`) with two
//! `tokio::sync::mpsc` channels bridging it to Tokio async:
//!
//! - **event channel** (`SofiaEvent`): C callback → Tokio async consumer.
//! - **command channel** (`SofiaCommand`): Tokio → Sofia thread dispatch.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use sofia_sip::{NuaAgent, SofiaEvent};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let mut agent = NuaAgent::new("sip:*:5060")?;
//!     while let Some(event) = agent.next_event().await {
//!         match event {
//!             SofiaEvent::IncomingInvite { handle, from, .. } => {
//!                 agent.respond(&handle, 200, "OK")?;
//!             }
//!             _ => {}
//!         }
//!     }
//!     Ok(())
//! }
//! ```

pub mod agent;
pub mod bridge;
pub mod command;
pub mod event;
pub mod handle;
pub mod root;

pub use agent::NuaAgent;
pub use bridge::SofiaBridge;
pub use command::SofiaCommand;
pub use event::SofiaEvent;
pub use handle::SofiaHandle;
pub use root::SuRoot;
