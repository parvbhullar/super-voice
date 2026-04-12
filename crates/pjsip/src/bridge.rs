// crates/pjsip/src/bridge.rs
//! Dedicated OS thread running the pjsip event loop with mpsc channel
//! bridge to Tokio async.
//!
//! Architecture (identical pattern to sofia-sip bridge):
//! ```text
//!   Tokio async            pjsip OS thread
//!   ──────────             ───────────────
//!   per-call event_rx ◄──  C callback → event_tx.send(PjCallEvent)
//!   send_command()    ──►  cmd_rx.try_recv() → pjsip API calls
//! ```

use crate::command::{PjCommand, PjCredential};
use crate::endpoint::{PjEndpoint, PjEndpointConfig};
use crate::event::{PjCallEvent, PjCallEventSender};
use crate::session::{CallEntry, CALL_REGISTRY};
use anyhow::Result;
use std::ffi::CString;
use std::ptr;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{debug, error, info, warn};

/// Bridge between the dedicated pjsip OS thread and Tokio async.
pub struct PjBridge {
    cmd_tx: UnboundedSender<PjCommand>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl PjBridge {
    /// Start the pjsip event loop on a dedicated OS thread.
    pub fn start(config: PjEndpointConfig) -> Result<Self> {
        let (cmd_tx, cmd_rx) = unbounded_channel::<PjCommand>();

        let thread_handle = std::thread::Builder::new()
            .name("pjsip".to_owned())
            .spawn(move || {
                pjsip_thread_main(config, cmd_rx);
            })?;

        Ok(Self {
            cmd_tx,
            thread_handle: Some(thread_handle),
        })
    }

    /// Send a command to the pjsip thread.
    pub fn send_command(&self, cmd: PjCommand) -> Result<()> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| anyhow::anyhow!("pjsip command channel closed"))?;
        Ok(())
    }
}

impl Drop for PjBridge {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(PjCommand::Shutdown);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

// ---------------------------------------------------------------------------
// pjsip thread entry point
// ---------------------------------------------------------------------------

fn pjsip_thread_main(config: PjEndpointConfig, mut cmd_rx: UnboundedReceiver<PjCommand>) {
    // pj_init() is called inside PjEndpoint::create(). It initializes pjlib and
    // automatically registers the calling (pjsip) thread — no pj_thread_register()
    // needed here. Calling pj_thread_register() *before* pj_init() is wrong: pjlib
    // global state (thread-count atomics, TLS key, etc.) is not set up yet and the
    // call will SIGSEGV.

    // 1. Create endpoint (initializes pjlib, modules, transport)
    eprintln!("[pjsip thread] creating endpoint");
    let endpoint = match PjEndpoint::create(config) {
        Ok(ep) => {
            eprintln!("[pjsip thread] endpoint created OK");
            ep
        }
        Err(e) => {
            error!("Failed to create pjsip endpoint: {e}");
            eprintln!("[pjsip thread] endpoint creation FAILED: {e}");
            return;
        }
    };

    info!("pjsip thread started");
    eprintln!("[pjsip thread] entering event loop");

    let mut shutting_down = false;

    // 2. Event loop: step pjsip + drain commands
    // Use zero timeout for handle_events to ensure the loop is non-blocking.
    // This allows Shutdown commands to be processed quickly.
    loop {
        // Process pjsip events (timers, retransmissions, incoming SIP).
        // Use 1ms timeout — short enough for responsive shutdown but not blocking.
        if let Err(e) = endpoint.handle_events(1) {
            warn!("pjsip handle_events error: {e}");
        }

        // Drain commands from Rust async layer
        loop {
            match cmd_rx.try_recv() {
                Ok(cmd) => {
                    handle_command(cmd, &endpoint, &mut shutting_down);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    shutting_down = true;
                    break;
                }
            }
        }

        if shutting_down {
            break;
        }
    }

    // 3. Explicit graceful shutdown: releases pool and calls pj_shutdown.
    info!("pjsip thread exiting");
    eprintln!("[pjsip thread] calling endpoint.shutdown()");
    endpoint.shutdown();
    eprintln!("[pjsip thread] shutdown complete");
}

// ---------------------------------------------------------------------------
// Command dispatch on the pjsip thread
// ---------------------------------------------------------------------------

fn handle_command(cmd: PjCommand, endpoint: &PjEndpoint, shutting_down: &mut bool) {
    match cmd {
        PjCommand::Shutdown => {
            *shutting_down = true;
        }

        PjCommand::CreateInvite {
            uri,
            from,
            sdp,
            event_tx,
            credential,
            headers,
        } => {
            if let Err(e) =
                create_outbound_invite(endpoint, &uri, &from, &sdp, event_tx, credential, headers)
            {
                warn!("CreateInvite failed: {e}");
            }
        }

        PjCommand::Respond {
            call_id,
            status,
            reason,
            sdp,
        } => {
            if let Err(e) =
                respond_to_invite(&call_id, status, &reason, sdp.as_deref(), endpoint)
            {
                warn!("Respond failed for {call_id}: {e}");
            }
        }

        PjCommand::Bye { call_id } => {
            if let Err(e) = send_bye(&call_id) {
                warn!("Bye failed for {call_id}: {e}");
            }
        }

        PjCommand::AnswerReInvite {
            call_id,
            status,
            sdp,
        } => {
            if let Err(e) = answer_reinvite(&call_id, status, sdp.as_deref(), endpoint) {
                warn!("AnswerReInvite failed for {call_id}: {e}");
            }
        }

        PjCommand::AcceptIncoming { call_id, event_tx } => {
            // Register the per-call event sender for an incoming call
            // that was already detected by on_rx_request.
            if let Ok(mut registry) = CALL_REGISTRY.lock() {
                if let Some(entry) = registry.get_mut(&call_id) {
                    entry.event_tx = event_tx;
                }
            }
        }

        PjCommand::SendReInvite { call_id, sdp } => {
            if let Err(e) = send_reinvite(&call_id, &sdp, endpoint) {
                warn!("SendReInvite failed for {call_id}: {e}");
            }
        }

        PjCommand::SendInfo {
            call_id,
            content_type,
            body,
        } => {
            if let Err(e) = send_info(&call_id, &content_type, &body) {
                warn!("SendInfo failed for {call_id}: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Outbound INVITE creation (UAC)
// ---------------------------------------------------------------------------

fn create_outbound_invite(
    endpoint: &PjEndpoint,
    uri: &str,
    from: &str,
    sdp_str: &str,
    event_tx: PjCallEventSender,
    credential: Option<PjCredential>,
    headers: Option<Vec<(String, String)>>,
) -> Result<()> {
    // Keep CStrings alive for the duration of the pjsip calls (PITFALL 1 from
    // research: CStrings must not be dropped before the pjsip call completes).
    let target = CString::new(uri)?;
    let from_cstr = CString::new(from)?;
    let to_cstr = CString::new(uri)?; // To = target for outbound
    let contact_cstr = CString::new(from)?;

    // Create pj_str_t from CStrings
    let target_pj = unsafe { pjsip_sys::pj_str(target.as_ptr() as *mut _) };
    let from_pj = unsafe { pjsip_sys::pj_str(from_cstr.as_ptr() as *mut _) };
    let to_pj = unsafe { pjsip_sys::pj_str(to_cstr.as_ptr() as *mut _) };
    let contact_pj = unsafe { pjsip_sys::pj_str(contact_cstr.as_ptr() as *mut _) };

    // 1. Create dialog (UAC)
    let mut dlg: *mut pjsip_sys::pjsip_dialog = ptr::null_mut();
    let status = unsafe {
        pjsip_sys::pjsip_dlg_create_uac(
            pjsip_sys::pjsip_ua_instance(),
            &from_pj,
            &contact_pj,
            &to_pj,
            &target_pj,
            &mut dlg,
        )
    };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_dlg_create_uac: {e}"))?;

    // 2. Set auth credentials if provided
    let _cred_cstrings: Option<(CString, CString, CString, CString)>;
    if let Some(ref cred) = credential {
        let mut cred_info: pjsip_sys::pjsip_cred_info = unsafe { std::mem::zeroed() };
        let realm_cstr = CString::new(cred.realm.as_str())?;
        let scheme_cstr = CString::new(cred.scheme.as_str())?;
        let user_cstr = CString::new(cred.username.as_str())?;
        let pass_cstr = CString::new(cred.password.as_str())?;

        cred_info.realm = unsafe { pjsip_sys::pj_str(realm_cstr.as_ptr() as *mut _) };
        cred_info.scheme = unsafe { pjsip_sys::pj_str(scheme_cstr.as_ptr() as *mut _) };
        cred_info.username = unsafe { pjsip_sys::pj_str(user_cstr.as_ptr() as *mut _) };
        cred_info.data = unsafe { pjsip_sys::pj_str(pass_cstr.as_ptr() as *mut _) };
        cred_info.data_type = 0; // PJSIP_CRED_DATA_PLAIN_PASSWD

        let status = unsafe {
            pjsip_sys::pjsip_auth_clt_set_credentials(&mut (*dlg).auth_sess, 1, &cred_info)
        };
        if status != 0 {
            warn!("failed to set auth credentials: {}", crate::PjStatus(status));
        }

        // Keep CStrings alive
        _cred_cstrings = Some((realm_cstr, scheme_cstr, user_cstr, pass_cstr));
    } else {
        _cred_cstrings = None;
    }

    // 3. Parse SDP offer
    let sdp_cstr = CString::new(sdp_str)?;
    let mut sdp_session: *mut pjsip_sys::pjmedia_sdp_session = ptr::null_mut();
    let status = unsafe {
        pjsip_sys::pjmedia_sdp_parse(
            endpoint.pool,
            sdp_cstr.as_ptr() as *mut _,
            sdp_str.len(),
            &mut sdp_session,
        )
    };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjmedia_sdp_parse: {e}"))?;

    // 4. Create INVITE session
    let mut inv: *mut pjsip_sys::pjsip_inv_session = ptr::null_mut();
    let status = unsafe { pjsip_sys::pjsip_inv_create_uac(dlg, sdp_session, 0, &mut inv) };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_create_uac: {e}"))?;

    // 5. Enable session timer on this session
    let mut timer_setting: pjsip_sys::pjsip_timer_setting = unsafe { std::mem::zeroed() };
    unsafe { pjsip_sys::pjsip_timer_setting_default(&mut timer_setting) };
    timer_setting.sess_expires = 1800; // 30 min
    timer_setting.min_se = 90;
    let status = unsafe { pjsip_sys::pjsip_timer_init_session(inv, &timer_setting) };
    if status != 0 {
        warn!("pjsip_timer_init_session: {}", crate::PjStatus(status));
    }

    // 6. Extract call ID from dialog (PITFALL 4: drop lock before any pjsip call)
    // dialog.call_id is *mut pjsip_cid_hdr — must dereference the raw pointer.
    let call_id = unsafe {
        let dlg_ref = &*dlg;
        let cid_hdr_ptr = dlg_ref.call_id;
        if !cid_hdr_ptr.is_null() && !(*cid_hdr_ptr).id.ptr.is_null() && (*cid_hdr_ptr).id.slen > 0 {
            let slice = std::slice::from_raw_parts(
                (*cid_hdr_ptr).id.ptr as *const u8,
                (*cid_hdr_ptr).id.slen as usize,
            );
            String::from_utf8_lossy(slice).into_owned()
        } else {
            uuid::Uuid::new_v4().to_string()
        }
    };

    // 7. Register in CALL_REGISTRY — drop lock before any pjsip call
    {
        let mut registry = CALL_REGISTRY
            .lock()
            .map_err(|e| anyhow::anyhow!("registry lock: {e}"))?;
        registry.insert(
            call_id.clone(),
            CallEntry {
                event_tx,
                inv_ptr: inv,
            },
        );
    } // lock dropped here

    // 8. Create and send the initial INVITE request
    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status = unsafe { pjsip_sys::pjsip_inv_invite(inv, &mut tdata) };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_invite: {e}"))?;

    // 9. Add custom headers if provided
    if let Some(hdrs) = headers {
        for (name, value) in &hdrs {
            let name_cstr = CString::new(name.as_str()).unwrap_or_default();
            let value_cstr = CString::new(value.as_str()).unwrap_or_default();
            let name_pj = unsafe { pjsip_sys::pj_str(name_cstr.as_ptr() as *mut _) };
            let value_pj = unsafe { pjsip_sys::pj_str(value_cstr.as_ptr() as *mut _) };
            unsafe {
                let hdr = pjsip_sys::pjsip_generic_string_hdr_create(
                    (*tdata).pool,
                    &name_pj,
                    &value_pj,
                );
                if !hdr.is_null() {
                    // pjsip_msg_add_hdr is an inline function — implement list
                    // insertion here: pj_list_insert_before(&msg->hdr, hdr)
                    // which appends hdr before the sentinel head (= appends to tail).
                    let hdr_base = hdr as *mut pjsip_sys::pjsip_hdr;
                    let msg_hdr = &mut (*(*tdata).msg).hdr as *mut pjsip_sys::pjsip_hdr;
                    // prev of msg_hdr sentinel points to current last element
                    let prev = (*msg_hdr).prev;
                    (*hdr_base).next = msg_hdr;
                    (*hdr_base).prev = prev;
                    (*prev).next = hdr_base;
                    (*msg_hdr).prev = hdr_base;
                }
            }
        }
    }

    // 10. Send the INVITE
    let status = unsafe { pjsip_sys::pjsip_inv_send_msg(inv, tdata) };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_send_msg: {e}"))?;

    info!(call_id = %call_id, target = %uri, "outbound INVITE sent");
    Ok(())
}

// ---------------------------------------------------------------------------
// Respond to incoming INVITE
// ---------------------------------------------------------------------------

/// Send a SIP response to an incoming INVITE session.
///
/// RESEARCH GAP FIX 2: When `sdp` is Some, parse it via `pjmedia_sdp_parse`
/// using the endpoint pool and pass the parsed session pointer to
/// `pjsip_inv_answer`. When `sdp` is None, pass `ptr::null_mut()`.
fn respond_to_invite(
    call_id: &str,
    status_code: u16,
    reason: &str,
    sdp: Option<&str>,
    endpoint: &PjEndpoint,
) -> Result<()> {
    // Lookup inv_ptr — drop lock before pjsip calls (PITFALL 4)
    let inv = {
        let registry = CALL_REGISTRY.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        registry
            .get(call_id)
            .map(|e| e.inv_ptr)
            .ok_or_else(|| anyhow::anyhow!("call {call_id} not found"))?
    }; // lock dropped here

    // RESEARCH GAP FIX 2: Parse SDP via pjmedia_sdp_parse before pjsip_inv_answer.
    let local_sdp: *mut pjsip_sys::pjmedia_sdp_session = if let Some(sdp_str) = sdp {
        let sdp_cstr = CString::new(sdp_str)?;
        let mut sdp_session: *mut pjsip_sys::pjmedia_sdp_session = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjmedia_sdp_parse(
                endpoint.pool,
                sdp_cstr.as_ptr() as *mut _,
                sdp_str.len(),
                &mut sdp_session,
            )
        };
        crate::check_status(status)
            .map_err(|e| anyhow::anyhow!("pjmedia_sdp_parse in respond_to_invite: {e}"))?;
        sdp_session
    } else {
        ptr::null_mut()
    };

    let reason_cstr = CString::new(reason).unwrap_or_default();
    let reason_pj = unsafe { pjsip_sys::pj_str(reason_cstr.as_ptr() as *mut _) };

    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status = unsafe {
        pjsip_sys::pjsip_inv_answer(
            inv,
            status_code as i32,
            &reason_pj,
            local_sdp,
            &mut tdata,
        )
    };
    crate::check_status(status)
        .map_err(|e| anyhow::anyhow!("pjsip_inv_answer: {e}"))?;

    let status = unsafe { pjsip_sys::pjsip_inv_send_msg(inv, tdata) };
    crate::check_status(status)
        .map_err(|e| anyhow::anyhow!("pjsip_inv_send_msg: {e}"))?;

    debug!(call_id = %call_id, status = %status_code, "responded to INVITE");
    Ok(())
}

// ---------------------------------------------------------------------------
// Answer a pending re-INVITE
// ---------------------------------------------------------------------------

fn answer_reinvite(
    call_id: &str,
    status_code: u16,
    sdp: Option<&str>,
    endpoint: &PjEndpoint,
) -> Result<()> {
    // Lookup inv_ptr — drop lock before pjsip calls (PITFALL 4)
    let inv = {
        let registry = CALL_REGISTRY.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        registry
            .get(call_id)
            .map(|e| e.inv_ptr)
            .ok_or_else(|| anyhow::anyhow!("call {call_id} not found"))?
    }; // lock dropped here

    // Parse SDP if provided
    let local_sdp: *mut pjsip_sys::pjmedia_sdp_session = if let Some(sdp_str) = sdp {
        let sdp_cstr = CString::new(sdp_str)?;
        let mut sdp_session: *mut pjsip_sys::pjmedia_sdp_session = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjmedia_sdp_parse(
                endpoint.pool,
                sdp_cstr.as_ptr() as *mut _,
                sdp_str.len(),
                &mut sdp_session,
            )
        };
        crate::check_status(status)
            .map_err(|e| anyhow::anyhow!("pjmedia_sdp_parse in answer_reinvite: {e}"))?;
        sdp_session
    } else {
        ptr::null_mut() // Use current negotiated SDP
    };

    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status = unsafe {
        pjsip_sys::pjsip_inv_answer(
            inv,
            status_code as i32,
            ptr::null(), // default reason
            local_sdp,
            &mut tdata,
        )
    };
    crate::check_status(status)
        .map_err(|e| anyhow::anyhow!("pjsip_inv_answer (re-INVITE): {e}"))?;

    if !tdata.is_null() {
        let status = unsafe { pjsip_sys::pjsip_inv_send_msg(inv, tdata) };
        crate::check_status(status)
            .map_err(|e| anyhow::anyhow!("pjsip_inv_send_msg (re-INVITE): {e}"))?;
    }

    debug!(call_id = %call_id, status = %status_code, "answered pending re-INVITE");
    Ok(())
}

// ---------------------------------------------------------------------------
// Send BYE
// ---------------------------------------------------------------------------

fn send_bye(call_id: &str) -> Result<()> {
    // Lookup inv_ptr — drop lock before pjsip calls
    let inv = {
        let registry = CALL_REGISTRY.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        registry
            .get(call_id)
            .map(|e| e.inv_ptr)
            .ok_or_else(|| anyhow::anyhow!("call {call_id} not found"))?
    }; // lock dropped here

    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status =
        unsafe { pjsip_sys::pjsip_inv_end_session(inv, 200, ptr::null(), &mut tdata) };
    crate::check_status(status)
        .map_err(|e| anyhow::anyhow!("pjsip_inv_end_session: {e}"))?;

    if !tdata.is_null() {
        let status = unsafe { pjsip_sys::pjsip_inv_send_msg(inv, tdata) };
        crate::check_status(status)
            .map_err(|e| anyhow::anyhow!("pjsip_inv_send_msg (BYE): {e}"))?;
    }

    // Remove from registry
    if let Ok(mut registry) = CALL_REGISTRY.lock() {
        registry.remove(call_id);
    }

    debug!(call_id = %call_id, "BYE sent");
    Ok(())
}

// ---------------------------------------------------------------------------
// pjsip callbacks (called on the pjsip OS thread)
// ---------------------------------------------------------------------------

/// Called by pjsip when an INVITE session's state changes.
///
/// This is the main event delivery point. We look up the per-call
/// event sender from the call registry and deliver the appropriate event.
pub(crate) extern "C" fn on_inv_state_changed(
    inv: *mut pjsip_sys::pjsip_inv_session,
    _event: *mut pjsip_sys::pjsip_event,
) {
    if inv.is_null() {
        return;
    }

    let inv_ref = unsafe { &*inv };
    let state = inv_ref.state;
    let call_id = extract_call_id(inv);

    let Some(call_id) = call_id else {
        warn!("on_inv_state_changed: no call_id");
        return;
    };

    let event_tx = {
        let registry = match CALL_REGISTRY.lock() {
            Ok(r) => r,
            Err(_) => return,
        };
        registry.get(&call_id).map(|e| e.event_tx.clone())
    };

    let Some(tx) = event_tx else {
        debug!(call_id = %call_id, "no event_tx registered for call");
        return;
    };

    // Map pjsip_inv_state to PjCallEvent
    let pj_event = match state {
        pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_CALLING => Some(PjCallEvent::Trying),

        pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_EARLY => {
            // PITFALL 5: For EARLY state, do NOT use pjmedia_sdp_neg_get_active_remote
            // (negotiation not complete yet). Read SDP from response body instead.
            let (status_code, sdp) = extract_early_response_info(inv);
            if let Some(sdp_body) = sdp {
                Some(PjCallEvent::EarlyMedia {
                    status: status_code,
                    sdp: sdp_body,
                })
            } else {
                Some(PjCallEvent::Ringing { status: status_code })
            }
        }

        pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_CONFIRMED => {
            // RESEARCH GAP FIX 1: include call_id in Confirmed so BYE can be
            // routed post-connect without additional state.
            let sdp = extract_confirmed_sdp(inv);
            Some(PjCallEvent::Confirmed {
                call_id: call_id.clone(),
                sdp,
            })
        }

        pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_DISCONNECTED => {
            let cause = inv_ref.cause as u16;
            let reason = if !inv_ref.cause_text.ptr.is_null() && inv_ref.cause_text.slen > 0 {
                let slice = unsafe {
                    std::slice::from_raw_parts(
                        inv_ref.cause_text.ptr as *const u8,
                        inv_ref.cause_text.slen as usize,
                    )
                };
                String::from_utf8_lossy(slice).into_owned()
            } else {
                format!("SIP {}", cause)
            };

            // Clean up registry entry
            if let Ok(mut registry) = CALL_REGISTRY.lock() {
                registry.remove(&call_id);
            }

            Some(PjCallEvent::Terminated { code: cause, reason })
        }

        _ => None,
    };

    if let Some(ev) = pj_event {
        if let Err(e) = tx.send(ev) {
            error!(call_id = %call_id, "failed to send PjCallEvent: {e}");
        }
    }
}

/// Called by pjsip when a re-INVITE is received mid-call.
///
/// Returning `PJ_SUCCESS` (0) means we handle the answer manually via
/// `pjsip_inv_answer` + `pjsip_inv_send_msg`. This accepts the re-INVITE
/// with the current active local SDP (hold/resume/codec-change) and prevents
/// pjsip from auto-responding 501 Not Implemented.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe extern "C" fn on_rx_reinvite(
    inv: *mut pjsip_sys::pjsip_inv_session,
    _offer: *const pjsip_sys::pjmedia_sdp_session,
    _rdata: *mut pjsip_sys::pjsip_rx_data,
) -> pjsip_sys::pj_status_t {
    if inv.is_null() {
        return 1; // non-success fallback
    }

    let call_id = extract_call_id(inv);

    // Fire ReInvite event so the bridge loop can optionally relay to LiveKit.
    if let Some(ref id) = call_id {
        let event_tx = {
            if let Ok(registry) = CALL_REGISTRY.lock() {
                registry.get(id).map(|e| e.event_tx.clone())
            } else {
                None
            }
        };
        if let Some(tx) = event_tx {
            // Extract the remote (offer) SDP string for the event.
            let offer_sdp = if !_offer.is_null() {
                let mut buf = vec![0u8; 4096];
                let len = pjsip_sys::pjmedia_sdp_print(
                    _offer,
                    buf.as_mut_ptr() as *mut libc::c_char,
                    buf.len(),
                );
                if len > 0 {
                    buf.truncate(len as usize);
                    String::from_utf8_lossy(&buf).into_owned()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            let _ = tx.send(PjCallEvent::ReInvite { sdp: offer_sdp });
        }
    }

    // Return PJ_SUCCESS — PJSIP keeps the re-INVITE pending.
    // The bridge loop will answer via PjCommand::AnswerReInvite once
    // the caller responds.
    0
}

/// Called when a new INVITE session is created (for incoming calls).
pub(crate) extern "C" fn on_inv_new_session(
    inv: *mut pjsip_sys::pjsip_inv_session,
    _event: *mut pjsip_sys::pjsip_event,
) {
    if inv.is_null() {
        return;
    }
    // For incoming calls, create a placeholder registry entry.
    // The actual event_tx is set when the Rust layer calls AcceptIncoming.
    let call_id = extract_call_id(inv).unwrap_or_default();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    if let Ok(mut registry) = CALL_REGISTRY.lock() {
        registry.insert(
            call_id.clone(),
            CallEntry {
                event_tx: tx,
                inv_ptr: inv,
            },
        );
    }
    debug!(call_id = %call_id, "new incoming INVITE session");
}

// ---------------------------------------------------------------------------
// Helper: extract call ID from inv_session -> dialog -> call_id
// ---------------------------------------------------------------------------

fn extract_call_id(inv: *mut pjsip_sys::pjsip_inv_session) -> Option<String> {
    unsafe {
        let inv_ref = &*inv;
        let dlg = inv_ref.dlg;
        if dlg.is_null() {
            return None;
        }
        let dlg_ref = &*dlg;
        // dialog.call_id is *mut pjsip_cid_hdr — must dereference the pointer.
        let cid_hdr_ptr = dlg_ref.call_id;
        if cid_hdr_ptr.is_null() {
            return None;
        }
        let id = &(*cid_hdr_ptr).id;
        if id.ptr.is_null() || id.slen <= 0 {
            return None;
        }
        let slice = std::slice::from_raw_parts(id.ptr as *const u8, id.slen as usize);
        Some(String::from_utf8_lossy(slice).into_owned())
    }
}

/// Extract status code and SDP from early (provisional) response body.
///
/// PITFALL 5: For EARLY state, the SDP negotiation is not complete so
/// `pjmedia_sdp_neg_get_active_remote` must NOT be used. Instead, read
/// the SDP directly from `pjsip_event.body.tsx_state.src.rdata`.
fn extract_early_response_info(inv: *mut pjsip_sys::pjsip_inv_session) -> (u16, Option<String>) {
    let inv_ref = unsafe { &*inv };
    // Use cause as the status code for provisional responses
    let status_code = inv_ref.cause as u16;

    // For early responses, try reading SDP from the last response message body.
    // The inv->last_answer is the most recent response received.
    let sdp = unsafe {
        let last_answer = inv_ref.last_answer;
        if !last_answer.is_null() {
            let msg = (*last_answer).msg;
            if !msg.is_null() {
                let msg_ref = &*msg;
                if !msg_ref.body.is_null() {
                    let body = &*msg_ref.body;
                    if body.len > 0 && !body.data.is_null() {
                        let slice = std::slice::from_raw_parts(
                            body.data as *const u8,
                            body.len as usize,
                        );
                        Some(String::from_utf8_lossy(slice).into_owned())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    (status_code, sdp)
}

/// Extract SDP from a confirmed session using the SDP negotiator.
///
/// For CONFIRMED state, use `pjmedia_sdp_neg_get_active_remote` + `pjmedia_sdp_print`.
fn extract_confirmed_sdp(inv: *mut pjsip_sys::pjsip_inv_session) -> Option<String> {
    let inv_ref = unsafe { &*inv };
    let mut remote_sdp: *const pjsip_sys::pjmedia_sdp_session = ptr::null();
    let neg_status = unsafe {
        pjsip_sys::pjmedia_sdp_neg_get_active_remote(inv_ref.neg, &mut remote_sdp)
    };
    if neg_status != 0 || remote_sdp.is_null() {
        return None;
    }

    // Print SDP to string
    let mut buf = vec![0u8; 4096];
    let len = unsafe {
        pjsip_sys::pjmedia_sdp_print(
            remote_sdp,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
        )
    };
    if len > 0 {
        buf.truncate(len as usize);
        Some(String::from_utf8_lossy(&buf).into_owned())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Send re-INVITE within an active dialog
// ---------------------------------------------------------------------------

fn send_reinvite(call_id: &str, sdp_str: &str, endpoint: &PjEndpoint) -> Result<()> {
    // Lookup inv_ptr — drop lock before pjsip calls
    let inv = {
        let registry = CALL_REGISTRY.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        registry
            .get(call_id)
            .map(|e| e.inv_ptr)
            .ok_or_else(|| anyhow::anyhow!("call {call_id} not found"))?
    };

    // Parse SDP offer
    let sdp_cstr = CString::new(sdp_str)?;
    let mut sdp_session: *mut pjsip_sys::pjmedia_sdp_session = ptr::null_mut();
    let status = unsafe {
        pjsip_sys::pjmedia_sdp_parse(
            endpoint.pool,
            sdp_cstr.as_ptr() as *mut _,
            sdp_str.len(),
            &mut sdp_session,
        )
    };
    crate::check_status(status)
        .map_err(|e| anyhow::anyhow!("pjmedia_sdp_parse in send_reinvite: {e}"))?;

    // Create re-INVITE request
    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status = unsafe {
        pjsip_sys::pjsip_inv_reinvite(
            inv,
            ptr::null(),  // no new contact
            sdp_session,  // new SDP offer
            &mut tdata,
        )
    };
    crate::check_status(status)
        .map_err(|e| anyhow::anyhow!("pjsip_inv_reinvite: {e}"))?;

    // Send it
    if !tdata.is_null() {
        let status = unsafe { pjsip_sys::pjsip_inv_send_msg(inv, tdata) };
        crate::check_status(status)
            .map_err(|e| anyhow::anyhow!("pjsip_inv_send_msg (re-INVITE): {e}"))?;
    }

    debug!(call_id = %call_id, "re-INVITE sent to gateway");
    Ok(())
}

// ---------------------------------------------------------------------------
// Send INFO request within an active dialog
// ---------------------------------------------------------------------------

fn send_info(call_id: &str, content_type: &str, body: &str) -> Result<()> {
    let inv = {
        let registry = CALL_REGISTRY.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        registry
            .get(call_id)
            .map(|e| e.inv_ptr)
            .ok_or_else(|| anyhow::anyhow!("call {call_id} not found"))?
    };

    let inv_ref = unsafe { &*inv };
    let dlg = inv_ref.dlg;
    if dlg.is_null() {
        return Err(anyhow::anyhow!("dialog is null for {call_id}"));
    }

    // Create INFO method (OTHER method type with name "INFO")
    let method_name = CString::new("INFO")?;
    let mut method: pjsip_sys::pjsip_method = unsafe { std::mem::zeroed() };
    method.id = pjsip_sys::pjsip_method_e_PJSIP_OTHER_METHOD;
    method.name = unsafe { pjsip_sys::pj_str(method_name.as_ptr() as *mut _) };

    // Create the request within the dialog
    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status = unsafe {
        pjsip_sys::pjsip_dlg_create_request(dlg, &method, -1, &mut tdata)
    };
    crate::check_status(status)
        .map_err(|e| anyhow::anyhow!("pjsip_dlg_create_request (INFO): {e}"))?;

    // Set body if provided.
    // Keep CStrings alive until after pjsip_msg_body_create copies the data.
    if !body.is_empty() && !tdata.is_null() {
        let body_cstr = CString::new(body)?;
        let body_len = body.len();

        // Parse content type into type/subtype
        let ct_parts: Vec<&str> = content_type.splitn(2, '/').collect();
        let (type_part, subtype_part) = if ct_parts.len() == 2 {
            (ct_parts[0], ct_parts[1])
        } else {
            ("application", "octet-stream")
        };

        let type_cstr = CString::new(type_part).unwrap_or_default();
        let subtype_cstr = CString::new(subtype_part).unwrap_or_default();

        unsafe {
            let pool = (*tdata).pool;
            let type_pj = pjsip_sys::pj_str(type_cstr.as_ptr() as *mut _);
            let subtype_pj = pjsip_sys::pj_str(subtype_cstr.as_ptr() as *mut _);

            let text_pj = pjsip_sys::pj_str_t {
                ptr: body_cstr.as_ptr() as *mut _,
                slen: body_len as i64,
            };

            // pjsip_msg_body_create clones the text into the pool.
            let msg_body = pjsip_sys::pjsip_msg_body_create(
                pool,
                &type_pj,
                &subtype_pj,
                &text_pj,
            );
            if !msg_body.is_null() {
                (*(*tdata).msg).body = msg_body;
            }
        }
    }

    // Send the INFO request
    let status = unsafe {
        pjsip_sys::pjsip_dlg_send_request(dlg, tdata, -1, ptr::null_mut())
    };
    crate::check_status(status)
        .map_err(|e| anyhow::anyhow!("pjsip_dlg_send_request (INFO): {e}"))?;

    debug!(call_id = %call_id, "INFO sent to gateway");
    Ok(())
}

// ---------------------------------------------------------------------------
// on_tsx_state_changed callback — detects incoming INFO requests
// ---------------------------------------------------------------------------

/// Called when a transaction within an INVITE session changes state.
///
/// We use this to detect incoming INFO requests (for DTMF relay).
/// When an incoming INFO arrives (tsx state = Trying, role = UAS, method = INFO),
/// extract Content-Type and body, then fire PjCallEvent::Info.
pub(crate) extern "C" fn on_tsx_state_changed(
    inv: *mut pjsip_sys::pjsip_inv_session,
    tsx: *mut pjsip_sys::pjsip_transaction,
    event: *mut pjsip_sys::pjsip_event,
) {
    if inv.is_null() || tsx.is_null() || event.is_null() {
        return;
    }

    unsafe {
        let tsx_ref = &*tsx;

        // Only interested in UAS (incoming) transactions in the Trying state.
        // PJSIP_ROLE_UAS = 1, PJSIP_TSX_STATE_TRYING = 2
        if tsx_ref.role != pjsip_sys::pjsip_role_e_PJSIP_ROLE_UAS
            || tsx_ref.state != pjsip_sys::pjsip_tsx_state_e_PJSIP_TSX_STATE_TRYING
        {
            return;
        }

        // Check method is INFO
        let method_name = &tsx_ref.method.name;
        if method_name.ptr.is_null() || method_name.slen <= 0 {
            return;
        }
        let method_slice = std::slice::from_raw_parts(
            method_name.ptr as *const u8,
            method_name.slen as usize,
        );
        if method_slice != b"INFO" {
            return;
        }

        // Extract Content-Type and body from the incoming request.
        // event.body.tsx_state.src.rdata has the incoming message.
        let rdata = (*event).body.tsx_state.src.rdata;
        if rdata.is_null() {
            return;
        }
        let msg = (*rdata).msg_info.msg;
        if msg.is_null() {
            return;
        }
        let msg_ref = &*msg;

        // Extract Content-Type header from msg body
        let content_type = {
            let body_ptr = msg_ref.body;
            if body_ptr.is_null() {
                String::new()
            } else {
                let ct = &(*body_ptr).content_type;
                let type_str = if !ct.type_.ptr.is_null() && ct.type_.slen > 0 {
                    let s = std::slice::from_raw_parts(
                        ct.type_.ptr as *const u8,
                        ct.type_.slen as usize,
                    );
                    String::from_utf8_lossy(s).into_owned()
                } else {
                    String::new()
                };
                let subtype_str = if !ct.subtype.ptr.is_null() && ct.subtype.slen > 0 {
                    let s = std::slice::from_raw_parts(
                        ct.subtype.ptr as *const u8,
                        ct.subtype.slen as usize,
                    );
                    String::from_utf8_lossy(s).into_owned()
                } else {
                    String::new()
                };
                if type_str.is_empty() {
                    String::new()
                } else {
                    format!("{}/{}", type_str, subtype_str)
                }
            }
        };

        // Extract body text
        let body = {
            let body_ptr = msg_ref.body;
            if body_ptr.is_null() {
                String::new()
            } else {
                let body_ref = &*body_ptr;
                if body_ref.data.is_null() || body_ref.len == 0 {
                    String::new()
                } else {
                    let data = std::slice::from_raw_parts(
                        body_ref.data as *const u8,
                        body_ref.len as usize,
                    );
                    String::from_utf8_lossy(data).into_owned()
                }
            }
        };

        // Fire the Info event
        let call_id = extract_call_id(inv);
        if let Some(ref id) = call_id {
            let event_tx = {
                if let Ok(registry) = CALL_REGISTRY.lock() {
                    registry.get(id).map(|e| e.event_tx.clone())
                } else {
                    None
                }
            };
            if let Some(tx) = event_tx {
                let _ = tx.send(PjCallEvent::Info {
                    content_type,
                    body,
                });
            }
        }

        // Auto-respond 200 OK to the INFO request.
        let inv_ref = &*inv;
        let dlg = inv_ref.dlg;
        if !dlg.is_null() {
            let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
            let status = pjsip_sys::pjsip_dlg_create_response(
                dlg,
                rdata,
                200,
                ptr::null(),
                &mut tdata,
            );
            if status == 0 && !tdata.is_null() {
                // Use the tsx from the event body for sending the response
                let tsx_ptr = (*event).body.tsx_state.tsx;
                if !tsx_ptr.is_null() {
                    pjsip_sys::pjsip_dlg_send_response(dlg, tsx_ptr, tdata);
                }
            }
        }
    }
}
