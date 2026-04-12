# Running and Validation

Steps to build, run, and validate the SIP proxy and bridge changes introduced in the `console_sip` branch.

## Prerequisites

| Dependency | Install | Purpose |
|------------|---------|---------|
| Rust toolchain | `rustup` | Build the binary |
| Redis | `brew install redis` (macOS) | Config store + CDR queue |
| sipbot (optional) | `cargo install sipbot` | SIP load testing |
| just (optional) | `brew install just` | Task runner for shortcuts |
| PJSIP (optional) | `just install-pjsip` | Carrier feature (SIP-to-SIP via pjproject) |

---

## Build

Two build profiles:

```bash
# Minimal build — no C dependencies, uses rsipstack for SIP
cargo build --release --no-default-features

# Carrier build — includes pjsip for production SIP-to-SIP
cargo build --release --features carrier
```

The carrier build requires pjproject headers and libraries. Run `just install-pjsip` first on a fresh system.

---

## Run Unit and Integration Tests

```bash
# Full minimal test suite (~30 seconds, 25+ test suites)
cargo test --no-default-features

# Specific modules added in this branch
cargo test proxy::sdp_filter --no-default-features --lib
cargo test proxy::session_timer --no-default-features --lib
cargo test proxy::parallel_dial --no-default-features --lib

# New integration tests
cargo test --test sdp_codec_filter_test --no-default-features
cargo test --test sip_proxy_fixes_test --no-default-features
cargo test --test bridge_fixes_test --no-default-features
cargo test --test ws_timeout_test --no-default-features
```

Expected: zero failures. The `sip_integration_test` suite requires `sipbot` and is skipped when it is not installed.

---

## Start the Server

```bash
# Start Redis
redis-server &

# Start active-call in carrier mode
just start
# or manually:
./target/release/active-call --conf config/carrier.toml
```

The server listens on:
- SIP UDP: `0.0.0.0:15060` (configurable)
- HTTP API: `0.0.0.0:8080`

Check health:
```bash
curl http://localhost:8080/api/v1/system/health
```

---

## Create an API Key

```bash
# Via just
just create-key myapp

# Or via the binary
./target/release/active-call api-key create myapp
```

Export for use in subsequent commands:
```bash
export API_KEY=sv_xxxxx
```

---

## Validate Each Feature

### 1. SDP Codec Filtering (488 rejection)

Create a trunk that only allows G.711:

```bash
curl -X POST http://localhost:8080/api/v1/trunks \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "pcmu-only",
    "direction": "both",
    "distribution": "weight_based",
    "gateways": [{"name": "gw1"}],
    "media": {
      "codecs": ["pcmu", "pcma"],
      "dtmf_mode": "rfc2833"
    }
  }'
```

Assign a DID:
```bash
curl -X POST http://localhost:8080/api/v1/dids \
  -H "Authorization: Bearer $API_KEY" \
  -d '{
    "number": "+15551234567",
    "trunk": "pcmu-only",
    "routing": {"mode": "sip_proxy"}
  }'
```

Call with Opus (should fail with 488):
```bash
sipbot call sip:+15551234567@127.0.0.1:15060 --codec opus
# Expected: 488 Not Acceptable Here
```

Call with PCMU (should succeed):
```bash
sipbot call sip:+15551234567@127.0.0.1:15060 --codec pcmu
# Expected: 200 OK with SDP answer
```

### 2. Parallel Dialing

Create a trunk with multiple gateways and `parallel` distribution:

```bash
curl -X POST http://localhost:8080/api/v1/trunks \
  -H "Authorization: Bearer $API_KEY" \
  -d '{
    "name": "fast-route",
    "direction": "both",
    "distribution": "parallel",
    "gateways": [
      {"name": "primary-us"},
      {"name": "backup-us"}
    ]
  }'
```

Place a call and observe logs — both gateways receive the INVITE, the first to answer wins:
```
dispatch: starting parallel dial
parallel_dial: gateway=primary-us INVITE sent
parallel_dial: gateway=backup-us INVITE sent
parallel_dial: gateway=primary-us answered — cancelling losers
```

### 3. Session Timer Expiry

Sessions receive a 30-minute timer by default. To test expiry, place a call and observe that `session_timer.is_expired()` fires a BYE after 30 minutes without any re-INVITE refresh. Gateway or caller re-INVITEs reset the timer.

Watch the logs:
```
dispatch: session timer expired — terminating call
```

### 4. SIP INFO DTMF Relay

Place a call, then send a SIP INFO with DTMF from either side. Both directions relay the message:

```
dispatch: relaying INFO from gateway to caller
dispatch: relaying INFO from caller to gateway
```

### 5. SIP-to-WebSocket Bridge

Start a local WebSocket echo server:
```python
# ws_echo.py
import asyncio, websockets
async def echo(ws):
    async for msg in ws:
        await ws.send(msg)
asyncio.run(websockets.serve(echo, "0.0.0.0", 9090))
```

Configure a DID with `ws_bridge` mode:
```bash
curl -X PUT http://localhost:8080/api/v1/dids/+15551234568 \
  -H "Authorization: Bearer $API_KEY" \
  -d '{
    "number": "+15551234568",
    "trunk": "pcmu-only",
    "routing": {
      "mode": "ws_bridge",
      "ws_config": {"url": "ws://127.0.0.1:9090", "codec": "pcmu"}
    }
  }'
```

Call the number:
```bash
sipbot call sip:+15551234568@127.0.0.1:15060 --codec pcmu --duration 5
# Expected: 200 OK with SDP answer, audio echoes back
```

### 6. WebSocket Connect Timeout

Point a DID at a non-listening address:
```bash
curl -X PUT http://localhost:8080/api/v1/dids/+15551234568 \
  -d '{
    "routing": {
      "mode": "ws_bridge",
      "ws_config": {"url": "ws://192.0.2.1:9999"}
    }
  }'
```

Place a call. The bridge fails within 10 seconds instead of hanging:
```
ws_bridge: WebSocket connect timed out after 10s
```

---

## Observe with the Diagnostic Endpoints

```bash
# List active calls
curl http://localhost:8080/api/v1/calls -H "Authorization: Bearer $API_KEY"

# Get a specific call
curl http://localhost:8080/api/v1/calls/<session-id> -H "Authorization: Bearer $API_KEY"

# System stats
curl http://localhost:8080/api/v1/system/stats -H "Authorization: Bearer $API_KEY"

# Trunk test (reachability)
curl -X POST http://localhost:8080/api/v1/diagnostics/trunk-test \
  -H "Authorization: Bearer $API_KEY" \
  -d '{"trunk": "pcmu-only"}'
```

---

## Stop the Server

```bash
just stop
# or
kill $(cat /tmp/active-call-test.pid)
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `cargo build --features carrier` fails | PJSIP missing | Run `just install-pjsip` |
| 488 on every call | Trunk `media.codecs` excludes the caller's codec | Add caller codec to list or remove `media.codecs` |
| WebSocket bridge hangs | No timeout wired | Confirm binary is from this branch (should fail within 10s) |
| Parallel dial behaves sequentially | Trunk `distribution` not set to `"parallel"` | Update trunk config |
| No SIP answer on bridge mode | Binary predates SDP answer fix | Rebuild from this branch |
