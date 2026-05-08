# Migrating to provider chains

The resilient-voice-pipeline change introduces ordered provider
chains for ASR, TTS, and LLM. The legacy single-`provider` field still
parses — your existing playbooks and configs continue to work
unchanged. This doc shows how to opt in to fallback.

## ASR

### Before

```yaml
asr:
  provider: tencent
```

### After (with offline fallback)

```yaml
asr:
  providers:
    - tencent
    - sensevoice
```

The wrapper tries `tencent` first. On a connection / timeout / 5xx
error, it advances to `sensevoice` (in-process ONNX). Each provider
gets its own circuit breaker.

### Single-provider equivalence

```yaml
# These are equivalent:
asr:
  provider: tencent

asr:
  providers:
    - tencent
```

A one-element chain skips the wrapper entirely (no fallback
overhead).

## TTS

### Before

```yaml
tts:
  provider: aliyun
  voice: zh-CN-XiaoxiaoNeural
```

### After (with offline fallback)

```yaml
tts:
  providers:
    - aliyun
    - supertonic
  voice: zh-CN-XiaoxiaoNeural
```

Per-provider config (voices, models, etc.) lives under each entry
when supported by the provider; for now `voice` is shared across
the chain.

## LLM

LLM uses richer per-provider config because endpoints differ across
providers (different base URLs, models, API keys).

### Before

```yaml
llm:
  provider: openai
  base_url: https://api.openai.com/v1
  api_key: $OPENAI_API_KEY
  model: gpt-4o-mini
```

### After (cloud → cloud → offline)

```yaml
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
    - provider: phi3        # requires --features offline-llm
```

### Notes

- `phi3` (aliases: `phi-3`, `candle`, `offline-llm`) loads the
  in-process Candle Phi-3-mini-4k-instruct GGUF. Drop tools from your
  playbook if you rely on the offline tier — Phi-3-mini ignores tool
  schemas.
- Cloud entries fall through to `DefaultLlmProvider` regardless of the
  `provider` value; the field is just a label used for circuit-breaker
  routing and metric tags.
- An explicitly empty `providers: []` is a config error and is rejected
  at parse time. To remove fallback, omit `providers` entirely or use
  the legacy single-`provider` form.

## Behavioural changes

| Behaviour | Before | After |
|---|---|---|
| Cloud TTS websocket drops mid-stream | Call drops | Call drops (mid-stream is application concern) |
| Cloud TTS fails before first audio | Call drops | Falls over to next provider |
| LLM 5xx during peak | Call drops | Falls over to next provider, up to 8 s budget |
| Redis unreachable during enqueue | CDR lost | CDR buffered, flushed on recovery |
| All providers in chain fail | Call drops | Call drops (with chain-exhausted error) |

## Local / self-hosted LLM

Two modes are supported. Use whichever fits your deploy.

### Option A — In-process via llama.cpp (recommended for offline-first)

Gemma 4 2B IT runs **inside the active-call process** via the
`offline-gemma4` cargo feature. No sidecar, no network hop, Metal/CUDA
acceleration picked up automatically.

```bash
# 1. Build with the feature
cargo build --release --no-default-features \
  --features "opus offline offline-gemma4"

# 2. Download the GGUF (~1.5 GB)
./active-call --download-models gemma4 --models-dir ./models
```

Playbook config:

```yaml
llm:
  providers:
    - provider: openai        # cloud primary
      base_url: https://api.openai.com/v1
      api_key: $OPENAI_API_KEY
      model: gpt-4o-mini
    - provider: gemma4        # in-process fallback, no base_url needed
```

The alias `gemma4` (also `gemma-4`, `gemma`) is recognised by the
binary and routes to `LlamaGemma4Provider`. No `base_url` or `api_key`
is required for the in-process tier.

**Memory**: ~1.5 GB resident for the Q4_K_M 2B model.

### Option B — vLLM sidecar (recommended for GPU-heavy workloads)

Any process that speaks the OpenAI `/v1/chat/completions` protocol works
as an LLM provider — no code changes required. Point `base_url` at the
local endpoint.

Start the sidecar (see `gemma4_llm_server.py`):

```bash
python gemma4_llm_server.py \
  --model prithivMLmods/gemma-4-E2B-it-FP8 \
  --host 0.0.0.0 --port 8002
```

Playbook config:

```yaml
llm:
  providers:
    - provider: gemma4-runpod   # any label — used for circuit-breaker key
      base_url: http://localhost:8002/v1
      api_key: unused            # vLLM ignores this; supply any non-empty string
      model: gemma-4
      timeout_ms: 15000
    - provider: openai
      base_url: https://api.openai.com/v1
      api_key: $OPENAI_API_KEY
      model: gpt-4o-mini
    - provider: phi3             # offline last resort (--features offline-llm)
```

When `base_url` is set, the entry routes to `DefaultLlmProvider`
regardless of the `provider` label. The label is only used for
circuit-breaker isolation and metric tags.

### Notes

- Tool calls: the in-process Gemma 4 tier drops tool schemas (plain text
  response only). The sidecar path forwards them as-is — Gemma 4 IT
  understands OpenAI function-calling JSON with vLLM ≥ 0.6.
- Set `timeout_ms` ≥ 15 s for first-token latency on GPU-cold starts.
  The fallback budget (default 8 s for the whole chain) is measured from
  the first attempt, not from an individual provider timeout.

## Rollback

The legacy single-`provider` form parses unchanged. To roll back from
fallback to a single provider, delete the `providers:` block and
restore the original `provider:` field. No code change required.

If you need to roll back the binary itself (e.g. revert the resilience
change set), the offline models stay on disk and are simply not
loaded; the bare `provider: tencent`-style configs continue to work.
