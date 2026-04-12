# Carrier API Reference

**Base URL:** `http://localhost:8080`
**Auth:** All `/api/v1/*` endpoints require `Authorization: Bearer <api_key>` header.

For the original voice agent API (WebSocket, playbooks), see [api.md](./api.md).

## Authentication

All carrier admin endpoints require a Bearer token. Create one via Redis:

```bash
RANDOM_HEX=$(openssl rand -hex 32)
HASH=$(echo -n "$RANDOM_HEX" | shasum -a 256 | cut -d' ' -f1)
redis-cli SADD "sv:api_keys" "myapp:${HASH}"
export API_KEY="sv_${RANDOM_HEX}"
```

Requests without a valid token return `401 Unauthorized`:

```json
{"error": "unauthorized"}
```

---

## Endpoints

Manage SIP listener endpoints (Sofia-SIP or rsipstack).

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/endpoints` | List all endpoints |
| POST | `/api/v1/endpoints` | Create and start an endpoint |
| GET | `/api/v1/endpoints/{name}` | Get endpoint details |
| PUT | `/api/v1/endpoints/{name}` | Update endpoint (stop, update, restart) |
| DELETE | `/api/v1/endpoints/{name}` | Stop and remove endpoint |

**Create Endpoint:**

```bash
curl -X POST /api/v1/endpoints \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "carrier-sip",
    "stack": "sofia",
    "bind_addr": "0.0.0.0",
    "port": 5060,
    "transport": "udp",
    "tls": {
      "enabled": true,
      "port": 5061,
      "cert_path": "/etc/certs/sip.pem"
    },
    "nat": {
      "mode": "auto",
      "external_ip": "203.0.113.1"
    },
    "auth": {
      "realm": "carrier.example.com",
      "username": "admin",
      "password": "secret"
    },
    "session_timer": {
      "enabled": true,
      "interval_secs": 1800
    }
  }'
```

**Response:** `{"name": "carrier-sip", "status": "running"}`

**Stack options:** `sofia` (carrier-grade, C FFI) or `rsipstack` (pure Rust, internal/WebRTC)

---

## Gateways

Manage outbound SIP gateways with automatic health monitoring.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/gateways` | List all gateways with health status |
| POST | `/api/v1/gateways` | Create gateway and start health monitoring |
| GET | `/api/v1/gateways/{name}` | Get gateway with health status |
| PUT | `/api/v1/gateways/{name}` | Update gateway config |
| DELETE | `/api/v1/gateways/{name}` | Stop monitoring and remove |

**Create Gateway:**

```bash
curl -X POST /api/v1/gateways \
  -H "Authorization: Bearer $API_KEY" \
  -d '{
    "name": "twilio-us",
    "proxy_addr": "sip.twilio.com:5060",
    "transport": "tls",
    "auth": {
      "username": "ACxxxx",
      "password": "auth_token",
      "realm": "sip.twilio.com"
    },
    "health_check_interval_secs": 30,
    "failure_threshold": 3,
    "recovery_threshold": 2
  }'
```

**Health states:** `active` → `disabled` (after N failures) → `active` (after M successes)

---

## Trunks

Group gateways with capacity limits, codec policies, and IP ACLs.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/trunks` | List all trunks |
| POST | `/api/v1/trunks` | Create a trunk |
| GET | `/api/v1/trunks/{name}` | Get trunk config |
| PUT | `/api/v1/trunks/{name}` | Full replacement update |
| PATCH | `/api/v1/trunks/{name}` | Partial update (JSON merge) |
| DELETE | `/api/v1/trunks/{name}` | Delete (409 if DIDs reference it) |

**Sub-Resources:**

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/trunks/{name}/credentials` | List credentials |
| POST | `/api/v1/trunks/{name}/credentials` | Add credential |
| DELETE | `/api/v1/trunks/{name}/credentials/{realm}` | Remove credential |
| GET | `/api/v1/trunks/{name}/acl` | List IP ACL entries |
| POST | `/api/v1/trunks/{name}/acl` | Add ACL entry |
| DELETE | `/api/v1/trunks/{name}/acl/{entry}` | Remove ACL entry |
| GET | `/api/v1/trunks/{name}/origination_uris` | List origination URIs |
| POST | `/api/v1/trunks/{name}/origination_uris` | Add origination URI |
| DELETE | `/api/v1/trunks/{name}/origination_uris/{uri}` | Remove URI |
| GET | `/api/v1/trunks/{name}/media` | Get media config |
| PUT | `/api/v1/trunks/{name}/media` | Set media config |
| GET | `/api/v1/trunks/{name}/capacity` | Get capacity config |
| PUT | `/api/v1/trunks/{name}/capacity` | Set capacity config |

**Create Trunk:**

```bash
curl -X POST /api/v1/trunks \
  -H "Authorization: Bearer $API_KEY" \
  -d '{
    "name": "us-carrier",
    "direction": "both",
    "distribution": "weight_based",
    "gateways": [
      {"name": "twilio-us", "weight": 60},
      {"name": "telnyx-us", "weight": 40}
    ],
    "credentials": [
      {"realm": "sip.twilio.com", "username": "user", "password": "pass"}
    ],
    "acl": ["10.0.0.0/24"],
    "capacity": {"max_calls": 100, "max_cps": 10.0},
    "nofailover_sip_codes": [404, 486, 603]
  }'
```

**Media Config (codec filtering):**

```bash
# Set trunk media config (caller SDP will be filtered to these codecs)
curl -X PUT /api/v1/trunks/us-carrier/media \
  -H "Authorization: Bearer $API_KEY" \
  -d '{
    "codecs": ["pcmu", "pcma"],
    "dtmf_mode": "rfc2833",
    "srtp": null,
    "media_mode": null
  }'
```

When `media.codecs` is set, inbound SDP offers are filtered to include only these codecs. Calls with no codec overlap receive 488 Not Acceptable Here.

**Distribution modes:**
- `weight_based` — weighted round-robin across gateways (default)
- `round_robin` — sequential rotation
- `hash_callid` / `hash_src_ip` / `hash_destination` — consistent hashing
- `parallel` — dial all gateways concurrently; first answer wins, losers are cancelled

**Parallel dialing example:**
```bash
curl -X POST /api/v1/trunks \
  -H "Authorization: Bearer $API_KEY" \
  -d '{
    "name": "us-failover",
    "direction": "both",
    "distribution": "parallel",
    "gateways": [
      {"name": "primary-us"},
      {"name": "backup-us"},
      {"name": "tertiary-us"}
    ]
  }'
```

**Session timers (RFC 4028):** Enabled by default on carrier-path calls with a 30-minute expiry. Gateway or caller re-INVITEs refresh the timer; expired sessions are torn down with BYE. No configuration required.

---

## DIDs (Phone Numbers)

Assign phone numbers to trunks with routing modes.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/dids` | List all DIDs |
| POST | `/api/v1/dids` | Assign a DID |
| GET | `/api/v1/dids/{number}` | Get DID config |
| PUT | `/api/v1/dids/{number}` | Update DID routing |
| DELETE | `/api/v1/dids/{number}` | Unassign DID |

**Routing Modes:**

```bash
# AI Agent — routes to playbook
{"mode": "ai_agent", "playbook": "support.md"}

# SIP Proxy — B2BUA bridge to outbound trunk
# Trunk codec list filters caller SDP; 488 on mismatch
{"mode": "sip_proxy"}

# WebRTC Bridge (v1) — for WebRTC-capable SIP UAs (ICE/DTLS in SDP)
# Caller must support ICE+DTLS; regular SIP phones will fail
{"mode": "webrtc_bridge", "webrtc_config": {"ice_servers": ["stun:stun.l.google.com:19302"], "ice_lite": false}}

# WebSocket Bridge — SIP to outbound WebSocket server
# SDP handshake performed on SIP leg; 200 OK sent to caller
# 10s connect timeout; immediate teardown on WS disconnect
{"mode": "ws_bridge", "ws_config": {"url": "wss://ai-backend.example.com/ws", "codec": "pcmu"}}
```

---

## Routing

Define call routing tables with match rules.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/routing/tables` | List routing tables |
| POST | `/api/v1/routing/tables` | Create routing table |
| GET | `/api/v1/routing/tables/{name}` | Get routing table |
| PUT | `/api/v1/routing/tables/{name}` | Update routing table |
| DELETE | `/api/v1/routing/tables/{name}` | Delete routing table |
| GET | `/api/v1/routing/tables/{name}/records` | List records |
| POST | `/api/v1/routing/tables/{name}/records` | Add record |
| DELETE | `/api/v1/routing/tables/{name}/records/{index}` | Remove record |
| POST | `/api/v1/routing/resolve` | Dry-run route resolution |

**Match types:** `Lpm` (longest prefix), `ExactMatch`, `Regex`, `Compare`, `HttpQuery`

**Example routing table:**

```json
{
  "name": "outbound",
  "records": [
    {"match_type": "Lpm", "value": "+1415", "targets": [{"trunk": "sf-carrier"}], "priority": 10},
    {"match_type": "Lpm", "value": "+44", "targets": [{"trunk": "uk-carrier"}], "priority": 20},
    {"match_type": "ExactMatch", "value": "__DEFAULT__", "targets": [{"trunk": "fallback"}], "is_default": true}
  ]
}
```

---

## Translations

Regex-based number rewriting.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/translations` | List translation classes |
| POST | `/api/v1/translations` | Create translation class |
| GET | `/api/v1/translations/{name}` | Get translation class |
| PUT | `/api/v1/translations/{name}` | Update translation class |
| DELETE | `/api/v1/translations/{name}` | Delete translation class |

```json
{
  "name": "normalize-e164",
  "rules": [{
    "caller_pattern": "^0(\\d+)$",
    "caller_replace": "+44$1",
    "destination_pattern": "^(\\d{10})$",
    "destination_replace": "+1$1",
    "direction": "inbound"
  }]
}
```

---

## Manipulations

Conditional SIP header modification.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/manipulations` | List manipulation classes |
| POST | `/api/v1/manipulations` | Create manipulation class |
| GET | `/api/v1/manipulations/{name}` | Get manipulation class |
| PUT | `/api/v1/manipulations/{name}` | Update manipulation class |
| DELETE | `/api/v1/manipulations/{name}` | Delete manipulation class |

```json
{
  "name": "region-headers",
  "rules": [{
    "condition_mode": "and",
    "conditions": [{"field": "caller_number", "pattern": "^\\+1415"}],
    "actions": [{"action_type": "set_header", "name": "X-Region", "value": "SF-Bay"}],
    "anti_actions": [{"action_type": "set_header", "name": "X-Region", "value": "Other"}]
  }]
}
```

**Action types:** `set_header`, `remove_header`, `set_var`, `log`, `hangup`, `sleep`

---

## Active Calls

Monitor and control live calls.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/calls` | List active calls |
| GET | `/api/v1/calls/{id}` | Get call detail |
| POST | `/api/v1/calls/{id}/hangup` | Terminate call |
| POST | `/api/v1/calls/{id}/transfer` | Transfer call |
| POST | `/api/v1/calls/{id}/mute` | Mute call |
| POST | `/api/v1/calls/{id}/unmute` | Unmute call |

---

## CDRs (Call Detail Records)

Query completed call records.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/cdrs` | List CDRs (paginated, filtered) |
| GET | `/api/v1/cdrs/{id}` | Get CDR detail |
| DELETE | `/api/v1/cdrs/{id}` | Delete CDR |
| GET | `/api/v1/cdrs/{id}/recording` | Get recording (501) |
| GET | `/api/v1/cdrs/{id}/sip-flow` | Get SIP flow (501) |

**Query parameters:** `trunk`, `did`, `status`, `start_date`, `end_date`, `page`, `page_size`

---

## Webhooks

Register HTTP callbacks for CDR events.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/webhooks` | List webhooks |
| POST | `/api/v1/webhooks` | Register webhook (sends test event) |
| PUT | `/api/v1/webhooks/{id}` | Update webhook |
| DELETE | `/api/v1/webhooks/{id}` | Remove webhook |

Webhooks deliver with `X-Webhook-Event` and optional `X-Webhook-Secret` headers. Retry: 3 attempts with exponential backoff. Fallback: disk JSON files.

---

## Security

Manage SIP security rules and view blocked IPs.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/security/firewall` | Get firewall config |
| PATCH | `/api/v1/security/firewall` | Update firewall (merge) |
| GET | `/api/v1/security/blocks` | List auto-blocked IPs |
| DELETE | `/api/v1/security/blocks/{ip}` | Unblock an IP |
| GET | `/api/v1/security/flood-tracker` | Flood tracking stats |
| GET | `/api/v1/security/auth-failures` | Auth failure stats |

---

## Diagnostics

Test and debug carrier infrastructure without placing calls.

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/v1/diagnostics/trunk-test` | Test gateway reachability |
| POST | `/api/v1/diagnostics/route-evaluate` | Dry-run route matching |
| GET | `/api/v1/diagnostics/registrations` | List SIP registrations |
| GET | `/api/v1/diagnostics/registrations/{user}` | Get registration status |
| GET | `/api/v1/diagnostics/summary` | Combined diagnostic summary |

---

## System

Health, info, cluster, and runtime management.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/system/health` | Health status + uptime + Redis + call count |
| GET | `/api/v1/system/info` | Version and build info |
| GET | `/api/v1/system/cluster` | List cluster nodes |
| POST | `/api/v1/system/reload` | Reload config from Redis |
| GET | `/api/v1/system/config` | Non-sensitive config summary |
| GET | `/api/v1/system/stats` | Runtime statistics |

---

## Error Responses

| Status | Meaning |
|--------|---------|
| 400 | Bad request (invalid JSON, missing fields) |
| 401 | Unauthorized (missing or invalid Bearer token) |
| 404 | Resource not found |
| 409 | Conflict (resource in use — engagement tracking) |
| 501 | Not implemented (recording, sip-flow placeholders) |
| 503 | Service unavailable (Redis down, gateway manager not initialized) |

```json
{"error": "trunk 'us-carrier' is referenced by DID '+14155551234' and cannot be deleted"}
```

---

## Route Count

| Group | Routes | Auth |
|-------|--------|------|
| Endpoints | 5 | Bearer |
| Gateways | 5 | Bearer |
| Trunks (core + sub-resources) | 19 | Bearer |
| DIDs | 5 | Bearer |
| Routing | 9 | Bearer |
| Translations | 5 | Bearer |
| Manipulations | 5 | Bearer |
| Active Calls | 6 | Bearer |
| CDRs | 5 | Bearer |
| Webhooks | 4 | Bearer |
| Security | 6 | Bearer |
| Diagnostics | 5 | Bearer |
| System | 6 | Bearer |
| Health | 1 | Bearer |
| **Carrier Total** | **86** | |
