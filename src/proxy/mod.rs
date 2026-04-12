pub mod media_bridge;
pub mod media_peer;
pub mod types;

// Phase 6 Plan 02
pub mod failover;
pub mod session;

// Phase 6 Plan 03
pub mod dispatch;

// Phase 7 Plan 01
pub mod bridge;

// SDP codec filtering for trunk-level codec restrictions
pub mod sdp_filter;

// RFC 4028 session timer state machine
pub mod session_timer;

// Parallel dialer — concurrent gateway attempts (first-answer-wins)
pub mod parallel_dial;

// Phase 12 Plan 03
#[cfg(feature = "carrier")]
pub mod pj_dialog_layer;
#[cfg(feature = "carrier")]
pub mod pj_failover;
