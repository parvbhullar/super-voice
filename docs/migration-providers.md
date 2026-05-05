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

## Rollback

The legacy single-`provider` form parses unchanged. To roll back from
fallback to a single provider, delete the `providers:` block and
restore the original `provider:` field. No code change required.

If you need to roll back the binary itself (e.g. revert the resilience
change set), the offline models stay on disk and are simply not
loaded; the bare `provider: tencent`-style configs continue to work.
