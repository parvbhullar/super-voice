# Carrier FFI Architecture & Usage Guide

## Overview

Super Voice Carrier Edition embeds two C libraries — **Sofia-SIP** and **SpanDSP** — directly into the Rust binary via FFI. This gives carrier-grade SIP signaling and telecom DSP processing without running separate processes.

```
┌─────────────────────────────────────────────────────────┐
│                 Super Voice Binary                      │
│                                                         │
│  ┌───────────────────────────────────────────────────┐  │
│  │              Rust Application Layer                │  │
│  │  AppState, ActiveCall, PlaybookRunner, HTTP API    │  │
│  └────────────┬──────────────────────┬───────────────┘  │
│               │                      │                   │
│  ┌────────────▼──────────┐  ┌───────▼───────────────┐  │
│  │     sofia-sip crate   │  │    spandsp crate      │  │
│  │   (safe Rust wrapper) │  │  (safe Rust wrapper)  │  │
│  │                       │  │                       │  │
│  │  NuaAgent             │  │  DtmfDetector         │  │
│  │  SofiaBridge          │  │  EchoCanceller        │  │
│  │  SofiaHandle          │  │  PlcProcessor         │  │
│  │  SofiaEvent           │  │  ToneDetector (stub)  │  │
│  │  SofiaCommand         │  │  FaxEngine (stub)     │  │
│  └────────────┬──────────┘  └───────┬───────────────┘  │
│               │                      │                   │
│  ┌────────────▼──────────┐  ┌───────▼───────────────┐  │
│  │   sofia-sip-sys crate │  │  spandsp-sys crate    │  │
│  │   (raw C FFI bindings)│  │  (raw C FFI bindings) │  │
│  │   bindgen + pkg-config│  │  bindgen + pkg-config │  │
│  └────────────┬──────────┘  └───────┬───────────────┘  │
│               │                      │                   │
│  ─────────────┼──────────────────────┼──── C boundary ─  │
│               │                      │                   │
│  ┌────────────▼──────────┐  ┌───────▼───────────────┐  │
│  │  libsofia-sip-ua.so   │  │   libspandsp.so       │  │
│  │  (system library)     │  │   (system library)    │  │
│  └───────────────────────┘  └───────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

## Crate Structure

```
super-voice/
├── Cargo.toml              # Workspace root + active-call package
├── crates/
│   ├── sofia-sip-sys/      # Raw FFI bindings (bindgen-generated)
│   │   ├── Cargo.toml
│   │   ├── build.rs        # pkg-config + bindgen
│   │   └── src/lib.rs      # Generated bindings re-export
│   │
│   ├── sofia-sip/          # Safe Rust wrapper
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs      # Public API
│   │       ├── event.rs    # SofiaEvent (5 variants)
│   │       ├── command.rs  # SofiaCommand (6 variants)
│   │       ├── handle.rs   # SofiaHandle (ref-counted C pointer)
│   │       ├── root.rs     # SuRoot (event loop wrapper)
│   │       ├── bridge.rs   # SofiaBridge (OS thread + channels)
│   │       └── agent.rs    # NuaAgent (high-level API)
│   │
│   ├── spandsp-sys/        # Raw FFI bindings (bindgen-generated)
│   │   ├── Cargo.toml
│   │   ├── build.rs        # pkg-config + bindgen
│   │   └── src/lib.rs
│   │
│   └── spandsp/            # Safe Rust wrapper
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs      # Public API
│           ├── dtmf.rs     # DtmfDetector
│           ├── echo.rs     # EchoCanceller
│           ├── plc.rs      # PlcProcessor
│           ├── tone.rs     # ToneDetector (stub, Phase 10)
│           └── fax.rs      # FaxEngine (stub, Phase 10)
│
├── src/
│   ├── media/
│   │   ├── engine.rs       # StreamEngine with register_processor()
│   │   ├── spandsp_adapters.rs  # Processor trait adapters (16kHz↔8kHz)
│   │   └── ...
│   └── ...
├── tests/
│   └── carrier_integration.rs   # 4 integration tests
├── scripts/
│   └── check_startup.sh         # Startup time validation
└── Dockerfile.carrier            # Multi-stage Docker build
```

## Feature Flags

```toml
[features]
default = ["carrier"]
carrier = ["sofia-sip", "spandsp"]   # C FFI carrier features
minimal = []                          # Pure Rust, no C dependencies
offline = ["ort", "hf-hub", ...]     # ONNX offline models (unchanged)
```

**Build commands:**

```bash
# Full carrier build (requires libsofia-sip-ua-dev + libspandsp-dev)
cargo build --features carrier

# Pure Rust build (no C dependencies)
cargo build --no-default-features

# Check both paths compile
cargo check --features carrier
cargo check --no-default-features
```

## Sofia-SIP Integration

### Architecture: Dedicated Thread + Channel Bridge

Sofia-SIP has its own single-threaded event loop (`su_root_step`). It cannot run inside Tokio. The integration uses a dedicated OS thread with two mpsc channels:

```
  Tokio async world              Sofia-SIP OS thread
  ─────────────────              ───────────────────
                                 ┌──────────────────────┐
  agent.next_event() ◄────────── │ su_root_step(root, 1)│
    (event_rx.recv)              │         │             │
                                 │    C callback fires   │
                                 │         │             │
                                 │  SofiaEvent created   │
                                 │         │             │
                                 │  event_tx.send(event) │
                                 │         │             │
  agent.respond(200) ──────────► │  cmd_rx.try_recv()    │
    (cmd_tx.send)                │         │             │
                                 │  nua_respond(nh, 200) │
                                 └──────────────────────┘
```

**Why a dedicated thread (not `spawn_blocking`):**
- `su_root_step()` must be called continuously with 1ms timeout
- `spawn_blocking` would monopolize a Tokio blocking thread permanently
- A named OS thread (`"sofia-sip"`) is cleaner and doesn't affect Tokio's thread pool

### Key Types

#### SofiaHandle

Wraps `*mut nua_handle_t` — a per-dialog reference-counted C pointer.

```rust
use sofia_sip::SofiaHandle;

// Clone increments C ref count (nua_handle_ref)
let handle2 = handle.clone();

// Drop decrements C ref count (nua_handle_unref)
drop(handle2);

// Safe to send across threads (only dereferenced on Sofia thread)
tokio::spawn(async move {
    agent.respond(&handle, 200, "OK")?;
});
```

#### SofiaEvent

Five variants, matching the CONTEXT.md locked spec:

```rust
pub enum SofiaEvent {
    IncomingInvite { handle: SofiaHandle, from: String, to: String, sdp: Option<String> },
    IncomingRegister { handle: SofiaHandle, contact: String },
    InviteResponse { handle: SofiaHandle, status: u16, phrase: String, sdp: Option<String> },
    Terminated { handle: SofiaHandle, reason: String },
    Info { handle: SofiaHandle, content_type: String, body: String },
}
```

`nua_r_options` responses are mapped to `InviteResponse` (no separate variant).

#### SofiaCommand

```rust
pub enum SofiaCommand {
    Respond { handle: SofiaHandle, status: u16, reason: String, sdp: Option<String> },
    Invite { handle: SofiaHandle, uri: String, sdp: String },
    Register { handle: SofiaHandle, registrar: String },
    Bye { handle: SofiaHandle },
    Options { uri: String },  // Handle created internally on Sofia thread
    Shutdown,
}
```

#### NuaAgent — High-Level API

```rust
use sofia_sip::{NuaAgent, SofiaEvent};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut agent = NuaAgent::new("sip:*:5060")?;

    while let Some(event) = agent.next_event().await {
        match event {
            SofiaEvent::IncomingInvite { handle, from, to, sdp } => {
                println!("INVITE from {from} to {to}");
                agent.respond(&handle, 180, "Ringing")?;
                agent.respond(&handle, 200, "OK")?;
            }
            SofiaEvent::Terminated { handle, reason } => {
                println!("Call terminated: {reason}");
            }
            _ => {}
        }
    }
    Ok(())
}
```

### Memory Safety Model

| C Resource | Rust Wrapper | Lifecycle |
|---|---|---|
| `su_root_t*` | `SuRoot` | Drop calls `su_root_destroy()` |
| `nua_t*` | Managed by `SofiaBridge` | Drop calls `nua_shutdown()` → polls until 200 → `nua_destroy()` |
| `nua_handle_t*` | `SofiaHandle` | Clone = `nua_handle_ref()`, Drop = `nua_handle_unref()` |
| Callback state | `Box<CallbackState>` | Leaked via `Box::into_raw`, reclaimed after event loop exits |

**No raw pointers escape the wrapper crate.** All C memory is tied to Rust Drop impls.

### C Callback Trampoline

The Sofia-SIP NUA callback is an `extern "C"` function registered with `nua_create`:

```rust
extern "C" fn sofia_event_trampoline(
    event: nua_event_e,
    status: c_int,
    phrase: *const c_char,
    _nua: *mut nua_t,
    magic: *mut nua_magic_t,      // our CallbackState*
    nh: *mut nua_handle_t,
    _hmagic: *mut nua_hmagic_t,
    _sip: *const sip_t,
    _tags: *mut tagi_t,
) {
    // 1. Cast magic back to &CallbackState
    // 2. Copy all C strings into Rust-owned Strings
    // 3. Ref-count the handle via nua_handle_ref()
    // 4. Send SofiaEvent through event_tx channel
}
```

**Key safety properties:**
- C data is copied immediately (no dangling references)
- Handle ref count is incremented before send (survives callback return)
- `event_tx.send()` is non-blocking (unbounded channel)
- Trampoline never blocks (would stall Sofia event loop)

### Sofia-SIP Tag System

Sofia-SIP uses variadic tag-list arguments (like `nua_create(root, callback, magic, NUTAG_URL(url), TAG_END())`). In Rust FFI:

```rust
// Tag type = address of global descriptor variable
fn nutag_url_type() -> tag_type_t {
    unsafe extern "C" {
        static mut nutag_url: tag_typedef_t;
    }
    (&raw mut nutag_url) as tag_type_t
}

// Tag value = pointer cast to usize
let tag_value = url_cstr.as_ptr() as tag_value_t;

// TAG_END = (0, 0) sentinel
const TAG_END_TYPE: tag_type_t = 0;
const TAG_END_VALUE: tag_value_t = 0;
```

## SpanDSP Integration

### Architecture: Frame-Based Processors

SpanDSP is simpler than Sofia-SIP — no event loop, no threading. It processes audio frames synchronously:

```
Audio Pipeline (16kHz)          SpanDSP (8kHz)
────────────────────            ──────────────

AudioFrame (16kHz PCM)
       │
  downsample 16k→8k
       │
       ├──► DtmfDetector.process_audio(8kHz samples)
       │         └──► callback fires on digit detect
       │
       ├──► EchoCanceller.process_audio(tx, rx)
       │         └──► modifies rx buffer in-place
       │
       └──► PlcProcessor.process_good_frame(samples)
                 └──► updates prediction model
       │
  upsample 8k→16k
       │
AudioFrame (16kHz PCM, modified)
```

### Key Types

#### DtmfDetector

```rust
use spandsp::DtmfDetector;

let mut detector = DtmfDetector::new()?;

// Feed 8kHz PCM samples (20ms = 160 samples)
detector.process_audio(&samples_8khz)?;

// Retrieve detected digits
let digits: Vec<char> = detector.get_digits();
// e.g., ['1', '2', '#']
```

**How DTMF detection works internally:**
1. SpanDSP uses Goertzel filters to detect dual-tone frequencies
2. C callback fires when a digit is detected
3. Rust trampoline (`extern "C" fn dtmf_callback`) accumulates digits in a `Vec<char>`
4. `get_digits()` drains the accumulated digits

#### EchoCanceller

```rust
use spandsp::EchoCanceller;

let mut aec = EchoCanceller::new(128)?;  // 128-sample tail length

// tx = near-end (what we send), rx = far-end (what we receive)
let tx: Vec<i16> = /* microphone signal at 8kHz */;
let mut rx: Vec<i16> = /* speaker signal at 8kHz */;

aec.process_audio(&tx, &mut rx)?;
// rx is now echo-cancelled
```

#### PlcProcessor

```rust
use spandsp::PlcProcessor;

let mut plc = PlcProcessor::new()?;

// Feed good frames to build prediction model
plc.process_good_frame(&mut samples_8khz)?;

// On packet loss, generate concealment samples
let mut concealed = vec![0i16; 160];
plc.fill_missing(&mut concealed)?;
```

### StreamEngine Integration

SpanDSP processors are registered in `StreamEngine` as named factories:

```rust
// In StreamEngine::default() (gated behind #[cfg(feature = "carrier")])
engine.register_processor("spandsp_dtmf", SpanDspDtmfDetector::create);
engine.register_processor("spandsp_echo", SpanDspEchoCancelProcessor::create);
engine.register_processor("spandsp_plc", SpanDspPlcProcessor::create);

// Later, during call setup:
let dtmf_proc = engine.create_processor("spandsp_dtmf")?;
processor_chain.add(dtmf_proc);
```

### Adapter Pattern (16kHz ↔ 8kHz)

The active-call pipeline operates at 16kHz (`INTERNAL_SAMPLERATE`). SpanDSP operates at 8kHz (G.711 rate). Adapter types handle resampling:

```rust
// src/media/spandsp_adapters.rs

impl Processor for SpanDspDtmfDetector {
    fn process_frame(&mut self, frame: &mut AudioFrame) -> Result<()> {
        if let Samples::PCM { samples } = &frame.samples {
            let downsampled = downsample_16k_to_8k(samples);  // drop every other sample
            self.inner.process_audio(&downsampled)?;
            let digits = self.inner.get_digits();
            if !digits.is_empty() {
                debug!(digits = ?digits, "SpanDSP DTMF detected");
            }
        }
        Ok(())
    }
}
```

**Resampling methods:**
- `downsample_16k_to_8k`: Drop every other sample (simple decimation)
- `upsample_8k_to_16k`: Linear interpolation between consecutive samples

### Per-Call Memory

| Processor | C State Size | Rust Overhead | Total |
|---|---|---|---|
| DtmfDetector | ~500 bytes | ~100 bytes (Vec, Box) | ~600 bytes |
| EchoCanceller | ~2 KB | ~50 bytes | ~2 KB |
| PlcProcessor | ~512 bytes | ~50 bytes | ~600 bytes |
| ToneDetector | stub (0) | ~50 bytes | ~50 bytes |
| FaxEngine | stub (0) | ~50 bytes | ~50 bytes |
| **Total per call** | | | **~3.3 KB** |

## Build System

### How -sys Crates Work

Each `-sys` crate has a `build.rs` that:
1. Uses `pkg-config` to find the C library headers and link flags
2. Runs `bindgen` to generate Rust FFI bindings from C headers
3. Outputs `bindings.rs` to `$OUT_DIR`

```rust
// crates/sofia-sip-sys/build.rs (simplified)
fn main() {
    let lib = pkg_config::probe_library("sofia-sip-ua")
        .expect("Sofia-SIP not found. Install: apt install libsofia-sip-ua-dev");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")                        // includes nua.h, sdp.h, etc.
        .allowlist_function("nua_.*|su_root_.*")    // minimal surface
        .opaque_type(".*")                          // treat all structs as opaque
        .generate()
        .expect("bindgen failed");

    bindings.write_to_file(out_dir.join("bindings.rs")).unwrap();
}
```

### System Dependencies

**Debian/Ubuntu:**
```bash
apt install libsofia-sip-ua-dev libspandsp-dev libtiff-dev clang libclang-dev
```

**macOS (Homebrew):**
```bash
brew install sofia-sip spandsp
```

**If packages are unavailable (Docker):**
Build from source — see `Dockerfile.carrier` stages `sofia-builder` and `spandsp-builder`.

### Docker Build

```dockerfile
# Stage 1: Build Sofia-SIP from source
FROM debian:bookworm AS sofia-builder
RUN git clone --branch rel-1-13-17 https://github.com/freeswitch/sofia-sip.git
RUN cd sofia-sip && ./bootstrap.sh && ./configure && make && make install

# Stage 2: Build SpanDSP from source
FROM debian:bookworm AS spandsp-builder
RUN git clone https://github.com/freeswitch/spandsp.git
RUN cd spandsp && ./bootstrap.sh && ./configure && make && make install

# Stage 3: Rust build
FROM rust:1.82-bookworm AS builder
COPY --from=sofia-builder /usr/local/lib/ /usr/local/lib/
COPY --from=sofia-builder /usr/local/include/ /usr/local/include/
COPY --from=spandsp-builder /usr/local/lib/ /usr/local/lib/
COPY --from=spandsp-builder /usr/local/include/ /usr/local/include/
RUN cargo build --release --features carrier

# Stage 4: Runtime
FROM debian:bookworm-slim
COPY --from=builder /app/target/release/active-call /usr/local/bin/
COPY --from=sofia-builder /usr/local/lib/libsofia*.so* /usr/local/lib/
COPY --from=spandsp-builder /usr/local/lib/libspandsp*.so* /usr/local/lib/
ENTRYPOINT ["active-call"]
```

```bash
docker build -f Dockerfile.carrier -t active-call:carrier .
docker run --net host active-call:carrier --config config.toml
```

## Testing

### Integration Tests

```bash
# Run all carrier integration tests (requires Sofia-SIP + SpanDSP installed)
cargo test --features carrier --test carrier_integration -- --nocapture
```

**Tests included:**
1. `test_sofia_agent_start_shutdown` — NuaAgent starts event loop, sends OPTIONS to self, shuts down cleanly
2. `test_spandsp_dtmf_detects_digit` — Generates 697Hz+1209Hz dual-tone (DTMF "1"), verifies SpanDSP detects it
3. `test_spandsp_dtmf_silent_frame` — Feeds silence, verifies no false positive digits
4. `test_both_stacks_coexist` — Runs rsipstack endpoint + SpanDSP DTMF in same binary without conflict

### Startup Validation

```bash
bash scripts/check_startup.sh target/release/active-call
# Output: PASS: Startup time 12ms is under 1000ms limit
```

### Unit Tests

```bash
# SpanDSP wrapper tests
cargo test -p spandsp --features carrier

# Sofia-SIP wrapper tests (requires system library)
cargo test -p sofia-sip --features carrier
```

## Phase Status & Next Steps

**Phase 1 complete.** All 9 requirements delivered:

| Requirement | Status | What |
|---|---|---|
| FFND-01 | Done | Sofia-SIP C FFI bindings via bindgen |
| FFND-02 | Done | Sofia-SIP Tokio bridge (dedicated thread + mpsc) |
| FFND-03 | Done | SpanDSP C FFI bindings via bindgen |
| FFND-04 | Done | SpanDSP processors in StreamEngine registry |
| FFND-05 | Done | pkg-config discovery with feature-flag gating |
| BLDP-01 | Done | Cargo workspace with 4 crates |
| BLDP-02 | Done | carrier/minimal feature flags |
| BLDP-03 | Done | Docker multi-stage build |
| BLDP-04 | Done | Binary starts in 12ms (<1s limit) |

**Next: Phase 2 — Redis State Layer** (`/gsd:plan-phase 2`)
- All dynamic config in Redis
- Pub/sub for config propagation
- Engagement tracking for safe deletion
- API key authentication
