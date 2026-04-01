---
phase: 12-replace-sofia-sip-with-pjsip-for-carrier-grade-sip-proxy
plan: "01"
subsystem: infra
tags: [pjsip, pjproject, bindgen, ffi, sip, c-bindings, pkg-config]

# Dependency graph
requires: []
provides:
  - pjsip-sys crate with raw bindgen FFI bindings to pjproject 2.14+
  - scripts/install-pjproject.sh for building pjproject from source
  - B2BUA-scoped allowlists (no opaque_type; struct fields accessible)
  - ARM endianness handling for macOS aarch64 builds
affects:
  - 12-replace-sofia-sip-with-pjsip-for-carrier-grade-sip-proxy (plans 02+)

# Tech tracking
tech-stack:
  added: [pjproject 2.14+, bindgen 0.71, pkg-config 0.3, libc 0.2]
  patterns:
    - pkg-config probe for C library in build.rs (same pattern as sofia-sip-sys but with libpjproject)
    - explicit allowlist bindgen (no opaque_type — struct fields accessible for B2BUA use)
    - derive_debug + derive_default on generated types for callback struct ergonomics
    - ARM endianness clang defines for pjproject header compatibility on aarch64

key-files:
  created:
    - scripts/install-pjproject.sh
    - crates/pjsip-sys/Cargo.toml
    - crates/pjsip-sys/pjsip_wrapper.h
    - crates/pjsip-sys/build.rs
    - crates/pjsip-sys/src/lib.rs
  modified: []

key-decisions:
  - "pjsip-sys uses links = \"pjsip\" (not \"pjproject\") in Cargo.toml to match the library name"
  - "ARM endianness clang args required: pjproject config.h treats aarch64 as bi-endian and errors without PJ_IS_LITTLE_ENDIAN=1"
  - "No .opaque_type(\".*\") unlike sofia-sip-sys: pjsip struct fields must be accessible for direct manipulation in B2BUA"
  - "derive_debug(true) and derive_default(true) enabled on all generated types for callback struct ergonomics"
  - "libpjproject pkg-config name used for probe (version 2.16 installed via Homebrew, satisfies >=2.14 requirement)"

patterns-established:
  - "pjsip-sys build.rs: CARGO_CFG_TARGET_ARCH check gates ARM endianness clang args"
  - "pjsip_wrapper.h: B2BUA-relevant headers only (no audio device, no video device headers)"

requirements-completed: [PJMIG-01]

# Metrics
duration: 4min
completed: 2026-04-01
---

# Phase 12 Plan 01: pjsip-sys Raw FFI Bindings Summary

**pjsip-sys crate providing raw bindgen FFI bindings to pjproject 2.14+ via pkg-config, with B2BUA-scoped allowlists and accessible struct fields (no opaque_type)**

## Performance

- **Duration:** ~4 min
- **Started:** 2026-04-01T18:09:43Z
- **Completed:** 2026-04-01T18:13:28Z
- **Tasks:** 1
- **Files created:** 5

## Accomplishments

- Created `scripts/install-pjproject.sh` — builds pjproject 2.14.1 from source with SIP-only config (no video, sound, codec libs), handles macOS OpenSSL path via `brew --prefix openssl` fallback
- Created `crates/pjsip-sys/` crate with `links = "pjsip"`, build.rs probing `libpjproject >= 2.14`, generating bindings with explicit B2BUA-scoped allowlists
- Fixed ARM endianness clang compatibility (pjproject config.h requires `PJ_IS_LITTLE_ENDIAN=1` on aarch64 — not auto-detected)
- Verified `cargo check -p pjsip-sys` compiles cleanly; generated bindings contain `pjsip_inv_session`, `pjsip_endpt_create`, `pjmedia_sdp_parse` (119 matching occurrences)

## Task Commits

1. **Task 1: Create install script and pjsip-sys crate** - `628be55` (feat)

**Plan metadata:** _(pending final commit)_

## Files Created/Modified

- `scripts/install-pjproject.sh` — build-from-source script for pjproject 2.14.1
- `crates/pjsip-sys/Cargo.toml` — crate manifest with `links = "pjsip"`, bindgen + pkg-config build-deps, libc dep
- `crates/pjsip-sys/pjsip_wrapper.h` — C aggregate header: pjlib, pjlib-util, pjsip core, pjsip-ua, pjsip-simple, pjmedia SDP only
- `crates/pjsip-sys/build.rs` — pkg-config probe, B2BUA allowlists, ARM endianness clang args, bindings generation
- `crates/pjsip-sys/src/lib.rs` — re-export of generated bindings.rs with FFI allow attributes

## Decisions Made

- **`links = "pjsip"` (not "pjproject"):** Cargo links key must match the library name used by pjproject; `libpjproject.pc` provides libs ending in `-aarch64-apple-darwin*` but the canonical link name is `pjsip`
- **ARM endianness fix:** pjproject config.h treats `aarch64` as bi-endian (same branch as ARM32) and errors without explicit `PJ_IS_LITTLE_ENDIAN=1`. bindgen's clang doesn't automatically set this from the target tuple, so build.rs must pass `-DPJ_IS_LITTLE_ENDIAN=1 -DPJ_IS_BIG_ENDIAN=0` for ARM targets.
- **No opaque_type:** Unlike `sofia-sip-sys` which uses `.opaque_type(".*")` for an opaque-pointer callback API, pjsip uses struct-based APIs where field access is required — explicitly NOT using opaque types.
- **Homebrew pjproject 2.16:** Installed via `brew install pjproject` (2.16) rather than building from source — satisfies `>=2.14` requirement and avoids long build time. Install script still provided for CI/Docker environments.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] ARM endianness clang args required for pjproject headers**
- **Found during:** Task 1 (cargo check -p pjsip-sys)
- **Issue:** bindgen/clang errors on pjproject `config.h` — `Endianness must be declared for this processor` on aarch64 because pjproject treats ARM as bi-endian and requires explicit `PJ_IS_LITTLE_ENDIAN`/`PJ_IS_BIG_ENDIAN` defines that clang doesn't auto-set from the target triple
- **Fix:** Added CARGO_CFG_TARGET_ARCH detection in build.rs; when `target_arch == "aarch64"` or starts with "arm", passes `-DPJ_IS_LITTLE_ENDIAN=1 -DPJ_IS_BIG_ENDIAN=0` as clang args
- **Files modified:** `crates/pjsip-sys/build.rs`
- **Verification:** `cargo check -p pjsip-sys` passes cleanly after fix
- **Committed in:** 628be55 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (Rule 1 - Bug)
**Impact on plan:** Auto-fix necessary for ARM/macOS correctness. No scope creep.

## Issues Encountered

- pjproject was not pre-installed on the system; installed via `brew install pjproject` (2.16, satisfying >=2.14). Install script (`scripts/install-pjproject.sh`) remains the documented path for CI/Docker.

## User Setup Required

None — pjproject installed via Homebrew. For CI/Docker, run `bash scripts/install-pjproject.sh`.

## Next Phase Readiness

- `pjsip-sys` crate is the foundation for Plan 02 (pjsip safe wrapper crate)
- All struct fields accessible in generated bindings (no opaque_type) — ready for Plan 02's `PjInvSession`, `PjEndpoint`, etc.
- pkg-config probe in build.rs will work on Linux with `libpjproject.pc` from the install script

## Self-Check: PASSED

All artifacts verified:
- scripts/install-pjproject.sh: FOUND
- crates/pjsip-sys/Cargo.toml: FOUND
- crates/pjsip-sys/build.rs: FOUND
- crates/pjsip-sys/pjsip_wrapper.h: FOUND
- crates/pjsip-sys/src/lib.rs: FOUND
- Commit 628be55: FOUND

---
*Phase: 12-replace-sofia-sip-with-pjsip-for-carrier-grade-sip-proxy*
*Completed: 2026-04-01*
