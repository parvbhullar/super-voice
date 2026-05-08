---
llm:
  providers:
    # Primary: Gemma 4 E2B FP8 vLLM sidecar (just download-gemma4-fp8 + gemma4_llm_server.py)
    - provider: "gemma4-sidecar"
      baseUrl: "${GEMMA4_BASE_URL:-http://localhost:8002/v1}"
      apiKey: "${GEMMA4_API_KEY:-unused}"
      model: "gemma-4"
      timeoutMs: 15000
    # Fallback: OpenAI-compatible cloud
    - provider: "openai"
      baseUrl: "${OPENAI_BASE_URL:-https://api.openai.com/v1}"
      apiKey: "${OPENAI_API_KEY}"
      model: "${OPENAI_MODEL:-gpt-4o-mini}"
      timeoutMs: 10000

tts:
  provider: "msedge"
  speaker: "en-US-AriaNeural"
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
