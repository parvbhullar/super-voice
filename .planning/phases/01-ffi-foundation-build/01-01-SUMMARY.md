---
phase: 01-ffi-foundation-build
plan: 01
subsystem: infra
tags: [rust, cargo-workspace, ffi, bindgen, pkg-config, sofia-sip, spandsp, feature-flags]

# Dependency graph
requires: []
provides:
  - Cargo workspace with crates/* members and resolver = "2"
  - carrier feature flag enabling Sofia-SIP and SpanDSP C dependencies
  - minimal feature flag for pure-Rust no-C build
  - sofia-sip-sys raw FFI crate with bindgen/pkg-config build.rs
  - spandsp-sys raw FFI crate with bindgen/pkg-config build.rs
affects: [02-sofia-sip-safe-wrapper, 03-spandsp-safe-wrapper, all subsequent phases]

# Tech tracking
tech-stack:
  added: [bindgen 0.71, pkg-config 0.3]
  patterns:
    - "-sys crate convention: raw FFI with links = <libname>, build.rs generates bindings via bindgen"
    - "pkg-config probe with version fallback and helpful panic message if library not installed"
    - "opaque_type('.*') for all Sofia-SIP types — treat C structs as opaque pointers"
    - "Feature-gated C deps: optional = true + carrier feature activates both sys crates"

key-files:
  created:
    - crates/sofia-sip-sys/Cargo.toml
    - crates/sofia-sip-sys/build.rs
    - crates/sofia-sip-sys/src/lib.rs
    - crates/spandsp-sys/Cargo.toml
    - crates/spandsp-sys/build.rs
    - crates/spandsp-sys/src/lib.rs
  modified:
    - Cargo.toml
    - src/media/track/rtc.rs

key-decisions:
  - "carrier feature initially points to -sys crates directly; will redirect to safe wrapper crates in Plans 02/03"
  - "Use opaque_type('.*') for all Sofia-SIP types — the callback-based opaque-pointer API is naturally opaque"
  - "SpanDSP version constraint: try >=3.0 first, fall back to any version with warning (0.0.6+ API is stable)"
  - "Sofia-SIP pkg-config name is sofia-sip-ua (not sofia-sip or sofia-sip-ua-glib)"

patterns-established:
  - "Pattern 1: FFI crate naming — <libname>-sys for raw bindings, <libname> for safe wrapper"
  - "Pattern 2: build.rs structure — probe pkg-config, add include paths to bindgen, emit link directives"
  - "Pattern 3: lib.rs structure — #![allow(...)] + include!(concat!(env!('OUT_DIR'), '/bindings.rs'))"

requirements-completed: [BLDP-01, BLDP-02, FFND-05]

# Metrics
duration: 15min
completed: 2026-03-28
---

# Phase 1 Plan 01: FFI Foundation Build Summary

**Cargo workspace with sofia-sip-sys and spandsp-sys raw FFI crates using bindgen + pkg-config, gated behind carrier/minimal feature flags**

## Performance

- **Duration:** ~15 min
- **Started:** 2026-03-28T21:21:30Z
- **Completed:** 2026-03-28T21:36:00Z
- **Tasks:** 3
- **Files modified:** 8

## Accomplishments

- Root Cargo.toml restructured as a workspace with `members = ["crates/*"]` and resolver = "2"
- `carrier` feature flag added (activates sofia-sip-sys + spandsp-sys); `minimal` feature flag added (pure Rust)
- `sofia-sip-sys` crate created: bindgen generates FFI bindings from Sofia-SIP headers via pkg-config discovery
- `spandsp-sys` crate created: bindgen generates FFI bindings from SpanDSP headers via pkg-config with version fallback
- `cargo check --no-default-features` passes cleanly on the pure-Rust path

## Task Commits

Each task was committed atomically:

1. **Task 1: Restructure root Cargo.toml as workspace and add feature flags** - `000f787` (feat)
2. **Task 2: Create sofia-sip-sys crate with bindgen + pkg-config build.rs** - `7d0dd5a` (feat)
3. **Task 3: Create spandsp-sys crate with bindgen + pkg-config build.rs** - `2198601` (feat)

## Files Created/Modified

- `Cargo.toml` - Added [workspace] section, carrier/minimal feature flags, optional sys crate deps
- `crates/sofia-sip-sys/Cargo.toml` - Raw FFI crate definition with links = "sofia-sip-ua"
- `crates/sofia-sip-sys/build.rs` - pkg-config probe + bindgen for Sofia-SIP headers (nua.h, sdp.h, su_root.h, nta.h, auth_module.h)
- `crates/sofia-sip-sys/src/lib.rs` - Include generated bindings with lint allowances
- `crates/spandsp-sys/Cargo.toml` - Raw FFI crate definition with links = "spandsp"
- `crates/spandsp-sys/build.rs` - pkg-config probe (>=3.0 with fallback) + bindgen for spandsp.h
- `crates/spandsp-sys/src/lib.rs` - Include generated bindings with lint allowances
- `src/media/track/rtc.rs` - Bug fix: non-exhaustive CodecType match when opus feature disabled

## Decisions Made

- `carrier` feature initially points to the `-sys` crates directly; Plans 02 and 03 will add safe wrapper crates and this dep will be updated
- `opaque_type(".*")` applied to all Sofia-SIP types since the callback-based API uses opaque C pointers throughout
- SpanDSP version fallback: probe `>=3.0` first, fall back to any version with a `cargo:warning` — dtmf_rx_* and echo_can_* APIs are stable since 0.0.6
- Sofia-SIP pkg-config name confirmed as `sofia-sip-ua`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed non-exhaustive match on CodecType when opus feature disabled**
- **Found during:** Task 1 verification (`cargo check --no-default-features`)
- **Issue:** `src/media/track/rtc.rs` had a `match codec` with `CodecType::Opus` arm guarded by `#[cfg(feature = "opus")]`. Without the opus feature active, the match was non-exhaustive (E0004 compile error), blocking the no-default-features check.
- **Fix:** Added `#[cfg(not(feature = "opus"))] CodecType::Opus => AudioCapability::pcmu()` fallback arm
- **Files modified:** `src/media/track/rtc.rs`
- **Verification:** `cargo check --no-default-features` passes with only warnings
- **Committed in:** `000f787` (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug fix)
**Impact on plan:** Auto-fix required for correctness — the pure-Rust build path was broken before this was found. No scope creep.

## Issues Encountered

- Sofia-SIP and SpanDSP are not installed on the macOS development machine. The sys crates correctly fail at `cargo check -p sofia-sip-sys` / `cargo check -p spandsp-sys` with the expected installation hint messages. This is expected behavior — they are gated behind the `carrier` feature and require the C libraries to be present.

## User Setup Required

To build with `--features carrier`, install the C libraries first:
- **macOS:** `brew install sofia-sip spandsp`
- **Debian/Ubuntu:** `apt-get install libsofia-sip-ua-dev libspandsp-dev`

## Next Phase Readiness

- Workspace structure complete — Plans 02 and 03 can create safe wrapper crates under `crates/`
- All crates automatically included via `members = ["crates/*"]` glob
- `carrier` feature flag ready to be updated once safe wrappers exist in Plans 02/03
- Pure-Rust build path (`--no-default-features`) verified working

---
*Phase: 01-ffi-foundation-build*
*Completed: 2026-03-28*
