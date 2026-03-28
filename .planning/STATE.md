# GSD State

## Project Reference

See: .planning/PROJECT.md (updated 2026-03-28)

**Core value:** Any voice call reaches an AI agent or gets routed to the right destination, reliably and at carrier scale, from a single Rust binary.
**Current focus:** Defining requirements

## Current Position

Phase: Not started (defining requirements)
Plan: --
Status: Defining requirements
Last activity: 2026-03-28 — Milestone v1.0 Carrier Edition started

## Accumulated Context

- Extensive architecture analysis completed across all repo components (src/, media-gateway/, third-party/freeswitch/, third-party/libresbc/, third-party/sip/, third-party/sayna/)
- Architecture document written: docs/plans/2026-03-28-architecture-design.md
- Detailed comparison of FreeSWITCH vs All-Rust vs Hybrid approaches completed
- Sofia-SIP FFI feasibility confirmed (callback-based, opaque pointers, su_root event loop)
- SpanDSP FFI feasibility confirmed (frame-based, stateless per-call, clean C API)
- LibreSBC patterns identified for carrier features (token bucket CPS, LPM routing, manipulation engine, Redis state)
- REST API plan designed (~90 endpoints) combining Vobiz, LibreSBC, and RustPBX patterns
- Entity renamed: SIP Profile → Endpoint
- Trunk is bidirectional by default (direction: inbound | outbound | both)
