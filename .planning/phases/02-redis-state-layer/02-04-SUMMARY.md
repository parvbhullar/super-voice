---
phase: 02-redis-state-layer
plan: 04
subsystem: http-api-auth
tags: [gap-closure, idempotent, auth-middleware, router-assembly, rust]
type: execute
gap_closure: true
depends_on: ["02-01", "02-02", "02-03"]
requirements: [RAPI-15]
dependency-graph:
  requires: ["02-03"]
  provides: ["live auth middleware on HTTP router"]
  affects: ["src/main.rs"]
tech-stack:
  added: []
  patterns: ["axum Router::merge composition"]
key-files:
  created: []
  modified: []
  verified-unchanged:
    - src/main.rs
    - src/handler/handler.rs
    - src/handler/mod.rs
decisions:
  - "Plan executed as idempotent no-op: the target merge line was already present at src/main.rs:317 when the plan started. Zero source files modified."
  - "Used --no-default-features --features \"opus,offline\" for cargo test because the default carrier feature pulls in pjsip, which has pre-existing compile errors unrelated to this plan (logged in deferred-items.md)."
metrics:
  duration: "~4 minutes (mostly cargo build/test)"
  completed: "2026-04-15"
---

# Phase 02 Plan 04: carrier_admin_router Gap Closure Summary

One-liner: Verified `src/main.rs:317` already merges `carrier_admin_router` into the main axum router — idempotent no-op closing the sole gap from 02-VERIFICATION.md.

## Execution Outcome

**The merge line was already present at the start of execution; no file modifications were required.**

- File: `src/main.rs`
- Line: **317**
- Exact text: `.merge(active_call::handler::carrier_admin_router(app_state.clone()))`
- Occurrence count: **exactly 1** (`grep -c` returns `1`)
- Location: inside the `let app = active_call::handler::call_router()...with_state(app_state.clone());` builder chain, between `.merge(active_call::handler::iceservers_router())` (line 316) and `.route("/", get(index))` (line 318)

Snippet of the live assembly block (lines 314–321):

```rust
let app = active_call::handler::call_router()
    .merge(active_call::handler::playbook_router())
    .merge(active_call::handler::iceservers_router())
    .merge(active_call::handler::carrier_admin_router(app_state.clone()))
    .route("/", get(index))
    .route("/console", get(console))
    .nest_service("/static", ServeDir::new("static"))
    .with_state(app_state.clone());
```

Per the plan's idempotency rule ("if a spot-check already shows the merge line present, do NOT add a second copy; simply verify exactly one instance exists and proceed"), Task 1 required no edit. `git diff` is empty for all source files.

## Line-Number Correction

`02-VERIFICATION.md` (gaps entry, `artifacts[0].issue` and `missing[0]`) references the main router assembly block as **"lines 307-312"**. The actual assembly block in the base commit (`9f677a3`) lives at **lines 314-321**, with the `carrier_admin_router` merge at **line 317**. The verifier appears to have been looking at a stale line range; the gap itself (wire carrier_admin_router into the main router) is nevertheless accurately described, and is already resolved.

Flagging this for the re-verification pass: the `artifacts[0].issue` text "Lines 307-312 merge call_router, playbook_router, iceservers_router — carrier_admin_router is absent" is outdated in two ways — wrong line range (should be 314–321) AND wrong conclusion (carrier_admin_router is present, not absent).

## Verification Results

### Task 1: Grep / compile check

| Check | Command | Result |
|-------|---------|--------|
| Merge-line count | `grep -c "\.merge(active_call::handler::carrier_admin_router(app_state\.clone())" src/main.rs` | `1` PASS |
| Merge-line location | `grep -n "carrier_admin_router(app_state" src/main.rs` | `317:        .merge(active_call::handler::carrier_admin_router(app_state.clone()))` PASS |
| `src/handler/handler.rs` unchanged | `git diff --quiet src/handler/handler.rs` | PASS |
| `src/handler/mod.rs` unchanged | `git diff --quiet src/handler/mod.rs` | PASS |
| `cargo check` (active-call crate, `--no-default-features --features "opus,offline"`) | exit 0 | PASS |
| `cargo check` (full workspace, default features incl. carrier/pjsip) | exit 101 | **FAIL — pre-existing, out of scope (see Deferred Issues)** |

### Task 2: Test suite results

All tests run with `--no-default-features --features "opus,offline"` to bypass the pre-existing `pjsip` crate breakage (see Deferred Issues).

| Test module | Command | Result | Duration |
|-------------|---------|--------|----------|
| `handler::handler` | `cargo test --lib --no-default-features --features "opus,offline" handler::handler` | **8 passed, 0 failed, 0 ignored** (482 filtered out) | 0.03 s (test phase) |
| `redis_state::auth` | `cargo test --lib --no-default-features --features "opus,offline" redis_state::auth` | **9 passed, 0 failed, 0 ignored** (481 filtered out) | 0.07 s (test phase) |
| Full `active-call` lib | `cargo test --lib --no-default-features --features "opus,offline"` | **490 passed, 0 failed, 0 ignored** | 10.27 s (test phase) |
| Full workspace `cargo test` (default features) | not attempted | N/A — blocked by pjsip pre-existing compile error (out of scope) |

All 9 plan-02-03 auth tests remain green:
- `test_create_key_returns_sv_prefixed_key`
- `test_validate_key_valid`
- `test_validate_key_invalid`
- `test_delete_key_removes_entry`
- `test_list_keys_returns_names`
- `test_auth_middleware_valid_token_passes`
- `test_auth_middleware_invalid_token_returns_401`
- `test_auth_middleware_no_header_returns_401`
- `test_auth_middleware_skips_health_path`

All 8 `handler::handler` tests remain green (`test_filter_headers`, `test_routing_tables_routes_exist`, `test_security_routes_exist`, `test_manipulation_routes_exist`, `test_call_handler_core_extras_are_session_scoped`, `test_system_routes_exist`, `test_translation_routes_exist`, `test_diagnostics_routes_exist`).

## Gap Status

**VERIFICATION.md gap[0] ("carrier_admin_router not wired into main.rs") — CLOSED.**

- Structural fix: merge line is present at `src/main.rs:317` (confirmed by grep).
- Compilation: `active-call` crate (which owns `src/main.rs`) compiles cleanly with `--no-default-features --features "opus,offline"`.
- Behavioral regression test: all 9 `redis_state::auth` tests + 8 `handler::handler` tests + full 490-test active-call lib suite pass.
- Live-stack reachability: because line 317 is inside the main `let app = ...` chain that is later handed to `axum::serve(listener, app)`, the auth-protected `carrier_admin_router` routes are now reachable by real HTTP requests. The `auth_middleware` layer that `carrier_admin_router` applies will execute on live traffic.

Truth #11 ("API requests with valid Bearer token succeed; requests without/invalid token return 401") is lifted from **PARTIAL → VERIFIED** by the combination of:
1. The implementation previously verified in plan 02-03 (auth_middleware + carrier_admin_router + 9 passing unit tests).
2. The structural wiring confirmed in this plan (line 317 merge inside the served app).

Requirement **RAPI-15** is lifted from **PARTIALLY SATISFIED → SATISFIED**.

Overall phase must-haves: **10/11 → 11/11 verified.**

## Files Modified

**None.** Plan 02-04 made zero source file modifications. Per the plan's idempotency rule, the target merge line was already present in the base commit (`9f677a3 docs(02): add gap closure plan 02-04 for carrier_admin_router wiring`).

## Commits

No per-task code commits (nothing to commit — zero source changes). A single documentation commit will be created by the orchestrator after this summary is written:

- `docs(02-04): record gap-closure verification as idempotent no-op` — adds `02-04-SUMMARY.md` and `deferred-items.md` to `.planning/phases/02-redis-state-layer/`.

## Deviations from Plan

### Auto-fixed Issues

None required. Plan executed exactly as written; Task 1's idempotency branch applied.

### Deferred Issues (out of scope — logged, not fixed)

**1. Pre-existing `crates/pjsip` compile errors**
- Found during: Task 1 `cargo check` execution.
- Issue: `crates/pjsip/src/endpoint.rs:343:27` and `391:27` — `addr.sin_family = libc::AF_INET as u16;` — expected `u8`, found `u16` (E0308). Also an `unused_unsafe` warning at `endpoint.rs:312`.
- Why out of scope: Plan 02-04 targets `src/main.rs` only and explicitly forbids touching other files. The pjsip crate is a sibling workspace member pulled in via the default `carrier` feature. These errors exist at the plan's base commit and are not caused by any plan 02-04 change (which modifies zero files).
- Action taken: Logged to `.planning/phases/02-redis-state-layer/deferred-items.md`. Used `cargo test --no-default-features --features "opus,offline"` to run Task 2's tests on the `active-call` crate without dragging in broken pjsip.
- Recommendation: Address in a separate pjsip maintenance plan. Likely fix is casting to `libc::sa_family_t` or `u8` on macOS/iOS targets where `sockaddr_in::sin_family` is `u8`.

**2. Environmental disk pressure (resolved in-flight)**
- Found during: First attempt at Task 2 tests — link step failed with `ld: write() failed, errno=28 (No space left on device)` because the build volume had only ~158 MiB free.
- Action taken: Removed `target/debug/incremental` (freed ~458 MiB), then `target/` entirely (freed ~3.1 GiB), then re-ran tests successfully. Build volume (`/System/Volumes/Data`) is now at ~2+ GiB free.
- Why not a scope issue: environmental, not code-related.

## Auth Gates

None encountered.

## TDD Gate Compliance

Not applicable — plan frontmatter is `type: execute`, not `type: tdd`.

## Self-Check: PASSED

- [x] `.planning/phases/02-redis-state-layer/02-04-SUMMARY.md` exists (this file).
- [x] `.planning/phases/02-redis-state-layer/deferred-items.md` exists (logged pjsip issue).
- [x] `src/main.rs:317` contains exactly one `.merge(active_call::handler::carrier_admin_router(app_state.clone()))` — verified by `grep -c` and `grep -n`.
- [x] `src/main.rs`, `src/handler/handler.rs`, `src/handler/mod.rs` all unchanged (`git status --short` clean on source tree at execution start and end).
- [x] `cargo check -p active-call --no-default-features --features "opus,offline"` exits 0.
- [x] `cargo test --lib --no-default-features --features "opus,offline" redis_state::auth` → 9 passed / 0 failed.
- [x] `cargo test --lib --no-default-features --features "opus,offline" handler::handler` → 8 passed / 0 failed.
- [x] `cargo test --lib --no-default-features --features "opus,offline"` → 490 passed / 0 failed.
- [x] No per-task code commits needed (zero source changes). Orchestrator will create the docs commit.
