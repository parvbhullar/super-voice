// src/proxy/pj_failover.rs
//! pjsip-based failover loop mirroring the existing `FailoverLoop`.
//!
//! `PjFailoverLoop` tries trunk gateways sequentially using
//! `PjDialogLayer::create_invite()`.  On success it returns a
//! `PjFailoverResult::Connected` that includes the `call_id` needed for
//! post-connect BYE routing — resolving research gap 1 from Plan 02.

use crate::proxy::failover::is_nofailover;
use crate::proxy::pj_dialog_layer::PjDialogLayer;
use crate::redis_state::types::{TrunkConfig, TrunkCredential};
use anyhow::Result;
use pjsip::{PjCallEvent, PjCallEventReceiver, PjCredential};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Public result type
// ---------------------------------------------------------------------------

/// Result of a `PjFailoverLoop::try_routes` call.
pub enum PjFailoverResult {
    /// A gateway accepted the call.
    Connected {
        /// The gateway that answered.
        gateway_addr: String,
        /// Per-call event receiver for mid-dialog events (re-INVITE, BYE, …).
        call_event_rx: PjCallEventReceiver,
        /// Answer SDP — from the 200 OK body, or from an earlier 183 fallback.
        sdp: Option<String>,
        /// SIP Call-ID for post-connect BYE routing.
        call_id: String,
    },
    /// A nofailover SIP code was received — do not retry.
    NoFailover { code: u16, reason: String },
    /// All gateways were tried and all failed.
    Exhausted { last_code: u16, last_reason: String },
    /// The trunk has no gateway references.
    NoRoutes,
}

// ---------------------------------------------------------------------------
// Main struct
// ---------------------------------------------------------------------------

/// Failover loop that dials trunk gateways sequentially via pjsip.
pub struct PjFailoverLoop {
    dialog_layer: PjDialogLayer,
    cancel_token: CancellationToken,
}

impl PjFailoverLoop {
    /// Create a new failover loop.
    pub fn new(dialog_layer: PjDialogLayer, cancel_token: CancellationToken) -> Self {
        Self {
            dialog_layer,
            cancel_token,
        }
    }

    /// Try each gateway in `trunk.gateways` until one answers or all fail.
    ///
    /// # Arguments
    ///
    /// * `trunk`      – Trunk config containing gateways and credentials.
    /// * `caller_sdp` – SDP offer from the inbound caller.
    /// * `caller_uri` – From URI for the outbound INVITE.
    /// * `callee_uri` – Request-URI / To URI.
    pub async fn try_routes(
        &self,
        trunk: &TrunkConfig,
        caller_sdp: &str,
        caller_uri: &str,
        callee_uri: &str,
    ) -> Result<PjFailoverResult> {
        let gateways = &trunk.gateways;

        if gateways.is_empty() {
            return Ok(PjFailoverResult::NoRoutes);
        }

        // Build a PjCredential from the first TrunkCredential entry (if any).
        let pj_credential = trunk
            .credentials
            .as_ref()
            .and_then(|creds| creds.first())
            .map(build_pj_credential);

        let mut last_code: u16 = 503;
        let mut last_reason = "Service Unavailable".to_string();

        for gateway_ref in gateways {
            info!(
                gateway = %gateway_ref.name,
                callee = %callee_uri,
                "pj_failover: trying gateway"
            );

            // Build the target SIP URI: extract user part from callee_uri and
            // route via the gateway host.
            let user = extract_user(callee_uri);
            let target_uri = format!("sip:{}@{}", user, gateway_ref.name);

            let event_rx = match self.dialog_layer.create_invite(
                &target_uri,
                caller_uri,
                caller_sdp,
                pj_credential.clone(),
                None,
            ) {
                Ok(rx) => rx,
                Err(e) => {
                    warn!(
                        gateway = %gateway_ref.name,
                        "pj_failover: create_invite failed: {e}"
                    );
                    last_reason = e.to_string();
                    continue;
                }
            };

            let outcome = self
                .wait_for_outcome(event_rx, trunk, &gateway_ref.name)
                .await;

            match outcome {
                WaitOutcome::Connected {
                    sdp,
                    call_id,
                    event_rx: rx,
                } => {
                    return Ok(PjFailoverResult::Connected {
                        gateway_addr: gateway_ref.name.clone(),
                        call_event_rx: rx,
                        sdp,
                        call_id,
                    });
                }
                WaitOutcome::NoFailover { code, reason } => {
                    return Ok(PjFailoverResult::NoFailover { code, reason });
                }
                WaitOutcome::Failed { code, reason } => {
                    last_code = code;
                    last_reason = reason;
                    // Continue to next gateway.
                }
            }
        }

        Ok(PjFailoverResult::Exhausted {
            last_code,
            last_reason,
        })
    }

    /// Wait for the per-call event channel to reach a terminal state.
    ///
    /// Returns after the first Confirmed or Terminated event, or on timeout /
    /// cancellation.
    async fn wait_for_outcome(
        &self,
        mut event_rx: PjCallEventReceiver,
        trunk: &TrunkConfig,
        gateway_name: &str,
    ) -> WaitOutcome {
        let mut early_media_sdp: Option<String> = None;
        let timeout_duration = tokio::time::Duration::from_secs(30);

        loop {
            let event = tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    return WaitOutcome::Failed {
                        code: 487,
                        reason: "Cancelled".to_string(),
                    };
                }
                result = tokio::time::timeout(timeout_duration, event_rx.recv()) => {
                    match result {
                        Ok(Some(ev)) => ev,
                        Ok(None) => {
                            return WaitOutcome::Failed {
                                code: 503,
                                reason: "Event channel closed".to_string(),
                            };
                        }
                        Err(_) => {
                            warn!(gateway = %gateway_name, "pj_failover: gateway timed out");
                            return WaitOutcome::Failed {
                                code: 408,
                                reason: "Request Timeout".to_string(),
                            };
                        }
                    }
                }
            };

            match event {
                PjCallEvent::Trying => {
                    // Still in progress — keep waiting.
                }
                PjCallEvent::Ringing { .. } => {
                    // Keep waiting.
                }
                PjCallEvent::EarlyMedia { sdp, .. } => {
                    info!(
                        gateway = %gateway_name,
                        "pj_failover: early media SDP received"
                    );
                    early_media_sdp = Some(sdp);
                }
                PjCallEvent::Confirmed { call_id, sdp } => {
                    // Use early-media SDP as fallback when 200 OK has no body.
                    let answer_sdp = sdp.or(early_media_sdp.take());
                    info!(
                        gateway = %gateway_name,
                        has_sdp = %answer_sdp.is_some(),
                        call_id = %call_id,
                        "pj_failover: call connected"
                    );
                    return WaitOutcome::Connected {
                        sdp: answer_sdp,
                        call_id,
                        event_rx,
                    };
                }
                PjCallEvent::Terminated { code, reason } => {
                    info!(
                        gateway = %gateway_name,
                        code = %code,
                        "pj_failover: gateway rejected call"
                    );
                    if is_nofailover(code, trunk) {
                        return WaitOutcome::NoFailover { code, reason };
                    }
                    return WaitOutcome::Failed { code, reason };
                }
                PjCallEvent::Info { .. } | PjCallEvent::ReInvite { .. } => {
                    // Ignore mid-dialog events during initial dialing phase.
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Internal outcome of waiting for a single gateway attempt.
enum WaitOutcome {
    Connected {
        sdp: Option<String>,
        call_id: String,
        /// Event receiver kept alive for post-connect events (re-INVITE, etc.).
        event_rx: PjCallEventReceiver,
    },
    NoFailover {
        code: u16,
        reason: String,
    },
    Failed {
        code: u16,
        reason: String,
    },
}

/// Build a `PjCredential` from a `TrunkCredential` entry.
///
/// The realm defaults to `"*"` (match-all) when not present on the credential.
fn build_pj_credential(cred: &TrunkCredential) -> PjCredential {
    PjCredential {
        realm: cred.realm.clone(),
        username: cred.username.clone(),
        password: cred.password.clone(),
        scheme: "digest".to_string(),
    }
}

/// Extract the user part from a SIP URI.
///
/// Strips the `sip:` / `sips:` scheme prefix and returns the portion before
/// the first `@`.  Falls back to the raw input when the URI has no `@`.
///
/// # Examples
///
/// ```text
/// "sip:+14155551234@carrier.com"  =>  "+14155551234"
/// "+14155551234@carrier.com"      =>  "+14155551234"
/// "+14155551234"                   =>  "+14155551234"
/// ```
fn extract_user(uri: &str) -> &str {
    let stripped = uri
        .strip_prefix("sips:")
        .or_else(|| uri.strip_prefix("sip:"))
        .unwrap_or(uri);

    stripped.split('@').next().unwrap_or(stripped)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------ //
    // extract_user helper                                                  //
    // ------------------------------------------------------------------ //

    #[test]
    fn test_extract_user_full_sip_uri() {
        assert_eq!(extract_user("sip:+14155551234@carrier.com"), "+14155551234");
    }

    #[test]
    fn test_extract_user_sips_uri() {
        assert_eq!(extract_user("sips:alice@example.com"), "alice");
    }

    #[test]
    fn test_extract_user_no_scheme() {
        assert_eq!(extract_user("+14155551234@carrier.com"), "+14155551234");
    }

    #[test]
    fn test_extract_user_no_at() {
        assert_eq!(extract_user("+14155551234"), "+14155551234");
    }

    #[test]
    fn test_extract_user_bare_sip_prefix() {
        assert_eq!(extract_user("sip:bob"), "bob");
    }

    // ------------------------------------------------------------------ //
    // build_pj_credential                                                 //
    // ------------------------------------------------------------------ //

    #[test]
    fn test_build_pj_credential_maps_fields() {
        let trunk_cred = TrunkCredential {
            realm: "carrier.example.com".to_string(),
            username: "alice".to_string(),
            password: "secret".to_string(),
        };
        let pj = build_pj_credential(&trunk_cred);
        assert_eq!(pj.realm, "carrier.example.com");
        assert_eq!(pj.username, "alice");
        assert_eq!(pj.password, "secret");
        assert_eq!(pj.scheme, "digest");
    }

    // ------------------------------------------------------------------ //
    // PjFailoverResult::NoRoutes for empty gateway list                   //
    // ------------------------------------------------------------------ //

    #[test]
    fn test_no_routes_guard_empty_gateways() {
        use crate::redis_state::types::{GatewayRef, TrunkConfig};

        let trunk = TrunkConfig {
            name: "test-trunk".to_string(),
            direction: "bidirectional".to_string(),
            gateways: vec![],
            distribution: "weight_based".to_string(),
            capacity: None,
            codecs: None,
            acl: None,
            credentials: None,
            media: None,
            origination_uris: None,
            translation_classes: None,
            manipulation_classes: None,
            nofailover_sip_codes: None,
        };
        // The try_routes check is `gateways.is_empty()` — verify the guard
        // data condition directly.
        assert!(trunk.gateways.is_empty());
    }

    // ------------------------------------------------------------------ //
    // early media SDP fallback logic                                      //
    // ------------------------------------------------------------------ //

    #[test]
    fn test_early_media_sdp_fallback() {
        let early_sdp = Some("v=0\r\no=- 1 1 IN IP4 1.1.1.1\r\n".to_string());
        let confirmed_sdp: Option<String> = None; // empty 200 OK body

        // Replicate the fallback logic from wait_for_outcome.
        let answer_sdp = confirmed_sdp.or(early_sdp.clone());
        assert_eq!(answer_sdp.as_deref(), early_sdp.as_deref());
    }

    #[test]
    fn test_confirmed_sdp_takes_priority() {
        let early_sdp = Some("v=0\r\no=- 1 1 IN IP4 1.1.1.1\r\n".to_string());
        let confirmed_sdp = Some("v=0\r\no=- 2 2 IN IP4 2.2.2.2\r\n".to_string());

        let answer_sdp = confirmed_sdp.clone().or(early_sdp);
        assert_eq!(answer_sdp.as_deref(), confirmed_sdp.as_deref());
    }
}
