---
phase: 13
plan: "04"
subsystem: cpaas-applications
tags: [twiml, answer-url, routing, ivr, webhooks]
dependency_graph:
  requires: [13-03]
  provides: [twiml-parsing, answer-url-fetch, did-application-routing, hangup-url-hook]
  affects: [proxy/routing/matcher.rs, proxy/proxy_call/sip_session.rs, call/app/]
tech_stack:
  added: [quick-xml TwiML parsing, reqwest form POST for answer_url]
  patterns: [AppAction::Chain for TwiML→IVR delegation, fire-and-forget tokio::spawn for hangup_url]
key_files:
  created:
    - src/call/app/twiml.rs
    - src/call/app/answer_url.rs
  modified:
    - src/call/app/mod.rs
    - src/call/app/ivr_config.rs
    - src/proxy/routing/matcher.rs
    - src/proxy/proxy_call/sip_session.rs
decisions:
  - "TwiML parsing uses quick-xml 0.39 Reader::read_event_into loop with explicit depth tracking; decode() not unescape() for BytesText in this version"
  - "TwimlApp delegates to IvrApp via AppAction::Chain in on_enter; no custom state machine needed"
  - "hangup_url hook lives in SipSession::cleanup() where server.database is available, not in TwimlApp::on_exit (no DB handle there)"
  - "DID->Application lookup uses .ok().flatten() so DB errors silently fall through to normal routing"
  - "IvrDefinition::from_entry_actions assigns synthetic __N DTMF keys; actions are sequential entries"
metrics:
  duration: "~35 minutes"
  completed: "2026-05-07"
  tasks: 4
  files: 6
---

# Phase 13 Plan 04: TwiML Parser + Answer URL Fetcher + Application Routing Summary

TwiML XML parser, async answer_url HTTP fetcher, DID→Application routing short-circuit, and TwimlApp factory with hangup_url hook — completing the CPaaS application call-flow pipeline.

## Tasks Completed

| # | Task | Commit | Key Files |
|---|------|--------|-----------|
| 1 | TwiML XML parser | 351954f | src/call/app/twiml.rs |
| 2 | answer_url fetcher | 351954f | src/call/app/answer_url.rs |
| 3 | DID→Application routing branch | d608ff9 | src/proxy/routing/matcher.rs |
| 4 | TwimlApp factory + hangup_url hook | 5406c1d | src/proxy/proxy_call/sip_session.rs, src/call/app/ivr_config.rs |

## What Was Built

### Task 1 — TwiML Parser (`src/call/app/twiml.rs`)

`parse_twiml(xml) -> Result<Vec<EntryAction>, TwimlError>` parses a TwiML `<Response>` document using quick-xml 0.39's event loop. Verb mapping:

- `<Play>` → `EntryAction::Play { prompt: url }`
- `<Say voice="x">text</Say>` → `EntryAction::Play { prompt_text, prompt_voice }`
- `<Dial>` → `EntryAction::Transfer { target }`
- `<Hangup/>` / `<Reject/>` → `EntryAction::Hangup {}`
- `<Gather numDigits action method>` + nested `<Say>` → `EntryAction::Collect` + optional `EntryAction::Webhook`
- `<Record>` → WARN + skip (no EntryAction::Record)
- Unknown verbs → WARN + skip (call does NOT abort per D-17)

10 unit tests, all passing.

### Task 2 — Answer URL Fetcher (`src/call/app/answer_url.rs`)

`fetch_answer_url(url, auth_headers, timeout_ms, params) -> Result<String, AnswerUrlError>` POSTs `caller, callee, call_id, application_id, account_id, direction` as `application/x-www-form-urlencoded`. Auth headers from the JSON object are applied per-key; non-string values skipped. Timeout from `answer_timeout_ms` (i32 → u64 millis, minimum 1s). `AnswerUrlError::cdr_failure_reason()` maps to CDR strings.

### Task 3 — Routing Short-Circuit (`src/proxy/routing/matcher.rs`)

Added DID→Application lookup at the TOP of `match_invite_impl`, before supersip_routing_tables and legacy rule walk. Queries `rustpbx_dids` by callee user, then `supersip_application_numbers` by `did_id`. Returns `RouteResult::Application { app_name: "twiml", app_params: { application_id } }`. DB errors use `.ok().flatten()` → silent fall-through to normal routing.

### Task 4 — TwimlApp + hangup_url (`src/proxy/proxy_call/sip_session.rs`)

**`IvrDefinition::from_entry_actions`** (added to `ivr_config.rs`): builds a minimal single-root-menu `IvrDefinition` from a flat `Vec<EntryAction>`, assigning synthetic `__N` DTMF keys. Used by TwimlApp to hand off to IvrApp.

**`TwimlApp`**: `CallApp` implementation. `on_enter` looks up the application row, calls `fetch_answer_url`, parses TwiML, calls `IvrDefinition::from_entry_actions`, creates an `IvrApp`, and returns `AppAction::Chain(Box::new(ivr))`. Disabled applications and fetch/parse errors return `AppAction::Hangup`.

**`BuiltinAppFactory`**: new `"twiml"` match arm creates a `TwimlApp` from `app_params.application_id`.

**hangup_url hook** in `SipSession::cleanup()`: detects `DialplanFlow::Application { app_name: "twiml" }`, extracts `application_id`, spawns a fire-and-forget task (5s timeout) that queries the application row and POSTs to `hangup_url` if present. Failures logged at WARN, never retried.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] quick-xml 0.39 uses `decode()` not `unescape()` for BytesText**
- **Found during:** Task 1 compilation
- **Issue:** `BytesText::unescape()` does not exist in quick-xml 0.39.2; the method is `decode()`
- **Fix:** Changed `e.unescape()?.into_owned()` to `e.decode().map_err(quick_xml::Error::from)?.into_owned()`
- **Files modified:** src/call/app/twiml.rs
- **Commit:** 351954f

**2. [Rule 1 - Bug] Gather depth tracking bug — `current_verb` overwritten by nested `<Say>`**
- **Found during:** Task 1 test run (test_gather_with_say failed)
- **Issue:** When `<Say>` inside `<Gather>` was processed, it overwrote `current_verb`, so the `</Gather>` end event couldn't find the verb name and skipped emitting the Collect action
- **Fix:** Refactored parser to use separate `top_verb`/`nested_verb` state variables with depth-gated checks; Gather closing detected by element name directly
- **Files modified:** src/call/app/twiml.rs
- **Commit:** 351954f (rewrite)

**3. [Rule 2 - Missing critical functionality] `IvrDefinition::from_entry_actions` not in plan but required**
- **Found during:** Task 4
- **Issue:** Plan noted "If IvrDefinition has no `from_entry_actions` constructor: add one" — it didn't exist
- **Fix:** Added `IvrDefinition::from_entry_actions(name, actions)` to `ivr_config.rs`
- **Files modified:** src/call/app/ivr_config.rs
- **Commit:** 5406c1d

**4. [Rule 2 - Missing critical functionality] `TwimlApp::on_exit` has no DB handle for hangup_url**
- **Found during:** Task 4 design
- **Issue:** `CallApp::on_exit` only receives `ExitReason`, no `ApplicationContext`; can't query DB for hangup_url there
- **Fix:** Moved hangup_url POST to `SipSession::cleanup()` where `server.database` is available; on_exit logs a debug message only
- **Files modified:** src/proxy/proxy_call/sip_session.rs
- **Commit:** 5406c1d

## Test Results

```
test result: ok. 1409 passed; 12 failed (pre-existing media E2E); 1 ignored
```

The 12 failures are identical to pre-plan baseline — all in `test_media_e2e`, `test_wholesale_e2e`, `test_rtp_e2e`, `file_track_tests` (RTP infrastructure, unrelated to this plan).

TwiML unit tests: 10/10 passing.

## Self-Check: PASSED

- src/call/app/twiml.rs — FOUND
- src/call/app/answer_url.rs — FOUND
- src/call/app/mod.rs (twiml + answer_url modules registered) — FOUND
- src/call/app/ivr_config.rs (from_entry_actions added) — FOUND
- src/proxy/routing/matcher.rs (DID→Application short-circuit) — FOUND
- src/proxy/proxy_call/sip_session.rs (TwimlApp + BuiltinAppFactory + hangup_url) — FOUND
- Commits 351954f, d608ff9, 5406c1d — confirmed in git log
