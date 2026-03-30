# Active Call

[![Crates.io](https://img.shields.io/crates/v/active-call.svg)](https://crates.io/crates/active-call)
[![Downloads](https://img.shields.io/crates/d/active-call.svg)](https://crates.io/crates/active-call)
[![Commits](https://img.shields.io/github/commit-activity/m/miuda-ai/active-call)](https://github.com/miuda-ai/active-call/commits/main)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

`active-call` is a standalone Rust crate for building AI Voice Agents. It provides high-performance infrastructure bridging AI models with real-world telephony and web communications.

📖 **Documentation** → [English](./docs/en/README.md) | [中文](./docs/zh/README.md) | [API Reference](./docs/api.md)

## Key Capabilities

### 1. Multi-Protocol Audio Gateway

- **SIP (Telephony)**: UDP, TCP, TLS (SIPS), WebSocket. Register as extension to FreeSWITCH / Asterisk / [RustPBX](https://github.com/restsend/rustpbx), or handle direct SIP calls. PSTN via [Twilio](./docs/twilio_integration.md) and [Telnyx](./docs/telnyx_integration.md).
- **WebRTC**: Browser-to-agent SRTP. *(Requires HTTPS or 127.0.0.1)*
- **Voice over WebSocket**: Push raw PCM/encoded audio, receive real-time events.

### 2. Dual-Engine Dialogue

- **Traditional Pipeline**: VAD → ASR → LLM → TTS. Supports OpenAI, Aliyun, Azure, Tencent and more.
- **Realtime Streaming**: Native OpenAI/Azure Realtime API — full-duplex, ultra-low latency.

### 3. Playbook — Stateful Voice Agents

Define personas, scenes, and flows in Markdown files:

```markdown
---
asr:
  provider: "sensevoice"
tts:
  provider: "supertonic"
  speaker: "F1"
llm:
  provider: "openai"
  model: "${OPENAI_MODEL}"
  apiKey: "${OPENAI_API_KEY}"
  features: ["intent_clarification", "emotion_resonance"]
dtmf:
  "0": { action: "hangup" }
posthook:
  url: "https://api.example.com/webhook"
  summary: "detailed"
---

# Scene: greeting
<dtmf digit="1" action="goto" scene="tech_support" />

You are a friendly AI for {{ company_name }}. Greet the caller warmly.

# Scene: tech_support
How can I help with your system? I can transfer you: <refer to="sip:human@domain.com" />
```

> 💡 `${VAR}` = environment variables (config-time). `{{var}}` = runtime variables (per-call).

### 4. Offline AI (Privacy-First)

Run ASR and TTS locally — no cloud API required:

- **Offline ASR**: [SenseVoice](https://github.com/FunAudioLLM/SenseVoice) — zh, en, ja, ko, yue
- **Offline TTS**: [Supertonic](https://github.com/supertone-inc/supertonic) — en, ko, es, pt, fr

```bash
# Download models
docker run --rm -v $(pwd)/data/models:/models \
  ghcr.io/miuda-ai/active-call:latest \
  --download-models all --models-dir /models --exit-after-download

# Run with offline models
docker run -d --net host \
  -v $(pwd)/data/models:/app/models \
  -v $(pwd)/config:/app/config \
  ghcr.io/miuda-ai/active-call:latest
```

> **Mainland China**: Add `-e HF_ENDPOINT=https://hf-mirror.com` to use the HuggingFace mirror.

### 5. High-Performance Media Core

| VAD Engine      | Time (60s audio) | RTF    | Note              |
| --------------- | ---------------- | ------ | ----------------- |
| **TinySilero**  | ~60 ms           | 0.0010 | >2.5× faster ONNX |
| **ONNX Silero** | ~158 ms          | 0.0026 | Standard baseline |
| **WebRTC VAD**  | ~3 ms            | 0.00005| Legacy            |

Codec support: PCM16, G.711 (PCMU/PCMA), G.722, Opus.

## Quick Start

```bash
# Webhook handler
./active-call --handler https://example.com/webhook

# Playbook handler
./active-call --handler config/playbook/greeting.md

# Outbound SIP call
./active-call --call sip:1001@127.0.0.1:5060 --handler greeting.md

# With external IP and codecs
./active-call --handler default.md --external-ip 1.2.3.4 --codecs pcmu,pcma,opus
```

### Docker

```bash
docker run -d --net host \
  --name active-call \
  -v $(pwd)/config.toml:/app/config.toml:ro \
  -v $(pwd)/config:/app/config \
  ghcr.io/miuda-ai/active-call:latest
```

### Playbook Handler Routing

```toml
[handler]
type = "playbook"
default = "greeting.md"

[[handler.rules]]
caller = "^\\+1\\d{10}$"
callee = "^sip:support@.*"
playbook = "support.md"

[[handler.rules]]
caller = "^\\+86\\d+"
playbook = "chinese.md"
```

## Carrier Edition (SBC + B2BUA)

Active Call includes a carrier-grade Session Border Controller with 84 REST API endpoints for full SIP infrastructure management. See [Carrier Architecture](./docs/CARRIER-ARCHITECTURE.md) for the complete design.

### Carrier Features

- **SIP Proxy (B2BUA)**: Dual-dialog bridge with RTP relay, codec optimization, failover
- **Bridge Modes**: SIP-to-WebRTC (G.711/Opus transcoding, ICE/DTLS), SIP-to-WebSocket
- **Routing Engine**: LPM, exact match, regex, HTTP query, weighted distribution, table jumps
- **Number Translation**: Regex-based caller/destination rewriting per direction
- **SIP Manipulation**: Conditional header modification with AND/OR logic
- **Capacity Management**: Token bucket CPS (Redis ZSET), concurrent call limits, auto-block with escalation
- **SIP Security**: IP firewall (CIDR), flood protection, brute-force blocking, UA blacklist, topology hiding
- **DSP Processing**: Echo cancellation, inband DTMF (Goertzel), T.38 fax, tone detection, PLC (via SpanDSP C FFI)
- **CDR Engine**: Dual-leg carrier CDR, Redis queue, webhook delivery with retry, disk fallback
- **Gateway Health**: OPTIONS ping with configurable thresholds, auto-disable/recover
- **Trunks**: Group gateways with weights/priorities, capacity limits, codec policies, IP ACLs, DID routing
- **Clustering**: Active-active via shared Redis (config, capacity, CDR, pub/sub)

### Carrier Quick Start

```bash
# Build with carrier features (requires libsofia-sip-ua-dev + libspandsp-dev)
cargo build --release --features carrier

# Or use Docker
docker build -f Dockerfile.carrier -t active-call:carrier .
docker run --net host active-call:carrier --config config.toml
```

### Carrier API Example

```bash
# Create an endpoint
curl -X POST http://localhost:8080/api/v1/endpoints \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"carrier-ext","stack":"sofia","bind_addr":"0.0.0.0","port":5060}'

# Create a gateway
curl -X POST http://localhost:8080/api/v1/gateways \
  -H "Authorization: Bearer $API_KEY" \
  -d '{"name":"twilio","proxy_addr":"sip.twilio.com:5060","transport":"tls"}'

# Create a trunk with weighted gateways
curl -X POST http://localhost:8080/api/v1/trunks \
  -H "Authorization: Bearer $API_KEY" \
  -d '{"name":"us-trunk","direction":"both","gateways":[{"name":"twilio","weight":60}]}'

# Assign a DID to route to AI agent
curl -X POST http://localhost:8080/api/v1/dids \
  -H "Authorization: Bearer $API_KEY" \
  -d '{"number":"+14155551234","trunk":"us-trunk","routing":{"mode":"ai_agent","playbook":"support.md"}}'

# Check system health
curl -H "Authorization: Bearer $API_KEY" http://localhost:8080/api/v1/system/health
```

### Feature Flags

```toml
[features]
carrier = ["sofia-sip", "spandsp"]   # C FFI carrier features (default)
minimal = []                          # Pure Rust, no C dependencies
```

Build with `--no-default-features` for a pure Rust binary without carrier SBC features.

### SIP Carrier Integration

#### TLS + SRTP (Required by Twilio)

```toml
tls_port      = 5061
tls_cert_file = "./certs/cert.pem"
tls_key_file  = "./certs/key.pem"
enable_srtp   = true
```

- [Twilio Elastic SIP Trunking Guide](./docs/twilio_integration.md)
- [Telnyx SIP Trunking Guide](./docs/telnyx_integration.md)

## Environment Variables

```bash
# OpenAI / Azure
OPENAI_API_KEY=sk-...
AZURE_OPENAI_API_KEY=...
AZURE_OPENAI_ENDPOINT=https://your-resource.openai.azure.com/

# Aliyun DashScope
DASHSCOPE_API_KEY=sk-...

# Tencent Cloud
TENCENT_APPID=...
TENCENT_SECRET_ID=...
TENCENT_SECRET_KEY=...

# Offline models
OFFLINE_MODELS_DIR=/path/to/models
```

## Demo

![Playbook demo](./docs/playbook.png)

## SDKs

- **Go**: [rustpbxgo](https://github.com/restsend/rustpbxgo) — Official Go client

## Documentation

| Language | Links |
|----------|-------|
| **English** | [Docs Hub](./docs/en/README.md) · [API Reference](./docs/api.md) · [Config Guide](./docs/en/config_guide.md) · [Playbook Tutorial](./docs/en/playbook_tutorial.md) · [Advanced Features](./docs/en/playbook_advanced_features.md) |
| **中文** | [文档中心](./docs/zh/README.md) · [API 文档](./docs/api.md) · [配置指南](./docs/zh/config_guide.md) · [Playbook 教程](./docs/zh/playbook_tutorial.md) · [高级特性](./docs/zh/playbook_advanced_features.md) |

## License

MIT — see [LICENSE](./LICENSE)
