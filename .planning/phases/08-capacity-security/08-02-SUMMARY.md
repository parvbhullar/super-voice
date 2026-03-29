---
phase: 08-capacity-security
plan: "02"
subsystem: security
tags: [security, sip, firewall, flood-protection, brute-force, validation, topology-hiding]
dependency_graph:
  requires: []
  provides: [SipSecurityModule, IpFirewall, FloodTracker, BruteForceTracker, validate_sip_message, hide_topology]
  affects: [src/lib.rs]
tech_stack:
  added: []
  patterns: [sliding-window, cidr-matching, regex-blacklist, facade-pattern]
key_files:
  created:
    - src/security/mod.rs
    - src/security/firewall.rs
    - src/security/flood_tracker.rs
    - src/security/brute_force.rs
    - src/security/message_validator.rs
    - src/security/topology.rs
  modified:
    - src/lib.rs
decisions:
  - "Manual CIDR bit-matching avoids adding ipnetwork crate — pure std::net::IpAddr with prefix_len comparison"
  - "FloodTracker and BruteForceTracker use VecDeque<Instant> sliding window inside RwLock<HashMap> for lock-free per-IP isolation"
  - "SipSecurityModule facade checks: whitelist → blacklist → UA regex → flood → brute-force (in that priority order)"
  - "Topology hiding uses substring matching (not regex) on internal_domains for performance-sensitive path"
metrics:
  duration_minutes: 3
  tasks_completed: 2
  files_created: 6
  files_modified: 1
  tests_written: 26
  completed_date: "2026-03-29"
---

# Phase 08 Plan 02: SIP Security Module Summary

**One-liner:** IP firewall with CIDR matching, sliding-window flood/brute-force trackers, SIP message validation, and topology hiding in a composable `SipSecurityModule` facade.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | IP Firewall, Flood Tracker, and Brute Force Tracker | 7e84d1b | src/security/{mod,firewall,flood_tracker,brute_force}.rs, src/lib.rs |
| 2 | SIP Message Validation and Topology Hiding | 7e84d1b | src/security/{message_validator,topology}.rs |

## What Was Built

### IpFirewall (`src/security/firewall.rs`)
IPv4 and IPv6 CIDR matching with whitelist/blacklist. Whitelist takes priority over blacklist. Manual CIDR bit-matching via `ip_in_cidr()` avoids pulling in the `ipnetwork` crate. Invalid entries are logged and skipped gracefully.

### FloodTracker (`src/security/flood_tracker.rs`)
Per-IP request counter using `VecDeque<Instant>` sliding window inside `RwLock<HashMap<IpAddr, FloodEntry>>`. Timestamps older than the window are pruned on each call. When count >= threshold, sets `blocked_until = now + block_duration`. Auto-block, check, list, and unblock operations all provided.

### BruteForceTracker (`src/security/brute_force.rs`)
Same sliding-window pattern as FloodTracker but tracks auth failures. `record_success()` clears the failure queue and removes any active block. Default: 5 failures in 60 seconds triggers a 3600-second block.

### SipMessageInfo / validate_sip_message (`src/security/message_validator.rs`)
Pure function operating on a simple `SipMessageInfo` struct. Checks: missing Max-Forwards, Max-Forwards=0, Content-Length mismatch. Caller constructs the struct from parsed SIP headers before calling.

### hide_topology (`src/security/topology.rs`)
Processes `SipHeaders` (Vec of name-value pairs) in-place. Keeps only the first (outermost) Via; removes subsequent Via headers. Removes Record-Route entries containing internal domain substrings. All other headers are preserved unchanged.

### SipSecurityModule (`src/security/mod.rs`)
Facade composing all sub-modules. Priority order for `check_request()`:
1. Firewall whitelist → Whitelisted (bypass all rate limits)
2. Firewall blacklist → Blacklisted
3. UA regex blacklist → UaBlocked
4. Flood tracker → FloodBlocked
5. Brute force check → BruteForceBlocked
6. Pass → Allowed

## Decisions Made

1. **Manual CIDR matching** — Avoids adding `ipnetwork` crate; `ip_in_cidr()` compares first `prefix_len` bits using bit-shift on u32/u128.

2. **VecDeque sliding window** — Simple O(n) prune on each call; appropriate for typical SIP rates where window sizes are small.

3. **Facade check priority** — Whitelist first so trusted IPs never hit rate limiters; UA check before flood so scanners don't consume flood tracking slots.

4. **Substring match for topology hiding** — `internal_domains` is a list of plain substrings; faster than regex on the critical per-message path.

## Test Results

26 tests, all passing:
- 8 firewall tests (IPv4/IPv6, CIDR, whitelist override)
- 4 flood tracker tests (auto-block, per-IP independence, block persistence, unblock)
- 3 brute force tests (auto-block, success reset, per-IP independence)
- 5 message validator tests (Max-Forwards=0, missing, CL mismatch, valid, no CL header)
- 3 topology tests (Via stripping, Record-Route stripping, other headers preserved)
- 3 SipSecurityModule UA tests (friendly-scanner, sipvicious, normal agent)

## Deviations from Plan

None — plan executed exactly as written.

## Self-Check

- [x] src/security/mod.rs exists
- [x] src/security/firewall.rs exists
- [x] src/security/flood_tracker.rs exists
- [x] src/security/brute_force.rs exists
- [x] src/security/message_validator.rs exists
- [x] src/security/topology.rs exists
- [x] Commit 7e84d1b verified
- [x] `cargo build` clean
- [x] 26/26 tests passing
