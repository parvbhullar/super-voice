# Changelog

## Unreleased — Resilient voice pipeline

Two-layer fallback (online → online → offline) for ASR, TTS, and LLM,
plus CDR durability across Redis outages.

### Added

- `providers: [...]` chain configuration on `asr`, `tts`, and `llm`
  playbook sections. Tries each provider in order on retryable
  failure (connection / timeout / 5xx).
- Per-provider `CircuitBreaker` (closed / open / half-open) shared
  across calls in the process via `resilience::registry`. Defaults:
  3 failures in 60 s opens, 30 s cooldown doubling to 300 s on
  repeated trial failures.
- Per-tier `FallbackBudget` (default 8 s) on the LLM wrapper so a
  cascading outage can't pin a call indefinitely.
- Offline LLM tier (Phi-3): Candle-backed Phi-3-mini-4k-instruct GGUF
  with token-by-token streaming, drop-tools chat-template rendering, and
  per-request `max_tokens` / `max_inference` bounds. Gated on the
  `offline-llm` cargo feature.
- Offline LLM tier (Gemma 4): llama.cpp-backed Gemma 4 2B IT Q4_K_M
  GGUF via the `llama-cpp-2` crate. Metal/CUDA acceleration picked up
  automatically; ~1.5 GB RAM vs ~2.4 GB for Phi-3. Provider aliases:
  `gemma4`, `gemma-4`, `gemma`. Gated on the new `offline-gemma4`
  cargo feature. Tool schemas are dropped on the in-process path
  (use the vLLM sidecar route for tool-call support).
- Eager initialization of every offline model referenced by a
  playbook chain at startup. Missing files are a hard boot failure
  rather than a mid-call surprise.
- `--download-models llm` for the Phi-3 GGUF + tokenizer.
- `--download-models gemma4` for the Gemma 4 2B IT Q4_K_M GGUF.
- `LocalCdrBuffer` between the call path and Redis. Outages route
  records to the buffer (capacity 10k); a 5 s background tick drains
  back to Redis on recovery; graceful shutdown spills the remainder
  to the existing hourly disk fallback within a 10 s deadline.
- Structured-log events at every fallback transition that downstream
  pipelines can scrape as Prometheus metrics:
  `provider_fallback_total`, `provider_circuit_state`,
  `llm_tier_used`, `cdr_buffer_depth`, `cdr_buffer_flush_total`,
  `cdr_buffer_spill_total`, `offline_model_init_seconds`.

### Changed

- `proxy/dispatch.rs` CDR enqueue now goes through `enqueue_resilient`
  with a 500 ms timeout. Redis hangs no longer block the call's
  hangup path.
- `LlmHandler::build_default_provider` dispatches by entry name —
  cloud entries route to `DefaultLlmProvider`, `phi3`-aliased entries
  route to `CandlePhi3Provider` (when built with `offline-llm`).
- `CircuitBreaker` carries an optional name (set by the registry to
  `tier:provider`) so state-transition events are fully tagged.

### Fixed (pre-work for the resilience change)

- `do_interrupt` now clears `current_play_id` alongside `tts_handle`,
  so a stale `TrackEnd` from the interrupted track can't match a
  future `play_id` and corrupt state.
- `TrackEnd` handler adds an SSRC mismatch guard: events from a
  previously-interrupted track are dropped if the active `tts_handle`
  has a different SSRC.
- `do_tts` filters whitespace-only chunks at entry, avoiding spurious
  `TrackEnd` events from streaming LLM punctuation.

### Compatibility

The legacy single-`provider: string` form on `asr`, `tts`, and `llm`
still parses unchanged. To opt in to fallback, replace `provider:` with
a `providers:` list. An explicitly empty `providers: []` is rejected
at parse time.

Rolling back the binary leaves the offline models on disk; bare
single-provider configs continue to work without code changes. The
buffer flush task and CDR buffer are off when no Redis is configured.
