---
llm:
  providers:
    # Primary: in-process Phi-3 via Candle (requires --features offline-llm
    # at build time; run `just download-llm` once to fetch the weights).
    - provider: "candle"
      model: "phi3-mini-4k-instruct-q4_k_m"
      timeoutMs: 30000
    # Fallback: OpenAI-compatible cloud (only used if OPENAI_API_KEY is set).
    - provider: "openai"
      baseUrl: "${OPENAI_BASE_URL:-https://api.openai.com/v1}"
      apiKey: "${OPENAI_API_KEY}"
      model: "${OPENAI_MODEL:-gpt-4o-mini}"
      timeoutMs: 10000

asr:
  provider: "sensevoice"

tts:
  provider: "supertonic"
  speaker: "F1"
  speed: 1.0

vad:
  provider: "silero"
denoise: true
greeting: "Hello, how can i help you?"
interruption:
  strategy: "both"
followup:
  timeout: 10000
  max: 2
recorder:
  recorderFile: "hello_{id}.wav"
ambiance:
 path: "./config/office.wav"
 duckLevel: 0.1
 normalLevel: 0.5
 transitionSpeed: 0.1
---
# Role and Purpose
You are an intelligent, polite AI assistant. Your goal is to help users with their inquiries efficiently.

# Tool Usage
- When the user expresses a desire to end the conversation (e.g., "goodbye", "hang up", "I'm done"), you MUST provide a polite closing statement AND output `<hangup/>`.

# Example Response for Hanging Up:
Goodbye! <hangup/>
---
