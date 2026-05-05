# Resilient Voice Pipeline

This document covers the fallback, circuit-breaker, offline-model and
CDR-buffer behaviour added by the resilient-voice-pipeline change.

## Why

In production, single-provider failures observably degrade calls:
cloud TTS websocket disconnects on ~0.1% of calls, LLM endpoint 5xx
during peak on ~0.5%, occasional Redis blips during deploys. Each one
used to either drop a call or lose a CDR. The resilient pipeline turns
those failures into recoverable fallbacks.

Three concerns, one design:

1. **Provider failure**: each AI tier (ASR / TTS / LLM) accepts an
   ordered chain of providers. On a retryable failure the wrapper
   advances to the next provider, with per-provider circuit breakers
   so a sick provider gets cooled off rather than re-probed.
2. **Cloud unavailability**: an in-process Candle Phi-3 LLM, plus
   ONNX SenseVoice ASR and Supertonic TTS, can serve traffic at
   degraded quality with no cloud reachable.
3. **Redis unavailability**: a bounded in-memory CDR buffer absorbs
   enqueue failures, drains back to Redis on recovery, spills to disk
   on shutdown.

## Provider chains

### Config shape

`asr`, `tts`, and `llm` each accept a `providers` list. The first
entry is primary; subsequent entries are tried in order on retryable
failure. Single-provider deploys keep working unchanged.

```yaml
asr:
  providers:
    - tencent
    - sensevoice          # offline ASR fallback
tts:
  providers:
    - aliyun
    - supertonic          # offline TTS fallback
llm:
  providers:
    - provider: openai
      base_url: https://api.openai.com/v1
      api_key: $OPENAI_API_KEY
      model: gpt-4o-mini
    - provider: deepseek
      base_url: https://api.deepseek.com/v1
      api_key: $DEEPSEEK_API_KEY
      model: deepseek-chat
    - provider: phi3      # in-process Candle (requires `offline-llm` feature)
```

The legacy single-`provider` field still parses; it's wrapped to a
one-element chain at parse time.

### What counts as retryable

`classify_error` in `src/resilience/fallback.rs`:

- **Retryable** (advance to next provider): connection / TLS error,
  websocket close, HTTP 5xx, request timeout, empty/malformed
  response.
- **Terminal** (return error to caller): HTTP 4xx, missing-key,
  config error.

### Fallback budget

Each LLM call has a per-tier total budget (default 8 s). Once
`Instant::now() - start >= budget`, the wrapper stops trying further
providers and returns an error. The application should play "please
hold" and retry rather than block the caller forever.

Configured as `FallbackBudget::default()` in
`src/resilience/fallback.rs`. Override per-handler when constructing
`FallbackLlmProvider`.

## Circuit breakers

Each provider in each chain has its own breaker, shared across calls
in the process via `resilience::registry`.

State machine:

- **Closed** — healthy, route normally.
- **Open** — failed N times in `window`, skip without attempt for
  `cooldown`.
- **HalfOpen** — cooldown elapsed, allow one trial. Success → Closed
  with reset cooldown. Failure → Open with doubled cooldown (capped
  at `max_cooldown`).

### Defaults

| Setting | Default | Override |
|---|---|---|
| `failure_threshold` | 3 | `CircuitBreakerConfig::failure_threshold` |
| `window` | 60 s | `CircuitBreakerConfig::window` |
| `cooldown` | 30 s | `CircuitBreakerConfig::cooldown` |
| `max_cooldown` | 300 s | `CircuitBreakerConfig::max_cooldown` |

Per-process state, not distributed. Each replica has its own opinion
about each provider; this is acceptable for a 30 s cooldown — the
complexity of a distributed CB isn't worth it for the recovery
latency we'd save.

### When to tune

- **Provider fails fast at the network layer** (TCP refused, TLS
  handshake): leave defaults. Three failures over 60 s is a clear
  signal.
- **Provider half-fails** (responses are returned but garbage):
  raise `failure_threshold` to 5 or extend `window` to 300 s so a
  single client-side hiccup doesn't trip the breaker.
- **Provider routinely 502s** (e.g. flaky CDN): lower `cooldown` to
  10 s and extend `max_cooldown` to 60 s to recover quickly without
  thrashing.

## Offline models

Offline tiers exist so a deploy can serve traffic with no cloud
reachable, at degraded quality, instead of dropping calls.

### What's available

| Tier | Provider name | Backend | File location |
|---|---|---|---|
| ASR | `sensevoice` | ONNX (`ort`) | `models/sensevoice/{model.int8.onnx,tokens.txt}` |
| TTS | `supertonic` | ONNX (`ort`) | `models/supertonic/{onnx/*,voice_styles/*}` |
| LLM | `phi3` / `phi-3` / `candle` / `offline-llm` | Candle (GGUF) | `models/llm/{Phi-3-mini-4k-instruct-q4.gguf,tokenizer.json}` |

### Setup

```bash
# Download all three (≈ 3.5 GB total)
active-call --download-models all
# Or selectively:
active-call --download-models sensevoice
active-call --download-models supertonic
active-call --download-models llm
```

The LLM tier requires a build with `--features offline-llm` (it adds
~250 MB of compile-time deps). Without that feature, a playbook that
references `phi3` will fail at startup with a clear error message.

### Eager init

When a playbook references an offline tier, the binary scans those
chains at startup, verifies each referenced model's files are
present, and loads the model into memory before serving traffic.
Missing files are a hard startup failure — better to fail boot than
mid-call.

### Memory budget

Approximate resident memory for each tier:

| Model | Memory |
|---|---|
| SenseVoice (ASR) | ~250 MB |
| Supertonic (TTS) | ~150 MB |
| Phi-3-mini Q4_K_M (LLM) | ~2.4 GB |

If your deploy can't carry the LLM, leave it out of the playbook
chain. The other two tiers stand alone.

## CDR buffer

When the call path can't reach Redis to enqueue a CDR, the record
goes into an in-process `LocalCdrBuffer` instead of being dropped.

### Behaviour

- **Fast path**: successful Redis enqueue returns immediately,
  bypassing the buffer.
- **Outage**: enqueue failure or 500 ms timeout pushes the record
  into the buffer.
- **Recovery**: a 5 s background tick drains the buffer back to Redis
  in batches of up to 256 records.
- **Capacity** (10 k records, ~10 MB): when full, the oldest record
  is spilled to disk via the existing hourly fallback path before the
  new record is pushed.
- **Shutdown**: graceful stop spills all remaining records to disk
  with a 10 s deadline. SIGTERM never loses a buffered record as long
  as disk is writable.

### What you can tune

`LocalCdrBuffer::with_capacity(fallback_dir, capacity)` for the cap,
`DEFAULT_FLUSH_INTERVAL` for the tick rate, `ENQUEUE_TIMEOUT` for the
hung-Redis-socket bound. Defaults match the resilient-pipeline design.

## Observability

The fallback wrappers emit structured-log events at every transition.
Field names are picked so a Loki / Vector → Prometheus pipeline can
scrape them as metrics without code changes:

| Metric | Tier | When |
|---|---|---|
| `provider_fallback_total{tier, from_provider, to_provider, reason}` | counter | each retryable failure or circuit-open skip |
| `provider_circuit_state{provider, state}` | gauge | each CB state transition |
| `llm_tier_used{tier}` | counter | each successful LLM completion (`tier` = chain index) |
| `offline_model_init_seconds{model}` | histogram | per-model duration on eager init |
| `cdr_buffer_depth` | gauge | each push and flush |
| `cdr_buffer_flush_total{result}` | counter | each drained record |
| `cdr_buffer_spill_total` | counter | each capacity-bound or shutdown spill |

### Reading the events

In tracing JSON output:

```json
{"level":"WARN","fields":{"metric":"provider_fallback_total","tier":"llm","from_provider":"openai","to_provider":"deepseek","reason":"5xx","error":"LLM request failed: 503","message":"llm provider failed, trying next"}}
```

In a Prom scrape pipeline (Vector example):

```toml
[transforms.parse]
type = "remap"
inputs = ["loki"]
source = '''
.metric_name = .fields.metric
.metric_kind = "counter"
'''
```

## Failure modes that are still possible

The resilient pipeline removes the most common single-provider
failures. It does not prevent:

- All providers in a tier failing simultaneously (the wrapper
  surfaces the last error after exhausting the chain).
- Tail-of-stream failures during streaming TTS / LLM: once a chunk
  has been sent to the caller, mid-stream failover would require
  rolling back partial output, which is application-level concern.
- Disk fallback overflow during a sustained Redis outage past
  ~10 k CDRs (oldest records spill but are still bounded by disk).
- Per-process circuit breaker state divergence across replicas (one
  replica's "Open" doesn't propagate; another may waste a request on
  a bad provider).

These are documented trade-offs from the design, not bugs.
