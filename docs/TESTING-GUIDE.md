# Testing Guide

## Prerequisites

### System Dependencies

```bash
# macOS
brew install sofia-sip spandsp redis sipsak

# Debian/Ubuntu
apt install libsofia-sip-ua-dev libspandsp-dev redis-server sipsak clang libclang-dev
```

### Start Redis

```bash
redis-server &
# Verify
redis-cli ping  # should return PONG
```

## Running Tests

### Full Test Suite

```bash
# All tests (carrier features enabled — requires Sofia-SIP + SpanDSP installed)
cargo test --features carrier

# Pure Rust tests only (no C library dependencies)
cargo test --no-default-features

# With output
cargo test --features carrier -- --nocapture
```

### By Module

```bash
# FFI crate tests
cargo test -p sofia-sip --features carrier
cargo test -p spandsp --features carrier

# Redis state layer (requires running Redis)
cargo test --lib redis_state::

# Routing engine
cargo test --lib routing::

# Translation + manipulation
cargo test --lib translation::
cargo test --lib manipulation::

# Proxy call / B2BUA
cargo test --lib proxy::

# Capacity + security
cargo test --lib capacity::
cargo test --lib security::

# CDR engine
cargo test --lib cdr::

# API handlers
cargo test --lib handler::

# Endpoint auth (digest validation)
cargo test endpoint_auth

# Gateway health thresholds
cargo test gateway_health

# DSP processors
cargo test --lib spandsp_adapters
```

### Integration Tests

```bash
# Carrier FFI integration (Sofia-SIP + SpanDSP coexistence)
cargo test --features carrier --test carrier_integration -- --nocapture

# Routing (LPM, HTTP query, jumps)
cargo test --test routing_integration

# Trunk distribution + API
cargo test --test trunk_api_integration
cargo test --test distribution_integration

# DID API
cargo test --test did_api_integration

# Bridge modes (WebRTC, WebSocket, mode dispatch)
cargo test --test bridge_modes_test

# Proxy call (B2BUA, failover, early media)
cargo test --test proxy_call_integration

# DSP processing (echo, DTMF, fax, PLC, tones)
cargo test --features carrier --test dsp_integration

# Diagnostics + system API
cargo test --test diagnostics_system_integration

# AI agent regression
cargo test --test regression_integration
```

### Startup Time Validation

```bash
cargo build --release --features carrier
bash scripts/check_startup.sh target/release/active-call
# Expected: PASS: Startup time Xms is under 1000ms limit
```

## Manual Testing

### 1. Start the Server

```bash
# Create config
cat > /tmp/test-config.toml << 'EOF'
addr = "0.0.0.0"
http_addr = "0.0.0.0:18080"
udp_port = 15060
redis_url = "redis://127.0.0.1:6379"
log_level = "info"

[handler]
type = "playbook"
default = "hello.md"
EOF

# Start
cargo run --release -- --conf /tmp/test-config.toml
```

### 2. Create an API Key

```bash
# Generate key material
RANDOM_HEX=$(openssl rand -hex 32)
HASH=$(echo -n "$RANDOM_HEX" | shasum -a 256 | cut -d' ' -f1)

# Store in Redis (format: "name:sha256_hash")
redis-cli SADD "sv:api_keys" "test:${HASH}"

# Your API key
export API_KEY="sv_${RANDOM_HEX}"
echo "API_KEY=$API_KEY"
```

### 3. Test the Carrier API

```bash
BASE="http://localhost:18080/api/v1"

# System health
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/system/health" | python3 -m json.tool

# System info
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/system/info" | python3 -m json.tool

# Create an endpoint
curl -s -X POST "$BASE/endpoints" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "carrier-ep",
    "stack": "rsipstack",
    "bind_addr": "0.0.0.0",
    "port": 25060,
    "transport": "udp"
  }' | python3 -m json.tool

# Create a gateway
curl -s -X POST "$BASE/gateways" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "test-gw",
    "proxy_addr": "127.0.0.1:5060",
    "transport": "udp",
    "health_check_interval_secs": 30,
    "failure_threshold": 3,
    "recovery_threshold": 2
  }' | python3 -m json.tool

# Create a trunk
curl -s -X POST "$BASE/trunks" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "test-trunk",
    "direction": "both",
    "gateways": [{"name": "test-gw", "weight": 100}],
    "distribution": "round_robin"
  }' | python3 -m json.tool

# Create a DID (AI agent mode)
curl -s -X POST "$BASE/dids" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "number": "+14155551234",
    "trunk": "test-trunk",
    "routing": {"mode": "ai_agent", "playbook": "hello.md"}
  }' | python3 -m json.tool

# Create a DID (proxy mode)
curl -s -X POST "$BASE/dids" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "number": "+14155559999",
    "trunk": "test-trunk",
    "routing": {"mode": "sip_proxy"}
  }' | python3 -m json.tool

# List all entities
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/endpoints" | python3 -m json.tool
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/gateways" | python3 -m json.tool
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/trunks" | python3 -m json.tool
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/dids" | python3 -m json.tool
```

### 4. Test Routing

```bash
# Create a routing table
curl -s -X POST "$BASE/routing/tables" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "outbound",
    "default_action": "block",
    "rules": [
      {"match_type": "lpm", "match_value": "+1415", "action": "route", "targets": {"primary": "test-trunk"}},
      {"match_type": "lpm", "match_value": "+44", "action": "route", "targets": {"primary": "test-trunk"}},
      {"match_type": "em", "match_value": "__DEFAULT__", "action": "route", "targets": {"primary": "test-trunk"}}
    ]
  }' | python3 -m json.tool

# Dry-run route evaluation
curl -s -X POST "$BASE/diagnostics/route-evaluate" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"destination": "+14155551234", "routing_table": "outbound"}' | python3 -m json.tool
```

### 5. Test Translation

```bash
# Create a translation class
curl -s -X POST "$BASE/translations" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "normalize-e164",
    "rules": [{
      "caller_pattern": "^0(\\d+)$",
      "caller_replacement": "+44$1",
      "destination_pattern": "^(\\d{10})$",
      "destination_replacement": "+1$1"
    }]
  }' | python3 -m json.tool
```

### 6. Test Security

```bash
# View firewall rules
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/security/firewall" | python3 -m json.tool

# Add IP to blacklist
curl -s -X PATCH "$BASE/security/firewall" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"blacklist": ["10.0.0.99/32"]}' | python3 -m json.tool

# View blocked IPs
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/security/blocks" | python3 -m json.tool
```

### 7. Test Webhooks

```bash
# Register a webhook (use httpbin or webhook.site for testing)
curl -s -X POST "$BASE/webhooks" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://httpbin.org/post",
    "events": ["cdr.new"],
    "secret": "test-secret-123"
  }' | python3 -m json.tool

# List webhooks
curl -s -H "Authorization: Bearer $API_KEY" "$BASE/webhooks" | python3 -m json.tool
```

## SIP Load Testing

### Using sipsak

```bash
# Single OPTIONS ping
sipsak -s sip:test@127.0.0.1:15060 -v

# Bulk OPTIONS (100 concurrent)
for i in $(seq 1 100); do
  sipsak -s sip:test@127.0.0.1:15060 &
done
wait

# Bulk INVITE (50 concurrent calls to a DID)
for i in $(seq 1 50); do
  sipsak -s sip:+14155551234@127.0.0.1:15060 -M -C sip:test${i}@127.0.0.1 &
done
wait

# Check health after load test
curl -s -H "Authorization: Bearer $API_KEY" \
  http://localhost:18080/api/v1/system/health | python3 -m json.tool
```

### Using sipp (if installed)

```bash
# Install sipp
brew install sipp  # macOS
apt install sip-tester  # Debian

# Create UAC scenario for bulk calling
cat > /tmp/uac.xml << 'SIPXML'
<?xml version="1.0" encoding="ISO-8859-1" ?>
<scenario name="Basic UAC">
  <send retrans="500">
    <![CDATA[
      INVITE sip:[service]@[remote_ip]:[remote_port] SIP/2.0
      Via: SIP/2.0/[transport] [local_ip]:[local_port];branch=[branch]
      From: sip:sipp@[local_ip]:[local_port];tag=[call_number]
      To: sip:[service]@[remote_ip]:[remote_port]
      Call-ID: [call_id]
      CSeq: 1 INVITE
      Contact: sip:sipp@[local_ip]:[local_port]
      Max-Forwards: 70
      Content-Type: application/sdp
      Content-Length: [len]

      v=0
      o=sipp 1 1 IN IP4 [local_ip]
      s=sipp
      c=IN IP4 [local_ip]
      t=0 0
      m=audio [auto_media_port] RTP/AVP 0
      a=rtpmap:0 PCMU/8000
    ]]>
  </send>
  <recv response="100" optional="true"/>
  <recv response="180" optional="true"/>
  <recv response="200" optional="true" timeout="5000"/>
  <send>
    <![CDATA[
      ACK sip:[service]@[remote_ip]:[remote_port] SIP/2.0
      Via: SIP/2.0/[transport] [local_ip]:[local_port];branch=[branch]
      From: sip:sipp@[local_ip]:[local_port];tag=[call_number]
      To: sip:[service]@[remote_ip]:[remote_port][peer_tag_param]
      Call-ID: [call_id]
      CSeq: 1 ACK
      Content-Length: 0
    ]]>
  </send>
  <pause milliseconds="1000"/>
  <send retrans="500">
    <![CDATA[
      BYE sip:[service]@[remote_ip]:[remote_port] SIP/2.0
      Via: SIP/2.0/[transport] [local_ip]:[local_port];branch=[branch]
      From: sip:sipp@[local_ip]:[local_port];tag=[call_number]
      To: sip:[service]@[remote_ip]:[remote_port][peer_tag_param]
      Call-ID: [call_id]
      CSeq: 2 BYE
      Content-Length: 0
    ]]>
  </send>
  <recv response="200"/>
</scenario>
SIPXML

# Run 100 calls at 10 CPS
sipp 127.0.0.1:15060 -sf /tmp/uac.xml \
  -r 10 -l 100 -m 100 \
  -s +14155551234 \
  -trace_stat

# Run 1000 calls at 50 CPS (stress test)
sipp 127.0.0.1:15060 -sf /tmp/uac.xml \
  -r 50 -l 500 -m 1000 \
  -s +14155551234 \
  -trace_stat
```

## WebSocket Testing

```bash
# Install websocat
brew install websocat  # macOS

# Connect to voice WebSocket
websocat ws://127.0.0.1:18080/call

# Send a JSON command after connecting
# {"command": "invite", "callee": "sip:+14155551234@127.0.0.1:15060"}
```

## Docker Testing

```bash
# Build carrier image
docker build -f Dockerfile.carrier -t super-voice:carrier .

# Run with Redis
docker run -d --name redis redis:7-alpine
docker run -d --net host \
  -e REDIS_URL=redis://127.0.0.1:6379 \
  -v $(pwd)/config:/app/config \
  super-voice:carrier --conf /app/config/test.toml

# Verify startup time
docker exec super-voice bash -c "time /app/active-call --help"

# Run tests inside container
docker run --rm super-voice:carrier cargo test --features carrier
```

## Test Coverage by Area

| Area | Test Count | Command |
|------|-----------|---------|
| Redis types (serde) | 16 | `cargo test redis_state::types` |
| ConfigStore CRUD | 17 | `cargo test config_store` |
| Pub/sub | 5 | `cargo test pubsub` |
| Runtime state | 6 | `cargo test runtime_state` |
| Engagement tracking | 10 | `cargo test engagement` |
| API auth | 9 | `cargo test auth` |
| Endpoint digest auth | 7 | `cargo test endpoint_auth` |
| Gateway thresholds | 5 | `cargo test gateway_health` |
| Distribution algorithms | 8 | `cargo test distribution` |
| Routing engine | 36 | `cargo test --lib routing` |
| Translation engine | 12 | `cargo test --lib translation` |
| Manipulation engine | 13 | `cargo test --lib manipulation` |
| Proxy call types | 21 | `cargo test --lib proxy::types` |
| Media bridge | 5 | `cargo test proxy::media_bridge` |
| Failover | 9 | `cargo test proxy::failover` |
| Session | 5 | `cargo test proxy::session` |
| Capacity guard | 14 | `cargo test capacity` |
| Security module | 26 | `cargo test security` |
| CDR types + queue | 12 | `cargo test --lib cdr` |
| SpanDSP wrappers | 23 | `cargo test -p spandsp` |
| SpanDSP adapters | 9 | `cargo test spandsp_adapters` |
| API route tests | ~60 | `cargo test --lib handler` |
| Integration tests | ~80 | `cargo test --test '*'` |
| **Total** | **~647** | `cargo test --features carrier` |

## Troubleshooting

### Sofia-SIP not found

```
error: sofia-sip-ua not found via pkg-config
```

Install Sofia-SIP: `brew install sofia-sip` (macOS) or `apt install libsofia-sip-ua-dev` (Debian).

### SpanDSP not found

```
warning: SpanDSP >=3.0 not found; trying any available version
```

This is a warning, not an error. SpanDSP 0.0.6 (from Debian/Homebrew) works fine. The warning means the `>=3.0` version check fell back to any available version.

### Redis tests fail

```
connection refused (os error 61)
```

Start Redis: `redis-server &` or `brew services start redis`.

### Auth returns 401 with valid key

Verify the key format in Redis:
```bash
redis-cli SMEMBERS "sv:api_keys"
# Should show: "name:sha256hash"
```

The hash must be SHA-256 of the hex part after `sv_` prefix. See "Create an API Key" section above.

### SIP tests require sipbot

Two tests (`test_sip_options_ping`, `test_sip_invite_call`) require the `sipbot` external binary. These are environment tests — skip them with:
```bash
cargo test -- --skip sip_integration
```

### Build without C dependencies

```bash
cargo build --no-default-features
# or
cargo build --features minimal
```

This compiles without Sofia-SIP and SpanDSP. Carrier features (SBC, DSP, digest auth) are unavailable in this mode.
