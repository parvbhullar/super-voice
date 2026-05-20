# Playbook Examples

This directory contains example Active-Call Playbook configurations.

A playbook is a Markdown file with a YAML frontmatter that selects the ASR, TTS,
LLM, and VAD providers (and other call options), followed by the system prompt
the LLM should follow during the call.

## ⚠️ Allowed provider values

A playbook MUST use only provider identifiers that are registered in
`StreamEngine::default()` for the current build. Anything else will trigger
`failed to prepare stream processors: ASR/TTS type not registered: <name>`
at call setup, the WebRTC handshake will still succeed, and then audio will
go nowhere — no greeting out, no transcript in. That failure is silent unless
you read the server log.

| Field            | Allowed values                                              |
|------------------|-------------------------------------------------------------|
| `asr.provider`   | `sensevoice`, `tencent`, `aliyun`                           |
| `tts.provider`   | `supertonic`, `aliyun`, `tencent`, `tencent_basic`, `deepgram` |
| `vad.provider`   | `silero`, `nop`                                             |
| `llm.provider`   | any value supported by the LLM dispatcher (e.g. `openai`, `aliyun`, `gemma4`, `gemma4-sidecar`, `candle`) |

For ASR/TTS fallback chains (`asr.providers: [...]`, `tts.providers: [...]`)
every entry must also come from the lists above.

**Explicitly not supported in this build (do not use):**

- `openai` as `asr.provider` or `tts.provider` — only the LLM dispatcher
  understands `openai`. For OpenAI ASR+TTS in one stream, see the separate
  Realtime API code path (set the `realtime:` block instead of `asr:` and
  `tts:`), not implemented as a default in any shipped playbook.
- `msedge` as `tts.provider`.
- Whisper variants (`whisper`, `faster-whisper`, `whisper-ct2`,
  `whisper-hindi2hinglish`, etc.) — not yet integrated. Tracked as a
  future change in `openspec/`.

The lint script `scripts/check_playbook_providers.sh` enforces this list and
runs as part of `bash scripts/run_tests.sh`.

## 🏁 First-time setup

The shipped playbooks default to fully-offline ASR and TTS, so a fresh clone
just needs the model weights on disk:

```bash
just download-sensevoice   # ~50 MB  — required by all playbooks (ASR)
just download-supertonic   # ~100 MB — required by all playbooks (TTS)
```

The default `hello.md` LLM chain prefers an in-process Gemma-4 sidecar and
falls back to OpenAI cloud. Pick one of:

```bash
# Option A: run the local Gemma-4 vLLM sidecar
just download-gemma4-fp8
python gemma4_llm_server.py --host 0.0.0.0 --port 8002

# Option B: rely on the cloud fallback
export OPENAI_API_KEY="sk-..."
```

Cloud ASR / TTS playbooks need their own credentials:

- `aliyun` ASR/TTS → `ALIYUN_API_KEY` / `DASHSCOPE_API_KEY` (see
  `webhook_example.md`, `simple_crm.md`, `advanced_example.md` — these were
  ported to `sensevoice`/`supertonic` for the default experience, but the
  template lines remain for users who want cloud).
- `tencent` ASR/TTS → `TENCENT_SECRET_ID`, `TENCENT_SECRET_KEY`, `TENCENT_APPID`.
- `deepgram` TTS → `DEEPGRAM_API_KEY`.

## 📚 Example index

### Starter

- **[hello.md](./hello.md)** — minimal: `sensevoice` ASR + `supertonic` TTS +
  `gemma4-sidecar`/`openai` LLM fallback chain. This is the default playbook
  the web UI runs when you click "Run".

### Scene / DTMF

- **[multi_scene.md](./multi_scene.md)** — scene switching, DTMF input,
  refer/hangup actions. Now uses the offline ASR/TTS stack.

### SIP integration

- **[simple_crm.md](./simple_crm.md)** ⭐ — SIP header extraction,
  `<set_var>`, BYE headers. Good entry point for SIP-driven flows.

### HTTP tools

- **[webhook_example.md](./webhook_example.md)** ⭐ — calling external HTTP
  APIs from the playbook (`<http url=… />`).

### Advanced

- **[advanced_example.md](./advanced_example.md)** 🚀 — full customer-service
  flow with headers, variables, HTTP tooling, multiple intents.
- **[fallback_example.md](./fallback_example.md)** — ASR / TTS / LLM
  fallback chains. Demonstrates provider-list ordering.
- **[gemma4_example.md](./gemma4_example.md)** — in-process Gemma 4 (no
  sidecar) with a cloud LLM backup.
- **[env_vars_example.md](./env_vars_example.md)** — `${VAR}` substitution
  across all configuration fields.

## 🛠 Configuration anatomy

```yaml
---
asr:
  provider: "sensevoice"                 # offline ONNX, no key required
  language: "auto"

tts:
  provider: "supertonic"                 # offline ONNX, no key required
  speaker: "F1"                          # F1, F2, M1, M2
  speed: 1.0

vad:
  provider: "silero"

llm:
  provider: "openai"                     # see allowed values above
  model: "gpt-4o-mini"
  apiKey: "${OPENAI_API_KEY}"

denoise: true
greeting: "Hello, how can I help you?"

sip:                                     # optional, only for SIP calls
  extract_headers: ["X-Customer-ID"]
  hangup_headers:
    X-Hangup-Reason: "{{ reason }}"
---

You are a helpful assistant. Keep responses concise.
```

The Markdown body after the frontmatter is the system prompt the LLM follows
during the call.

## 🎯 Running a playbook

The webapp at `http://localhost:18080/` lets you pick a playbook from the
dropdown, edit it in the textarea, and click **Run** to start a WebRTC call.
The dropdown loads from `/api/playbooks`; the textarea content (not the
dropdown name) is what actually gets sent for the call. If you edit the
textarea, save back with the **Save** button before relying on the file on
disk.

For SIP calls, enter a callee URI and click **SIP Call**.

## 🐛 Troubleshooting

### "ASR type not registered: openai" / "TTS type not registered: msedge"

Your playbook references a provider that is not in `StreamEngine::default()`.
Pick a value from the [allowed list](#-allowed-provider-values) and try again,
or run `bash scripts/check_playbook_providers.sh` to be told which file and
field is wrong.

### Voice connects but I hear silence

Usually one of:
- Missing model weights (`models/sensevoice/`, `models/supertonic/`). Run the
  matching `just download-*` recipe.
- Cloud provider chosen without API keys in the environment.
- LLM sidecar referenced but not running (`gemma4-sidecar` expects an HTTP
  service on `GEMMA4_BASE_URL`; otherwise the chain falls through to
  `openai`, which needs `OPENAI_API_KEY`).

### Variables not landing in BYE headers

- Only SIP calls deliver BYE headers; WebRTC does not.
- `sip.hangup_headers` must be set in the playbook frontmatter.
- The variable must be set via `<set_var>` *before* `<hangup/>`.

## 🤝 Contributing examples

1. Fork the repo.
2. Create a new file in `config/playbook/`.
3. Use only [allowed providers](#-allowed-provider-values).
4. Run `bash scripts/check_playbook_providers.sh` and make sure it passes.
5. Add the example to the index above.
6. Submit a PR.

Naming convention: `<use_case>_<feature>.md`, e.g.
`customer_service_basic.md`, `order_assistant_webhook.md`.

## 🔭 Roadmap (not yet supported)

- OpenAI ASR + OpenAI TTS as standalone providers (today only OpenAI Realtime
  is wired up, via the separate `realtime:` block).
- Whisper STT (the `whisper-hindi2hinglish-ct2` model in
  `super-model/inference/stt/legacy/`). Integration paths under
  consideration: CTranslate2 FFI, ONNX-converted weights, or a
  faster-whisper HTTP sidecar.
- Startup-time validation that every referenced provider has its model
  weights or credentials available, so the failure is loud instead of silent.

Track these in the `openspec/changes/` directory.
