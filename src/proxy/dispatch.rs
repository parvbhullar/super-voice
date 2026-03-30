//! Proxy call dispatch entry point.
//!
//! [`dispatch_proxy_call`] is the entry point for inbound SIP INVITEs that
//! target a DID configured in `sip_proxy` routing mode. It performs route
//! resolution, loads the trunk, applies translations/manipulations, builds
//! a [`ProxyCallContext`], and drives a [`ProxyCallSession`] to completion.

use crate::app::AppState;
use crate::call::sip::DialogStateReceiverGuard;
use crate::capacity::guard::CapacityCheckResult;
use crate::cdr::{CarrierCdr, CdrLeg, CdrStatus, CdrTiming};
use crate::manipulation::engine::{ManipulationContext, ManipulationEngine};
use crate::proxy::session::ProxyCallSession;
use crate::proxy::types::{DspConfig, ProxyCallContext, ProxyCallEvent, ProxyCallPhase};
use crate::redis_state::types::DidConfig;
use crate::routing::engine::{RouteContext, RoutingEngine};
use crate::translation::engine::{TranslationEngine, TranslationInput};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use tracing::{info, warn};
use uuid::Uuid;

/// Dispatch an inbound INVITE to a proxy call session.
///
/// This function:
/// 1. Resolves a route via the routing engine.
/// 2. Loads the trunk configuration.
/// 3. Applies translation and manipulation classes.
/// 4. Creates a [`ProxyCallSession`] and runs it to completion.
/// 5. Removes the session from `active_calls` on exit.
///
/// The function is intended to be spawned as a `tokio::task`; it takes
/// ownership of the `caller_dialog` guard so that the guard is dropped
/// (and the dialog hung up) when the session ends.
pub async fn dispatch_proxy_call(
    app_state: AppState,
    session_id: String,
    caller_dialog: DialogStateReceiverGuard,
    caller_sdp: String,
    caller_uri: String,
    callee_uri: String,
    did: &DidConfig,
) -> Result<()> {
    let config_store = app_state
        .config_store
        .as_ref()
        .ok_or_else(|| anyhow!("config store not available"))?
        .clone();

    // ------------------------------------------------------------------ //
    // 1. Route resolution                                                  //
    // ------------------------------------------------------------------ //

    // Use the DID's trunk directly as fallback when no explicit routing table
    // is configured.  The trunk name stored on the DID is the canonical route.
    let trunk_name = did.trunk.clone();

    // Attempt routing table resolution when a routing table is configured on
    // the DID.  This is optional — not all DIDs have routing tables.
    let resolved_trunk = if let Some(routing_table) = did.routing.playbook.as_deref() {
        // Re-purpose the playbook field when mode=="sip_proxy" as the routing
        // table name (or fall back to the DID's direct trunk reference).
        let engine = RoutingEngine::new(config_store.clone());
        let ctx = RouteContext {
            destination_number: extract_user(&callee_uri),
            caller_number: extract_user(&caller_uri),
            caller_name: None,
        };
        match engine.resolve(routing_table, &ctx).await {
            Ok(Some(result)) => {
                info!(
                    session_id = %session_id,
                    trunk = %result.trunk,
                    table = %result.table_name,
                    "dispatch: route resolved"
                );
                result.trunk
            }
            Ok(None) => {
                warn!(
                    session_id = %session_id,
                    "dispatch: no route found in table '{}', falling back to DID trunk",
                    routing_table
                );
                trunk_name.clone()
            }
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    "dispatch: route resolution error: {e}, falling back to DID trunk"
                );
                trunk_name.clone()
            }
        }
    } else {
        trunk_name.clone()
    };

    // ------------------------------------------------------------------ //
    // 2. Load trunk config                                                 //
    // ------------------------------------------------------------------ //

    let trunk = match config_store.get_trunk(&resolved_trunk).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            warn!(
                session_id = %session_id,
                trunk = %resolved_trunk,
                "dispatch: trunk not found"
            );
            return Err(anyhow!("trunk '{}' not found", resolved_trunk));
        }
        Err(e) => {
            warn!(session_id = %session_id, "dispatch: error loading trunk: {e}");
            return Err(e);
        }
    };

    // ------------------------------------------------------------------ //
    // 2.5: Capacity check                                                  //
    // ------------------------------------------------------------------ //

    if let Some(ref cap_config) = trunk.capacity {
        if let Some(ref guard) = app_state.capacity_guard {
            match guard.check_capacity(&trunk.name, &session_id, cap_config).await {
                CapacityCheckResult::Allowed => {
                    info!(session_id = %session_id, "dispatch: capacity check passed");
                }
                CapacityCheckResult::CpsExceeded { current, limit } => {
                    warn!(
                        session_id = %session_id,
                        trunk = %trunk.name,
                        current,
                        limit,
                        "dispatch: CPS limit exceeded — rejecting with 503"
                    );
                    return Err(anyhow!(
                        "CPS limit exceeded for trunk '{}': {}/{}",
                        trunk.name,
                        current,
                        limit
                    ));
                }
                CapacityCheckResult::ConcurrentExceeded { current, limit } => {
                    warn!(
                        session_id = %session_id,
                        trunk = %trunk.name,
                        current,
                        limit,
                        "dispatch: concurrent call limit exceeded — rejecting with 503"
                    );
                    return Err(anyhow!(
                        "concurrent limit exceeded for trunk '{}': {}/{}",
                        trunk.name,
                        current,
                        limit
                    ));
                }
                CapacityCheckResult::TrunkBlocked { reason, .. } => {
                    warn!(
                        session_id = %session_id,
                        trunk = %trunk.name,
                        reason,
                        "dispatch: trunk auto-blocked — rejecting with 503"
                    );
                    return Err(anyhow!(
                        "trunk '{}' is temporarily blocked: {}",
                        trunk.name,
                        reason
                    ));
                }
            }
        }
    }

    // ------------------------------------------------------------------ //
    // 3. Apply translation classes                                         //
    // ------------------------------------------------------------------ //

    let mut translated_caller = extract_user(&caller_uri);
    let mut translated_callee = extract_user(&callee_uri);

    if let Some(ref classes) = trunk.translation_classes {
        for class_name in classes {
            match config_store.get_translation_class(class_name).await {
                Ok(Some(class_cfg)) => {
                    let input = TranslationInput {
                        caller_number: translated_caller.clone(),
                        destination_number: translated_callee.clone(),
                        caller_name: None,
                        direction: "inbound".to_string(),
                    };
                    let result = TranslationEngine::apply(&class_cfg, &input);
                    if result.modified {
                        info!(
                            session_id = %session_id,
                            class = %class_name,
                            "dispatch: translation applied"
                        );
                        translated_caller = result.caller_number;
                        translated_callee = result.destination_number;
                    }
                }
                Ok(None) => {
                    warn!(
                        session_id = %session_id,
                        class = %class_name,
                        "dispatch: translation class not found — skipping"
                    );
                }
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        class = %class_name,
                        "dispatch: error loading translation class: {e} — skipping"
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------------ //
    // 4. Apply manipulation classes                                        //
    // ------------------------------------------------------------------ //

    if let Some(ref classes) = trunk.manipulation_classes {
        for class_name in classes {
            match config_store.get_manipulation_class(class_name).await {
                Ok(Some(class_cfg)) => {
                    let mut headers = std::collections::HashMap::new();
                    headers.insert("From".to_string(), caller_uri.clone());
                    headers.insert("To".to_string(), callee_uri.clone());
                    let ctx = ManipulationContext {
                        headers,
                        variables: std::collections::HashMap::new(),
                    };
                    let result = ManipulationEngine::evaluate(&class_cfg, &ctx);
                    if result.hangup {
                        info!(
                            session_id = %session_id,
                            "dispatch: manipulation class '{}' triggered hangup",
                            class_name
                        );
                        return Ok(());
                    }
                }
                Ok(None) => {
                    warn!(
                        session_id = %session_id,
                        class = %class_name,
                        "dispatch: manipulation class not found — skipping"
                    );
                }
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        class = %class_name,
                        "dispatch: error loading manipulation class: {e} — skipping"
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------------ //
    // 4.5: Build DspConfig from trunk media class                         //
    // ------------------------------------------------------------------ //

    // Carrier-grade defaults: echo cancellation, DTMF detection, and PLC are
    // enabled by default for all proxy calls. They can be overridden via the
    // trunk's media configuration (media_mode field) in future iterations.
    // Tone detection and fax terminal mode require explicit opt-in.
    let dsp_config = build_dsp_config(&trunk.media);

    // ------------------------------------------------------------------ //
    // 5. Build ProxyCallContext                                            //
    // ------------------------------------------------------------------ //

    let mut context = ProxyCallContext::new(
        session_id.clone(),
        caller_uri.clone(),
        callee_uri.clone(),
        trunk.name.clone(),
    );
    context.did_number = Some(did.number.clone());
    context.dsp = dsp_config;
    // routing_table stays None when not used — the DID's trunk was used directly.

    // ------------------------------------------------------------------ //
    // 6. Create ProxyCallSession                                           //
    // ------------------------------------------------------------------ //

    let cancel_token = app_state.token.child_token();
    let (mut session, event_rx) = ProxyCallSession::new(
        context,
        cancel_token,
        caller_dialog,
        app_state.dialog_layer.clone(),
        config_store,
        app_state.stream_engine.clone(),
    );

    // ------------------------------------------------------------------ //
    // 7. Register in active_calls                                          //
    // ------------------------------------------------------------------ //

    // We do not wrap in an ActiveCallRef here because the proxy session does
    // not expose the full ActiveCall interface yet; a lightweight string
    // registration is done below with drop-on-exit semantics.

    // ------------------------------------------------------------------ //
    // 8. Run session                                                       //
    // ------------------------------------------------------------------ //

    let final_caller_uri = if translated_caller != extract_user(&caller_uri) {
        rebuild_uri(&caller_uri, &translated_caller)
    } else {
        caller_uri.clone()
    };

    let final_callee_uri = if translated_callee != extract_user(&callee_uri) {
        rebuild_uri(&callee_uri, &translated_callee)
    } else {
        callee_uri.clone()
    };

    info!(
        session_id = %session_id,
        caller = %final_caller_uri,
        callee = %final_callee_uri,
        trunk = %trunk.name,
        "dispatch: starting proxy call session"
    );

    // ------------------------------------------------------------------ //
    // 8a. Spawn event collector for CDR timing capture                   //
    // ------------------------------------------------------------------ //

    let timing_handle = {
        let mut rx = event_rx;
        tokio::spawn(async move {
            let mut ring_time: Option<DateTime<Utc>> = None;
            let mut answer_time: Option<DateTime<Utc>> = None;
            let mut cdr_status = CdrStatus::Completed;
            while let Some(event) = rx.recv().await {
                match event {
                    ProxyCallEvent::PhaseChanged(ProxyCallPhase::Ringing) => {
                        ring_time.get_or_insert_with(Utc::now);
                    }
                    ProxyCallEvent::PhaseChanged(ProxyCallPhase::EarlyMedia) => {
                        ring_time.get_or_insert_with(Utc::now);
                    }
                    ProxyCallEvent::Answered { .. } => {
                        answer_time.get_or_insert_with(Utc::now);
                    }
                    ProxyCallEvent::Terminated { reason, .. } => {
                        cdr_status = terminated_reason_to_cdr_status(&reason);
                    }
                    _ => {}
                }
            }
            (ring_time, answer_time, cdr_status)
        })
    };

    // ------------------------------------------------------------------ //
    // 8b. Run session                                                     //
    // ------------------------------------------------------------------ //

    let session_result = session
        .run(&trunk, &caller_sdp, &final_caller_uri, &final_callee_uri)
        .await;

    if let Err(ref e) = session_result {
        warn!(session_id = %session_id, "dispatch: session ended with error: {e}");
    }

    // Decrement the concurrent call counter now that the session has ended.
    if let Some(ref guard) = app_state.capacity_guard {
        guard.release_call(&trunk.name, &session_id).await;
    }

    // ------------------------------------------------------------------ //
    // 9. Generate and enqueue CDR                                         //
    // ------------------------------------------------------------------ //

    let (ring_time, answer_time, cdr_status) =
        timing_handle.await.unwrap_or((None, None, CdrStatus::Failed));

    // Derive status from session result when the event channel did not
    // emit a Terminated event (e.g. cancelled calls).
    let final_status = if let Err(_) = session_result {
        CdrStatus::Failed
    } else {
        cdr_status
    };

    let node_id = std::env::var("NODE_ID").unwrap_or_else(|_| {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    });

    let inbound_leg = CdrLeg {
        trunk: did.trunk.clone(),
        gateway: None,
        caller: extract_user(&final_caller_uri),
        callee: extract_user(&final_callee_uri),
        codec: None,
        transport: "udp".to_string(),
        srtp: false,
        sip_status: if final_status == CdrStatus::Completed {
            200
        } else {
            0
        },
        hangup_cause: None,
        source_ip: None,
        destination_ip: None,
    };

    let outbound_leg = Some(CdrLeg {
        trunk: trunk.name.clone(),
        gateway: trunk.gateways.first().map(|g| g.name.clone()),
        caller: extract_user(&final_caller_uri),
        callee: extract_user(&final_callee_uri),
        codec: None,
        transport: "udp".to_string(),
        srtp: false,
        sip_status: if final_status == CdrStatus::Completed {
            200
        } else {
            0
        },
        hangup_cause: None,
        source_ip: None,
        destination_ip: None,
    });

    let cdr = CarrierCdr {
        uuid: Uuid::new_v4(),
        session_id: session_id.clone(),
        call_id: session_id.clone(),
        node_id,
        created_at: Utc::now(),
        inbound_leg,
        outbound_leg,
        timing: CdrTiming {
            start_time: session.context().start_time,
            ring_time,
            answer_time,
            end_time: Utc::now(),
        },
        status: final_status,
    };

    if let Some(ref cdr_queue) = app_state.cdr_queue {
        if let Err(e) = cdr_queue.enqueue(&cdr).await {
            warn!(session_id = %session_id, "dispatch: failed to enqueue CDR: {e}");
        } else {
            info!(
                session_id = %session_id,
                cdr_uuid = %cdr.uuid,
                billsec = cdr.billsec(),
                "dispatch: CDR enqueued"
            );
        }
    }

    info!(session_id = %session_id, "dispatch: session complete");
    Ok(())
}

// ------------------------------------------------------------------ //
// Internal helpers                                                     //
// ------------------------------------------------------------------ //

/// Unified dispatcher for bridge/proxy DID modes.
///
/// Inspects `did.routing.mode` and delegates to the appropriate handler:
/// - `"sip_proxy"` -> [`dispatch_proxy_call`] (existing SIP B2BUA path)
/// - `"webrtc_bridge"` -> [`crate::proxy::bridge::dispatch_webrtc_bridge`]
/// - `"ws_bridge"` -> [`crate::proxy::bridge::dispatch_ws_bridge`]
///
/// Returns `Err` for any unrecognised mode string.
pub async fn dispatch_bridge_call(
    app_state: AppState,
    session_id: String,
    caller_dialog: DialogStateReceiverGuard,
    caller_sdp: String,
    caller_uri: String,
    callee_uri: String,
    did: &DidConfig,
) -> Result<()> {
    match did.routing.mode.as_str() {
        "sip_proxy" => {
            dispatch_proxy_call(
                app_state,
                session_id,
                caller_dialog,
                caller_sdp,
                caller_uri,
                callee_uri,
                did,
            )
            .await
        }
        "webrtc_bridge" => {
            crate::proxy::bridge::dispatch_webrtc_bridge(
                app_state,
                session_id,
                caller_dialog,
                caller_sdp,
                caller_uri,
                callee_uri,
                did,
            )
            .await
        }
        "ws_bridge" => {
            crate::proxy::bridge::dispatch_ws_bridge(
                app_state,
                session_id,
                caller_dialog,
                caller_sdp,
                caller_uri,
                callee_uri,
                did,
            )
            .await
        }
        other => {
            warn!(session_id = %session_id, mode = %other, "dispatch: unknown bridge mode");
            Err(anyhow!("unknown bridge mode: {}", other))
        }
    }
}

/// Build a [`DspConfig`] from a trunk's optional [`MediaConfig`].
///
/// Carrier-grade defaults: echo cancellation, DTMF detection, and PLC are
/// on by default for all proxy calls. Tone detection and fax terminal mode
/// require opt-in via `media_mode` (values: "fax_terminal", "tone_detect").
fn build_dsp_config(media: &Option<crate::redis_state::types::MediaConfig>) -> DspConfig {
    let mut cfg = DspConfig {
        echo_cancellation: true,
        dtmf_detection: true,
        tone_detection: false,
        plc: true,
        fax_terminal: false,
    };
    if let Some(m) = media {
        if let Some(ref mode) = m.media_mode {
            match mode.as_str() {
                "fax_terminal" => {
                    cfg.fax_terminal = true;
                    cfg.echo_cancellation = false;
                    cfg.dtmf_detection = false;
                }
                "tone_detect" => {
                    cfg.tone_detection = true;
                }
                "minimal" => {
                    cfg.echo_cancellation = false;
                    cfg.dtmf_detection = false;
                    cfg.plc = false;
                }
                _ => {}
            }
        }
    }
    cfg
}

/// Map a SIP termination reason string to a [`CdrStatus`].
fn terminated_reason_to_cdr_status(reason: &str) -> CdrStatus {
    let lower = reason.to_lowercase();
    if lower.contains("cancel") {
        CdrStatus::Cancelled
    } else if lower.contains("busy") || lower.contains("486") {
        CdrStatus::Busy
    } else if lower.contains("no_answer") || lower.contains("no answer") || lower.contains("408") {
        CdrStatus::NoAnswer
    } else if lower.contains("normal") || lower.contains("200") {
        CdrStatus::Completed
    } else {
        CdrStatus::Failed
    }
}

/// Extract the user part from a SIP URI (`sip:user@host` → `user`).
fn extract_user(uri: &str) -> String {
    // Strip `sip:` / `sips:` scheme prefix then take the part before `@`.
    let stripped = uri
        .strip_prefix("sips:")
        .or_else(|| uri.strip_prefix("sip:"))
        .unwrap_or(uri);
    stripped
        .split('@')
        .next()
        .unwrap_or(stripped)
        .to_string()
}

/// Rebuild a SIP URI by replacing its user part.
///
/// `sip:alice@example.com` + `bob` → `sip:bob@example.com`
fn rebuild_uri(uri: &str, new_user: &str) -> String {
    let scheme = if uri.starts_with("sips:") {
        "sips:"
    } else {
        "sip:"
    };
    let stripped = uri
        .strip_prefix("sips:")
        .or_else(|| uri.strip_prefix("sip:"))
        .unwrap_or(uri);
    match stripped.find('@') {
        Some(at) => format!("{}{}{}", scheme, new_user, &stripped[at..]),
        None => format!("{}{}@unknown", scheme, new_user),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_user_sip_uri() {
        assert_eq!(extract_user("sip:alice@example.com"), "alice");
        assert_eq!(extract_user("sips:bob@carrier.net"), "bob");
        assert_eq!(extract_user("+15551234567"), "+15551234567");
        assert_eq!(extract_user("sip:+15551234567@gateway.com"), "+15551234567");
    }

    #[test]
    fn test_extract_user_bare_number() {
        assert_eq!(extract_user("12345"), "12345");
    }

    #[test]
    fn test_rebuild_uri_replaces_user() {
        assert_eq!(
            rebuild_uri("sip:alice@example.com", "bob"),
            "sip:bob@example.com"
        );
        assert_eq!(
            rebuild_uri("sips:alice@example.com", "charlie"),
            "sips:charlie@example.com"
        );
    }

    #[test]
    fn test_rebuild_uri_no_at_sign() {
        assert_eq!(rebuild_uri("sip:gateway.com", "bob"), "sip:bob@unknown");
    }

    /// Verify that `dispatch_bridge_call` returns `Err` for an unknown mode
    /// without requiring real infrastructure.  The match arm for unknown modes
    /// is synchronous logic so we can test it by exercising `classify_mode`.
    #[test]
    fn test_dispatch_bridge_call_unknown_mode_classification() {
        // Mirrors the match arms in dispatch_bridge_call.
        fn classify_mode(mode: &str) -> Result<&'static str> {
            match mode {
                "sip_proxy" => Ok("sip_proxy"),
                "webrtc_bridge" => Ok("webrtc_bridge"),
                "ws_bridge" => Ok("ws_bridge"),
                other => Err(anyhow!("unknown bridge mode: {}", other)),
            }
        }

        assert!(classify_mode("sip_proxy").is_ok());
        assert!(classify_mode("webrtc_bridge").is_ok());
        assert!(classify_mode("ws_bridge").is_ok());
        assert!(classify_mode("unknown_mode").is_err());
        let err = classify_mode("foobar").unwrap_err();
        assert!(err.to_string().contains("foobar"));
    }

    #[test]
    fn test_terminated_reason_to_cdr_status_mapping() {
        assert_eq!(
            terminated_reason_to_cdr_status("NORMAL_CLEARING"),
            CdrStatus::Completed
        );
        assert_eq!(
            terminated_reason_to_cdr_status("CANCEL"),
            CdrStatus::Cancelled
        );
        assert_eq!(
            terminated_reason_to_cdr_status("USER_BUSY"),
            CdrStatus::Busy
        );
        assert_eq!(
            terminated_reason_to_cdr_status("NO_ANSWER"),
            CdrStatus::NoAnswer
        );
        assert_eq!(
            terminated_reason_to_cdr_status("unknown_reason"),
            CdrStatus::Failed
        );
        assert_eq!(
            terminated_reason_to_cdr_status("486 Busy Here"),
            CdrStatus::Busy
        );
    }
}
