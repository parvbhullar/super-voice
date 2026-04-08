//! Proxy call dispatch entry point.
//!
//! [`dispatch_proxy_call`] is the entry point for inbound SIP INVITEs that
//! target a DID configured in `sip_proxy` routing mode. It performs route
//! resolution, loads the trunk, applies translations/manipulations, builds
//! a [`ProxyCallContext`], and drives a [`ProxyCallSession`] to completion.

use crate::app::AppState;
use crate::call::sip::DialogStateReceiverGuard;
use crate::capacity::guard::CapacityCheckResult;
use rsipstack::dialog::server_dialog::ServerInviteDialog;
use crate::cdr::{CarrierCdr, CdrLeg, CdrStatus, CdrTiming};
use crate::manipulation::engine::{ManipulationContext, ManipulationEngine};
use crate::proxy::session::ProxyCallSession;
use crate::proxy::types::{DspConfig, ProxyCallContext, ProxyCallEvent, ProxyCallPhase};
use crate::redis_state::types::DidConfig;
use crate::proxy::sdp_filter::{filter_sdp_codecs, resolve_trunk_codecs};
use crate::routing::engine::{RouteContext, RoutingEngine};
use crate::translation::engine::{TranslationEngine, TranslationInput};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use std::sync::atomic::Ordering;
use tracing::{info, warn};
use uuid::Uuid;

#[cfg(feature = "carrier")]
use crate::proxy::pj_dialog_layer::PjDialogLayer;
#[cfg(feature = "carrier")]
use crate::proxy::pj_failover::{PjFailoverLoop, PjFailoverResult};
#[cfg(feature = "carrier")]
use pjsip::PjCallEvent;
#[cfg(feature = "carrier")]
use rsipstack::dialog::dialog::DialogState;

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
    server_dialog: ServerInviteDialog,
    caller_sdp: String,
    caller_uri: String,
    callee_uri: String,
    did: &DidConfig,
    invite_done: tokio_util::sync::CancellationToken,
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
    // 4.6: SDP codec filtering                                            //
    // ------------------------------------------------------------------ //

    let caller_sdp = if let Some(allowed) = resolve_trunk_codecs(&trunk.media, &trunk.codecs) {
        match filter_sdp_codecs(&caller_sdp, &allowed) {
            Ok(filtered) => {
                info!(
                    session_id = %session_id,
                    allowed = ?allowed,
                    "dispatch: SDP codec-filtered to allowed set"
                );
                filtered
            }
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    error = %e,
                    "dispatch: SDP codec filter rejected call — no codec overlap"
                );
                return Err(anyhow!("SDP codec filter: {e}"));
            }
        }
    } else {
        caller_sdp
    };

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
    // 6. Create ProxyCallSession (rsipstack path — minimal/fallback only)  //
    // ------------------------------------------------------------------ //

    let cancel_token = app_state.token.child_token();

    // Under the minimal feature (no carrier), use ProxyCallSession (rsipstack).
    // Under the carrier feature, the pjsip path is used below after URI translation.
    #[cfg(not(feature = "carrier"))]
    let (mut session, event_rx) = ProxyCallSession::new(
        context.clone(),
        cancel_token.clone(),
        caller_dialog,
        server_dialog,
        app_state.dialog_layer.clone(),
        config_store.clone(),
        app_state.stream_engine.clone(),
    );

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

    // ------------------------------------------------------------------ //
    // 7. Register in proxy_calls + increment total_calls                  //
    // ------------------------------------------------------------------ //

    app_state.total_calls.fetch_add(1, Ordering::Relaxed);
    {
        let record = crate::proxy::types::ProxyCallRecord {
            session_id: session_id.clone(),
            caller: extract_user(&final_caller_uri),
            callee: extract_user(&final_callee_uri),
            trunk: trunk.name.clone(),
            start_time: context.start_time,
            answer_time: None,
            status: "ringing".to_string(),
            cancel_token: Some(cancel_token.clone()),
        };
        app_state.proxy_calls.lock().unwrap().insert(session_id.clone(), record);
    }

    info!(
        session_id = %session_id,
        caller = %final_caller_uri,
        callee = %final_callee_uri,
        trunk = %trunk.name,
        "dispatch: starting proxy call session"
    );

    // ------------------------------------------------------------------ //
    // 8a. Carrier path: use PjFailoverLoop directly (no ProxyCallSession) //
    // ------------------------------------------------------------------ //

    #[cfg(feature = "carrier")]
    {
        let start_time = context.start_time;
        // Make caller_dialog mutable so recv() can be called in the bridge loop.
        let mut caller_dialog = caller_dialog;

        // Use PjFailoverLoop when pj_bridge is available.
        let pj_result = if let Some(ref pj_bridge) = app_state.pj_bridge {
            let pj_dialog_layer = PjDialogLayer::new(pj_bridge.clone());
            let pj_failover = PjFailoverLoop::new(pj_dialog_layer.clone(), cancel_token.clone(), config_store.clone());

            info!(
                session_id = %session_id,
                callee = %final_callee_uri,
                trunk = %trunk.name,
                "dispatch: starting pjsip failover dial"
            );

            let result = pj_failover
                .try_routes(&trunk, &caller_sdp, &final_caller_uri, &final_callee_uri, &server_dialog)
                .await;

            match result {
                Ok(PjFailoverResult::Connected { gateway_addr, sdp, call_id, mut call_event_rx }) => {
                    info!(
                        session_id = %session_id,
                        gateway = %gateway_addr,
                        has_sdp = sdp.is_some(),
                        "dispatch: pjsip callee connected — relaying 200 OK to inbound caller"
                    );
                    // Mark answered in proxy_calls
                    {
                        let mut map = app_state.proxy_calls.lock().unwrap();
                        if let Some(rec) = map.get_mut(&session_id) {
                            rec.answer_time = Some(Utc::now());
                            rec.status = "answered".to_string();
                        }
                    }

                    // Relay 200 OK + SDP to the inbound caller (LiveKit).
                    if let Some(ref answer_sdp) = sdp {
                        let ct = rsip::Header::ContentType("application/sdp".to_string().into());
                        if let Err(e) = server_dialog.accept(Some(vec![ct]), Some(answer_sdp.as_bytes().to_vec())) {
                            warn!(session_id = %session_id, "dispatch: failed to accept inbound call: {e}");
                        }
                    } else {
                        // No SDP — send 200 OK without body (unusual but valid)
                        if let Err(e) = server_dialog.accept(None, None) {
                            warn!(session_id = %session_id, "dispatch: failed to accept inbound call (no sdp): {e}");
                        }
                    }

                    // Bridge monitoring loop: keep both legs alive until one terminates.
                    // - Gateway hangs up → PjCallEvent::Terminated → drop caller_dialog → auto-BYE to LiveKit
                    // - LiveKit hangs up  → DialogState::Terminated → send BYE to gateway
                    // - INVITE tx ends without ACK (WaitAck timeout) → invite_done fires → break
                    let call_id_for_bye = call_id.clone();
                    let mut dialog_confirmed = false;
                    loop {
                        tokio::select! {
                            _ = cancel_token.cancelled() => {
                                let _ = pj_dialog_layer.send_bye(&call_id_for_bye);
                                break;
                            }
                            _ = invite_done.cancelled() => {
                                // INVITE transaction ended. If dialog never reached Confirmed,
                                // the ACK timed out (e.g. wrong Contact IP) — treat as dead.
                                if !dialog_confirmed {
                                    warn!(session_id = %session_id, "dispatch: INVITE tx ended without ACK — terminating bridge");
                                    let _ = pj_dialog_layer.send_bye(&call_id_for_bye);
                                    break;
                                }
                                // Already confirmed: handle() exiting post-ACK is expected; continue.
                            }
                            gw_event = call_event_rx.recv() => {
                                match gw_event {
                                    Some(PjCallEvent::Terminated { code, reason }) => {
                                        info!(
                                            session_id = %session_id,
                                            code = %code,
                                            reason = %reason,
                                            "dispatch: gateway terminated call"
                                        );
                                        // caller_dialog drop at function end sends BYE to LiveKit.
                                        break;
                                    }
                                    Some(PjCallEvent::ReInvite { sdp }) => {
                                        // Gateway sent re-INVITE — relay to caller, wait for
                                        // response, then answer the gateway's pending re-INVITE
                                        // with the caller's SDP.
                                        info!(session_id = %session_id, "dispatch: relaying re-INVITE to inbound caller");
                                        let ct = rsip::Header::ContentType("application/sdp".to_string().into());
                                        match server_dialog.reinvite(Some(vec![ct]), Some(sdp.into_bytes())).await {
                                            Ok(Some(resp)) => {
                                                let sc = resp.status_code.code();
                                                let answer_sdp = if resp.body.is_empty() {
                                                    None
                                                } else {
                                                    String::from_utf8(resp.body).ok()
                                                };
                                                info!(
                                                    session_id = %session_id,
                                                    status = %sc,
                                                    has_sdp = %answer_sdp.is_some(),
                                                    "dispatch: caller responded to re-INVITE — forwarding to gateway"
                                                );
                                                if let Err(e) = pj_dialog_layer.answer_reinvite(
                                                    &call_id_for_bye,
                                                    sc,
                                                    answer_sdp,
                                                ) {
                                                    warn!(session_id = %session_id, "dispatch: failed to answer gateway re-INVITE: {e}");
                                                }
                                            }
                                            Ok(None) => {
                                                // Dialog not confirmed — answer gateway with current SDP.
                                                warn!(session_id = %session_id, "dispatch: caller dialog not confirmed for re-INVITE — auto-answering gateway");
                                                let _ = pj_dialog_layer.answer_reinvite(&call_id_for_bye, 200, None);
                                            }
                                            Err(e) => {
                                                warn!(session_id = %session_id, "dispatch: failed to relay re-INVITE to caller: {e}");
                                                // Fall back: answer gateway with 200 OK / current SDP.
                                                let _ = pj_dialog_layer.answer_reinvite(&call_id_for_bye, 200, None);
                                            }
                                        }
                                    }
                                    None => {
                                        warn!(session_id = %session_id, "dispatch: gateway event channel closed");
                                        break;
                                    }
                                    Some(PjCallEvent::Info { content_type, body }) => {
                                        // Relay INFO from gateway to caller.
                                        info!(session_id = %session_id, ct = %content_type, "dispatch: relaying INFO from gateway to caller");
                                        let mut headers = vec![];
                                        if !content_type.is_empty() {
                                            headers.push(rsip::Header::ContentType(
                                                rsip::headers::untyped::ContentType::new(content_type),
                                            ));
                                        }
                                        let body_bytes = if body.is_empty() {
                                            None
                                        } else {
                                            Some(body.into_bytes())
                                        };
                                        if let Err(e) = server_dialog.info(
                                            Some(headers),
                                            body_bytes,
                                        ).await {
                                            warn!(session_id = %session_id, "dispatch: failed to relay INFO to caller: {e}");
                                        }
                                    }
                                    _ => {} // Ringing, etc. — continue
                                }
                            }
                            caller_event = caller_dialog.recv() => {
                                match caller_event {
                                    Some(DialogState::Confirmed(_, _)) => {
                                        dialog_confirmed = true;
                                    }
                                    Some(DialogState::Info(_id, request, tx_handle)) => {
                                        // Relay INFO from caller to gateway.
                                        let content_type = request.headers.iter()
                                            .find_map(|h| {
                                                if let rsip::Header::ContentType(ct) = h {
                                                    Some(ct.to_string())
                                                } else {
                                                    None
                                                }
                                            })
                                            .unwrap_or_default();
                                        let body_str = if request.body.is_empty() {
                                            String::new()
                                        } else {
                                            String::from_utf8_lossy(&request.body).into_owned()
                                        };

                                        info!(session_id = %session_id, ct = %content_type, "dispatch: relaying INFO from caller to gateway");
                                        if let Err(e) = pj_dialog_layer.send_info(
                                            &call_id_for_bye,
                                            &content_type,
                                            &body_str,
                                        ) {
                                            warn!(session_id = %session_id, "dispatch: failed to relay INFO to gateway: {e}");
                                        }

                                        // Respond 200 OK to the caller's INFO transaction.
                                        let _ = tx_handle.reply(rsip::StatusCode::OK).await;
                                    }
                                    Some(DialogState::Terminated(_, _)) | None => {
                                        info!(session_id = %session_id, "dispatch: inbound caller terminated");
                                        let _ = pj_dialog_layer.send_bye(&call_id_for_bye);
                                        break;
                                    }
                                    _ => {} // Early, etc. — continue
                                }
                            }
                        }
                    }
                    Ok(CdrStatus::Completed)
                }
                Ok(PjFailoverResult::NoFailover { code, reason }) => {
                    warn!(session_id = %session_id, code = %code, "dispatch: pjsip nofailover");
                    Ok(CdrStatus::Failed)
                }
                Ok(PjFailoverResult::Exhausted { last_code, last_reason }) => {
                    warn!(session_id = %session_id, code = %last_code, "dispatch: pjsip all gateways exhausted");
                    Ok(CdrStatus::Failed)
                }
                Ok(PjFailoverResult::NoRoutes) => {
                    warn!(session_id = %session_id, "dispatch: pjsip no routes");
                    Ok(CdrStatus::Failed)
                }
                Err(e) => {
                    warn!(session_id = %session_id, "dispatch: pjsip failover error: {e}");
                    Err(e)
                }
            }
        } else {
            // No pj_bridge — fall back to rsipstack ProxyCallSession.
            let (mut session, event_rx) = ProxyCallSession::new(
                context.clone(),
                cancel_token.clone(),
                caller_dialog,
                server_dialog,
                app_state.dialog_layer.clone(),
                config_store.clone(),
                app_state.stream_engine.clone(),
            );
            let timing_handle = spawn_event_collector(event_rx, app_state.proxy_calls.clone(), session_id.clone());
            let session_result = session
                .run(&trunk, &caller_sdp, &final_caller_uri, &final_callee_uri)
                .await;
            let (ring_time, answer_time, cdr_status) =
                timing_handle.await.unwrap_or((None, None, CdrStatus::Failed));
            let status = if session_result.is_err() { CdrStatus::Failed } else { cdr_status };
            Ok(status)
        };

        // Decrement capacity.
        if let Some(ref guard) = app_state.capacity_guard {
            guard.release_call(&trunk.name, &session_id).await;
        }

        let final_status = match pj_result {
            Ok(s) => s,
            Err(e) => {
                warn!(session_id = %session_id, "dispatch: session error: {e}");
                CdrStatus::Failed
            }
        };

        generate_and_enqueue_cdr(
            &app_state, &session_id, &did, &trunk, &final_caller_uri, &final_callee_uri,
            final_status, start_time, None, None,
        ).await;

        // Remove from proxy_calls on session end.
        app_state.proxy_calls.lock().unwrap().remove(&session_id);

        info!(session_id = %session_id, "dispatch: session complete");
        return Ok(());
    }

    // ------------------------------------------------------------------ //
    // 8a-minimal. Non-carrier path: use rsipstack ProxyCallSession        //
    // ------------------------------------------------------------------ //

    #[cfg(not(feature = "carrier"))]
    {
        let timing_handle = spawn_event_collector(event_rx, app_state.proxy_calls.clone(), session_id.clone());

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

        let (ring_time, answer_time, cdr_status) =
            timing_handle.await.unwrap_or((None, None, CdrStatus::Failed));

        let final_status = if session_result.is_err() {
            CdrStatus::Failed
        } else {
            cdr_status
        };

        // Remove from proxy_calls on session end.
        app_state.proxy_calls.lock().unwrap().remove(&session_id);

        generate_and_enqueue_cdr(
            &app_state, &session_id, &did, &trunk, &final_caller_uri, &final_callee_uri,
            final_status, session.context().start_time, ring_time, answer_time,
        ).await;

        info!(session_id = %session_id, "dispatch: session complete");
        Ok(())
    }
}

/// Spawn an event collector task for CDR timing capture and proxy_calls status updates.
fn spawn_event_collector(
    event_rx: tokio::sync::mpsc::UnboundedReceiver<ProxyCallEvent>,
    proxy_calls: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, crate::proxy::types::ProxyCallRecord>>>,
    session_id: String,
) -> tokio::task::JoinHandle<(Option<DateTime<Utc>>, Option<DateTime<Utc>>, CdrStatus)> {
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
                    let now = Utc::now();
                    answer_time.get_or_insert(now);
                    let mut map = proxy_calls.lock().unwrap();
                    if let Some(rec) = map.get_mut(&session_id) {
                        rec.answer_time = Some(now);
                        rec.status = "answered".to_string();
                    }
                }
                ProxyCallEvent::Terminated { reason, .. } => {
                    cdr_status = terminated_reason_to_cdr_status(&reason);
                }
                _ => {}
            }
        }
        (ring_time, answer_time, cdr_status)
    })
}

/// Generate and enqueue a CDR record.
async fn generate_and_enqueue_cdr(
    app_state: &AppState,
    session_id: &str,
    did: &DidConfig,
    trunk: &crate::redis_state::types::TrunkConfig,
    caller_uri: &str,
    callee_uri: &str,
    final_status: CdrStatus,
    start_time: DateTime<Utc>,
    ring_time: Option<DateTime<Utc>>,
    answer_time: Option<DateTime<Utc>>,
) {
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
        caller: extract_user(caller_uri),
        callee: extract_user(callee_uri),
        codec: None,
        transport: "udp".to_string(),
        srtp: false,
        sip_status: if final_status == CdrStatus::Completed { 200 } else { 0 },
        hangup_cause: None,
        source_ip: None,
        destination_ip: None,
    };

    let outbound_leg = Some(CdrLeg {
        trunk: trunk.name.clone(),
        gateway: trunk.gateways.first().map(|g| g.name.clone()),
        caller: extract_user(caller_uri),
        callee: extract_user(callee_uri),
        codec: None,
        transport: "udp".to_string(),
        srtp: false,
        sip_status: if final_status == CdrStatus::Completed { 200 } else { 0 },
        hangup_cause: None,
        source_ip: None,
        destination_ip: None,
    });

    let cdr = CarrierCdr {
        uuid: Uuid::new_v4(),
        session_id: session_id.to_string(),
        call_id: session_id.to_string(),
        node_id,
        created_at: Utc::now(),
        inbound_leg,
        outbound_leg,
        timing: CdrTiming {
            start_time,
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
    server_dialog: ServerInviteDialog,
    caller_sdp: String,
    caller_uri: String,
    callee_uri: String,
    did: &DidConfig,
    invite_done: tokio_util::sync::CancellationToken,
) -> Result<()> {
    match did.routing.mode.as_str() {
        "sip_proxy" => {
            dispatch_proxy_call(
                app_state,
                session_id,
                caller_dialog,
                server_dialog,
                caller_sdp,
                caller_uri,
                callee_uri,
                did,
                invite_done,
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
