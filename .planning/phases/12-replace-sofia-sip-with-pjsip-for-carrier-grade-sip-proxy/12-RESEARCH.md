# Phase 12: Replace sofia-sip with pjsip for Carrier-Grade SIP Proxy - Research

**Researched:** 2026-04-01
**Domain:** pjproject (pjsip) C FFI, Rust bindgen, B2BUA threading model
**Confidence:** HIGH — full implementation is pre-specified in `docs/plans/2026-04-01-pjsip-migration.md`; research confirms and augments that plan

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- pjsip (pjproject 2.14.x) replaces sofia-sip — struct-based API vs variadic tag-list API
- pjsip gives Session Timers (RFC 4028), PRACK (RFC 3262), NAPTR (RFC 3263), UPDATE (RFC 3311), Replaces (RFC 3891) as built-in modules
- pjsip is actively maintained (Teluu/Ooma) vs sofia-sip (dead upstream, FreeSWITCH fork only)
- No glib dependency (sofia-sip requires glib2)
- Dedicated OS thread runs `pjsip_endpt_handle_events()` (same pattern as sofia-sip bridge)
- Tokio mpsc channels bridge async Rust to pjsip thread
- Per-call event isolation via CALL_REGISTRY (HashMap<String, CallEntry>) — NOT demux over shared channel
- Each call gets its own `PjCallEventSender`/`PjCallEventReceiver` pair
- `crates/pjsip-sys/` — raw bindgen, links to pjproject via pkg-config
- `crates/pjsip/` — safe wrapper: PjBridge, PjEndpoint, PjCallEvent, PjCommand, PjInvSession
- Struct fields NOT opaque (unlike sofia-sip-sys which used `.opaque_type(".*")`)
- bindgen allowlists limited to B2BUA-relevant APIs only
- `PjDialogLayer` adapter wraps PjBridge for FailoverLoop/ProxyCallSession consumption
- `PjFailoverLoop` mirrors existing `FailoverLoop` but uses PjCallEvent instead of rsipstack DialogState
- `PjsipEndpoint` implements `SipEndpoint` trait, accepts stack="pjsip" or "sofia" (backward compat)
- EndpointManager routes "sofia" and "pjsip" stack names to PjsipEndpoint
- AppStateInner gets `pj_bridge: Option<Arc<PjBridge>>` under carrier feature
- Module initialization on PjEndpoint::create: pjsip_ua_init_module, pjsip_inv_usage_init, pjsip_100rel_init_module, pjsip_timer_init_module, pjsip_replaces_init_module
- UAC flow: pjsip_dlg_create_uac -> pjsip_inv_create_uac -> pjsip_timer_init_session -> pjsip_inv_invite -> pjsip_inv_send_msg
- Auth credentials via pjsip_auth_clt_set_credentials on dialog
- Custom headers via pjsip_generic_string_hdr_create on tx_data
- SDP parsed via pjmedia_sdp_parse
- Callback event mapping: CALLING->Trying, EARLY->Ringing/EarlyMedia, CONFIRMED->Confirmed, DISCONNECTED->Terminated
- dispatch.rs routing/CDR/capacity/translation/manipulation logic unchanged
- ProxyCallContext, ProxyCallEvent, ProxyCallPhase types unchanged
- MediaBridge / RTP relay unchanged
- rsipstack for WebRTC/WS bridge modes unchanged
- All Redis config types unchanged

### Claude's Discretion

- Exact pjsip-sys bindgen allowlist tuning (may need adjustments based on actual header layout)
- Pool sizing for pjsip allocations (initial 4096/increment 4096 — may need tuning)
- Error handling strategy for pjsip thread panics (catch_unwind vs let-it-crash)
- Whether to keep sofia-sip crates in workspace or move to separate branch
- Test port selection strategy for integration tests

### Deferred Ideas (OUT OF SCOPE)

- Full PJMEDIA integration (audio device, conferencing) — not needed for proxy
- SCTP transport support
- ICE/STUN/TURN via pjnath (rsipstack handles this for WebRTC)
- Gateway mode fax (T.38 SIP negotiation via pjsip) — deferred to v2
- Removing sofia-sip crates from workspace entirely — keep for rollback reference
- pjsip thread pool (multiple worker threads) — single thread sufficient for initial deployment
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| PJMIG-01 | pjsip-sys crate with bindgen FFI bindings to pjproject 2.14.x | Build system pattern confirmed in design doc; pkg-config probe `libpjproject`; allowlist covers B2BUA surface |
| PJMIG-02 | pjsip safe wrapper crate (PjBridge, PjEndpoint, PjCallEvent, PjCommand, pool/error helpers) | Full implementation specified in design doc; threading model confirmed; CALL_REGISTRY pattern validated |
| PJMIG-03 | PjDialogLayer adapter and PjFailoverLoop for proxy integration | Adapter pattern fully specified; maps cleanly to existing FailoverLoop interface |
| PJMIG-04 | PjsipEndpoint replacing SofiaEndpoint in carrier feature | Backward-compat design: accepts both "sofia" and "pjsip" stack names; drop-in for EndpointManager |
| PJMIG-05 | Cargo.toml feature flag migration (carrier = pjsip instead of sofia-sip) | Blast radius confirmed: only 3 src/ files reference sofia_sip; clean swap |
| PJMIG-06 | Integration tests verifying both UAS and UAC legs through pjsip | Smoke test in crates/pjsip/tests/smoke.rs; integration tests require local SIP infra |
</phase_requirements>

---

## Summary

This phase replaces the sofia-sip C FFI layer (carrier feature) with pjproject 2.14.x (pjsip). The architectural pattern is identical to the existing sofia-sip bridge: a dedicated OS thread runs the pjsip event loop, tokio mpsc channels carry events/commands across the thread boundary, and per-call event isolation is achieved via a CALL_REGISTRY HashMap.

The primary technical risk is the bindgen step: pjsip headers expose structs (not opaque), and some contain bitfields and platform-conditional types that can cause layout issues. The design doc addresses this by not using `.opaque_type(".*")` and by targeting a specific allowlist. The second major risk is pjsip's strict thread-affinity requirement — ALL pjsip API calls must happen on the thread that called `pj_init()`, enforced by the bridge serialization pattern.

The full implementation is already pre-specified in `docs/plans/2026-04-01-pjsip-migration.md` which provides complete, compilable code for every file. Research confirms the design decisions are sound and identifies a small set of known pitfalls to check during implementation.

**Primary recommendation:** Follow the design doc exactly. The blast radius is narrow (3 src/ files reference sofia-sip), the new code is additive until the final Cargo.toml swap, and sofia-sip crates remain in workspace for rollback.

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| pjproject | 2.14.1 | SIP stack (pjsip + pjlib + pjsip-ua) | Built-in RFC 4028/3262/3263/3311/3891; actively maintained; struct-based API maps cleanly through bindgen |
| bindgen | 0.71 | Generate Rust FFI from pjsip C headers | Same version used in existing sofia-sip-sys/build.rs; 0.71 is current stable |
| pkg-config | 0.3 | Locate pjproject at build time | Same pattern as sofia-sip-sys; pjproject 2.14 ships with `libpjproject.pc` |
| once_cell | 1 | Lazy static CALL_REGISTRY | Preferred over std::sync::OnceLock for Mutex<HashMap> pattern |
| uuid | 1 (v4 feature) | Fallback call_id generation | When dialog call_id extraction fails |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| libc | 0.2 | C types (sockaddr_in, AF_INET, etc.) | Transport binding, pjsip struct interop |
| tokio::sync::mpsc | (via tokio 1) | Command/event channels | Bridge between OS thread and async Rust |
| anyhow | 1 | Error propagation | All Result-returning functions in pjsip crate |
| tracing | 0.1 | Structured logging | pjsip thread diagnostics |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| once_cell::Lazy | std::sync::OnceLock + LazyLock (Rust 1.80+) | Design doc uses once_cell; consistent with existing codebase patterns |
| Global CALL_REGISTRY | mod_data[] slots on pjsip_inv_session | mod_data is the "pjsip way" but requires unsafe pointer casts; HashMap with call_id string is simpler to audit |

**Installation:**
```bash
# pjproject (system dependency — must be done before cargo build)
chmod +x scripts/install-pjproject.sh && bash scripts/install-pjproject.sh

# Rust crate deps added to Cargo.toml (no uv/pip — this is a Rust project)
# pjsip-sys and pjsip are local workspace crates, not registry deps
```

---

## Architecture Patterns

### Recommended Project Structure
```
crates/
├── pjsip-sys/           # Raw bindgen FFI over pjproject headers
│   ├── Cargo.toml       # links = "pjsip"; build-deps: bindgen, pkg-config
│   ├── pjsip_wrapper.h  # Includes only B2BUA-relevant pjsip headers
│   ├── build.rs         # pkg-config probe + bindgen generation
│   └── src/lib.rs       # include!(concat!(env!("OUT_DIR"), "/bindings.rs"))
└── pjsip/               # Safe Rust wrapper
    ├── Cargo.toml
    └── src/
        ├── lib.rs        # pub re-exports
        ├── error.rs      # PjStatus, check_status()
        ├── pool.rs       # CachingPool, Pool (pj_pool_t wrapper)
        ├── endpoint.rs   # PjEndpoint (pjsip_endpoint lifecycle + transport + modules)
        ├── event.rs      # PjCallEvent enum, PjCallEventSender/Receiver type aliases
        ├── command.rs    # PjCommand enum, PjCredential
        ├── session.rs    # CALL_REGISTRY, CallEntry, PjInvSession
        └── bridge.rs     # PjBridge (OS thread + callbacks + command dispatch)

src/
├── endpoint/
│   ├── mod.rs            # swap sofia_endpoint -> pjsip_endpoint under carrier feature
│   ├── manager.rs        # add "sofia"|"pjsip" => PjsipEndpoint arm
│   └── pjsip_endpoint.rs # NEW: SipEndpoint impl wrapping PjBridge
└── proxy/
    ├── pj_dialog_layer.rs # NEW: adapter — create_invite(), send_bye(), respond()
    └── pj_failover.rs     # NEW: PjFailoverLoop (mirrors failover.rs with PjCallEvent)
```

### Pattern 1: OS Thread Bridge (same as sofia-sip)
**What:** Dedicated std::thread runs the pjsip C event loop; tokio channels provide async interop
**When to use:** Mandatory — pjsip is not async-safe and has thread-affinity requirements
**Example:**
```rust
// Source: crates/sofia-sip/src/bridge.rs (replicated in pjsip/src/bridge.rs)
// Thread runs tight loop: handle_events(5ms) then drain cmd_rx
loop {
    endpoint.handle_events(5)?;  // 5ms max timeout
    loop {
        match cmd_rx.try_recv() {
            Ok(cmd) => handle_command(cmd, &endpoint, &mut shutting_down),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => { shutting_down = true; break; }
        }
    }
    if shutting_down { break; }
}
```

### Pattern 2: Per-Call Event Isolation via CALL_REGISTRY
**What:** Global HashMap<call_id, CallEntry> stores per-call event senders; callbacks look up and deliver
**When to use:** Every incoming/outgoing INVITE — replaces rsipstack's DialogStateReceiver
**Example:**
```rust
// Source: docs/plans/2026-04-01-pjsip-migration.md, bridge.rs on_inv_state_changed
pub(crate) static CALL_REGISTRY: Lazy<Mutex<HashMap<String, CallEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// In callback (pjsip OS thread):
let event_tx = {
    let registry = CALL_REGISTRY.lock().unwrap();
    registry.get(&call_id).map(|e| e.event_tx.clone())
};
if let Some(tx) = event_tx {
    let _ = tx.send(PjCallEvent::Confirmed { sdp });
}
```

### Pattern 3: UAC INVITE Flow (6 steps)
**What:** Create dialog -> create inv_session -> init timer -> create INVITE tdata -> add headers -> send
**When to use:** Every outbound INVITE from PjFailoverLoop
```rust
// Source: docs/plans/2026-04-01-pjsip-migration.md, create_outbound_invite
// 1. pjsip_dlg_create_uac(ua_instance, &from, &contact, &to, &target, &mut dlg)
// 2. pjsip_auth_clt_set_credentials(&mut dlg.auth_sess, 1, &cred_info) [if auth]
// 3. pjmedia_sdp_parse(pool, sdp_cstr, len, &mut sdp_session)
// 4. pjsip_inv_create_uac(dlg, sdp_session, 0, &mut inv)
// 5. pjsip_timer_init_session(inv, &timer_setting)
// 6. pjsip_inv_invite(inv, &mut tdata) -> add headers -> pjsip_inv_send_msg(inv, tdata)
```

### Pattern 4: UAS Response Flow
**What:** Respond to incoming INVITE with pjsip_inv_answer + pjsip_inv_send_msg
**When to use:** PjDialogLayer::respond() — sending 180/200/4xx to inbound leg
```rust
// Source: docs/plans/2026-04-01-pjsip-migration.md, respond_to_invite
pjsip_inv_answer(inv, status_code as i32, ptr::null(), ptr::null_mut(), &mut tdata)
pjsip_inv_send_msg(inv, tdata)
```

### Anti-Patterns to Avoid

- **Calling pjsip APIs from Tokio tasks:** pjsip requires all calls on the thread that called `pj_init()`. All API calls MUST go through the command channel to the pjsip OS thread.
- **Reusing sofia-sip `.opaque_type(".*")`:** pjsip bindgen must NOT use this; struct field access is required for reading call_id, SDP, cause code from inv_session.
- **Sharing pj_pool_t across calls:** Each allocation chain from a pool is freed atomically on pool release. Using the endpoint's main pool for per-call allocations causes memory growth. For production, consider per-call pools (complexity tradeoff vs initial 4096/4096 sizing).
- **Ignoring PJ_ETIMEDOUT (120004) from handle_events:** This is not an error — the timeout window elapsed with no events. Treating it as an error causes the event loop to exit prematurely.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Auth challenge/response | Custom Digest handler | `pjsip_auth_clt_set_credentials` + pjsip auto-retry | pjsip handles 401/407 challenge-response transparently when credentials are set on the dialog |
| Session timer refresh | Custom timer logic | `pjsip_timer_init_module` + `pjsip_timer_init_session` | pjsip sends re-INVITE/UPDATE for session refresh automatically |
| PRACK sequencing | Custom 100rel state machine | `pjsip_100rel_init_module` | RFC 3262 PRACK sequencing is complex; pjsip handles retransmission and ordering |
| SDP parsing | Custom SDP parser | `pjmedia_sdp_parse` + `pjmedia_sdp_neg_*` | pjsip's SDP negotiator (pjmedia_sdp_neg) handles offer/answer model |
| pkg-config detection | Custom header search | `pkg_config::Config::new().probe("libpjproject")` | pjproject 2.14 ships `libpjproject.pc`; pkg-config handles lib/include paths correctly |
| DNS NAPTR/SRV resolution | Custom DNS resolver | `pjlib-util/resolver.h` + `pjsip_endpt_resolve` | pjsip resolves NAPTR/SRV records per RFC 3263 when target is a domain name |

**Key insight:** pjsip's module system handles the hard parts (auth retry, session refresh, PRACK) automatically once initialized. The Rust layer only needs to start modules and set configuration — pjsip drives the state machines.

---

## Common Pitfalls

### Pitfall 1: pj_str_t Lifetime Dangling
**What goes wrong:** `pj_str(ptr)` creates a `pj_str_t` that holds a raw pointer to the original CString. If the CString is dropped before the pjsip call completes, the pointer dangles.
**Why it happens:** `pj_str_t` has `ptr: *mut c_char` — it does NOT own the memory. It is a view.
**How to avoid:** Keep all `CString` values alive in the same scope as the pjsip call that uses them. Pattern: declare all CStrings at the top of the function, then create pj_str_t values from them, then make pjsip calls.
**Warning signs:** Segfaults or corrupted SIP headers on the wire.

### Pitfall 2: bindgen Bitfield Layout Mismatches
**What goes wrong:** Some pjsip structs contain bitfields (e.g., `pjsip_inv_session.options`). bindgen generates `__bindgen_bitfield_unit` wrappers that may not match C layout on all targets.
**Why it happens:** C bitfield layout is implementation-defined; clang's layout may differ from GCC.
**How to avoid:** Build pjproject with the same compiler/flags that clang (used by bindgen) expects. On macOS use Homebrew's clang. On Linux use system clang matching GCC ABI. Run `cargo test -p pjsip-sys` with a layout verification test.
**Warning signs:** Compiler warnings about `improper_ctypes`; incorrect callback values.

### Pitfall 3: pjsip Thread-Affinity Violation
**What goes wrong:** Calling `pjsip_endpt_handle_events` or any pjsip API from a non-pjsip thread causes assertion failures or memory corruption.
**Why it happens:** pjsip uses thread-local pools and asserts thread identity in debug builds.
**How to avoid:** All pjsip API calls go through the command channel. The bridge thread is the ONLY thread that touches pjsip objects. `CALL_REGISTRY` is accessed only from the pjsip thread (Mutex guards the data; never hold the lock across an await).
**Warning signs:** `PJ_ASSERT_RETURN` failures; random crashes under load.

### Pitfall 4: CALL_REGISTRY Mutex Deadlock from Callback
**What goes wrong:** `on_inv_state_changed` is called from within `pjsip_endpt_handle_events`. If the callback tries to lock CALL_REGISTRY and the command handler is also holding it, deadlock occurs.
**Why it happens:** Both the event loop and the command handler (same thread) may access CALL_REGISTRY. Since it's a `std::sync::Mutex` (not re-entrant), this deadlocks.
**How to avoid:** In `handle_command`, always drop the CALL_REGISTRY lock before calling any pjsip API that might trigger callbacks. Structure code as: lock, read/write registry, unlock, then call pjsip APIs.
**Warning signs:** Thread hangs during high call volume.

### Pitfall 5: `pjmedia_sdp_neg_get_active_remote` on Unconfirmed Sessions
**What goes wrong:** Calling the SDP negotiator before the offer/answer exchange is complete returns `PJ_EINVALIDOP`.
**Why it happens:** The SDP negotiator state machine must reach `PJMEDIA_SDP_NEG_STATE_DONE` before `get_active_remote` works.
**How to avoid:** Only extract SDP in `PJSIP_INV_STATE_CONFIRMED` or after PRACK acknowledgement. For `PJSIP_INV_STATE_EARLY` (183), use the message body from `pjsip_event->body.tsx_state.src.rdata` instead.
**Warning signs:** `pjmedia_sdp_parse` fails on early responses; early media not bridged.

### Pitfall 6: OpenSSL Path on macOS
**What goes wrong:** `./configure` fails to find OpenSSL when building pjproject on macOS because Homebrew installs it to a non-default location.
**Why it happens:** macOS ships LibreSSL at `/usr/lib`, but pjsip needs OpenSSL headers.
**How to avoid:** The install script already handles this with `--with-ssl=$(brew --prefix openssl) 2>/dev/null` fallback. Confirm: `pkg-config --modversion openssl` should return a version after brew install.
**Warning signs:** `configure: error: SSL not found` during pjproject build.

---

## Code Examples

Verified patterns from the design doc (all from `docs/plans/2026-04-01-pjsip-migration.md`):

### pjsip-sys build.rs skeleton
```rust
// Source: docs/plans/2026-04-01-pjsip-migration.md Task 1.2
let pjsip = pkg_config::Config::new()
    .atleast_version("2.14")
    .probe("libpjproject")
    .unwrap_or_else(|e| panic!("pjproject not found: {e}\nInstall: bash scripts/install-pjproject.sh"));

let mut builder = bindgen::Builder::default()
    .header(wrapper_path.to_str().unwrap())
    .allowlist_function("pjsip_endpt_.*")
    .allowlist_function("pjsip_inv_.*")
    .allowlist_function("pjsip_dlg_.*")
    .allowlist_type("pjsip_inv_session")
    .allowlist_type("pjsip_inv_state")
    // ... (full allowlist in design doc)
    .derive_debug(true)
    .derive_default(true);
// NOTE: No .opaque_type(".*") — struct fields must be accessible
```

### pjsip_inv_state constants (enum variant names after bindgen)
```rust
// Source: docs/plans/2026-04-01-pjsip-migration.md, on_inv_state_changed
// bindgen generates these as associated constants on pjsip_inv_state type:
pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_CALLING      // 1xx sent/received, no SDP
pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_EARLY         // provisional response
pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_CONFIRMED     // 200 OK + ACK
pjsip_sys::pjsip_inv_state_PJSIP_INV_STATE_DISCONNECTED  // BYE/CANCEL/error
```

### PjEndpointConfig default (Session Timers + PRACK enabled)
```rust
// Source: docs/plans/2026-04-01-pjsip-migration.md Task 2.3
PjEndpointConfig {
    bind_addr: "0.0.0.0".to_string(),
    port: 5060,
    transport: "udp".to_string(),
    session_timers: true,
    session_expires: 1800,  // 30 min
    min_se: 90,
    enable_100rel: true,
    ..Default::default()
}
```

### CachingPool + Pool pattern
```rust
// Source: docs/plans/2026-04-01-pjsip-migration.md Task 2.1
let mut cp: pjsip_sys::pj_caching_pool = unsafe { std::mem::zeroed() };
unsafe { pjsip_sys::pj_caching_pool_init(&mut cp, ptr::null(), 0) };
// Create per-call pool:
let pool = unsafe {
    pjsip_sys::pj_pool_create(factory_ptr, name.as_ptr(), 4096, 4096, ptr::null_mut())
};
// Release on drop:
// unsafe { pjsip_sys::pj_pool_release(pool) };
```

### Extract call_id from pjsip_inv_session
```rust
// Source: docs/plans/2026-04-01-pjsip-migration.md, extract_call_id
unsafe {
    let dlg = (*inv).dlg;
    let call_id_pj = &(*dlg).call_id;
    let slice = std::slice::from_raw_parts(
        call_id_pj.id.ptr as *const u8,
        call_id_pj.id.slen as usize,
    );
    String::from_utf8_lossy(slice).into_owned()
}
```

---

## Blast Radius Analysis

### Files referencing sofia-sip (must change)

**Source files (3 total):**
- `src/endpoint/sofia_endpoint.rs` — DELETE or replace with `pjsip_endpoint.rs`
- `src/endpoint/mod.rs` — swap `sofia_endpoint` module declaration and re-export
- `src/endpoint/manager.rs` — swap `SofiaEndpoint` import and match arm

**Cargo.toml (root):**
- Change `carrier = ["dep:sofia-sip", "dep:sofia-sip-sys", ...]` to `carrier = ["dep:pjsip", "dep:pjsip-sys", ...]`
- Add `pjsip` and `pjsip-sys` optional deps
- Keep `sofia-sip` and `sofia-sip-sys` optional but remove from carrier feature (keep for rollback)

**Files NOT changing (verified):**
- `src/proxy/dispatch.rs` — will add pjsip path but existing rsipstack path stays
- `src/proxy/failover.rs` — new `PjFailoverLoop` is parallel, not a replacement
- `src/proxy/session.rs` — will accept PjCallEvent via new session type
- `src/call/sip.rs` — `DialogStateReceiverGuard` stays for rsipstack path
- `src/app.rs` — adds `pj_bridge` field; keeps `dialog_layer` for rsipstack path

**New files (additive):**
- `crates/pjsip-sys/` (whole crate)
- `crates/pjsip/` (whole crate)
- `scripts/install-pjproject.sh`
- `src/endpoint/pjsip_endpoint.rs`
- `src/proxy/pj_dialog_layer.rs`
- `src/proxy/pj_failover.rs`

---

## State of the Art

| Old Approach | Current Approach | Impact |
|--------------|------------------|--------|
| `opaque_type(".*")` for sofia-sip | Named allowlist with struct field access for pjsip | pjsip requires struct field access for call_id, inv state, SDP negotiator |
| Variadic tag-list API (nua_invite, nua_respond) | Struct-based API (pjsip_inv_create_uac, pjsip_inv_answer) | Bindgen maps cleanly; no transmute hacks needed for function signatures |
| Global NuaAgent (single sofia instance) | PjBridge per endpoint (multi-instance safe) | pjsip allows multiple endpoints; future multi-port support becomes trivial |
| Session timer as stub/TODO | pjsip_timer_init_module as built-in | RFC 4028 works out of the box; no hand-rolling |

**Deprecated/outdated:**
- `SofiaEndpoint` — replaced by `PjsipEndpoint`; keep sofia crates in workspace only for rollback reference
- `SofiaCommand` / `SofiaEvent` — replaced by `PjCommand` / `PjCallEvent`; new types are richer (per-call isolation, EarlyMedia variant, ReInvite variant)

---

## Open Questions

1. **call_id passthrough for BYE after connect**
   - What we know: `PjFailoverLoop::wait_for_outcome` returns `call_id: String::new()` as a TODO in the design doc
   - What's unclear: How `PjCallEvent::Confirmed` carries the call_id back to the failover caller so BYE can be sent later
   - Recommendation: Add `call_id: String` field to `PjCallEvent::Confirmed` variant; populate from `extract_call_id(inv)` in the callback

2. **AppState bridge access for PjDialogLayer construction**
   - What we know: `PjsipEndpoint::bridge()` returns `Option<Arc<PjBridge>>`; AppState needs to hold this
   - What's unclear: The exact wiring path from endpoint start to `dispatch_proxy_call` receiving a `PjDialogLayer`
   - Recommendation: Add `pj_bridge: Option<Arc<PjBridge>>` to `AppStateInner` and populate after endpoint start in app.rs

3. **pjsip_inv_answer signature for UAS responses with SDP**
   - What we know: `pjsip_inv_answer(inv, code, NULL, NULL, &mut tdata)` works for code-only responses
   - What's unclear: The `local_sdp` parameter for 200 OK with SDP body — needs `pjmedia_sdp_session*` not a string
   - Recommendation: For UAS 200 OK with SDP: parse the SDP string first with `pjmedia_sdp_parse`, then pass the parsed struct as `local_sdp` to `pjsip_inv_answer`

4. **Thread registration for pjsip debug builds**
   - What we know: pjsip debug builds call `pj_thread_register` to track thread identity
   - What's unclear: Whether the std::thread-spawned pjsip thread needs explicit `pj_thread_register` before calling any pjsip API
   - Recommendation: Call `pj_thread_register("pjsip", desc, &mut thread)` at the start of `pjsip_thread_main` as the first thing after thread spawn, before `PjEndpoint::create`

---

## Sources

### Primary (HIGH confidence)
- `docs/plans/2026-04-01-pjsip-migration.md` — complete pre-specified implementation with verified code patterns for every file in this phase
- `crates/sofia-sip/src/bridge.rs` — existing bridge pattern replicated 1:1 for pjsip
- `crates/sofia-sip-sys/build.rs` — bindgen build.rs pattern; pjsip-sys deviates at opaque_type and allowlist
- `src/endpoint/mod.rs`, `manager.rs`, `sofia_endpoint.rs` — exact files requiring modification confirmed
- `src/proxy/failover.rs`, `session.rs`, `dispatch.rs` — files adapting to new PjFailoverLoop

### Secondary (MEDIUM confidence)
- `docs/plans/2026-04-01-pjsip-migration.md` pjsip inv_state constant names — validated against known pjsip 2.14 header conventions; exact bindgen enum variant names may have minor prefix differences

### Tertiary (LOW confidence)
- pj_thread_register requirement (Open Question 4) — documented in pjsip guides but not verified against 2.14.1 specifically in this research session

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — pjproject 2.14.1, bindgen 0.71, pkg-config 0.3 all confirmed in design doc
- Architecture: HIGH — full implementation is pre-specified; pattern mirrors existing sofia-sip bridge
- Pitfalls: HIGH for thread affinity, pj_str_t lifetime, PJ_ETIMEDOUT; MEDIUM for bitfield layout (platform-dependent)
- Blast radius: HIGH — only 3 src/ files reference sofia_sip; grep confirmed

**Research date:** 2026-04-01
**Valid until:** 2026-05-01 (pjproject 2.14.x API is stable; bindgen 0.71 is current)
