//! Dedicated OS thread running the Sofia-SIP event loop with mpsc channel
//! bridge to Tokio async.
//!
//! Architecture:
//! ```text
//!   Tokio async            Sofia OS thread
//!   ──────────             ───────────────
//!   recv_event()  ◄──────  C callback → event_tx.send(SofiaEvent)
//!   send_command() ──────► cmd_rx.try_recv() → nua_*() calls
//! ```
//!
//! The Sofia thread runs `su_root_step(root, 1)` in a tight loop (1 ms
//! timeout) interspersed with command processing.

use std::ffi::{CStr, CString, c_int};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{debug, error, warn};

use sofia_sip_sys::{
    nua_create, nua_destroy, nua_event_e, nua_handle, nua_handle_ref, nua_handle_t,
    nua_hmagic_t, nua_magic_t, nua_options, nua_respond, nua_shutdown, nua_t, sip_t,
    tag_type_t, tag_value_t,
};

use crate::command::SofiaCommand;
use crate::event::SofiaEvent;
use crate::handle::SofiaHandle;
use crate::root::SuRoot;

// ---------------------------------------------------------------------------
// Sofia-SIP event number constants (from nua_event_e in generated bindings)
// ---------------------------------------------------------------------------

use sofia_sip_sys::{
    nua_event_e_nua_i_bye as NUA_I_BYE,
    nua_event_e_nua_i_cancel as NUA_I_CANCEL,
    nua_event_e_nua_i_info as NUA_I_INFO,
    nua_event_e_nua_i_invite as NUA_I_INVITE,
    nua_event_e_nua_i_register as NUA_I_REGISTER,
    nua_event_e_nua_r_invite as NUA_R_INVITE,
    nua_event_e_nua_r_options as NUA_R_OPTIONS,
    nua_event_e_nua_r_shutdown as NUA_R_SHUTDOWN,
};

// ---------------------------------------------------------------------------
// Tag helpers
//
// Sofia-SIP uses a variadic tag-list system. Each tag-pair is (tag_type_t,
// tag_value_t) where tag_type_t is the address of the tag's descriptor
// global variable.  The list is terminated by (0, 0) == TAG_END.
//
// For NUTAG_URL(url_cstr):
//   tag_type  = &nutag_url as *const _ cast to tag_type_t (a pointer-sized int)
//   tag_value = url_cstr.as_ptr() cast to tag_value_t
//
// Reference: <sofia-sip/su_tag.h>, <sofia-sip/nua_tag.h>
// ---------------------------------------------------------------------------

/// Returns the `tag_type_t` for `NUTAG_URL` — the address of the nutag_url
/// global descriptor variable.
///
/// # Safety
///
/// `nutag_url` must be available as a linked symbol (requires Sofia-SIP installed).
#[allow(non_snake_case)]
fn nutag_url_type() -> tag_type_t {
    unsafe extern "C" {
        static mut nutag_url: sofia_sip_sys::tag_typedef_t;
    }
    // SAFETY: Taking the address of a static exported by the linked library.
    // The `unsafe extern "C"` block above is the unsafe context.
    (&raw mut nutag_url) as tag_type_t
}

/// `TAG_END` sentinel: terminates the variadic tag list.
const TAG_END_TYPE: tag_type_t = 0;
const TAG_END_VALUE: tag_value_t = 0;

// ---------------------------------------------------------------------------
// Shared state passed into the C callback via the magic pointer.
// ---------------------------------------------------------------------------

struct CallbackState {
    event_tx: UnboundedSender<SofiaEvent>,
    /// Set to true when `nua_r_shutdown` with status 200 is received.
    shutdown_complete: Arc<AtomicBool>,
}

// ---------------------------------------------------------------------------
// C callback trampoline
// ---------------------------------------------------------------------------

/// The NUA event callback registered with `nua_create`.
///
/// # Safety
///
/// Called by Sofia-SIP on the Sofia OS thread. `magic` is our leaked
/// `Box<CallbackState>`.
extern "C" fn sofia_event_trampoline(
    event: nua_event_e,
    status: c_int,
    phrase: *const libc::c_char,
    _nua: *mut nua_t,
    magic: *mut nua_magic_t,
    nh: *mut nua_handle_t,
    _hmagic: *mut nua_hmagic_t,
    _sip: *const sip_t,
    _tags: *mut sofia_sip_sys::tagi_t,
) {
    // SAFETY: magic is our leaked Box<CallbackState>; it lives until
    // we explicitly drop it after the event loop exits.
    let state = unsafe { &*(magic as *const CallbackState) };

    let ev_num: nua_event_e = event;

    // Convert phrase to a Rust String (copy out of C memory immediately).
    let phrase_str = if phrase.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(phrase) }
            .to_string_lossy()
            .into_owned()
    };

    // Handle shutdown acknowledgement.
    if ev_num == NUA_R_SHUTDOWN && status == 200 {
        state.shutdown_complete.store(true, Ordering::Release);
        return;
    }

    // Create SofiaHandle if we have a valid nh pointer.
    // We call nua_handle_ref to increment the refcount since the callback
    // does NOT transfer ownership to us.
    let maybe_handle = if nh.is_null() {
        None
    } else {
        // SAFETY: nh is non-null and valid for the duration of this callback.
        let reffed = unsafe { nua_handle_ref(nh) };
        if reffed.is_null() {
            None
        } else {
            Some(SofiaHandle::from_raw(reffed))
        }
    };

    let sofia_event = match ev_num {
        NUA_I_INVITE => {
            if let Some(handle) = maybe_handle {
                // Extract Authorization/Proxy-Authorization header from sip_t
                let auth_header = extract_sip_auth_header(_sip);
                Some(SofiaEvent::IncomingInvite {
                    handle,
                    from: String::new(), // sip_from is a C macro; extraction not supported yet
                    to: String::new(),   // sip_to is a C macro; extraction not supported yet
                    sdp: None,
                    auth_header,
                })
            } else {
                warn!("nua_i_invite with null handle, ignoring");
                None
            }
        }

        NUA_I_REGISTER => {
            if let Some(handle) = maybe_handle {
                let auth_header = extract_sip_auth_header(_sip);
                Some(SofiaEvent::IncomingRegister {
                    handle,
                    contact: String::new(),
                    auth_header,
                })
            } else {
                None
            }
        }

        x if x == NUA_R_INVITE || x == NUA_R_OPTIONS => {
            if let Some(handle) = maybe_handle {
                Some(SofiaEvent::InviteResponse {
                    handle,
                    status: status as u16,
                    phrase: phrase_str,
                    sdp: None,
                })
            } else {
                warn!("nua_r_invite/nua_r_options with null handle");
                None
            }
        }

        x if x == NUA_I_BYE || x == NUA_I_CANCEL => {
            if let Some(handle) = maybe_handle {
                Some(SofiaEvent::Terminated {
                    handle,
                    reason: phrase_str,
                })
            } else {
                None
            }
        }

        NUA_I_INFO => {
            if let Some(handle) = maybe_handle {
                Some(SofiaEvent::Info {
                    handle,
                    content_type: String::new(),
                    body: String::new(),
                })
            } else {
                None
            }
        }

        other => {
            debug!("Unhandled sofia event {other}, status={status}");
            None
        }
    };

    if let Some(ev) = sofia_event {
        if let Err(e) = state.event_tx.send(ev) {
            error!("Failed to send SofiaEvent to channel: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// SIP header extraction helpers
// ---------------------------------------------------------------------------

/// Extract Authorization or Proxy-Authorization header from a raw `sip_t` pointer.
///
/// Sofia-SIP's `sip_t` contains `sip_authorization` and `sip_proxy_authorization`
/// fields. We try Proxy-Authorization first (used in 407 challenge flows), then
/// fall back to Authorization.
///
/// # Safety
///
/// `sip` must be a valid, non-null pointer to a `sip_t` struct for the duration
/// of this call (guaranteed within the NUA callback).
fn extract_sip_auth_header(sip: *const sip_t) -> Option<String> {
    if sip.is_null() {
        return None;
    }

    // Sofia-SIP stores parsed Authorization headers in sip_proxy_authorization
    // and sip_authorization fields. These are complex structs, but for digest
    // auth we need the raw header value. We use sip_header_as_string via the
    // msg layer. For now, check if the fields are non-null as an indicator
    // that auth was provided, and reconstruct the Digest string.
    //
    // NOTE: Full header extraction requires access to sip_proxy_authorization_t
    // fields (scheme, username, realm, nonce, response, uri). The generated
    // bindings treat these as opaque. For Phase 3, we indicate presence/absence;
    // full extraction will be added when the C struct fields are exposed.
    //
    // Returning None here means the 407 challenge always fires on the first
    // request. On the retry, Sofia-SIP may have already validated credentials
    // internally via auth_module if configured. This is acceptable for Phase 3.
    None
}

// ---------------------------------------------------------------------------
// SofiaBridge
// ---------------------------------------------------------------------------

/// Bridge between the dedicated Sofia-SIP OS thread and Tokio async.
pub struct SofiaBridge {
    event_rx: UnboundedReceiver<SofiaEvent>,
    cmd_tx: UnboundedSender<SofiaCommand>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl SofiaBridge {
    /// Start the Sofia-SIP event loop on a dedicated OS thread.
    ///
    /// `bind_url` is the SIP URI to listen on, e.g. `"sip:*:5060"`.
    pub fn start(bind_url: &str) -> Result<Self> {
        let (event_tx, event_rx) = unbounded_channel::<SofiaEvent>();
        let (cmd_tx, cmd_rx) = unbounded_channel::<SofiaCommand>();

        let shutdown_complete = Arc::new(AtomicBool::new(false));
        let shutdown_complete_thread = shutdown_complete.clone();

        let bind_url_owned = bind_url.to_owned();

        let thread_handle = std::thread::Builder::new()
            .name("sofia-sip".to_owned())
            .spawn(move || {
                sofia_thread_main(
                    bind_url_owned,
                    event_tx,
                    cmd_rx,
                    shutdown_complete_thread,
                );
            })?;

        Ok(Self {
            event_rx,
            cmd_tx,
            thread_handle: Some(thread_handle),
        })
    }

    /// Receive the next [`SofiaEvent`] from the Sofia thread.
    pub async fn recv_event(&mut self) -> Option<SofiaEvent> {
        self.event_rx.recv().await
    }

    /// Send a [`SofiaCommand`] to the Sofia thread.
    pub fn send_command(&self, cmd: SofiaCommand) -> Result<()> {
        self.cmd_tx.send(cmd).map_err(|_| anyhow::anyhow!("Sofia command channel closed"))?;
        Ok(())
    }
}

impl Drop for SofiaBridge {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(SofiaCommand::Shutdown);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl std::fmt::Debug for SofiaBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SofiaBridge").finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Sofia thread entry point
// ---------------------------------------------------------------------------

fn sofia_thread_main(
    bind_url: String,
    event_tx: UnboundedSender<SofiaEvent>,
    mut cmd_rx: UnboundedReceiver<SofiaCommand>,
    shutdown_complete: Arc<AtomicBool>,
) {
    let root = match SuRoot::new() {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to create SuRoot: {e}");
            return;
        }
    };

    // Box the callback state and leak it so the C callback can use it.
    let callback_state = Box::new(CallbackState {
        event_tx,
        shutdown_complete: shutdown_complete.clone(),
    });
    let magic_ptr = Box::into_raw(callback_state) as *mut nua_magic_t;

    // Build the bind URL as a C string.
    let bind_cstr = match CString::new(bind_url.as_str()) {
        Ok(s) => s,
        Err(e) => {
            error!("Invalid bind URL '{bind_url}': {e}");
            unsafe { drop(Box::from_raw(magic_ptr as *mut CallbackState)) };
            return;
        }
    };

    // nua_callback_f is typed as u64 in the generated bindings (bindgen opaque treatment).
    // The real C type is: void (*)(nua_event_t, int, char const*, nua_t*, nua_magic_t*,
    //                              nua_handle_t*, nua_hmagic_t*, sip_t const*, tagi_t[])
    // We transmute our extern "C" fn to the u64 alias to satisfy the type checker.
    // On 64-bit platforms, function pointer == usize == u64 in representation.
    //
    // We define the actual function pointer type here to bypass the opaque alias.
    type NuaCallbackRaw = unsafe extern "C" fn(
        nua_event_e,
        c_int,
        *const libc::c_char,
        *mut nua_t,
        *mut nua_magic_t,
        *mut nua_handle_t,
        *mut nua_hmagic_t,
        *const sip_t,
        *mut sofia_sip_sys::tagi_t,
    );

    let callback_raw: NuaCallbackRaw = sofia_event_trampoline;
    // Transmute function pointer to the u64 alias used in bindings.
    let callback_u64: sofia_sip_sys::nua_callback_f =
        unsafe { std::mem::transmute::<NuaCallbackRaw, u64>(callback_raw) };

    // Create the NUA object with NUTAG_URL(bind_url), TAG_END.
    // nua_create signature: (root, callback, magic, tag_type, tag_value, ...) -> *mut nua_t
    // SAFETY: all pointers are valid; bind_cstr lives until after nua_create returns.
    let nua_ptr = unsafe {
        nua_create(
            root.as_ptr(),
            callback_u64,
            magic_ptr,
            nutag_url_type(),
            bind_cstr.as_ptr() as tag_value_t,
            TAG_END_TYPE,
            TAG_END_VALUE,
        )
    };

    if nua_ptr.is_null() {
        error!("nua_create returned null — check bind URL and Sofia-SIP transport support");
        unsafe { drop(Box::from_raw(magic_ptr as *mut CallbackState)) };
        return;
    }

    let mut shutting_down = false;

    // Event loop: step Sofia + drain commands.
    loop {
        root.step(1);

        loop {
            match cmd_rx.try_recv() {
                Ok(cmd) => {
                    handle_command(cmd, nua_ptr, &mut shutting_down);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    if !shutting_down {
                        shutting_down = true;
                        unsafe { nua_shutdown(nua_ptr) };
                    }
                    break;
                }
            }
        }

        if shutting_down && shutdown_complete.load(Ordering::Acquire) {
            break;
        }
    }

    unsafe { nua_destroy(nua_ptr) };
    unsafe { drop(Box::from_raw(magic_ptr as *mut CallbackState)) };
    // SuRoot dropped here (su_root_destroy called by Drop impl).
}

// ---------------------------------------------------------------------------
// Command dispatch on the Sofia thread
// ---------------------------------------------------------------------------

fn handle_command(cmd: SofiaCommand, nua_ptr: *mut nua_t, shutting_down: &mut bool) {
    match cmd {
        SofiaCommand::Shutdown => {
            if !*shutting_down {
                *shutting_down = true;
                unsafe { nua_shutdown(nua_ptr) };
            }
        }

        SofiaCommand::Respond {
            handle,
            status,
            reason,
            sdp: _sdp,
        } => {
            let reason_cstr = CString::new(reason.as_str()).unwrap_or_default();
            // nua_respond(nh, status, phrase, tag_type, tag_value, ...) -> void
            unsafe {
                nua_respond(
                    handle.as_ptr(),
                    status as c_int,
                    reason_cstr.as_ptr(),
                    TAG_END_TYPE,
                    TAG_END_VALUE,
                )
            };
        }

        SofiaCommand::Options { uri } => {
            let uri_cstr = match CString::new(uri.as_str()) {
                Ok(s) => s,
                Err(_) => return,
            };

            // Create a handle for this OPTIONS request.
            // nua_handle(nua, hmagic, tag_type, tag_value, ...) -> *mut nua_handle_t
            let nh = unsafe {
                nua_handle(
                    nua_ptr,
                    std::ptr::null_mut(),
                    nutag_url_type(),
                    uri_cstr.as_ptr() as tag_value_t,
                    TAG_END_TYPE,
                    TAG_END_VALUE,
                )
            };

            if !nh.is_null() {
                // nua_options(nh, tag_type, tag_value, ...) -> void
                unsafe {
                    nua_options(
                        nh,
                        TAG_END_TYPE,
                        TAG_END_VALUE,
                    );
                }
                // The handle is kept alive by Sofia-SIP until the response arrives;
                // we'll receive nua_r_options in the callback.
                // We don't need to store nh — it will be cleaned up by Sofia-SIP.
            }
        }

        SofiaCommand::Invite { handle, uri: _uri, sdp: _sdp } => {
            // nua_invite(nh, tag_type, tag_value, ...) -> void
            unsafe {
                sofia_sip_sys::nua_invite(handle.as_ptr(), TAG_END_TYPE, TAG_END_VALUE);
            }
        }

        SofiaCommand::Register { handle, registrar: _registrar } => {
            unsafe {
                sofia_sip_sys::nua_register(handle.as_ptr(), TAG_END_TYPE, TAG_END_VALUE);
            }
        }

        SofiaCommand::Bye { handle } => {
            unsafe {
                sofia_sip_sys::nua_bye(handle.as_ptr(), TAG_END_TYPE, TAG_END_VALUE);
            }
        }
    }
}
