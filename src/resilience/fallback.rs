//! Shared helpers for provider-fallback wrappers.
//!
//! All three tiers (ASR, TTS, LLM) classify errors the same way:
//!
//! - **Retryable**: connection error, HTTP 5xx, timeout, no permit (circuit
//!   open). The wrapper moves to the next provider in the chain.
//! - **Terminal**: HTTP 4xx, schema/validation errors. The wrapper fails
//!   immediately and does NOT try the rest of the chain.
//!
//! Each wrapper uses `is_retryable_error` against the underlying provider's
//! error to decide whether failover is appropriate.

use anyhow::Error;

/// Marker for whether an error should trigger fallback to the next provider
/// in the chain. Use in concrete clients to decide between continue/abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Try next provider in the chain.
    Retryable,
    /// Fail immediately; the error is the user's problem, not the provider's.
    Terminal,
}

/// Best-effort classification of an `anyhow::Error` from a provider.
///
/// Heuristic: retryable on common network / 5xx / timeout markers; terminal
/// on auth (401/403) and 4xx markers. When in doubt, retryable — it's safer
/// to try the fallback than to drop a call on an ambiguous error.
pub fn classify_error(err: &Error) -> FailureKind {
    let msg = format!("{err:?}").to_lowercase();
    // Terminal markers first — these stop the chain.
    const TERMINAL_MARKERS: &[&str] = &[
        "401", "403", "unauthorized", "forbidden",
        "400", "404", "422", "invalid request",
    ];
    for marker in TERMINAL_MARKERS {
        if msg.contains(marker) {
            return FailureKind::Terminal;
        }
    }
    // Retryable markers — these advance the chain.
    const RETRYABLE_MARKERS: &[&str] = &[
        "500", "502", "503", "504",
        "timeout", "timed out",
        "connection", "connect error", "connection refused", "reset",
        "websocket", "broken pipe", "eof",
    ];
    for marker in RETRYABLE_MARKERS {
        if msg.contains(marker) {
            return FailureKind::Retryable;
        }
    }
    // Default: retryable. Better to try the fallback than drop the call on
    // an ambiguous error.
    FailureKind::Retryable
}

/// Per-tier total budget. Each tier has a maximum total time across all
/// fallback attempts; if exceeded, the wrapper aborts. Defaults are tuned
/// to stay under the playbook's typical wait_input_timeout.
#[derive(Debug, Clone, Copy)]
pub struct FallbackBudget {
    pub total: std::time::Duration,
}

impl Default for FallbackBudget {
    fn default() -> Self {
        Self {
            total: std::time::Duration::from_secs(8),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn classifies_5xx_as_retryable() {
        let e = anyhow!("upstream returned HTTP 503 Service Unavailable");
        assert_eq!(classify_error(&e), FailureKind::Retryable);
    }

    #[test]
    fn classifies_4xx_as_terminal() {
        let e = anyhow!("HTTP 401 Unauthorized: invalid api key");
        assert_eq!(classify_error(&e), FailureKind::Terminal);
    }

    #[test]
    fn classifies_400_as_terminal() {
        let e = anyhow!("400 Bad Request: malformed body");
        assert_eq!(classify_error(&e), FailureKind::Terminal);
    }

    #[test]
    fn classifies_timeout_as_retryable() {
        let e = anyhow!("request timed out after 10s");
        assert_eq!(classify_error(&e), FailureKind::Retryable);
    }

    #[test]
    fn classifies_connection_error_as_retryable() {
        let e = anyhow!("connection refused: dial tcp 1.2.3.4:443");
        assert_eq!(classify_error(&e), FailureKind::Retryable);
    }

    #[test]
    fn classifies_websocket_drop_as_retryable() {
        let e = anyhow!("websocket closed unexpectedly");
        assert_eq!(classify_error(&e), FailureKind::Retryable);
    }

    #[test]
    fn unknown_errors_default_to_retryable() {
        let e = anyhow!("some weird thing happened");
        assert_eq!(classify_error(&e), FailureKind::Retryable);
    }
}
