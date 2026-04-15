# Deferred Items — Phase 02 (logged by plan 02-04 executor)

## Pre-existing build errors in `crates/pjsip` (out of scope for plan 02-04)

Discovered while running `cargo check` as part of plan 02-04's Task 1 acceptance
verification. These errors exist in the base commit and are NOT caused by any
changes in this plan (which modifies zero files — the target merge line was
already present in `src/main.rs:317` when the plan started).

- `crates/pjsip/src/endpoint.rs:343:27` — `addr.sin_family = libc::AF_INET as u16;` — expected `u8`, found `u16` (E0308)
- `crates/pjsip/src/endpoint.rs:391:27` — same mismatch (E0308)
- `crates/pjsip/src/endpoint.rs:312:28` — unnecessary `unsafe` block warning (unused_unsafe)

**Impact on 02-04 acceptance:**
- `cargo check` (full workspace, default features) exits 101 because of these
  pre-existing pjsip errors.
- `cargo check -p active-call --no-default-features --features "opus,offline"`
  (the crate that contains `src/main.rs`) exits 0 cleanly.
- `cargo test --lib --no-default-features --features "opus,offline"` is used
  in place of the plan's `cargo test` commands to avoid the broken dependency.

**Root cause (likely):** libc crate version change; on some Apple targets
`sockaddr_in::sin_family` is typed `u8`, not `u16`. The cast should be
`libc::AF_INET as u8` on those targets, or use `libc::sa_family_t`.

**Disposition:** Not fixed in 02-04 (strictly out of scope per plan). Should
be addressed by a separate pjsip maintenance plan.
