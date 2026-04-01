# Replace Sofia-SIP with PJSIP for Carrier-Grade SIP Proxy

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the sofia-sip C FFI layer with pjsip (pjproject) for both inbound and outbound SIP legs in the carrier proxy path, gaining Session Timers (RFC 4028), PRACK (RFC 3262), NAPTR (RFC 3263), UPDATE (RFC 3311), and Replaces (RFC 3891) for free.

**Architecture:** A dedicated OS thread runs `pjsip_endpt_handle_events()` with tokio mpsc channels bridging to async Rust. Each call gets its own per-call event channel via pjsip's `mod_data[]` slots on `pjsip_inv_session`, eliminating the demux problem sofia-sip had. The Rust proxy layer (routing, CDR, failover, capacity, translation, manipulation) stays unchanged. rsipstack remains for WebRTC/WS bridge modes.

**Tech Stack:** pjproject 2.14.x (C), bindgen, tokio mpsc channels, existing Rust proxy layer

**Context — Why this migration:**
- sofia-sip's variadic tag-list API made UAC (outbound INVITE) implementation impossible to complete — URI/SDP params are still stubs after months
- pjsip uses struct-based APIs that map cleanly through bindgen
- pjsip gives Session Timers + PRACK + NAPTR + UPDATE + Replaces as built-in modules
- pjsip is actively maintained (Teluu/Ooma, v2.14.1 2024) vs sofia-sip (dead upstream, only FreeSWITCH fork)
- pjsip has no glib dependency (sofia-sip requires glib2, fragile on macOS)

---

## Current State Reference

**Files being replaced (sofia-sip):**
- `crates/sofia-sip-sys/` — raw C FFI bindings (bindgen over sofia-sip headers)
- `crates/sofia-sip/` — safe Rust wrapper (NuaAgent, SofiaBridge, SofiaCommand, SofiaEvent, SofiaHandle)
- `src/endpoint/sofia_endpoint.rs` — SipEndpoint impl using sofia-sip

**Files being adapted (proxy layer):**
- `src/proxy/failover.rs` — FailoverLoop (currently uses rsipstack DialogLayer for outbound)
- `src/proxy/session.rs` — ProxyCallSession (currently uses rsipstack DialogStateReceiverGuard)
- `src/proxy/dispatch.rs` — dispatch_proxy_call (passes rsipstack dialog_layer to session)
- `src/endpoint/mod.rs` — feature-gates sofia_endpoint module
- `src/endpoint/manager.rs` — creates SofiaEndpoint for stack="sofia"
- `src/app.rs` — AppStateInner holds dialog_layer: Arc<DialogLayer>
- `src/call/sip.rs` — DialogStateReceiverGuard wraps rsipstack types
- `Cargo.toml` — workspace members, feature flags, dependencies

**Files staying unchanged:**
- `src/proxy/types.rs` — ProxyCallContext, ProxyCallEvent, ProxyCallPhase, DspConfig
- `src/proxy/media_bridge.rs` — RTP relay, codec negotiation
- `src/proxy/media_peer.rs` — media peer abstraction
- `src/proxy/bridge.rs` — WebRTC/WS bridge dispatch
- `src/routing/` — routing engine (LPM, regex, HTTP, weighted)
- `src/translation/` — number translation engine
- `src/manipulation/` — header manipulation engine
- `src/capacity/` — CPS/concurrent capacity enforcement
- `src/cdr/` — CDR generation and webhook delivery
- `src/endpoint/rsip_endpoint.rs` — rsipstack endpoint (stays for WebRTC/WS)

---

## Phase 1: pjsip-sys Crate (Raw FFI Bindings)

### Task 1.1: Install pjproject system dependency

**Files:**
- Create: `scripts/install-pjproject.sh`

**Step 1: Write the install script**

```bash
#!/usr/bin/env bash
# scripts/install-pjproject.sh
# Build and install pjproject 2.14.1 from source.
# Installs to /usr/local by default; override with PREFIX env var.

set -euo pipefail

PJPROJECT_VERSION="2.14.1"
PREFIX="${PREFIX:-/usr/local}"
WORKDIR="$(mktemp -d)"

echo "==> Downloading pjproject ${PJPROJECT_VERSION}..."
cd "$WORKDIR"
curl -fsSL "https://github.com/pjsip/pjproject/archive/refs/tags/${PJPROJECT_VERSION}.tar.gz" \
    | tar xz

cd "pjproject-${PJPROJECT_VERSION}"

echo "==> Configuring (SIP-only, no video, no sound device)..."
./configure \
    --prefix="$PREFIX" \
    --disable-video \
    --disable-sound \
    --disable-v4l2 \
    --disable-opencore-amr \
    --disable-silk \
    --disable-bcg729 \
    --disable-libyuv \
    --disable-libwebrtc \
    --enable-shared \
    --with-ssl=/usr/local/opt/openssl 2>/dev/null || \
./configure \
    --prefix="$PREFIX" \
    --disable-video \
    --disable-sound \
    --disable-v4l2 \
    --disable-opencore-amr \
    --disable-silk \
    --disable-bcg729 \
    --disable-libyuv \
    --disable-libwebrtc \
    --enable-shared

echo "==> Building..."
make dep
make -j"$(nproc 2>/dev/null || sysctl -n hw.ncpu)"

echo "==> Installing to ${PREFIX}..."
make install

echo "==> Cleaning up..."
rm -rf "$WORKDIR"

echo "==> pjproject ${PJPROJECT_VERSION} installed to ${PREFIX}"
echo "    Verify: pkg-config --modversion libpjproject"
```

**Step 2: Run the install script**

Run: `chmod +x scripts/install-pjproject.sh && bash scripts/install-pjproject.sh`
Expected: pjproject libraries installed, `pkg-config --modversion libpjproject` returns `2.14.1`

**Step 3: Commit**

```bash
git add scripts/install-pjproject.sh
git commit -m "chore: add pjproject install script for pjsip migration"
```

---

### Task 1.2: Create pjsip-sys crate with bindgen

**Files:**
- Create: `crates/pjsip-sys/Cargo.toml`
- Create: `crates/pjsip-sys/pjsip_wrapper.h`
- Create: `crates/pjsip-sys/build.rs`
- Create: `crates/pjsip-sys/src/lib.rs`

**Step 1: Create Cargo.toml**

```toml
# crates/pjsip-sys/Cargo.toml
[package]
name = "pjsip-sys"
version = "0.1.0"
edition = "2024"
description = "Raw FFI bindings to pjproject (pjsip, pjlib, pjsip-ua)"
license = "MIT"
links = "pjsip"

[build-dependencies]
bindgen = "0.71"
pkg-config = "0.3"

[dependencies]
libc = "0.2"
```

**Step 2: Create the wrapper header**

This header includes only the pjsip headers needed for a SIP B2BUA.
Do NOT include pjmedia audio device headers — we handle media in Rust.

```c
/* crates/pjsip-sys/pjsip_wrapper.h */

/* pjlib base framework */
#include <pj/types.h>
#include <pj/pool.h>
#include <pj/log.h>
#include <pj/os.h>
#include <pj/string.h>
#include <pj/timer.h>
#include <pj/errno.h>

/* pjlib-util (DNS resolver for NAPTR/SRV) */
#include <pjlib-util/resolver.h>
#include <pjlib-util/srv_resolver.h>
#include <pjlib-util/dns.h>

/* pjsip core: transport, transaction, message parsing */
#include <pjsip/sip_transport.h>
#include <pjsip/sip_transaction.h>
#include <pjsip/sip_endpoint.h>
#include <pjsip/sip_module.h>
#include <pjsip/sip_event.h>
#include <pjsip/sip_msg.h>
#include <pjsip/sip_uri.h>
#include <pjsip/sip_auth.h>
#include <pjsip/sip_dialog.h>
#include <pjsip/sip_ua_layer.h>
#include <pjsip/sip_util.h>
#include <pjsip/sip_resolve.h>

/* pjsip-ua: INVITE session (dialog + offer/answer) */
#include <pjsip-ua/sip_inv.h>
#include <pjsip-ua/sip_regc.h>
#include <pjsip-ua/sip_replaces.h>
#include <pjsip-ua/sip_xfer.h>
#include <pjsip-ua/sip_100rel.h>
#include <pjsip-ua/sip_timer.h>

/* pjsip-simple: presence / SUBSCRIBE / NOTIFY */
#include <pjsip-simple/evsub.h>

/* SDP (used for offer/answer, lives in pjmedia but has no audio deps) */
#include <pjmedia/sdp.h>
#include <pjmedia/sdp_neg.h>
```

**Step 3: Create build.rs**

```rust
// crates/pjsip-sys/build.rs
use std::env;
use std::path::PathBuf;

fn main() {
    // Try pkg-config first (works when pjproject is installed system-wide).
    let pjsip = pkg_config::Config::new()
        .atleast_version("2.14")
        .probe("libpjproject")
        .unwrap_or_else(|e| {
            panic!(
                "pjproject not found: {e}\n\
                Install with: bash scripts/install-pjproject.sh"
            );
        });

    // Emit link directives from pkg-config.
    for path in &pjsip.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for lib_name in &pjsip.libs {
        println!("cargo:rustc-link-lib={lib_name}");
    }

    // Build include path args for clang.
    let include_args: Vec<String> = pjsip
        .include_paths
        .iter()
        .map(|p| format!("-I{}", p.display()))
        .collect();

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let wrapper_path = PathBuf::from(&manifest_dir).join("pjsip_wrapper.h");

    let mut builder = bindgen::Builder::default()
        .header(wrapper_path.to_str().unwrap())
        // --- Allowlist: only what we need for B2BUA ---
        // pjlib
        .allowlist_function("pj_init")
        .allowlist_function("pj_shutdown")
        .allowlist_function("pj_pool_create")
        .allowlist_function("pj_pool_release")
        .allowlist_function("pj_caching_pool_.*")
        .allowlist_function("pj_thread_.*")
        .allowlist_function("pj_log_.*")
        .allowlist_function("pj_str")
        .allowlist_function("pj_strerror")
        .allowlist_type("pj_pool_t")
        .allowlist_type("pj_pool_factory")
        .allowlist_type("pj_caching_pool")
        .allowlist_type("pj_str_t")
        .allowlist_type("pj_status_t")
        .allowlist_type("pj_bool_t")
        .allowlist_type("pj_thread_t")
        // pjsip endpoint
        .allowlist_function("pjsip_endpt_.*")
        .allowlist_type("pjsip_endpoint")
        .allowlist_type("pjsip_host_port")
        // pjsip transport
        .allowlist_function("pjsip_udp_transport_start.*")
        .allowlist_function("pjsip_tcp_transport_start.*")
        .allowlist_function("pjsip_tls_transport_start.*")
        .allowlist_type("pjsip_transport.*")
        .allowlist_type("pjsip_tls_setting")
        // pjsip transaction
        .allowlist_function("pjsip_tsx_.*")
        .allowlist_type("pjsip_transaction")
        // pjsip module
        .allowlist_function("pjsip_endpt_register_module")
        .allowlist_function("pjsip_endpt_unregister_module")
        .allowlist_type("pjsip_module")
        // pjsip message
        .allowlist_type("pjsip_msg.*")
        .allowlist_type("pjsip_hdr.*")
        .allowlist_type("pjsip_generic_string_hdr")
        .allowlist_type("pjsip_uri")
        .allowlist_type("pjsip_sip_uri")
        .allowlist_type("pjsip_method.*")
        .allowlist_type("pjsip_status_code")
        .allowlist_type("pjsip_rx_data")
        .allowlist_type("pjsip_tx_data")
        .allowlist_function("pjsip_msg_.*")
        .allowlist_function("pjsip_hdr_.*")
        .allowlist_function("pjsip_generic_string_hdr_.*")
        .allowlist_function("pjsip_parse_uri")
        .allowlist_function("pjsip_uri_print")
        // pjsip-ua: dialog
        .allowlist_function("pjsip_dlg_.*")
        .allowlist_type("pjsip_dialog")
        .allowlist_type("pjsip_role_e")
        // pjsip-ua: INVITE session
        .allowlist_function("pjsip_inv_.*")
        .allowlist_type("pjsip_inv_session")
        .allowlist_type("pjsip_inv_state")
        .allowlist_type("pjsip_inv_callback")
        // pjsip-ua: session timer (RFC 4028)
        .allowlist_function("pjsip_timer_.*")
        .allowlist_type("pjsip_timer_setting")
        // pjsip-ua: 100rel / PRACK (RFC 3262)
        .allowlist_function("pjsip_100rel_.*")
        // pjsip-ua: Replaces (RFC 3891)
        .allowlist_function("pjsip_replaces_.*")
        .allowlist_type("pjsip_replaces_hdr")
        // pjsip-ua: REFER / transfer (RFC 3515)
        .allowlist_function("pjsip_xfer_.*")
        // pjsip-ua: registration client
        .allowlist_function("pjsip_regc_.*")
        .allowlist_type("pjsip_regc")
        // pjsip auth
        .allowlist_function("pjsip_auth_.*")
        .allowlist_type("pjsip_auth_clt_pref")
        .allowlist_type("pjsip_cred_info")
        // pjsip resolver (NAPTR/SRV - RFC 3263)
        .allowlist_function("pjsip_resolve")
        .allowlist_function("pjsip_endpt_resolve")
        .allowlist_type("pjsip_resolve_callback")
        .allowlist_type("pjsip_server_addresses")
        // pjlib-util DNS
        .allowlist_function("pj_dns_resolver_.*")
        .allowlist_type("pj_dns_resolver")
        // SDP
        .allowlist_function("pjmedia_sdp_.*")
        .allowlist_type("pjmedia_sdp_session")
        .allowlist_type("pjmedia_sdp_media")
        .allowlist_type("pjmedia_sdp_attr")
        .allowlist_type("pjmedia_sdp_conn")
        .allowlist_type("pjmedia_sdp_neg.*")
        // pjsip event subscription (SUBSCRIBE/NOTIFY)
        .allowlist_function("pjsip_evsub_.*")
        .allowlist_type("pjsip_evsub.*")
        // Use default layout (not opaque) so struct fields are accessible
        .derive_debug(true)
        .derive_default(true);

    // Add include paths from pkg-config.
    for arg in &include_args {
        builder = builder.clang_arg(arg);
    }

    let bindings = builder
        .generate()
        .expect("Failed to generate pjsip bindings");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write pjsip bindings");
}
```

**Key difference from sofia-sip-sys build.rs:**
- Uses `libpjproject` pkg-config (not `sofia-sip-ua`)
- Does NOT use `.opaque_type(".*")` — we need struct field access
- Allowlists are specific to B2BUA needs
- `derive_default(true)` enables `Default` for callback structs

**Step 4: Create src/lib.rs**

```rust
// crates/pjsip-sys/src/lib.rs
//! Raw FFI bindings to pjproject (pjsip, pjlib, pjsip-ua).
//!
//! Generated by bindgen from `pjsip_wrapper.h`.
//! These are unsafe C bindings — use `crates/pjsip/` for the safe wrapper.

#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::all
)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
```

**Step 5: Add to workspace**

The workspace `Cargo.toml` already has `members = ["crates/*"]`, so the new crate
is auto-discovered. Verify with:

Run: `cd /Users/parvbhullar/Drives/Vault/Projects/Unpod/super-voice && cargo check -p pjsip-sys`
Expected: Compiles successfully, bindings generated

**Step 6: Commit**

```bash
git add crates/pjsip-sys/
git commit -m "feat: add pjsip-sys crate with bindgen bindings for pjproject"
```

---

## Phase 2: pjsip Safe Wrapper Crate

### Task 2.1: Create pjsip crate skeleton with core types

**Files:**
- Create: `crates/pjsip/Cargo.toml`
- Create: `crates/pjsip/src/lib.rs`
- Create: `crates/pjsip/src/error.rs`
- Create: `crates/pjsip/src/pool.rs`

**Step 1: Create Cargo.toml**

```toml
# crates/pjsip/Cargo.toml
[package]
name = "pjsip"
version = "0.1.0"
edition = "2024"
description = "Safe Rust wrapper for pjproject SIP library"
license = "MIT"

[dependencies]
pjsip-sys = { path = "../pjsip-sys" }
libc = "0.2"
anyhow = "1"
tokio = { version = "1", features = ["sync", "rt", "macros"] }
tracing = "0.1"
```

**Step 2: Create error.rs — pjsip status code wrapper**

```rust
// crates/pjsip/src/error.rs
//! Error types wrapping pj_status_t.

use std::ffi::CStr;
use std::fmt;

/// Wrapper around pjsip's `pj_status_t` error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PjStatus(pub i32);

impl PjStatus {
    pub const SUCCESS: Self = Self(0); // PJ_SUCCESS

    /// Returns true if status indicates success (PJ_SUCCESS == 0).
    pub fn is_ok(self) -> bool {
        self.0 == 0
    }

    /// Convert to a human-readable error string via pj_strerror.
    pub fn message(self) -> String {
        let mut buf = [0u8; 256];
        let pj_str = unsafe {
            pjsip_sys::pj_strerror(
                self.0,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
            )
        };
        // pj_strerror returns a pj_str_t; extract via slen + ptr.
        if pj_str.slen > 0 && !pj_str.ptr.is_null() {
            let slice = unsafe {
                std::slice::from_raw_parts(pj_str.ptr as *const u8, pj_str.slen as usize)
            };
            String::from_utf8_lossy(slice).into_owned()
        } else {
            format!("pjsip error {}", self.0)
        }
    }
}

impl fmt::Display for PjStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_ok() {
            write!(f, "PJ_SUCCESS")
        } else {
            write!(f, "{}", self.message())
        }
    }
}

impl std::error::Error for PjStatus {}

/// Convert a pj_status_t to Result.
pub fn check_status(status: i32) -> Result<(), PjStatus> {
    if status == 0 {
        Ok(())
    } else {
        Err(PjStatus(status))
    }
}
```

**Step 3: Create pool.rs — memory pool wrapper**

pjsip uses pool-based allocation. Every operation needs a pool.

```rust
// crates/pjsip/src/pool.rs
//! Safe wrapper around pj_pool_t and pj_caching_pool.

use crate::error::{PjStatus, check_status};
use std::ffi::CString;
use std::ptr;

/// Global caching pool factory — must be initialized once at startup.
pub struct CachingPool {
    inner: pjsip_sys::pj_caching_pool,
}

impl CachingPool {
    /// Initialize the caching pool factory.
    ///
    /// Call this once at application startup, after `pj_init()`.
    pub fn new() -> Self {
        let mut cp: pjsip_sys::pj_caching_pool = unsafe { std::mem::zeroed() };
        unsafe {
            pjsip_sys::pj_caching_pool_init(&mut cp, ptr::null(), 0);
        }
        Self { inner: cp }
    }

    /// Get a pointer to the pool factory (needed by pjsip_endpt_create).
    pub fn factory_ptr(&mut self) -> *mut pjsip_sys::pj_pool_factory {
        &mut self.inner.factory as *mut pjsip_sys::pj_pool_factory
    }

    /// Create a named pool with the given initial and increment sizes.
    pub fn create_pool(&mut self, name: &str, initial: usize, increment: usize) -> Pool {
        let name_cstr = CString::new(name).unwrap_or_default();
        let pool = unsafe {
            pjsip_sys::pj_pool_create(
                self.factory_ptr(),
                name_cstr.as_ptr(),
                initial,
                increment,
                ptr::null_mut(),
            )
        };
        Pool { ptr: pool }
    }
}

impl Drop for CachingPool {
    fn drop(&mut self) {
        unsafe { pjsip_sys::pj_caching_pool_destroy(&mut self.inner) };
    }
}

/// Safe wrapper around a pj_pool_t.
pub struct Pool {
    ptr: *mut pjsip_sys::pj_pool_t,
}

impl Pool {
    pub fn as_ptr(&self) -> *mut pjsip_sys::pj_pool_t {
        self.ptr
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { pjsip_sys::pj_pool_release(self.ptr) };
        }
    }
}

// SAFETY: Pool is only used on the pjsip thread.
unsafe impl Send for Pool {}
```

**Step 4: Create lib.rs**

```rust
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
//! Each INVITE session stores a per-call event sender in its `mod_data[]`
//! slot, giving every call its own isolated event channel.

pub mod bridge;
pub mod command;
pub mod endpoint;
pub mod error;
pub mod event;
pub mod pool;
pub mod session;

pub use bridge::PjBridge;
pub use command::PjCommand;
pub use endpoint::PjEndpoint;
pub use error::{PjStatus, check_status};
pub use event::PjCallEvent;
pub use pool::{CachingPool, Pool};
pub use session::PjInvSession;
```

**Step 5: Verify it compiles**

Run: `cargo check -p pjsip`
Expected: Compiles (modules declared but empty — that's fine, we'll fill them next)

**Step 6: Commit**

```bash
git add crates/pjsip/
git commit -m "feat: add pjsip crate skeleton with error and pool wrappers"
```

---

### Task 2.2: Implement PjCallEvent and PjCommand types

**Files:**
- Create: `crates/pjsip/src/event.rs`
- Create: `crates/pjsip/src/command.rs`

**Step 1: Create event.rs — per-call events from pjsip to Rust**

These map 1:1 to what `FailoverLoop::wait_for_outcome()` currently expects from
rsipstack's `DialogState` enum (see `src/proxy/failover.rs:219-267`).

```rust
// crates/pjsip/src/event.rs
//! Events sent from pjsip callbacks to per-call async receivers.

/// A SIP event for a specific call, delivered via per-call mpsc channel.
///
/// Maps to the states that `ProxyCallSession` and `FailoverLoop` care about.
#[derive(Debug, Clone)]
pub enum PjCallEvent {
    /// INVITE transaction is in progress (1xx received, no SDP).
    Trying,

    /// Early dialog established (typically 180 Ringing, no SDP).
    Ringing {
        /// SIP status code (180, 183, etc.)
        status: u16,
    },

    /// Early media received (183 Session Progress with SDP body).
    EarlyMedia {
        /// SIP status code (183).
        status: u16,
        /// SDP body from the provisional response.
        sdp: String,
    },

    /// Call confirmed (200 OK received, ACK sent by pjsip).
    Confirmed {
        /// SDP answer from the 200 OK body.
        sdp: Option<String>,
    },

    /// Call terminated (BYE, CANCEL, error, or timeout).
    Terminated {
        /// Final SIP status code.
        code: u16,
        /// Reason phrase or description.
        reason: String,
    },

    /// Mid-dialog INFO received (used for DTMF, etc).
    Info {
        /// Content-Type header value.
        content_type: String,
        /// Raw body of the INFO.
        body: String,
    },

    /// Mid-dialog re-INVITE or UPDATE received (hold/resume/codec change).
    ReInvite {
        /// New SDP offer.
        sdp: String,
    },
}

/// Sender half of a per-call event channel.
pub type PjCallEventSender = tokio::sync::mpsc::UnboundedSender<PjCallEvent>;

/// Receiver half of a per-call event channel.
pub type PjCallEventReceiver = tokio::sync::mpsc::UnboundedReceiver<PjCallEvent>;
```

**Step 2: Create command.rs — commands from Rust to pjsip thread**

```rust
// crates/pjsip/src/command.rs
//! Commands sent from async Rust to the pjsip OS thread.

use crate::event::PjCallEventSender;

/// A command sent from async Rust to the dedicated pjsip thread.
///
/// The bridge thread drains these and dispatches pjsip API calls.
#[derive(Debug)]
pub enum PjCommand {
    /// Create a UAS INVITE session for an incoming call.
    /// (Used when pjsip auto-creates the session via on_rx_request callback.)
    /// The Rust layer provides a per-call event sender.
    AcceptIncoming {
        /// Opaque call ID assigned by the pjsip callback.
        call_id: String,
        /// Per-call event sender for this call's events.
        event_tx: PjCallEventSender,
    },

    /// Send a SIP response to an incoming INVITE.
    Respond {
        /// Opaque call ID identifying the inv_session.
        call_id: String,
        /// SIP status code (180, 200, 486, etc.).
        status: u16,
        /// Reason phrase.
        reason: String,
        /// Optional SDP body to include in the response.
        sdp: Option<String>,
    },

    /// Create an outbound INVITE (UAC).
    CreateInvite {
        /// Target SIP URI (e.g. `sip:+14155551234@carrier.com`).
        uri: String,
        /// From URI.
        from: String,
        /// SDP offer body.
        sdp: String,
        /// Per-call event sender — events for this call go here.
        event_tx: PjCallEventSender,
        /// Optional digest auth credentials for downstream carrier.
        credential: Option<PjCredential>,
        /// Optional custom headers to add to the INVITE.
        headers: Option<Vec<(String, String)>>,
    },

    /// Send BYE to terminate an active call.
    Bye {
        /// Opaque call ID identifying the inv_session.
        call_id: String,
    },

    /// Shut down the pjsip endpoint gracefully.
    Shutdown,
}

/// Digest authentication credential for outbound requests.
#[derive(Debug, Clone)]
pub struct PjCredential {
    pub realm: String,
    pub username: String,
    pub password: String,
    /// "digest" for RFC 2617/8760.
    pub scheme: String,
}
```

**Step 3: Verify it compiles**

Run: `cargo check -p pjsip`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add crates/pjsip/src/event.rs crates/pjsip/src/command.rs
git commit -m "feat(pjsip): add PjCallEvent and PjCommand types"
```

---

### Task 2.3: Implement PjEndpoint — pjsip endpoint lifecycle

**Files:**
- Create: `crates/pjsip/src/endpoint.rs`

**Step 1: Write the endpoint wrapper**

This wraps `pjsip_endpoint` initialization: pj_init, caching pool, endpoint create,
transport start, module registration, INVITE session layer init, 100rel init, timer init.

```rust
// crates/pjsip/src/endpoint.rs
//! Safe wrapper around pjsip_endpoint — the core SIP processing engine.
//!
//! Handles:
//! - pjlib initialization
//! - Caching pool creation
//! - Endpoint creation
//! - Transport binding (UDP/TCP/TLS)
//! - Module registration for INVITE sessions, 100rel, session timers
//!
//! All operations MUST happen on the pjsip OS thread.

use crate::error::check_status;
use crate::pool::CachingPool;
use anyhow::{Result, anyhow};
use std::ffi::CString;
use std::ptr;

/// Configuration for creating a PjEndpoint.
#[derive(Debug, Clone)]
pub struct PjEndpointConfig {
    /// Bind address (e.g. "0.0.0.0").
    pub bind_addr: String,
    /// SIP port (e.g. 5060).
    pub port: u16,
    /// Transport: "udp", "tcp", or "tls".
    pub transport: String,
    /// TLS certificate file path (required when transport = "tls").
    pub tls_cert_file: Option<String>,
    /// TLS private key file path.
    pub tls_privkey_file: Option<String>,
    /// Enable session timers (RFC 4028). Default: true.
    pub session_timers: bool,
    /// Session-Expires value in seconds. Default: 1800 (30 minutes).
    pub session_expires: u32,
    /// Min-SE value in seconds. Default: 90.
    pub min_se: u32,
    /// Enable 100rel / PRACK (RFC 3262). Default: true.
    pub enable_100rel: bool,
}

impl Default for PjEndpointConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0".to_string(),
            port: 5060,
            transport: "udp".to_string(),
            tls_cert_file: None,
            tls_privkey_file: None,
            session_timers: true,
            session_expires: 1800,
            min_se: 90,
            enable_100rel: true,
        }
    }
}

/// Wraps a running pjsip_endpoint with all required modules initialized.
///
/// MUST only be used from the pjsip OS thread.
pub struct PjEndpoint {
    pub(crate) endpt: *mut pjsip_sys::pjsip_endpoint,
    pub(crate) caching_pool: CachingPool,
    pub(crate) pool: *mut pjsip_sys::pj_pool_t,
    config: PjEndpointConfig,
}

// SAFETY: PjEndpoint is only accessed from the dedicated pjsip thread.
// The bridge serializes all access through the command channel.
unsafe impl Send for PjEndpoint {}

impl PjEndpoint {
    /// Initialize pjlib + create endpoint + bind transport + register modules.
    ///
    /// # Safety
    ///
    /// Must be called on the pjsip OS thread. Not safe to call concurrently.
    pub fn create(config: PjEndpointConfig) -> Result<Self> {
        // 1. Initialize pjlib
        let status = unsafe { pjsip_sys::pj_init() };
        check_status(status).map_err(|e| anyhow!("pj_init failed: {e}"))?;

        // 2. Create caching pool
        let mut caching_pool = CachingPool::new();

        // 3. Create endpoint
        let name = CString::new("super-voice").unwrap();
        let mut endpt: *mut pjsip_sys::pjsip_endpoint = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjsip_endpt_create(
                caching_pool.factory_ptr(),
                name.as_ptr(),
                &mut endpt,
            )
        };
        check_status(status).map_err(|e| anyhow!("pjsip_endpt_create failed: {e}"))?;

        // 4. Create a pool for general allocations
        let pool = unsafe {
            pjsip_sys::pjsip_endpt_create_pool(
                endpt,
                CString::new("pjsip-main").unwrap().as_ptr(),
                4096,
                4096,
            )
        };
        if pool.is_null() {
            return Err(anyhow!("failed to create main pool"));
        }

        let mut ep = Self {
            endpt,
            caching_pool,
            pool,
            config,
        };

        // 5. Initialize required modules
        ep.init_modules()?;

        // 6. Start transport
        ep.start_transport()?;

        Ok(ep)
    }

    /// Initialize INVITE session, 100rel, session timer, and replaces modules.
    fn init_modules(&mut self) -> Result<()> {
        // UA layer (required for dialog/INVITE session)
        let status = unsafe { pjsip_sys::pjsip_ua_init_module(self.endpt, ptr::null()) };
        check_status(status).map_err(|e| anyhow!("pjsip_ua_init_module failed: {e}"))?;

        // INVITE session module
        let inv_cb = pjsip_sys::pjsip_inv_callback {
            on_state_changed: Some(crate::bridge::on_inv_state_changed),
            on_new_session: Some(crate::bridge::on_inv_new_session),
            on_rx_offer: None,
            on_rx_reinvite: None,
            on_tsx_state_changed: None,
            ..unsafe { std::mem::zeroed() }
        };
        let status = unsafe { pjsip_sys::pjsip_inv_usage_init(self.endpt, &inv_cb) };
        check_status(status).map_err(|e| anyhow!("pjsip_inv_usage_init failed: {e}"))?;

        // 100rel / PRACK (RFC 3262)
        if self.config.enable_100rel {
            let status = unsafe { pjsip_sys::pjsip_100rel_init_module(self.endpt) };
            check_status(status).map_err(|e| anyhow!("pjsip_100rel_init_module failed: {e}"))?;
        }

        // Session timers (RFC 4028)
        if self.config.session_timers {
            let status = unsafe { pjsip_sys::pjsip_timer_init_module(self.endpt) };
            check_status(status).map_err(|e| anyhow!("pjsip_timer_init_module failed: {e}"))?;
        }

        // Replaces (RFC 3891)
        let status = unsafe { pjsip_sys::pjsip_replaces_init_module(self.endpt) };
        check_status(status).map_err(|e| anyhow!("pjsip_replaces_init_module failed: {e}"))?;

        tracing::info!(
            session_timers = self.config.session_timers,
            prack = self.config.enable_100rel,
            "pjsip modules initialized"
        );

        Ok(())
    }

    /// Bind a SIP transport (UDP, TCP, or TLS).
    fn start_transport(&mut self) -> Result<()> {
        let bind_str = format!("{}:{}", self.config.bind_addr, self.config.port);
        tracing::info!(transport = %self.config.transport, bind = %bind_str, "starting pjsip transport");

        match self.config.transport.as_str() {
            "udp" => self.start_udp_transport(),
            "tcp" => self.start_tcp_transport(),
            "tls" => self.start_tls_transport(),
            other => Err(anyhow!("unsupported transport: {other}")),
        }
    }

    fn start_udp_transport(&mut self) -> Result<()> {
        let mut addr: pjsip_sys::pj_sockaddr_in = unsafe { std::mem::zeroed() };
        addr.sin_family = libc::AF_INET as u16;
        addr.sin_port = self.config.port.to_be();
        // 0.0.0.0 = INADDR_ANY
        addr.sin_addr.s_addr = 0;

        let mut tp: *mut pjsip_sys::pjsip_transport = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjsip_udp_transport_start(
                self.endpt,
                &addr,
                ptr::null(),    // published address (auto)
                1,              // async count
                &mut tp,
            )
        };
        check_status(status).map_err(|e| anyhow!("pjsip_udp_transport_start failed: {e}"))?;
        Ok(())
    }

    fn start_tcp_transport(&mut self) -> Result<()> {
        let mut addr: pjsip_sys::pj_sockaddr_in = unsafe { std::mem::zeroed() };
        addr.sin_family = libc::AF_INET as u16;
        addr.sin_port = self.config.port.to_be();
        addr.sin_addr.s_addr = 0;

        let mut factory: *mut pjsip_sys::pjsip_tpfactory = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjsip_tcp_transport_start(
                self.endpt,
                &addr,
                1,              // async count
                &mut factory,
            )
        };
        check_status(status).map_err(|e| anyhow!("pjsip_tcp_transport_start failed: {e}"))?;
        Ok(())
    }

    fn start_tls_transport(&mut self) -> Result<()> {
        let cert = self.config.tls_cert_file.as_deref()
            .ok_or_else(|| anyhow!("TLS requires tls_cert_file"))?;
        let key = self.config.tls_privkey_file.as_deref()
            .ok_or_else(|| anyhow!("TLS requires tls_privkey_file"))?;

        let mut tls_setting: pjsip_sys::pjsip_tls_setting = unsafe { std::mem::zeroed() };
        unsafe { pjsip_sys::pjsip_tls_setting_default(&mut tls_setting) };

        let cert_cstr = CString::new(cert)?;
        let key_cstr = CString::new(key)?;
        tls_setting.cert_file = unsafe { pjsip_sys::pj_str(cert_cstr.as_ptr() as *mut _) };
        tls_setting.privkey_file = unsafe { pjsip_sys::pj_str(key_cstr.as_ptr() as *mut _) };

        let mut addr: pjsip_sys::pj_sockaddr_in = unsafe { std::mem::zeroed() };
        addr.sin_family = libc::AF_INET as u16;
        addr.sin_port = self.config.port.to_be();
        addr.sin_addr.s_addr = 0;

        let mut factory: *mut pjsip_sys::pjsip_tpfactory = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjsip_tls_transport_start(
                self.endpt,
                &tls_setting,
                &addr,
                ptr::null(),
                1,
                &mut factory,
            )
        };
        check_status(status).map_err(|e| anyhow!("pjsip_tls_transport_start failed: {e}"))?;
        Ok(())
    }

    /// Run one iteration of the event loop (non-blocking, max_timeout ms).
    ///
    /// Call this in a tight loop on the pjsip thread.
    pub fn handle_events(&self, max_timeout_ms: u32) -> Result<()> {
        let timeout = pjsip_sys::pj_time_val {
            sec: (max_timeout_ms / 1000) as i64,
            msec: (max_timeout_ms % 1000) as i64,
        };
        let status = unsafe {
            pjsip_sys::pjsip_endpt_handle_events(self.endpt, &timeout)
        };
        // PJ_ETIMEDOUT is not an error — it means no events in the timeout window.
        if status != 0 {
            let pj_etimedout = 120004; // PJ_ETIMEDOUT
            if status != pj_etimedout {
                check_status(status).map_err(|e| anyhow!("handle_events: {e}"))?;
            }
        }
        Ok(())
    }
}

impl Drop for PjEndpoint {
    fn drop(&mut self) {
        if !self.endpt.is_null() {
            unsafe { pjsip_sys::pjsip_endpt_destroy(self.endpt) };
        }
        unsafe { pjsip_sys::pj_shutdown() };
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p pjsip`
Expected: Compiles (bridge callbacks referenced but not yet defined — we add those next)

**Step 3: Commit**

```bash
git add crates/pjsip/src/endpoint.rs
git commit -m "feat(pjsip): add PjEndpoint with transport binding and module init"
```

---

### Task 2.4: Implement PjBridge — OS thread + channel bridge

**Files:**
- Create: `crates/pjsip/src/bridge.rs`
- Create: `crates/pjsip/src/session.rs`

This is the core integration layer. It mirrors `crates/sofia-sip/src/bridge.rs`
architecture but uses pjsip's struct-based API instead of variadic tags.

**Step 1: Create session.rs — per-call session wrapper**

```rust
// crates/pjsip/src/session.rs
//! Per-call INVITE session wrapper.

use crate::event::PjCallEventSender;
use std::collections::HashMap;
use std::sync::Mutex;

/// Global registry of active call sessions.
///
/// Maps call_id -> per-call state. Accessed from the pjsip thread only
/// (via callbacks), so we use a simple Mutex.
pub(crate) static CALL_REGISTRY: once_cell::sync::Lazy<Mutex<HashMap<String, CallEntry>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// Per-call state stored in the registry.
pub(crate) struct CallEntry {
    /// Per-call event sender for delivering events to the Rust async layer.
    pub event_tx: PjCallEventSender,
    /// Raw pointer to the pjsip_inv_session (for sending BYE, etc).
    pub inv_ptr: *mut pjsip_sys::pjsip_inv_session,
}

// SAFETY: inv_ptr is only dereferenced on the pjsip thread.
unsafe impl Send for CallEntry {}

/// Wrapper around pjsip_inv_session for external use.
#[derive(Debug)]
pub struct PjInvSession {
    pub call_id: String,
}
```

**Step 2: Create bridge.rs — the OS thread + callbacks**

```rust
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
use crate::session::{CALL_REGISTRY, CallEntry};
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
    // 1. Create endpoint (initializes pjlib, modules, transport)
    let endpoint = match PjEndpoint::create(config) {
        Ok(ep) => ep,
        Err(e) => {
            error!("Failed to create pjsip endpoint: {e}");
            return;
        }
    };

    info!("pjsip thread started");

    let mut shutting_down = false;

    // 2. Event loop: step pjsip + drain commands
    loop {
        // Process pjsip events (timers, retransmissions, incoming SIP)
        if let Err(e) = endpoint.handle_events(5) {
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

    // 3. Cleanup: drop endpoint triggers pjsip_endpt_destroy + pj_shutdown
    info!("pjsip thread exiting");
    drop(endpoint);
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
            if let Err(e) = create_outbound_invite(endpoint, &uri, &from, &sdp, event_tx, credential, headers) {
                warn!("CreateInvite failed: {e}");
            }
        }

        PjCommand::Respond {
            call_id,
            status,
            reason,
            sdp,
        } => {
            if let Err(e) = respond_to_invite(&call_id, status, &reason, sdp.as_deref()) {
                warn!("Respond failed for {call_id}: {e}");
            }
        }

        PjCommand::Bye { call_id } => {
            if let Err(e) = send_bye(&call_id) {
                warn!("Bye failed for {call_id}: {e}");
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
    if let Some(cred) = credential {
        let mut cred_info: pjsip_sys::pjsip_cred_info = unsafe { std::mem::zeroed() };
        let realm_cstr = CString::new(cred.realm)?;
        let scheme_cstr = CString::new(cred.scheme)?;
        let user_cstr = CString::new(cred.username)?;
        let pass_cstr = CString::new(cred.password)?;

        cred_info.realm = unsafe { pjsip_sys::pj_str(realm_cstr.as_ptr() as *mut _) };
        cred_info.scheme = unsafe { pjsip_sys::pj_str(scheme_cstr.as_ptr() as *mut _) };
        cred_info.username = unsafe { pjsip_sys::pj_str(user_cstr.as_ptr() as *mut _) };
        cred_info.data = unsafe { pjsip_sys::pj_str(pass_cstr.as_ptr() as *mut _) };
        cred_info.data_type = 0; // PJSIP_CRED_DATA_PLAIN_PASSWD

        let status = unsafe {
            pjsip_sys::pjsip_auth_clt_set_credentials(
                &mut (*dlg).auth_sess,
                1,
                &cred_info,
            )
        };
        if status != 0 {
            warn!("failed to set auth credentials: {}", crate::PjStatus(status));
        }
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
    let status = unsafe {
        pjsip_sys::pjsip_inv_create_uac(dlg, sdp_session, 0, &mut inv)
    };
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

    // 6. Generate call ID and register in call registry
    let call_id = unsafe {
        let dlg_ref = &*dlg;
        let call_id_pj = &dlg_ref.call_id;
        if !call_id_pj.id.ptr.is_null() && call_id_pj.id.slen > 0 {
            let slice = std::slice::from_raw_parts(
                call_id_pj.id.ptr as *const u8,
                call_id_pj.id.slen as usize,
            );
            String::from_utf8_lossy(slice).into_owned()
        } else {
            uuid::Uuid::new_v4().to_string()
        }
    };

    {
        let mut registry = CALL_REGISTRY.lock().map_err(|e| anyhow::anyhow!("registry lock: {e}"))?;
        registry.insert(
            call_id.clone(),
            CallEntry {
                event_tx,
                inv_ptr: inv,
            },
        );
    }

    // 7. Create and send the initial INVITE request
    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status = unsafe { pjsip_sys::pjsip_inv_invite(inv, &mut tdata) };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_invite: {e}"))?;

    // 8. Add custom headers if provided
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
                    pjsip_sys::pjsip_msg_add_hdr(
                        (*tdata).msg,
                        hdr as *mut pjsip_sys::pjsip_hdr,
                    );
                }
            }
        }
    }

    // 9. Send the INVITE
    let status = unsafe { pjsip_sys::pjsip_inv_send_msg(inv, tdata) };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_send_msg: {e}"))?;

    info!(call_id = %call_id, target = %uri, "outbound INVITE sent");
    Ok(())
}

// ---------------------------------------------------------------------------
// Respond to incoming INVITE
// ---------------------------------------------------------------------------

fn respond_to_invite(call_id: &str, status_code: u16, reason: &str, sdp: Option<&str>) -> Result<()> {
    let inv = {
        let registry = CALL_REGISTRY.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        registry
            .get(call_id)
            .map(|e| e.inv_ptr)
            .ok_or_else(|| anyhow::anyhow!("call {call_id} not found"))?
    };

    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status = unsafe {
        pjsip_sys::pjsip_inv_answer(inv, status_code as i32, ptr::null(), ptr::null_mut(), &mut tdata)
    };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_answer: {e}"))?;

    let status = unsafe { pjsip_sys::pjsip_inv_send_msg(inv, tdata) };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_send_msg: {e}"))?;

    debug!(call_id = %call_id, status = %status_code, "responded to INVITE");
    Ok(())
}

// ---------------------------------------------------------------------------
// Send BYE
// ---------------------------------------------------------------------------

fn send_bye(call_id: &str) -> Result<()> {
    let inv = {
        let registry = CALL_REGISTRY.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        registry
            .get(call_id)
            .map(|e| e.inv_ptr)
            .ok_or_else(|| anyhow::anyhow!("call {call_id} not found"))?
    };

    let mut tdata: *mut pjsip_sys::pjsip_tx_data = ptr::null_mut();
    let status = unsafe { pjsip_sys::pjsip_inv_end_session(inv, 200, ptr::null(), &mut tdata) };
    crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_end_session: {e}"))?;

    if !tdata.is_null() {
        let status = unsafe { pjsip_sys::pjsip_inv_send_msg(inv, tdata) };
        crate::check_status(status).map_err(|e| anyhow::anyhow!("pjsip_inv_send_msg (BYE): {e}"))?;
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
        pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_CALLING => {
            Some(PjCallEvent::Trying)
        }
        pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_EARLY => {
            // Check for SDP in the response (183 Session Progress)
            let (status_code, sdp) = extract_response_info(inv);
            if let Some(sdp_body) = sdp {
                Some(PjCallEvent::EarlyMedia {
                    status: status_code,
                    sdp: sdp_body,
                })
            } else {
                Some(PjCallEvent::Ringing {
                    status: status_code,
                })
            }
        }
        pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_CONFIRMED => {
            let (_, sdp) = extract_response_info(inv);
            Some(PjCallEvent::Confirmed { sdp })
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

            Some(PjCallEvent::Terminated {
                code: cause,
                reason,
            })
        }
        _ => None,
    };

    if let Some(ev) = pj_event {
        if let Err(e) = tx.send(ev) {
            error!(call_id = %call_id, "failed to send PjCallEvent: {e}");
        }
    }
}

/// Called when a new INVITE session is created (for incoming calls).
pub(crate) extern "C" fn on_inv_new_session(
    inv: *mut pjsip_sys::pjsip_inv_session,
    _event: *mut pjsip_sys::pjsip_event,
) {
    if inv.is_null() {
        return;
    }
    // For incoming calls, we create a placeholder registry entry.
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
        let call_id = &dlg_ref.call_id;
        if call_id.id.ptr.is_null() || call_id.id.slen <= 0 {
            return None;
        }
        let slice = std::slice::from_raw_parts(
            call_id.id.ptr as *const u8,
            call_id.id.slen as usize,
        );
        Some(String::from_utf8_lossy(slice).into_owned())
    }
}

/// Extract SIP status code and SDP body from the last response.
fn extract_response_info(inv: *mut pjsip_sys::pjsip_inv_session) -> (u16, Option<String>) {
    let inv_ref = unsafe { &*inv };
    let status_code = inv_ref.cause as u16;

    // Try to get SDP from the negotiator
    let mut remote_sdp: *const pjsip_sys::pjmedia_sdp_session = ptr::null();
    let neg_status = unsafe {
        pjsip_sys::pjmedia_sdp_neg_get_active_remote(inv_ref.neg, &mut remote_sdp)
    };
    if neg_status == 0 && !remote_sdp.is_null() {
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
            let sdp = String::from_utf8_lossy(&buf).into_owned();
            return (status_code, Some(sdp));
        }
    }

    (status_code, None)
}
```

**Step 2: Add once_cell and uuid to pjsip crate deps**

Modify `crates/pjsip/Cargo.toml`:
```toml
[dependencies]
pjsip-sys = { path = "../pjsip-sys" }
libc = "0.2"
anyhow = "1"
tokio = { version = "1", features = ["sync", "rt", "macros"] }
tracing = "0.1"
once_cell = "1"
uuid = { version = "1", features = ["v4"] }
```

**Step 3: Verify it compiles**

Run: `cargo check -p pjsip`
Expected: Compiles successfully

**Step 4: Write a smoke test**

Create: `crates/pjsip/tests/smoke.rs`

```rust
//! Smoke test: verify PjBridge starts and shuts down cleanly.

use pjsip::{PjBridge, PjCommand};
use pjsip::endpoint::PjEndpointConfig;

#[test]
fn test_bridge_start_and_shutdown() {
    let config = PjEndpointConfig {
        bind_addr: "127.0.0.1".to_string(),
        port: 15060, // Use a high port to avoid conflicts
        transport: "udp".to_string(),
        session_timers: false,
        enable_100rel: false,
        ..Default::default()
    };
    let bridge = PjBridge::start(config).expect("bridge should start");
    bridge.send_command(PjCommand::Shutdown).expect("shutdown should send");
    // Drop triggers thread join
    drop(bridge);
}
```

**Step 5: Run the smoke test**

Run: `cargo test -p pjsip --test smoke -- --nocapture`
Expected: PASS — bridge starts, shuts down cleanly

**Step 6: Commit**

```bash
git add crates/pjsip/
git commit -m "feat(pjsip): add PjBridge with OS thread, callbacks, and per-call event routing"
```

---

## Phase 3: PjDialogLayer Adapter + FailoverLoop Integration

### Task 3.1: Create PjDialogLayer adapter

**Files:**
- Create: `src/proxy/pj_dialog_layer.rs`
- Modify: `src/proxy/mod.rs`

This adapter provides the same interface that `FailoverLoop` needs, but routes
through pjsip instead of rsipstack.

**Step 1: Create pj_dialog_layer.rs**

```rust
// src/proxy/pj_dialog_layer.rs
//! Adapter that bridges pjsip's per-call event model to the interface
//! expected by FailoverLoop and ProxyCallSession.
//!
//! Replaces rsipstack's DialogLayer for the SIP-to-SIP proxy path.

use anyhow::Result;
use pjsip::bridge::PjBridge;
use pjsip::command::{PjCommand, PjCredential};
use pjsip::event::{PjCallEvent, PjCallEventReceiver};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Adapter providing outbound INVITE creation via pjsip.
///
/// Used by `FailoverLoop` as a drop-in replacement for rsipstack's `DialogLayer`.
#[derive(Clone)]
pub struct PjDialogLayer {
    bridge: Arc<PjBridge>,
}

impl PjDialogLayer {
    pub fn new(bridge: Arc<PjBridge>) -> Self {
        Self { bridge }
    }

    /// Create an outbound INVITE to `uri` and return a per-call event receiver.
    ///
    /// The caller waits on the receiver for `PjCallEvent::Confirmed` /
    /// `PjCallEvent::Terminated` / etc. — same pattern as rsipstack's
    /// `DialogStateReceiver`.
    pub fn create_invite(
        &self,
        uri: &str,
        from: &str,
        sdp: &str,
        credential: Option<PjCredential>,
        headers: Option<Vec<(String, String)>>,
    ) -> Result<PjCallEventReceiver> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.bridge.send_command(PjCommand::CreateInvite {
            uri: uri.to_string(),
            from: from.to_string(),
            sdp: sdp.to_string(),
            event_tx: tx,
            credential,
            headers,
        })?;
        Ok(rx)
    }

    /// Send BYE to terminate a call.
    pub fn send_bye(&self, call_id: &str) -> Result<()> {
        self.bridge.send_command(PjCommand::Bye {
            call_id: call_id.to_string(),
        })
    }

    /// Respond to an incoming INVITE.
    pub fn respond(&self, call_id: &str, status: u16, reason: &str, sdp: Option<String>) -> Result<()> {
        self.bridge.send_command(PjCommand::Respond {
            call_id: call_id.to_string(),
            status,
            reason: reason.to_string(),
            sdp,
        })
    }
}
```

**Step 2: Register in mod.rs**

Modify `src/proxy/mod.rs` — add after `pub mod bridge;`:

```rust
// Phase 8: pjsip carrier proxy
#[cfg(feature = "carrier")]
pub mod pj_dialog_layer;
```

**Step 3: Commit**

```bash
git add src/proxy/pj_dialog_layer.rs src/proxy/mod.rs
git commit -m "feat: add PjDialogLayer adapter for pjsip-based proxy"
```

---

### Task 3.2: Create PjFailoverLoop — pjsip-based failover

**Files:**
- Create: `src/proxy/pj_failover.rs`
- Modify: `src/proxy/mod.rs`

This is the pjsip equivalent of `src/proxy/failover.rs`. It uses
`PjDialogLayer::create_invite()` instead of rsipstack's `DialogLayer::do_invite_async()`.

**Step 1: Create pj_failover.rs**

```rust
// src/proxy/pj_failover.rs
//! pjsip-based failover loop for SIP-to-SIP proxy.
//!
//! Mirrors `failover.rs` but uses PjDialogLayer + PjCallEvent instead
//! of rsipstack DialogLayer + DialogState.

use crate::proxy::pj_dialog_layer::PjDialogLayer;
use crate::redis_state::types::TrunkConfig;
use anyhow::Result;
use pjsip::command::PjCredential;
use pjsip::event::{PjCallEvent, PjCallEventReceiver};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Result of a pjsip failover dial attempt.
pub enum PjFailoverResult {
    /// A gateway accepted the call.
    Connected {
        gateway_addr: String,
        call_event_rx: PjCallEventReceiver,
        /// Answer SDP from 200 OK or fallback from 183.
        sdp: Option<String>,
        /// pjsip call_id for sending BYE later.
        call_id: String,
    },
    /// A nofailover SIP code was received — do not retry.
    NoFailover { code: u16, reason: String },
    /// All gateways were tried and all failed.
    Exhausted { last_code: u16, last_reason: String },
    /// The trunk has no gateway references.
    NoRoutes,
}

/// pjsip-based failover loop.
pub struct PjFailoverLoop {
    dialog_layer: PjDialogLayer,
    cancel_token: CancellationToken,
}

impl PjFailoverLoop {
    pub fn new(dialog_layer: PjDialogLayer, cancel_token: CancellationToken) -> Self {
        Self {
            dialog_layer,
            cancel_token,
        }
    }

    /// Try each gateway in `trunk.gateways` until one answers or all fail.
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

        // Build credential from trunk config if present.
        let credential = trunk.credentials.as_ref().map(|c| PjCredential {
            realm: c.realm.clone().unwrap_or_default(),
            username: c.username.clone(),
            password: c.password.clone(),
            scheme: "digest".to_string(),
        });

        let mut last_code: u16 = 503;
        let mut last_reason = "Service Unavailable".to_string();

        for gateway_ref in gateways {
            info!(
                gateway = %gateway_ref.name,
                callee = %callee_uri,
                "pj_failover: trying gateway"
            );

            let target_uri = format!("sip:{}@{}", extract_user(callee_uri), gateway_ref.name);

            let mut event_rx = match self.dialog_layer.create_invite(
                &target_uri,
                caller_uri,
                caller_sdp,
                credential.clone(),
                None,
            ) {
                Ok(rx) => rx,
                Err(e) => {
                    warn!(gateway = %gateway_ref.name, "pj_failover: create_invite failed: {e}");
                    last_reason = e.to_string();
                    continue;
                }
            };

            // Wait for outcome on this gateway.
            let result = self
                .wait_for_outcome(&mut event_rx, trunk, &gateway_ref.name)
                .await;

            match result {
                WaitOutcome::Connected { sdp, call_id } => {
                    return Ok(PjFailoverResult::Connected {
                        gateway_addr: gateway_ref.name.clone(),
                        call_event_rx: event_rx,
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

    /// Wait for a single gateway attempt to reach a terminal state.
    async fn wait_for_outcome(
        &self,
        event_rx: &mut PjCallEventReceiver,
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
                ev = tokio::time::timeout(timeout_duration, event_rx.recv()) => {
                    match ev {
                        Ok(Some(event)) => event,
                        Ok(None) => {
                            return WaitOutcome::Failed {
                                code: 503,
                                reason: "Event channel closed".to_string(),
                            };
                        }
                        Err(_) => {
                            warn!(gateway=%gateway_name, "pj_failover: gateway timed out");
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
                    // Still in progress.
                }
                PjCallEvent::Ringing { .. } => {
                    // Ringing without SDP.
                }
                PjCallEvent::EarlyMedia { sdp, .. } => {
                    info!(gateway=%gateway_name, "pj_failover: early media SDP received");
                    early_media_sdp = Some(sdp);
                }
                PjCallEvent::Confirmed { sdp } => {
                    let answer_sdp = sdp.or_else(|| early_media_sdp.take());
                    info!(gateway=%gateway_name, has_sdp=%answer_sdp.is_some(), "pj_failover: call connected");
                    // Extract call_id (placeholder — will be passed through event in production)
                    return WaitOutcome::Connected {
                        sdp: answer_sdp,
                        call_id: String::new(), // TODO: pass call_id through PjCallEvent
                    };
                }
                PjCallEvent::Terminated { code, reason } => {
                    info!(gateway=%gateway_name, code=%code, "pj_failover: gateway rejected");
                    if crate::proxy::failover::is_nofailover(code, trunk) {
                        return WaitOutcome::NoFailover { code, reason };
                    }
                    return WaitOutcome::Failed { code, reason };
                }
                _ => {
                    // INFO, ReInvite during dialing — ignore.
                }
            }
        }
    }
}

enum WaitOutcome {
    Connected {
        sdp: Option<String>,
        call_id: String,
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

/// Extract user part from SIP URI (reuse from dispatch.rs logic).
fn extract_user(uri: &str) -> String {
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
```

**Step 2: Register in mod.rs**

Add to `src/proxy/mod.rs`:

```rust
#[cfg(feature = "carrier")]
pub mod pj_failover;
```

**Step 3: Commit**

```bash
git add src/proxy/pj_failover.rs src/proxy/mod.rs
git commit -m "feat: add PjFailoverLoop — pjsip-based gateway failover"
```

---

## Phase 4: Wire Into the Proxy Layer

### Task 4.1: Update Cargo.toml feature flags

**Files:**
- Modify: `Cargo.toml` (root)

**Step 1: Replace sofia-sip with pjsip in the carrier feature**

In `Cargo.toml`, change the `carrier` feature and dependencies:

Replace:
```toml
carrier = ["dep:sofia-sip", "dep:sofia-sip-sys", "dep:spandsp"]
```

With:
```toml
carrier = ["dep:pjsip", "dep:pjsip-sys", "dep:spandsp"]
```

Replace:
```toml
sofia-sip = { path = "crates/sofia-sip", optional = true }
sofia-sip-sys = { path = "crates/sofia-sip-sys", optional = true }
```

With:
```toml
pjsip = { path = "crates/pjsip", optional = true }
pjsip-sys = { path = "crates/pjsip-sys", optional = true }
```

Keep sofia-sip crates in the workspace (`crates/*` glob), but they won't be
compiled unless explicitly requested.

**Step 2: Verify it compiles**

Run: `cargo check --features carrier`
Expected: Compiles (will fail on sofia_endpoint references — we fix those next)

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: swap carrier feature from sofia-sip to pjsip"
```

---

### Task 4.2: Replace SofiaEndpoint with PjsipEndpoint

**Files:**
- Create: `src/endpoint/pjsip_endpoint.rs`
- Modify: `src/endpoint/mod.rs`
- Modify: `src/endpoint/manager.rs`

**Step 1: Create pjsip_endpoint.rs**

```rust
// src/endpoint/pjsip_endpoint.rs
//! `PjsipEndpoint` — SIP endpoint backed by pjproject.
//!
//! Only compiled when the `carrier` feature is enabled.
//! Replaces `SofiaEndpoint`.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use pjsip::bridge::PjBridge;
use pjsip::endpoint::PjEndpointConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::endpoint::SipEndpoint;
use crate::redis_state::EndpointConfig;

/// SIP endpoint using the pjproject stack.
///
/// Suitable for carrier-grade, high-throughput SIP deployments.
/// Supports Session Timers (RFC 4028), PRACK (RFC 3262), NAPTR (RFC 3263).
pub struct PjsipEndpoint {
    name: String,
    config: EndpointConfig,
    bridge: Option<Arc<PjBridge>>,
    running: Arc<AtomicBool>,
}

impl PjsipEndpoint {
    /// Build a `PjsipEndpoint` from an [`EndpointConfig`].
    pub fn from_config(config: &EndpointConfig) -> Result<Self> {
        if config.stack != "pjsip" && config.stack != "sofia" {
            return Err(anyhow!(
                "PjsipEndpoint requires stack='pjsip' or 'sofia' (migrated), got '{}'",
                config.stack
            ));
        }
        Ok(Self {
            name: config.name.clone(),
            config: config.clone(),
            bridge: None,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Get a reference to the PjBridge for creating PjDialogLayer.
    pub fn bridge(&self) -> Option<Arc<PjBridge>> {
        self.bridge.clone()
    }
}

#[async_trait]
impl SipEndpoint for PjsipEndpoint {
    fn name(&self) -> &str {
        &self.name
    }

    fn stack(&self) -> &str {
        "pjsip"
    }

    fn listen_addr(&self) -> String {
        format!("{}:{}", self.config.bind_addr, self.config.port)
    }

    async fn start(&mut self) -> Result<()> {
        let pj_config = PjEndpointConfig {
            bind_addr: self.config.bind_addr.clone(),
            port: self.config.port,
            transport: self.config.transport.clone(),
            tls_cert_file: self.config.tls.as_ref().and_then(|t| t.cert_file.clone()),
            tls_privkey_file: self.config.tls.as_ref().and_then(|t| t.key_file.clone()),
            session_timers: true,
            session_expires: 1800,
            min_se: 90,
            enable_100rel: true,
        };

        tracing::info!(
            name = %self.name,
            bind = %self.listen_addr(),
            transport = %self.config.transport,
            "starting pjsip endpoint"
        );

        let bridge = PjBridge::start(pj_config)?;
        self.bridge = Some(Arc::new(bridge));
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(bridge) = self.bridge.take() {
            bridge.send_command(pjsip::PjCommand::Shutdown)?;
        }
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(name = %self.name, "stopped pjsip endpoint");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
```

**Step 2: Update mod.rs — replace sofia_endpoint with pjsip_endpoint**

In `src/endpoint/mod.rs`, replace:
```rust
#[cfg(feature = "carrier")]
pub mod sofia_endpoint;
```
With:
```rust
#[cfg(feature = "carrier")]
pub mod pjsip_endpoint;
```

Replace:
```rust
#[cfg(feature = "carrier")]
pub use sofia_endpoint::SofiaEndpoint;
```
With:
```rust
#[cfg(feature = "carrier")]
pub use pjsip_endpoint::PjsipEndpoint;
```

**Step 3: Update manager.rs — replace SofiaEndpoint with PjsipEndpoint**

In `src/endpoint/manager.rs`, replace:
```rust
#[cfg(feature = "carrier")]
use crate::endpoint::SofiaEndpoint;
```
With:
```rust
#[cfg(feature = "carrier")]
use crate::endpoint::PjsipEndpoint;
```

In `create_endpoint`, replace:
```rust
#[cfg(feature = "carrier")]
"sofia" => Box::new(SofiaEndpoint::from_config(config)?),
```
With:
```rust
#[cfg(feature = "carrier")]
"sofia" | "pjsip" => Box::new(PjsipEndpoint::from_config(config)?),
```

This accepts both `"sofia"` (for backward compat with existing Redis configs)
and `"pjsip"` as the stack name.

**Step 4: Verify it compiles**

Run: `cargo check --features carrier`
Expected: Compiles successfully

**Step 5: Commit**

```bash
git add src/endpoint/pjsip_endpoint.rs src/endpoint/mod.rs src/endpoint/manager.rs
git commit -m "feat: replace SofiaEndpoint with PjsipEndpoint in carrier feature"
```

---

### Task 4.3: Update AppState to hold PjDialogLayer for proxy path

**Files:**
- Modify: `src/app.rs`

**Step 1: Add pjsip bridge to AppStateInner**

In `AppStateInner` struct (around line 49), add after `dialog_layer`:

```rust
/// pjsip bridge for carrier SIP proxy (Some when carrier feature enabled + endpoint started).
#[cfg(feature = "carrier")]
pub pj_bridge: Option<Arc<pjsip::bridge::PjBridge>>,
```

**Step 2: Initialize during app startup**

In the app initialization code where the carrier endpoint is started,
capture the bridge reference:

```rust
#[cfg(feature = "carrier")]
{
    // After endpoint_manager starts the carrier endpoint:
    if let Some(ep) = endpoint_manager.get_endpoint("carrier-sip") {
        // Downcast to PjsipEndpoint to get bridge
        // (alternatively, add a bridge() method to SipEndpoint trait)
    }
}
```

Note: The exact location depends on your app startup code. The key point is that
`pj_bridge` must be set before any proxy calls are dispatched.

**Step 3: Pass PjDialogLayer to dispatch_proxy_call**

This requires updating `dispatch_proxy_call` to accept either rsipstack `DialogLayer`
or `PjDialogLayer`. The cleanest approach is to add a `pj_dialog_layer` field to `AppState`:

```rust
// In dispatch.rs, the proxy call session creation changes from:
let failover = FailoverLoop::new(
    self.dialog_layer.clone(),
    self.cancel_token.clone(),
);
// To (under carrier feature):
let pj_failover = PjFailoverLoop::new(
    pj_dialog_layer.clone(),
    cancel_token.clone(),
);
```

**Step 4: Commit**

```bash
git add src/app.rs src/proxy/dispatch.rs
git commit -m "feat: wire PjDialogLayer into proxy dispatch path"
```

---

## Phase 5: Integration Testing

### Task 5.1: Unit tests for PjFailoverLoop

**Files:**
- Create: `src/proxy/pj_failover_test.rs` or add `#[cfg(test)] mod tests` in `pj_failover.rs`

Test the following scenarios (matching existing failover.rs tests):
1. `NoRoutes` when gateway list is empty
2. `is_nofailover` reused from existing tests (pure function, stack-agnostic)
3. Early media SDP fallback logic
4. Timeout handling
5. Cancel token propagation

### Task 5.2: Smoke test — outbound INVITE via pjsip

**Files:**
- Create: `crates/pjsip/tests/outbound_invite.rs`

Test that `PjBridge` can:
1. Start with UDP transport on a test port
2. Send a `CreateInvite` command
3. Receive a `Terminated` event (because no one answers on the test port)
4. Shut down cleanly

### Task 5.3: End-to-end proxy test with active-call-tester

Use the existing `active-call-tester/` infrastructure to:
1. Configure a trunk pointing to a local SIP echo server
2. Send an INVITE through the proxy
3. Verify the call is bridged and CDR is generated
4. Verify session timers are present in the SIP headers (tcpdump/wireshark)

---

## Phase 6: Cleanup

### Task 6.1: Remove sofia-sip from default build

**Files:**
- Delete: `src/endpoint/sofia_endpoint.rs`
- Keep: `crates/sofia-sip/` and `crates/sofia-sip-sys/` (in workspace but uncompiled)

Do NOT delete the sofia-sip crates yet — keep them for reference and in case
rollback is needed. They won't compile unless someone adds them back to features.

**Step 1: Delete sofia_endpoint.rs**

```bash
git rm src/endpoint/sofia_endpoint.rs
```

**Step 2: Clean up any remaining sofia-sip imports**

Search for `sofia_sip` or `sofia-sip` in `src/`:

Run: `grep -r "sofia" src/ --include="*.rs" -l`
Expected: No matches (or only comments/docs)

**Step 3: Commit**

```bash
git commit -m "chore: remove sofia_endpoint.rs (replaced by pjsip_endpoint)"
```

---

## Architecture Diagram (Final State)

```
Carrier A                                           Carrier B
    │                                                   ▲
    │ INVITE                                    INVITE  │
    ▼                                                   │
┌──────────────────────────────────────────────────────────┐
│                  pjsip OS Thread                          │
│            pjsip_endpt_handle_events()                   │
│                                                          │
│  ┌─────────────────┐          ┌─────────────────┐       │
│  │  UAS inv_session │          │  UAC inv_session │       │
│  │  (inbound leg)   │          │  (outbound leg)  │       │
│  │  Session Timers  │          │  Session Timers  │       │
│  │  PRACK           │          │  PRACK           │       │
│  │  Digest Auth     │          │  Digest Auth     │       │
│  └────────┬─────────┘          └────────┬─────────┘       │
│           │ per-call mpsc               │ per-call mpsc   │
└───────────┼─────────────────────────────┼─────────────────┘
            │                             │
            ▼                             ▼
┌──────────────────────────────────────────────────────────┐
│                Rust Async Proxy Layer                      │
│                                                          │
│  dispatch_proxy_call()                                   │
│    ├── DID lookup + route resolution (Redis)             │
│    ├── Trunk config + capacity check                     │
│    ├── Translation + manipulation                        │
│    ├── PjFailoverLoop::try_routes()                      │
│    │     └── PjDialogLayer::create_invite() per gateway  │
│    ├── ProxyCallSession bridge_loop                      │
│    │     └── Monitor both per-call event channels        │
│    ├── MediaBridge (RTP relay + DSP)                     │
│    └── CDR generation + webhook delivery                 │
│                                                          │
│  rsipstack DialogLayer (unchanged, for WebRTC/WS only)   │
└──────────────────────────────────────────────────────────┘
```

---

## Risk Mitigation Checklist

- [ ] pjsip-sys bindings compile on macOS (CI) and Linux (production)
- [ ] pjsip bridge starts/stops without leaking OS threads
- [ ] Per-call event channels are cleaned up on call termination
- [ ] Session timers are visible in SIP traces (tcpdump)
- [ ] PRACK exchanges complete for carriers requiring 100rel
- [ ] Existing rsipstack WebRTC/WS paths are unaffected
- [ ] CDR timing is correct (ring_time, answer_time, end_time)
- [ ] Capacity enforcement still works with pjsip path
- [ ] Failover nofailover codes still stop retry correctly
- [ ] Memory: no pool leaks after sustained call load (monitor `pj_pool` stats)

---

## Dependencies Between Tasks

```
Phase 1: pjsip-sys
  └── Task 1.1 (install pjproject) → Task 1.2 (bindgen crate)

Phase 2: pjsip wrapper
  └── Task 2.1 (skeleton) → Task 2.2 (events/commands) → Task 2.3 (endpoint) → Task 2.4 (bridge)

Phase 3: Adapter
  └── Task 3.1 (PjDialogLayer) → Task 3.2 (PjFailoverLoop)

Phase 4: Integration
  └── Task 4.1 (Cargo.toml) → Task 4.2 (PjsipEndpoint) → Task 4.3 (AppState wiring)

Phase 5: Testing (can start after Phase 3)
  └── Task 5.1 || Task 5.2 → Task 5.3

Phase 6: Cleanup (after Phase 5 passes)
  └── Task 6.1
```
