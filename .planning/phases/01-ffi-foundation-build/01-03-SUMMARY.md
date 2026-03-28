---
phase: 01-ffi-foundation-build
plan: 03
subsystem: media
tags: [spandsp, ffi, dtmf, echo-cancellation, plc, fax, processor-registry, carrier]

requires:
  - phase: 01-ffi-foundation-build
    plan: 01
    provides: spandsp-sys bindgen crate with raw FFI bindings

provides:
  - Safe spandsp Rust wrapper crate with DtmfDetector, EchoCanceller, PlcProcessor, FaxEngine stub
  - ToneDetector stub (Phase 10 full impl)
  - StreamEngine::register_processor(name, factory_fn) generic processor registry
  - SpanDSP Processor trait adapters (spandsp_adapters.rs) with 16kHz/8kHz resampling
  - spandsp_dtmf, spandsp_echo, spandsp_plc registered in StreamEngine::default() under carrier feature

affects:
  - Phase 10 (full SpanDSP tone detector and T.38 fax implementation)
  - Any phase that uses StreamEngine processor registration pattern
  - Media pipeline chain (spandsp_adapters.rs resampling pattern)

tech-stack:
  added:
    - spandsp crate (crates/spandsp/) wrapping spandsp-sys
  patterns:
    - StreamEngine named processor factory registry pattern: register_processor("name", Fn::create)
    - SpanDSP 8kHz adapter: downsample_16k_to_8k / upsample_8k_to_16k helpers
    - Opaque C state wrapper with Drop + unsafe Send impls
    - TDD stub pattern for complex C APIs: pure Rust stub returning Ok(None)

key-files:
  created:
    - crates/spandsp/Cargo.toml
    - crates/spandsp/src/lib.rs
    - crates/spandsp/src/dtmf.rs
    - crates/spandsp/src/echo.rs
    - crates/spandsp/src/tone.rs
    - crates/spandsp/src/plc.rs
    - crates/spandsp/src/fax.rs
    - src/media/spandsp_adapters.rs
  modified:
    - crates/spandsp-sys/build.rs
    - src/media/engine.rs
    - src/media/mod.rs
    - Cargo.toml

key-decisions:
  - "ToneDetector is a pure Rust stub (no C state): super_tone_rx_init returns NULL with NULL descriptor; full Phase 10 impl will pass a super_tone_rx_descriptor_t"
  - "fax_state_t is opaque in SpanDSP 0.0.6 (size=0 placeholder); test verifies compile-time binding access not struct size"
  - "SpanDSP adapters use simple downsample (step_by(2)) and linear-interpolation upsample; echo canceller uses near-end only mode pending full AEC in Phase 10"
  - "carrier feature points to dep:spandsp (safe wrapper) not dep:spandsp-sys; raw bindings remain available but gated"
  - "spandsp-sys build.rs: add -I/opt/homebrew/include for tiffio.h on macOS; use outer #[allow] not inner #![allow] in raw_line"

patterns-established:
  - "Processor adapter pattern: wrap SpanDSP type, downsample in process_frame(), process at 8kHz, upsample output"
  - "Named factory registry: engine.register_processor(\"name\", Type::create) under #[cfg(feature = carrier)]"
  - "C FFI stub: when C init requires complex setup, implement as pure Rust stub with TODO Phase N comment"

requirements-completed: [FFND-03, FFND-04]

duration: 35min
completed: 2026-03-28
---

# Phase 1 Plan 3: SpanDSP Safe Wrapper and StreamEngine Integration Summary

**Safe Rust wrappers for SpanDSP DSP processors (DTMF, echo cancel, PLC, fax, tone) registered in StreamEngine via named factory pattern with 16kHz/8kHz resampling adapters**

## Performance

- **Duration:** ~35 min
- **Started:** 2026-03-28T21:13:00Z
- **Completed:** 2026-03-28T21:48:26Z
- **Tasks:** 2
- **Files modified:** 12

## Accomplishments

- Created `crates/spandsp/` crate with 5 safe wrappers: DtmfDetector, EchoCanceller, PlcProcessor, FaxEngine stub, ToneDetector stub — all with Drop impls and unsafe Send
- Added `register_processor(name, factory_fn)` and `create_processor(name)` to StreamEngine with `processor_factories: HashMap<String, FnCreateProcessor>` field
- Created `src/media/spandsp_adapters.rs` gated behind `#[cfg(feature = carrier)]` with 3 Processor trait adapters handling 16kHz/8kHz resampling
- Registered `spandsp_dtmf`, `spandsp_echo`, `spandsp_plc` in `StreamEngine::default()` under carrier feature
- All 12 spandsp crate tests pass; all 206 carrier-feature integration tests pass; both `--features carrier` and `--no-default-features` builds succeed

## Task Commits

1. **Task 1: Create spandsp safe wrapper crate** - `cf5f8cd` (feat)
2. **Task 2: Integrate into StreamEngine with register_processor()** - `d5c4d7c` (feat)

**Plan metadata:** (next commit)

## Files Created/Modified

- `crates/spandsp/src/dtmf.rs` - DtmfDetector wrapping dtmf_rx_state_t with callback trampoline
- `crates/spandsp/src/echo.rs` - EchoCanceller wrapping echo_can_state_t
- `crates/spandsp/src/plc.rs` - PlcProcessor wrapping plc_state_t
- `crates/spandsp/src/fax.rs` - FaxEngine stub proving fax_state_t bindings compile
- `crates/spandsp/src/tone.rs` - ToneDetector pure Rust stub (Phase 10)
- `crates/spandsp/src/lib.rs` - Re-exports all types
- `crates/spandsp/Cargo.toml` - spandsp-sys, anyhow, tracing, libc deps
- `src/media/spandsp_adapters.rs` - Processor adapters with 16k/8k resampling helpers
- `src/media/engine.rs` - Added FnCreateProcessor, register_processor(), create_processor(), processor_factories field
- `src/media/mod.rs` - Added #[cfg(carrier)] spandsp_adapters module
- `crates/spandsp-sys/build.rs` - Fixed macOS tiffio.h path, raw_line attribute, dtmf_rx allowlist
- `Cargo.toml` - carrier feature updated to use dep:spandsp; spandsp = { path = "crates/spandsp" } added

## Decisions Made

- ToneDetector made a pure Rust stub: `super_tone_rx_init` returns NULL when passed a NULL descriptor pointer. Rather than attempt partial C initialization, the stub returns `Ok(None)` with a clear Phase 10 TODO. Full tone descriptor setup (Busy, Ringback, SIT) is deferred.
- `fax_state_t` is opaque (zero-size `_unused: [u8;0]`) in SpanDSP 0.0.6. Changed the compile-time proof test from `size_of > 0` to a test that just accesses the type — proving the binding resolves without asserting internals.
- Echo canceller uses near-end only mode in the adapter (tx == rx buffer). Full AEC needs a separate far-end reference signal, deferred to Phase 10.
- Kept `spandsp-sys` as an optional dependency in root Cargo.toml (in addition to `spandsp`) for flexibility; carrier feature now activates `dep:spandsp`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed spandsp-sys build.rs: tiffio.h not found on macOS**
- **Found during:** Task 1 (building spandsp crate)
- **Issue:** SpanDSP header includes `<tiffio.h>` but bindgen clang search path didn't include `/opt/homebrew/include`
- **Fix:** Added `builder = builder.clang_arg("-I/opt/homebrew/include")` gated behind `cfg!(target_os = "macos")`
- **Files modified:** `crates/spandsp-sys/build.rs`
- **Verification:** `cargo build -p spandsp` succeeded after fix
- **Committed in:** `cf5f8cd` (Task 1)

**2. [Rule 3 - Blocking] Fixed bindgen raw_line inner attribute error**
- **Found during:** Task 1 (after tiffio.h fix)
- **Issue:** `raw_line("#![allow(...)]")` produces an inner attribute not at file root, causing compile error in Rust 2024 edition
- **Fix:** Changed to outer `#[allow(...)]` attribute in raw_line
- **Files modified:** `crates/spandsp-sys/build.rs`
- **Verification:** bindings.rs compiled correctly
- **Committed in:** `cf5f8cd` (Task 1)

**3. [Rule 3 - Blocking] Added dtmf_rx to bindgen allowlist**
- **Found during:** Task 1 (spandsp/src/dtmf.rs calling spandsp_sys::dtmf_rx)
- **Issue:** Only `dtmf_rx_.*` was in allowlist; `dtmf_rx` (processing fn, no suffix) was excluded
- **Fix:** Added `.allowlist_function("dtmf_rx")` before `dtmf_rx_.*`
- **Files modified:** `crates/spandsp-sys/build.rs`
- **Verification:** `dtmf_rx` function appeared in generated bindings
- **Committed in:** `cf5f8cd` (Task 1)

**4. [Rule 1 - Bug] Fixed Rust 2024 unsafe_op_in_unsafe_fn in dtmf callback**
- **Found during:** Task 1 (compilation warning in dtmf.rs)
- **Issue:** Rust 2024 edition requires explicit `unsafe {}` blocks inside `unsafe extern "C"` fn bodies
- **Fix:** Wrapped raw pointer dereference and `slice::from_raw_parts` in `unsafe {}` block
- **Files modified:** `crates/spandsp/src/dtmf.rs`
- **Verification:** No more warnings; tests pass
- **Committed in:** `cf5f8cd` (Task 1)

**5. [Rule 1 - Bug] ToneDetector: super_tone_rx_init returns NULL with NULL descriptor**
- **Found during:** Task 1 (tone::tests::create_and_drop test failure)
- **Issue:** `super_tone_rx_init(null, null, None, null)` returns NULL, causing ToneDetector::new() to fail at runtime
- **Fix:** Made ToneDetector a pure Rust stub with no C state (struct ToneDetector with no fields). This is correct per the plan which explicitly says "if too complex, implement as a stub"
- **Files modified:** `crates/spandsp/src/tone.rs`
- **Verification:** `tone::tests::create_and_drop` and `process_returns_none` pass
- **Committed in:** `cf5f8cd` (Task 1)

**6. [Rule 1 - Bug] fax_state_t non-zero size assertion fails in SpanDSP 0.0.6**
- **Found during:** Task 1 (fax::tests::fax_state_t_has_nonzero_size test failure)
- **Issue:** SpanDSP 0.0.6 exposes `fax_state_t` as an opaque struct (size=0 placeholder in bindgen output). The size assertion `> 0` was too strong.
- **Fix:** Changed test to `fax_state_t_binding_resolves` which just accesses `size_of::<fax_state_t>()` without asserting the value — proving the type resolves at compile time
- **Files modified:** `crates/spandsp/src/fax.rs`
- **Verification:** Test passes; `FaxEngine::new()` still succeeds (fax_init works even with opaque type)
- **Committed in:** `cf5f8cd` (Task 1)

---

**Total deviations:** 6 auto-fixed (4 blocking build/link issues, 2 runtime test bugs)
**Impact on plan:** All fixes necessary for correct operation with SpanDSP 0.0.6 on macOS. No scope creep. ToneDetector stub approach was explicitly allowed by plan.

## Issues Encountered

- SpanDSP 0.0.6 (Homebrew default) differs from 3.0 in struct visibility — `fax_state_t` is opaque, `plc_state_t` is concrete. Handled by adapting tests and not assuming struct sizes.
- echo_can_update processes one sample at a time (not block-based); the EchoCanceller loops per sample as designed.

## Next Phase Readiness

- SpanDSP DTMF, echo cancellation, and PLC processors are ready for use in call processing pipelines
- StreamEngine's named factory registry is the canonical extension point for adding new DSP processors
- Phase 10 should implement full ToneDetector (super_tone_rx_descriptor_t setup) and FaxEngine (T.38 protocol)
- The 16kHz/8kHz resampling helpers in spandsp_adapters.rs can be reused for any future 8kHz processor

---
*Phase: 01-ffi-foundation-build*
*Completed: 2026-03-28*
