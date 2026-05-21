# Supervoice v1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Pipecat-based speech service that ingests audio, performs VAD/EOU/STT, forwards user transcripts as text to a remote Agent Bridge over WSS, receives agent text replies, runs TTS, and emits audio back — with no LLM ever in-process.

**Architecture:** Python 3.12 + Pipecat orchestrator. WebRTC transport for v1. Single `AgentBridgeProcessor` replaces Pipecat's in-process LLM with a persistent WSS connection to a remote Agent Bridge. VAD + Smart-Turn EOU sit behind a `TurnDetector` protocol so they can later be swapped for a Rust+PyO3 implementation (echokit pattern) without touching the pipeline. Voice profiles abstract STT/TTS provider selection with config-driven failover chains.

**Tech Stack:** Python 3.12, uv, Pipecat 0.0.92+, FastAPI, Pydantic v2, pytest + pytest-asyncio, ruff, pyrefly. STT: Deepgram (primary), Cartesia (fallback). TTS: Cartesia (primary), ElevenLabs (fallback). VAD: Silero (Pipecat-bundled). EOU: SmartTurnAnalyzerV3 (Pipecat-bundled).

**Reference implementations to lift from:**
- `/Users/parvbhullar/Drives/Vault/Projects/Unpod/super/super/core/voice/pipecat/lite_v2/` — SessionState, idle monitor, tool patterns
- `/Users/parvbhullar/Drives/Vault/Projects/Unpod/super-voice/third-party/dograh/api/services/pipecat/` — service_factory provider abstraction
- `/Users/parvbhullar/Drives/Vault/Projects/Unpod/super-voice/third-party/sayna/src/pipeline/tts/sanitize.rs` — TTS sanitize regex set
- `/Users/parvbhullar/Drives/Vault/Projects/Unpod/super-voice/third-party/open-source-av-ragbot/` — Pipecat composition patterns

**Working directory:** `/Users/parvbhullar/Drives/Vault/Projects/Unpod/super-voice/supervoice/`

**Out of scope for v1:** SIP/Twilio (use existing media-gateway/ downstream), pronunciation overrides, mid-call language switching, custom voice cloning, Agent Bridge implementation itself (we build the client side and a mock server for tests), the developer-facing SDK (separate effort).

---

## Phase 1 — Echo loop (Week 1)

Goal at end of phase: a developer joins a WebRTC room via the test client, speaks, and the system echoes back the transcript synthesized as speech.

### Task 1: Project scaffold

**Files:**
- Create: `supervoice/pyproject.toml`
- Create: `supervoice/src/supervoice/__init__.py`
- Create: `supervoice/.python-version`
- Create: `supervoice/.gitignore`
- Create: `supervoice/README.md`

**Step 1: Initialize uv project**

```bash
cd /Users/parvbhullar/Drives/Vault/Projects/Unpod/super-voice/supervoice
uv init --package --python 3.12 --name supervoice src
mv src/supervoice src/_supervoice && mv src/_supervoice src/supervoice  # ensure flat layout
```

**Step 2: Write `supervoice/pyproject.toml`**

```toml
[project]
name = "supervoice"
version = "0.1.0"
description = "Speech pipeline with text-only Agent Bridge boundary"
requires-python = ">=3.12"
dependencies = [
    "pipecat-ai[silero,webrtc,deepgram,cartesia,elevenlabs]>=0.0.92",
    "fastapi>=0.115",
    "uvicorn[standard]>=0.32",
    "pydantic>=2.9",
    "pydantic-settings>=2.6",
    "websockets>=13.0",
    "loguru>=0.7",
    "pyyaml>=6.0",
]

[dependency-groups]
dev = [
    "pytest>=8.3",
    "pytest-asyncio>=0.24",
    "pytest-mock>=3.14",
    "ruff>=0.7",
    "pyrefly>=0.10",
    "httpx>=0.27",
]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.ruff]
line-length = 88
target-version = "py312"

[tool.pytest.ini_options]
asyncio_mode = "auto"
testpaths = ["tests"]
```

**Step 3: Write `.python-version`**

```
3.12
```

**Step 4: Write `.gitignore`**

```
.venv/
__pycache__/
*.pyc
.pytest_cache/
.ruff_cache/
.pyrefly_cache/
*.log
.env
```

**Step 5: Install and verify**

Run: `cd supervoice && uv sync`
Expected: dependencies install, `.venv/` created.

Run: `uv run python -c "import pipecat; print(pipecat.__version__)"`
Expected: prints a version >= 0.0.92.

**Step 6: Commit**

```bash
git add supervoice/pyproject.toml supervoice/.python-version supervoice/.gitignore supervoice/src/supervoice/__init__.py
git commit -m "chore(supervoice): bootstrap project scaffold"
```

---

### Task 2: Config module

**Files:**
- Create: `supervoice/src/supervoice/config.py`
- Create: `supervoice/tests/test_config.py`
- Create: `supervoice/.env.example`

**Step 1: Write the failing test**

```python
# supervoice/tests/test_config.py
import os
import pytest
from supervoice.config import Settings


def test_settings_loads_from_env(monkeypatch):
    monkeypatch.setenv("AGENT_BRIDGE_URL", "ws://localhost:7000/bridge")
    monkeypatch.setenv("DEEPGRAM_API_KEY", "dg_test")
    monkeypatch.setenv("CARTESIA_API_KEY", "ct_test")
    s = Settings()
    assert s.agent_bridge_url == "ws://localhost:7000/bridge"
    assert s.deepgram_api_key.get_secret_value() == "dg_test"
    assert s.cartesia_api_key.get_secret_value() == "ct_test"
    assert s.host == "0.0.0.0"
    assert s.port == 8080


def test_settings_missing_required_raises(monkeypatch):
    monkeypatch.delenv("AGENT_BRIDGE_URL", raising=False)
    with pytest.raises(Exception):
        Settings()
```

**Step 2: Run, expect failure**

Run: `cd supervoice && uv run pytest tests/test_config.py -v`
Expected: ImportError (module does not exist yet).

**Step 3: Implement**

```python
# supervoice/src/supervoice/config.py
from pydantic import SecretStr
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    """Runtime configuration loaded from environment variables."""

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    host: str = "0.0.0.0"
    port: int = 8080

    agent_bridge_url: str
    agent_bridge_reconnect_max_attempts: int = 5
    agent_bridge_reconnect_initial_delay_ms: int = 200

    deepgram_api_key: SecretStr
    cartesia_api_key: SecretStr
    elevenlabs_api_key: SecretStr | None = None

    idle_warning_timeout_s: int = 30
    idle_disconnect_timeout_s: int = 60
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_config.py -v`
Expected: 2 passed.

**Step 5: Write `.env.example`**

```
AGENT_BRIDGE_URL=ws://localhost:7000/bridge
DEEPGRAM_API_KEY=replace_me
CARTESIA_API_KEY=replace_me
ELEVENLABS_API_KEY=
```

**Step 6: Commit**

```bash
git add supervoice/src/supervoice/config.py supervoice/tests/test_config.py supervoice/.env.example
git commit -m "feat(supervoice): typed settings via pydantic-settings"
```

---

### Task 3: SessionState (lift from lite_v2)

**Files:**
- Create: `supervoice/src/supervoice/session/__init__.py`
- Create: `supervoice/src/supervoice/session/state.py`
- Create: `supervoice/tests/test_session_state.py`

**Reference:** `/Users/parvbhullar/Drives/Vault/Projects/Unpod/super/super/core/voice/pipecat/lite_v2/state.py:1-82`. Adapt — do not copy verbatim. Trim fields we don't use (handover, chat_context).

**Step 1: Write the failing test**

```python
# supervoice/tests/test_session_state.py
import time
from supervoice.session.state import SessionState


def test_session_state_defaults():
    s = SessionState(session_id="abc")
    assert s.session_id == "abc"
    assert s.is_processing is False
    assert s.idle_warning_count == 0
    assert s.shutdown is False
    assert s.transcript == []


def test_mark_processing_resets_idle():
    s = SessionState(session_id="abc")
    s.mark_idle()
    t0 = s.idle_since
    time.sleep(0.01)
    s.mark_processing()
    assert s.is_processing is True
    assert s.idle_since is None


def test_append_transcript():
    s = SessionState(session_id="abc")
    s.append_transcript(role="user", text="hello")
    s.append_transcript(role="agent", text="hi there")
    assert len(s.transcript) == 2
    assert s.transcript[0] == {"role": "user", "text": "hello"}
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_session_state.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/session/state.py
from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any, Literal


@dataclass
class SessionState:
    """Per-call mutable state. One instance lives for the duration of a call."""

    session_id: str
    is_processing: bool = False
    idle_since: float | None = None
    idle_warning_count: int = 0
    shutdown: bool = False
    transcript: list[dict[str, Any]] = field(default_factory=list)
    started_at: float = field(default_factory=time.time)
    ended_at: float | None = None
    voice_profile_id: str | None = None

    def mark_processing(self) -> None:
        self.is_processing = True
        self.idle_since = None

    def mark_idle(self) -> None:
        self.is_processing = False
        self.idle_since = time.time()

    def append_transcript(
        self, role: Literal["user", "agent", "system"], text: str
    ) -> None:
        self.transcript.append({"role": role, "text": text})

    def end(self) -> None:
        self.shutdown = True
        self.ended_at = time.time()
```

Also create `supervoice/src/supervoice/session/__init__.py` (empty).

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_session_state.py -v`
Expected: 3 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/session/ supervoice/tests/test_session_state.py
git commit -m "feat(supervoice): SessionState dataclass with idle + transcript tracking"
```

---

### Task 4: TurnDetector protocol (the swap seam)

**Files:**
- Create: `supervoice/src/supervoice/turn/__init__.py`
- Create: `supervoice/src/supervoice/turn/protocol.py`
- Create: `supervoice/tests/test_turn_protocol.py`

This is the seam where a future Rust+PyO3 echokit-style implementation will plug in. V1 implementation lives in `pipecat_impl.py` (next task).

**Step 1: Write the failing test**

```python
# supervoice/tests/test_turn_protocol.py
from supervoice.turn.protocol import TurnDetector


def test_protocol_is_runtime_checkable():
    class StubDetector:
        async def is_speech(self, frame_pcm: bytes) -> bool:
            return True

        async def is_turn_end(self, transcript_so_far: str, silence_ms: int) -> bool:
            return False

    d = StubDetector()
    assert isinstance(d, TurnDetector)
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_turn_protocol.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/turn/protocol.py
from __future__ import annotations

from typing import Protocol, runtime_checkable


@runtime_checkable
class TurnDetector(Protocol):
    """Hot-path turn detection — VAD + EOU semantics behind one interface.

    V1: Pipecat-backed Silero + SmartTurnAnalyzerV3 (pipecat_impl.py).
    V2: Rust+PyO3 echokit-style crate, swapped in without changing the pipeline.
    """

    async def is_speech(self, frame_pcm: bytes) -> bool:
        """True if the 20-30ms PCM frame contains speech."""
        ...

    async def is_turn_end(
        self, transcript_so_far: str, silence_ms: int
    ) -> bool:
        """True if the user has finished their turn (semantic, not just silence)."""
        ...
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_turn_protocol.py -v`
Expected: 1 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/turn/ supervoice/tests/test_turn_protocol.py
git commit -m "feat(supervoice): TurnDetector protocol — swap seam for VAD/EOU"
```

---

### Task 5: Pipecat-backed TurnDetector implementation

**Files:**
- Create: `supervoice/src/supervoice/turn/pipecat_impl.py`

This is a thin adapter — Pipecat already has `SileroVADAnalyzer` and `SmartTurnAnalyzerV3` built-in. We expose them through our protocol so the pipeline imports only our types.

**Step 1: Implement (no separate test — exercised by pipeline integration test in Task 12)**

```python
# supervoice/src/supervoice/turn/pipecat_impl.py
from __future__ import annotations

from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.audio.turn.smart_turn import LocalSmartTurnAnalyzerV3

from .protocol import TurnDetector


class PipecatTurnDetector:
    """V1 implementation: wraps Pipecat's bundled Silero VAD + SmartTurn EOU.

    These analyzers are normally passed directly to the transport. We keep
    references here so the pipeline-builder can fetch them via the protocol.
    """

    def __init__(self, vad_stop_secs: float = 0.2) -> None:
        self.vad = SileroVADAnalyzer()
        self.vad.params.stop_secs = vad_stop_secs
        self.turn = LocalSmartTurnAnalyzerV3()

    async def is_speech(self, frame_pcm: bytes) -> bool:
        # In v1 we don't drive the analyzer manually — the transport does.
        # This method exists for the swap seam (V2 Rust impl will use it).
        raise NotImplementedError(
            "Pipecat v1 drives VAD inside transport; use .vad directly."
        )

    async def is_turn_end(
        self, transcript_so_far: str, silence_ms: int
    ) -> bool:
        raise NotImplementedError(
            "Pipecat v1 drives EOU inside transport; use .turn directly."
        )


# Static check that our class satisfies the Protocol structurally.
_: TurnDetector = PipecatTurnDetector()  # type: ignore[assignment]
```

**Step 2: Commit**

```bash
git add supervoice/src/supervoice/turn/pipecat_impl.py
git commit -m "feat(supervoice): Pipecat-backed TurnDetector (Silero+SmartTurn)"
```

---

### Task 6: STT factory

**Files:**
- Create: `supervoice/src/supervoice/speech/__init__.py`
- Create: `supervoice/src/supervoice/speech/stt_factory.py`
- Create: `supervoice/tests/test_stt_factory.py`

**Reference:** `third-party/dograh/api/services/pipecat/service_factory.py:42-180`. Trim to two providers.

**Step 1: Write the failing test**

```python
# supervoice/tests/test_stt_factory.py
from pydantic import SecretStr
from supervoice.speech.stt_factory import create_stt, STTProviderConfig


def test_create_deepgram():
    cfg = STTProviderConfig(
        provider="deepgram", api_key=SecretStr("dg_test"), language="en"
    )
    stt = create_stt(cfg)
    # Class name check — avoids importing private types.
    assert stt.__class__.__name__ == "DeepgramSTTService"


def test_create_cartesia():
    cfg = STTProviderConfig(
        provider="cartesia", api_key=SecretStr("ct_test"), language="en"
    )
    stt = create_stt(cfg)
    assert stt.__class__.__name__ == "CartesiaSTTService"


def test_unknown_provider_raises():
    cfg = STTProviderConfig(
        provider="acme", api_key=SecretStr("x"), language="en"
    )
    try:
        create_stt(cfg)
    except ValueError as e:
        assert "acme" in str(e)
    else:
        raise AssertionError("expected ValueError")
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_stt_factory.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/speech/stt_factory.py
from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, SecretStr


STTProvider = Literal["deepgram", "cartesia"]


class STTProviderConfig(BaseModel):
    provider: STTProvider | str
    api_key: SecretStr
    language: str = "en"
    sample_rate: int = 16000


def create_stt(config: STTProviderConfig):
    """Return a Pipecat STT service for the requested provider.

    Imported lazily so a missing provider extra doesn't break startup
    for users who only need the other.
    """
    if config.provider == "deepgram":
        from pipecat.services.deepgram.stt import DeepgramSTTService

        return DeepgramSTTService(
            api_key=config.api_key.get_secret_value(),
            language=config.language,
            sample_rate=config.sample_rate,
        )
    if config.provider == "cartesia":
        from pipecat.services.cartesia.stt import CartesiaSTTService

        return CartesiaSTTService(
            api_key=config.api_key.get_secret_value(),
            language=config.language,
        )
    raise ValueError(f"unknown STT provider: {config.provider}")
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_stt_factory.py -v`
Expected: 3 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/speech/ supervoice/tests/test_stt_factory.py
git commit -m "feat(supervoice): STT factory with Deepgram + Cartesia"
```

---

### Task 7: TTS factory + sanitize

**Files:**
- Create: `supervoice/src/supervoice/speech/tts_factory.py`
- Create: `supervoice/src/supervoice/speech/sanitize.py`
- Create: `supervoice/tests/test_tts_factory.py`
- Create: `supervoice/tests/test_sanitize.py`

**Reference for sanitize:** `third-party/sayna/src/pipeline/tts/sanitize.rs`. Port the regex set to Python.

**Step 1: Write sanitize test**

```python
# supervoice/tests/test_sanitize.py
from supervoice.speech.sanitize import sanitize_for_tts


def test_strips_markdown_bold_and_italic():
    assert sanitize_for_tts("Hello **world** and *friend*") == (
        "Hello world and friend"
    )


def test_strips_inline_code():
    assert sanitize_for_tts("Run `git status` now") == "Run git status now"


def test_strips_code_blocks():
    text = "Use this:\n```python\nprint('hi')\n```\nDone."
    assert sanitize_for_tts(text) == "Use this:\n\nDone."


def test_strips_urls():
    assert sanitize_for_tts("See https://example.com/foo for details") == (
        "See for details"
    )


def test_strips_markdown_headers():
    assert sanitize_for_tts("# Title\nbody") == "Title\nbody"


def test_collapses_whitespace():
    assert sanitize_for_tts("hello   world\n\n\nfoo") == "hello world\n\nfoo"
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_sanitize.py -v`
Expected: ImportError.

**Step 3: Implement sanitize**

```python
# supervoice/src/supervoice/speech/sanitize.py
from __future__ import annotations

import re

_CODE_BLOCK = re.compile(r"```.*?```", flags=re.DOTALL)
_INLINE_CODE = re.compile(r"`([^`]+)`")
_BOLD = re.compile(r"\*\*([^*]+)\*\*")
_ITALIC = re.compile(r"(?<!\*)\*([^*]+)\*(?!\*)")
_HEADER = re.compile(r"^#{1,6}\s+", flags=re.MULTILINE)
_URL = re.compile(r"https?://\S+")
_MULTI_SPACE = re.compile(r"[ \t]+")
_MULTI_NEWLINE = re.compile(r"\n{3,}")


def sanitize_for_tts(text: str) -> str:
    """Strip markdown, code, URLs from agent text before TTS synthesis."""
    text = _CODE_BLOCK.sub("", text)
    text = _INLINE_CODE.sub(r"\1", text)
    text = _BOLD.sub(r"\1", text)
    text = _ITALIC.sub(r"\1", text)
    text = _HEADER.sub("", text)
    text = _URL.sub("", text)
    text = _MULTI_SPACE.sub(" ", text)
    text = _MULTI_NEWLINE.sub("\n\n", text)
    return text.strip(" \t")
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_sanitize.py -v`
Expected: 6 passed.

**Step 5: Write TTS factory test**

```python
# supervoice/tests/test_tts_factory.py
from pydantic import SecretStr
from supervoice.speech.tts_factory import create_tts, TTSProviderConfig


def test_create_cartesia_tts():
    cfg = TTSProviderConfig(
        provider="cartesia",
        api_key=SecretStr("ct_test"),
        voice_id="abc-female-en",
    )
    tts = create_tts(cfg)
    assert tts.__class__.__name__ == "CartesiaTTSService"


def test_create_elevenlabs_tts():
    cfg = TTSProviderConfig(
        provider="elevenlabs",
        api_key=SecretStr("el_test"),
        voice_id="rachel",
    )
    tts = create_tts(cfg)
    assert tts.__class__.__name__ == "ElevenLabsTTSService"


def test_unknown_tts_provider_raises():
    cfg = TTSProviderConfig(
        provider="acme", api_key=SecretStr("x"), voice_id="v"
    )
    try:
        create_tts(cfg)
    except ValueError:
        return
    raise AssertionError("expected ValueError")
```

**Step 6: Run, expect failure**

Run: `uv run pytest tests/test_tts_factory.py -v`
Expected: ImportError.

**Step 7: Implement TTS factory**

```python
# supervoice/src/supervoice/speech/tts_factory.py
from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, SecretStr


TTSProvider = Literal["cartesia", "elevenlabs"]


class TTSProviderConfig(BaseModel):
    provider: TTSProvider | str
    api_key: SecretStr
    voice_id: str
    sample_rate: int = 24000


def create_tts(config: TTSProviderConfig):
    if config.provider == "cartesia":
        from pipecat.services.cartesia.tts import CartesiaTTSService

        return CartesiaTTSService(
            api_key=config.api_key.get_secret_value(),
            voice_id=config.voice_id,
            sample_rate=config.sample_rate,
        )
    if config.provider == "elevenlabs":
        from pipecat.services.elevenlabs.tts import ElevenLabsTTSService

        return ElevenLabsTTSService(
            api_key=config.api_key.get_secret_value(),
            voice_id=config.voice_id,
            sample_rate=config.sample_rate,
        )
    raise ValueError(f"unknown TTS provider: {config.provider}")
```

**Step 8: Run, expect pass**

Run: `uv run pytest tests/test_tts_factory.py -v`
Expected: 3 passed.

**Step 9: Commit**

```bash
git add supervoice/src/supervoice/speech/tts_factory.py supervoice/src/supervoice/speech/sanitize.py supervoice/tests/test_tts_factory.py supervoice/tests/test_sanitize.py
git commit -m "feat(supervoice): TTS factory + sanitize-for-TTS"
```

---

### Task 8: Stub AgentBridgeProcessor (echo mode)

**Files:**
- Create: `supervoice/src/supervoice/bridge/__init__.py`
- Create: `supervoice/src/supervoice/bridge/processor.py`
- Create: `supervoice/tests/test_bridge_echo.py`

This is the v0 of the load-bearing piece: behave like an in-process LLM that just echoes the user's transcript. Real WSS bridge comes in Phase 2.

**Step 1: Write the failing test**

```python
# supervoice/tests/test_bridge_echo.py
import pytest
from unittest.mock import MagicMock

from pipecat.frames.frames import TranscriptionFrame, TextFrame

from supervoice.bridge.processor import AgentBridgeProcessor


@pytest.mark.asyncio
async def test_echo_mode_emits_transcript_as_agent_text():
    proc = AgentBridgeProcessor(echo_mode=True)
    # Capture frames pushed downstream.
    pushed: list = []
    proc.push_frame = MagicMock(side_effect=lambda f, d=None: pushed.append(f))

    frame = TranscriptionFrame(text="hello world", user_id="u1", timestamp="t1")
    await proc.process_frame(frame, direction=None)

    # Should have pushed a TextFrame with the echoed text.
    text_frames = [f for f in pushed if isinstance(f, TextFrame)]
    assert len(text_frames) >= 1
    assert "hello world" in text_frames[-1].text
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_bridge_echo.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/bridge/processor.py
from __future__ import annotations

from pipecat.frames.frames import (
    Frame,
    TranscriptionFrame,
    TextFrame,
    LLMFullResponseStartFrame,
    LLMFullResponseEndFrame,
)
from pipecat.processors.frame_processor import (
    FrameProcessor,
    FrameDirection,
)


class AgentBridgeProcessor(FrameProcessor):
    """Replaces Pipecat's in-process LLM service.

    v0 (echo_mode=True): echoes the user's transcript back as agent text.
    v1 (echo_mode=False, Task 11+): ships transcript over WSS to remote
        Agent Bridge, streams back agent text.
    """

    def __init__(self, echo_mode: bool = False) -> None:
        super().__init__()
        self._echo_mode = echo_mode

    async def process_frame(
        self, frame: Frame, direction: FrameDirection | None
    ) -> None:
        await super().process_frame(frame, direction)

        if isinstance(frame, TranscriptionFrame) and self._echo_mode:
            await self.push_frame(LLMFullResponseStartFrame())
            await self.push_frame(TextFrame(f"You said: {frame.text}"))
            await self.push_frame(LLMFullResponseEndFrame())
            return

        # Pass-through for all other frames.
        await self.push_frame(frame, direction)
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_bridge_echo.py -v`
Expected: 1 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/bridge/ supervoice/tests/test_bridge_echo.py
git commit -m "feat(supervoice): AgentBridgeProcessor v0 (echo mode)"
```

---

### Task 9: Pipeline builder

**Files:**
- Create: `supervoice/src/supervoice/pipeline/__init__.py`
- Create: `supervoice/src/supervoice/pipeline/builder.py`
- Create: `supervoice/tests/test_pipeline_builder.py`

**Step 1: Write the failing test**

```python
# supervoice/tests/test_pipeline_builder.py
from pydantic import SecretStr

from supervoice.pipeline.builder import build_pipeline, PipelineConfig
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


def test_pipeline_has_expected_processors():
    cfg = PipelineConfig(
        stt=STTProviderConfig(
            provider="deepgram", api_key=SecretStr("x"), language="en"
        ),
        tts=TTSProviderConfig(
            provider="cartesia", api_key=SecretStr("x"), voice_id="v"
        ),
        echo_mode=True,
        transport=None,  # injected by caller; tests pass None
    )
    pipeline, bridge = build_pipeline(cfg)
    # Pipecat Pipeline exposes processors as ._processors
    names = [p.__class__.__name__ for p in pipeline._processors]
    # We expect at minimum STT, AgentBridgeProcessor, sanitize, TTS in order.
    assert "DeepgramSTTService" in names
    assert "AgentBridgeProcessor" in names
    assert "CartesiaTTSService" in names
    # bridge handle returned for runtime control
    assert bridge.__class__.__name__ == "AgentBridgeProcessor"
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_pipeline_builder.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/pipeline/builder.py
from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from pipecat.pipeline.pipeline import Pipeline
from pipecat.frames.frames import TextFrame
from pipecat.processors.frame_processor import FrameProcessor, FrameDirection

from supervoice.bridge.processor import AgentBridgeProcessor
from supervoice.speech.stt_factory import STTProviderConfig, create_stt
from supervoice.speech.tts_factory import TTSProviderConfig, create_tts
from supervoice.speech.sanitize import sanitize_for_tts


class TTSSanitizeFilter(FrameProcessor):
    """Strip markdown/URLs from TextFrames before they hit TTS."""

    async def process_frame(
        self, frame: Any, direction: FrameDirection | None
    ) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, TextFrame):
            frame = TextFrame(sanitize_for_tts(frame.text))
        await self.push_frame(frame, direction)


@dataclass
class PipelineConfig:
    stt: STTProviderConfig
    tts: TTSProviderConfig
    transport: Any  # Pipecat transport; passed through to pipeline
    echo_mode: bool = False


def build_pipeline(
    config: PipelineConfig,
) -> tuple[Pipeline, AgentBridgeProcessor]:
    """Construct the processor chain.

    Order:
        transport.input
          -> STT
          -> AgentBridgeProcessor (echo or WSS)
          -> TTSSanitizeFilter
          -> TTS
          -> transport.output
    """
    stt = create_stt(config.stt)
    tts = create_tts(config.tts)
    bridge = AgentBridgeProcessor(echo_mode=config.echo_mode)

    processors: list[Any] = [stt, bridge, TTSSanitizeFilter(), tts]
    if config.transport is not None:
        processors = [
            config.transport.input(),
            *processors,
            config.transport.output(),
        ]

    return Pipeline(processors), bridge
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_pipeline_builder.py -v`
Expected: 1 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/pipeline/ supervoice/tests/test_pipeline_builder.py
git commit -m "feat(supervoice): pipeline builder with sanitize filter"
```

---

### Task 10: WebRTC transport + FastAPI app

**Files:**
- Create: `supervoice/src/supervoice/pipeline/transport.py`
- Create: `supervoice/src/supervoice/main.py`
- Create: `supervoice/tests/test_main_health.py`

**Step 1: Write the failing test (health check)**

```python
# supervoice/tests/test_main_health.py
import pytest
from httpx import ASGITransport, AsyncClient

from supervoice.main import app


@pytest.mark.asyncio
async def test_health_endpoint():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as ac:
        r = await ac.get("/health")
        assert r.status_code == 200
        assert r.json() == {"status": "ok"}
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_main_health.py -v`
Expected: ImportError.

**Step 3: Implement transport adapter**

```python
# supervoice/src/supervoice/pipeline/transport.py
from __future__ import annotations

from pipecat.transports.network.small_webrtc import (
    SmallWebRTCTransport,
    SmallWebRTCConnection,
    TransportParams,
)

from supervoice.turn.pipecat_impl import PipecatTurnDetector


def create_webrtc_transport(
    connection: SmallWebRTCConnection,
) -> SmallWebRTCTransport:
    """WebRTC transport with VAD + SmartTurn EOU wired in.

    Audio params chosen to match Pipecat defaults (16kHz pipeline).
    """
    detector = PipecatTurnDetector()
    params = TransportParams(
        audio_in_enabled=True,
        audio_out_enabled=True,
        audio_in_sample_rate=16000,
        audio_out_sample_rate=24000,
        vad_analyzer=detector.vad,
        turn_analyzer=detector.turn,
    )
    return SmallWebRTCTransport(webrtc_connection=connection, params=params)
```

**Step 4: Implement main**

```python
# supervoice/src/supervoice/main.py
from __future__ import annotations

from contextlib import asynccontextmanager

from fastapi import FastAPI, WebSocket
from loguru import logger

from supervoice.config import Settings


@asynccontextmanager
async def lifespan(app: FastAPI):
    app.state.settings = Settings()
    logger.info("supervoice booted")
    yield


app = FastAPI(lifespan=lifespan)


@app.get("/health")
async def health() -> dict[str, str]:
    return {"status": "ok"}


@app.websocket("/call")
async def call_endpoint(ws: WebSocket) -> None:
    """WebRTC signaling — full call wiring lands in Task 12."""
    await ws.accept()
    await ws.send_json({"event": "not_implemented", "phase": 1})
    await ws.close()
```

**Step 5: Run, expect pass**

Run: `uv run pytest tests/test_main_health.py -v`
Expected: 1 passed.

**Step 6: Commit**

```bash
git add supervoice/src/supervoice/pipeline/transport.py supervoice/src/supervoice/main.py supervoice/tests/test_main_health.py
git commit -m "feat(supervoice): WebRTC transport adapter + FastAPI shell"
```

---

### Task 11: Wire echo call end-to-end

**Files:**
- Modify: `supervoice/src/supervoice/main.py`
- Modify: `supervoice/src/supervoice/session/__init__.py`
- Create: `supervoice/src/supervoice/session/handler.py`
- Create: `supervoice/tests/test_call_handler_echo.py`

**Step 1: Write a handler-level integration test (stubs transport)**

```python
# supervoice/tests/test_call_handler_echo.py
import pytest
from unittest.mock import AsyncMock, MagicMock

from pydantic import SecretStr

from supervoice.session.handler import run_echo_call
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


@pytest.mark.asyncio
async def test_run_echo_call_constructs_pipeline_and_runs():
    """Smoke test: handler should build pipeline and call runner.run()."""
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    fake_runner = MagicMock()
    fake_runner.run = AsyncMock()
    runner_factory = MagicMock(return_value=fake_runner)

    await run_echo_call(
        session_id="abc",
        transport=fake_transport,
        stt=STTProviderConfig(
            provider="deepgram", api_key=SecretStr("x"), language="en"
        ),
        tts=TTSProviderConfig(
            provider="cartesia", api_key=SecretStr("x"), voice_id="v"
        ),
        runner_factory=runner_factory,
    )

    fake_runner.run.assert_awaited()
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_call_handler_echo.py -v`
Expected: ImportError.

**Step 3: Implement handler**

```python
# supervoice/src/supervoice/session/handler.py
from __future__ import annotations

from typing import Any, Callable

from loguru import logger
from pipecat.pipeline.runner import PipelineRunner
from pipecat.pipeline.task import PipelineTask, PipelineParams

from supervoice.pipeline.builder import PipelineConfig, build_pipeline
from supervoice.session.state import SessionState
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


async def run_echo_call(
    session_id: str,
    transport: Any,
    stt: STTProviderConfig,
    tts: TTSProviderConfig,
    runner_factory: Callable[..., PipelineRunner] = PipelineRunner,
) -> None:
    """Build and run an echo-mode pipeline for one call."""
    state = SessionState(session_id=session_id)
    config = PipelineConfig(
        stt=stt, tts=tts, transport=transport, echo_mode=True
    )
    pipeline, _bridge = build_pipeline(config)

    task = PipelineTask(
        pipeline,
        params=PipelineParams(allow_interruptions=True),
    )
    runner = runner_factory()
    logger.info("starting echo call", session_id=session_id)
    try:
        await runner.run(task)
    finally:
        state.end()
        logger.info("echo call ended", session_id=session_id)
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_call_handler_echo.py -v`
Expected: 1 passed.

**Step 5: Wire `/call` endpoint to handler**

Replace the body of `call_endpoint` in `main.py`:

```python
# supervoice/src/supervoice/main.py — replace existing call_endpoint
from pydantic import SecretStr
from pipecat.transports.network.small_webrtc import SmallWebRTCConnection

from supervoice.pipeline.transport import create_webrtc_transport
from supervoice.session.handler import run_echo_call
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


@app.websocket("/call")
async def call_endpoint(ws: WebSocket) -> None:
    settings: Settings = app.state.settings
    await ws.accept()

    # SmallWebRTCConnection handles SDP exchange over this WS.
    connection = SmallWebRTCConnection(ws)
    await connection.initialize()

    transport = create_webrtc_transport(connection)

    stt = STTProviderConfig(
        provider="deepgram",
        api_key=settings.deepgram_api_key,
        language="en",
    )
    tts = TTSProviderConfig(
        provider="cartesia",
        api_key=settings.cartesia_api_key,
        voice_id="sonic-english",  # placeholder; voice profile in Task 17
    )

    session_id = connection.peer_id or "anon"
    await run_echo_call(
        session_id=session_id, transport=transport, stt=stt, tts=tts
    )
```

**Step 6: Run all tests**

Run: `uv run pytest -v`
Expected: all passing.

**Step 7: Manual smoke test**

Run: `cd supervoice && uv run uvicorn supervoice.main:app --host 0.0.0.0 --port 8080`
Open the Pipecat WebRTC test client (or use Pipecat's built-in `pipecat.transports.network.small_webrtc.client_html`) pointed at `ws://localhost:8080/call`. Speak. Verify echo.

**Step 8: Commit**

```bash
git add supervoice/src/supervoice/session/handler.py supervoice/src/supervoice/main.py supervoice/tests/test_call_handler_echo.py
git commit -m "feat(supervoice): end-to-end echo call via WebRTC"
```

**End of Phase 1.** A user can place a WebRTC call and hear their own transcript echoed back. Pipeline shape is correct.

---

## Phase 2 — Real Agent Bridge boundary (Week 2)

Goal at end of phase: `AgentBridgeProcessor` talks to a real remote Agent Bridge over persistent WSS, streams text both directions, handles barge-in.

### Task 12: Bridge wire protocol

**Files:**
- Create: `supervoice/src/supervoice/bridge/protocol.py`
- Create: `supervoice/tests/test_bridge_protocol.py`

The wire format is frozen here. Any change after this requires bumping a version.

**Step 1: Write the failing test**

```python
# supervoice/tests/test_bridge_protocol.py
import json
from supervoice.bridge.protocol import (
    UserTextEvent,
    UserInterruptEvent,
    AgentTextDeltaEvent,
    AgentTextEndEvent,
    parse_event,
)


def test_user_text_event_roundtrip():
    evt = UserTextEvent(turn_id=1, text="hello", final=True)
    raw = evt.model_dump_json()
    parsed = parse_event(json.loads(raw))
    assert isinstance(parsed, UserTextEvent)
    assert parsed.turn_id == 1
    assert parsed.text == "hello"
    assert parsed.final is True


def test_agent_text_delta_parses():
    raw = '{"event":"agent.text.delta","turn_id":1,"text":"hi"}'
    parsed = parse_event(json.loads(raw))
    assert isinstance(parsed, AgentTextDeltaEvent)
    assert parsed.text == "hi"


def test_agent_text_end_parses():
    raw = '{"event":"agent.text.end","turn_id":1}'
    parsed = parse_event(json.loads(raw))
    assert isinstance(parsed, AgentTextEndEvent)


def test_unknown_event_raises():
    try:
        parse_event({"event": "acme"})
    except ValueError:
        return
    raise AssertionError("expected ValueError")
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_bridge_protocol.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/bridge/protocol.py
from __future__ import annotations

from typing import Any, Literal, Union

from pydantic import BaseModel, Field


class UserTextEvent(BaseModel):
    event: Literal["user.text"] = "user.text"
    turn_id: int
    text: str
    final: bool = True


class UserInterruptEvent(BaseModel):
    event: Literal["user.interrupted"] = "user.interrupted"
    turn_id: int


class AgentTextDeltaEvent(BaseModel):
    event: Literal["agent.text.delta"] = "agent.text.delta"
    turn_id: int
    text: str


class AgentTextEndEvent(BaseModel):
    event: Literal["agent.text.end"] = "agent.text.end"
    turn_id: int


BridgeEvent = Union[
    UserTextEvent, UserInterruptEvent, AgentTextDeltaEvent, AgentTextEndEvent
]

_TYPE_MAP: dict[str, type[BaseModel]] = {
    "user.text": UserTextEvent,
    "user.interrupted": UserInterruptEvent,
    "agent.text.delta": AgentTextDeltaEvent,
    "agent.text.end": AgentTextEndEvent,
}


def parse_event(raw: dict[str, Any]) -> BridgeEvent:
    et = raw.get("event")
    cls = _TYPE_MAP.get(et) if isinstance(et, str) else None
    if cls is None:
        raise ValueError(f"unknown event type: {et}")
    return cls.model_validate(raw)  # type: ignore[return-value]
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_bridge_protocol.py -v`
Expected: 4 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/bridge/protocol.py supervoice/tests/test_bridge_protocol.py
git commit -m "feat(supervoice): bridge wire protocol v1"
```

---

### Task 13: Bridge WSS client (reconnect, framing)

**Files:**
- Create: `supervoice/src/supervoice/bridge/client.py`
- Create: `supervoice/tests/test_bridge_client.py`

**Step 1: Write the failing test (uses an in-memory WSS server)**

```python
# supervoice/tests/test_bridge_client.py
import asyncio
import json

import pytest
import websockets

from supervoice.bridge.client import AgentBridgeClient
from supervoice.bridge.protocol import (
    UserTextEvent,
    AgentTextDeltaEvent,
    AgentTextEndEvent,
)


@pytest.mark.asyncio
async def test_client_send_user_text_and_receive_agent_text():
    received_by_server: list[str] = []
    send_back: list[dict] = [
        AgentTextDeltaEvent(turn_id=1, text="hi").model_dump(),
        AgentTextDeltaEvent(turn_id=1, text=" there").model_dump(),
        AgentTextEndEvent(turn_id=1).model_dump(),
    ]

    async def handler(ws):
        msg = await ws.recv()
        received_by_server.append(msg)
        for evt in send_back:
            await ws.send(json.dumps(evt))

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]

    client = AgentBridgeClient(url=f"ws://127.0.0.1:{port}")
    await client.connect()

    received: list = []

    async def consume():
        async for evt in client.events():
            received.append(evt)
            if isinstance(evt, AgentTextEndEvent):
                break

    consumer = asyncio.create_task(consume())
    await client.send(UserTextEvent(turn_id=1, text="hello", final=True))
    await asyncio.wait_for(consumer, timeout=2.0)

    assert json.loads(received_by_server[0])["text"] == "hello"
    assert any(
        isinstance(e, AgentTextDeltaEvent) and e.text == "hi" for e in received
    )

    await client.close()
    server.close()
    await server.wait_closed()
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_bridge_client.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/bridge/client.py
from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator

import websockets
from loguru import logger

from .protocol import BridgeEvent, parse_event


class AgentBridgeClient:
    """Persistent WSS client to the remote Agent Bridge.

    v1: single connection per call, no reconnect (reconnect is Task 14).
    """

    def __init__(self, url: str) -> None:
        self._url = url
        self._ws: websockets.WebSocketClientProtocol | None = None
        self._recv_queue: asyncio.Queue[BridgeEvent] = asyncio.Queue()
        self._reader_task: asyncio.Task[None] | None = None

    async def connect(self) -> None:
        self._ws = await websockets.connect(self._url)
        self._reader_task = asyncio.create_task(self._read_loop())

    async def _read_loop(self) -> None:
        assert self._ws is not None
        try:
            async for raw in self._ws:
                try:
                    evt = parse_event(json.loads(raw))
                except (ValueError, json.JSONDecodeError) as e:
                    logger.warning("invalid bridge frame", error=str(e))
                    continue
                await self._recv_queue.put(evt)
        except websockets.ConnectionClosed:
            logger.info("bridge connection closed")

    async def send(self, event: BridgeEvent) -> None:
        assert self._ws is not None, "call connect() first"
        await self._ws.send(event.model_dump_json())

    async def events(self) -> AsyncIterator[BridgeEvent]:
        while True:
            evt = await self._recv_queue.get()
            yield evt

    async def close(self) -> None:
        if self._reader_task is not None:
            self._reader_task.cancel()
        if self._ws is not None:
            await self._ws.close()
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_bridge_client.py -v`
Expected: 1 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/bridge/client.py supervoice/tests/test_bridge_client.py
git commit -m "feat(supervoice): AgentBridgeClient — persistent WSS"
```

---

### Task 14: Reconnect with backoff

**Files:**
- Modify: `supervoice/src/supervoice/bridge/client.py`
- Create: `supervoice/tests/test_bridge_reconnect.py`

**Step 1: Write the failing test**

```python
# supervoice/tests/test_bridge_reconnect.py
import asyncio
import json

import pytest
import websockets

from supervoice.bridge.client import AgentBridgeClient
from supervoice.bridge.protocol import UserTextEvent


@pytest.mark.asyncio
async def test_client_reconnects_after_server_drop():
    connect_count = 0

    async def handler(ws):
        nonlocal connect_count
        connect_count += 1
        if connect_count == 1:
            await ws.close()  # force drop
            return
        # second connection: receive and ack
        msg = await ws.recv()
        assert json.loads(msg)["text"] == "after-reconnect"

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]

    client = AgentBridgeClient(
        url=f"ws://127.0.0.1:{port}",
        reconnect_max_attempts=3,
        reconnect_initial_delay_ms=50,
    )
    await client.connect()

    # First connection closes; client should reconnect.
    await asyncio.sleep(0.3)
    await client.send(UserTextEvent(turn_id=1, text="after-reconnect"))
    await asyncio.sleep(0.2)
    assert connect_count >= 2

    await client.close()
    server.close()
    await server.wait_closed()
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_bridge_reconnect.py -v`
Expected: AssertionError on connect_count.

**Step 3: Update implementation**

Replace `supervoice/src/supervoice/bridge/client.py`:

```python
# supervoice/src/supervoice/bridge/client.py
from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator

import websockets
from loguru import logger

from .protocol import BridgeEvent, parse_event


class AgentBridgeClient:
    def __init__(
        self,
        url: str,
        reconnect_max_attempts: int = 5,
        reconnect_initial_delay_ms: int = 200,
    ) -> None:
        self._url = url
        self._ws: websockets.WebSocketClientProtocol | None = None
        self._recv_queue: asyncio.Queue[BridgeEvent] = asyncio.Queue()
        self._reader_task: asyncio.Task[None] | None = None
        self._closed = False
        self._reconnect_max = reconnect_max_attempts
        self._reconnect_initial_ms = reconnect_initial_delay_ms
        self._connected = asyncio.Event()

    async def connect(self) -> None:
        self._reader_task = asyncio.create_task(self._supervise())
        await self._connected.wait()

    async def _supervise(self) -> None:
        attempt = 0
        while not self._closed:
            try:
                self._ws = await websockets.connect(self._url)
                self._connected.set()
                attempt = 0
                await self._read_loop()
            except (OSError, websockets.WebSocketException) as e:
                logger.warning("bridge connect failed", error=str(e))
            if self._closed:
                return
            attempt += 1
            if attempt > self._reconnect_max:
                logger.error("bridge reconnect exhausted")
                return
            delay = (self._reconnect_initial_ms / 1000.0) * (2 ** (attempt - 1))
            await asyncio.sleep(delay)
            self._connected.clear()

    async def _read_loop(self) -> None:
        assert self._ws is not None
        try:
            async for raw in self._ws:
                try:
                    evt = parse_event(json.loads(raw))
                except (ValueError, json.JSONDecodeError) as e:
                    logger.warning("invalid bridge frame", error=str(e))
                    continue
                await self._recv_queue.put(evt)
        except websockets.ConnectionClosed:
            logger.info("bridge connection closed; will reconnect")

    async def send(self, event: BridgeEvent) -> None:
        await self._connected.wait()
        assert self._ws is not None
        await self._ws.send(event.model_dump_json())

    async def events(self) -> AsyncIterator[BridgeEvent]:
        while not self._closed:
            evt = await self._recv_queue.get()
            yield evt

    async def close(self) -> None:
        self._closed = True
        if self._ws is not None:
            await self._ws.close()
        if self._reader_task is not None:
            self._reader_task.cancel()
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_bridge_reconnect.py tests/test_bridge_client.py -v`
Expected: 2 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/bridge/client.py supervoice/tests/test_bridge_reconnect.py
git commit -m "feat(supervoice): bridge client reconnect with exponential backoff"
```

---

### Task 15: AgentBridgeProcessor — real WSS mode

**Files:**
- Modify: `supervoice/src/supervoice/bridge/processor.py`
- Create: `supervoice/tests/test_bridge_processor_wss.py`

**Step 1: Write the failing test**

```python
# supervoice/tests/test_bridge_processor_wss.py
import asyncio
import json

import pytest
import websockets
from unittest.mock import MagicMock

from pipecat.frames.frames import (
    TranscriptionFrame,
    TextFrame,
    StartInterruptionFrame,
)

from supervoice.bridge.client import AgentBridgeClient
from supervoice.bridge.processor import AgentBridgeProcessor
from supervoice.bridge.protocol import AgentTextDeltaEvent, AgentTextEndEvent


@pytest.mark.asyncio
async def test_bridge_processor_streams_agent_text_downstream():
    received: list[str] = []

    async def handler(ws):
        msg = await ws.recv()
        received.append(msg)
        for evt in [
            AgentTextDeltaEvent(turn_id=1, text="hi"),
            AgentTextDeltaEvent(turn_id=1, text=" there"),
            AgentTextEndEvent(turn_id=1),
        ]:
            await ws.send(evt.model_dump_json())

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]

    client = AgentBridgeClient(url=f"ws://127.0.0.1:{port}")
    await client.connect()

    proc = AgentBridgeProcessor(echo_mode=False, client=client)
    await proc.start()

    pushed: list = []
    proc.push_frame = MagicMock(
        side_effect=lambda f, d=None: pushed.append(f) or asyncio.sleep(0)
    )

    await proc.process_frame(
        TranscriptionFrame(text="hello", user_id="u", timestamp="t"), None
    )
    await asyncio.sleep(0.3)

    text_frames = [f for f in pushed if isinstance(f, TextFrame)]
    joined = "".join(f.text for f in text_frames)
    assert "hi" in joined and "there" in joined

    assert json.loads(received[0])["text"] == "hello"

    await proc.stop()
    await client.close()
    server.close()
    await server.wait_closed()
```

**Step 2: Run, expect failure**

Expected: AttributeError (start/stop not present, client param not accepted).

**Step 3: Update implementation**

```python
# supervoice/src/supervoice/bridge/processor.py
from __future__ import annotations

import asyncio

from loguru import logger
from pipecat.frames.frames import (
    Frame,
    TranscriptionFrame,
    TextFrame,
    LLMFullResponseStartFrame,
    LLMFullResponseEndFrame,
    StartInterruptionFrame,
)
from pipecat.processors.frame_processor import FrameProcessor, FrameDirection

from .client import AgentBridgeClient
from .protocol import (
    AgentTextDeltaEvent,
    AgentTextEndEvent,
    UserInterruptEvent,
    UserTextEvent,
)


class AgentBridgeProcessor(FrameProcessor):
    def __init__(
        self,
        echo_mode: bool = False,
        client: AgentBridgeClient | None = None,
    ) -> None:
        super().__init__()
        self._echo_mode = echo_mode
        self._client = client
        self._turn_id = 0
        self._consumer_task: asyncio.Task[None] | None = None
        if not echo_mode and client is None:
            raise ValueError("non-echo mode requires a client")

    async def start(self) -> None:
        if self._client is not None:
            self._consumer_task = asyncio.create_task(self._consume_bridge())

    async def stop(self) -> None:
        if self._consumer_task is not None:
            self._consumer_task.cancel()

    async def _consume_bridge(self) -> None:
        assert self._client is not None
        try:
            async for evt in self._client.events():
                if isinstance(evt, AgentTextDeltaEvent):
                    if not self._response_started:
                        await self.push_frame(LLMFullResponseStartFrame())
                        self._response_started = True
                    await self.push_frame(TextFrame(evt.text))
                elif isinstance(evt, AgentTextEndEvent):
                    await self.push_frame(LLMFullResponseEndFrame())
                    self._response_started = False
        except asyncio.CancelledError:
            return

    _response_started: bool = False

    async def process_frame(
        self, frame: Frame, direction: FrameDirection | None
    ) -> None:
        await super().process_frame(frame, direction)

        if isinstance(frame, TranscriptionFrame):
            self._turn_id += 1
            if self._echo_mode:
                await self.push_frame(LLMFullResponseStartFrame())
                await self.push_frame(TextFrame(f"You said: {frame.text}"))
                await self.push_frame(LLMFullResponseEndFrame())
                return
            assert self._client is not None
            await self._client.send(
                UserTextEvent(turn_id=self._turn_id, text=frame.text, final=True)
            )
            return

        if isinstance(frame, StartInterruptionFrame) and self._client is not None:
            await self._client.send(UserInterruptEvent(turn_id=self._turn_id))

        await self.push_frame(frame, direction)
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_bridge_processor_wss.py -v`
Expected: 1 passed.

**Step 5: Verify echo test still passes**

Run: `uv run pytest tests/test_bridge_echo.py -v`
Expected: 1 passed.

**Step 6: Commit**

```bash
git add supervoice/src/supervoice/bridge/processor.py supervoice/tests/test_bridge_processor_wss.py
git commit -m "feat(supervoice): AgentBridgeProcessor real WSS mode + interrupt propagation"
```

---

### Task 16: Mock Agent Bridge server (for E2E tests)

**Files:**
- Create: `supervoice/tests/conftest.py`
- Create: `supervoice/tests/fixtures/mock_bridge.py`

**Step 1: Implement mock bridge**

```python
# supervoice/tests/fixtures/mock_bridge.py
"""Mock Agent Bridge that echoes any user.text as a 3-chunk agent stream.

Used by E2E tests in place of a real LLM-backed Agent Bridge.
"""
from __future__ import annotations

import asyncio
import json
from typing import AsyncIterator

import websockets


async def mock_bridge_handler(ws):
    async for raw in ws:
        msg = json.loads(raw)
        if msg.get("event") != "user.text":
            continue
        turn_id = msg["turn_id"]
        text = msg["text"]
        for chunk in [f"You said ", text, "."]:
            await ws.send(
                json.dumps(
                    {
                        "event": "agent.text.delta",
                        "turn_id": turn_id,
                        "text": chunk,
                    }
                )
            )
            await asyncio.sleep(0.01)
        await ws.send(
            json.dumps({"event": "agent.text.end", "turn_id": turn_id})
        )


async def start_mock_bridge(host: str = "127.0.0.1", port: int = 0):
    server = await websockets.serve(mock_bridge_handler, host, port)
    bound_port = server.sockets[0].getsockname()[1]
    return server, f"ws://{host}:{bound_port}"
```

**Step 2: Add fixture**

```python
# supervoice/tests/conftest.py
import pytest_asyncio

from .fixtures.mock_bridge import start_mock_bridge


@pytest_asyncio.fixture
async def mock_bridge():
    server, url = await start_mock_bridge()
    try:
        yield url
    finally:
        server.close()
        await server.wait_closed()
```

**Step 3: Commit**

```bash
git add supervoice/tests/conftest.py supervoice/tests/fixtures/
git commit -m "test(supervoice): mock Agent Bridge fixture"
```

---

### Task 17: Wire real bridge into the call handler

**Files:**
- Modify: `supervoice/src/supervoice/session/handler.py`
- Create: `supervoice/tests/test_handler_bridge_mode.py`

**Step 1: Write the failing test**

```python
# supervoice/tests/test_handler_bridge_mode.py
import pytest
from unittest.mock import AsyncMock, MagicMock

from pydantic import SecretStr

from supervoice.session.handler import run_bridge_call
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


@pytest.mark.asyncio
async def test_run_bridge_call_connects_and_runs(mock_bridge):
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    fake_runner = MagicMock()
    fake_runner.run = AsyncMock()
    runner_factory = MagicMock(return_value=fake_runner)

    await run_bridge_call(
        session_id="abc",
        transport=fake_transport,
        stt=STTProviderConfig(
            provider="deepgram", api_key=SecretStr("x"), language="en"
        ),
        tts=TTSProviderConfig(
            provider="cartesia", api_key=SecretStr("x"), voice_id="v"
        ),
        agent_bridge_url=mock_bridge,
        runner_factory=runner_factory,
    )
    fake_runner.run.assert_awaited()
```

**Step 2: Run, expect failure**

Expected: ImportError (run_bridge_call not defined).

**Step 3: Implement**

Add to `supervoice/src/supervoice/session/handler.py`:

```python
from supervoice.bridge.client import AgentBridgeClient


async def run_bridge_call(
    session_id: str,
    transport: Any,
    stt: STTProviderConfig,
    tts: TTSProviderConfig,
    agent_bridge_url: str,
    runner_factory: Callable[..., PipelineRunner] = PipelineRunner,
) -> None:
    """Production call mode: AgentBridgeProcessor talks to remote bridge over WSS."""
    state = SessionState(session_id=session_id)

    client = AgentBridgeClient(url=agent_bridge_url)
    await client.connect()

    config = PipelineConfig(
        stt=stt, tts=tts, transport=transport, echo_mode=False
    )
    # Build pipeline first, then inject the connected client into the bridge.
    pipeline, bridge = build_pipeline(config)
    bridge._client = client  # noqa: SLF001 — controlled injection
    await bridge.start()

    task = PipelineTask(
        pipeline,
        params=PipelineParams(allow_interruptions=True),
    )
    runner = runner_factory()
    try:
        await runner.run(task)
    finally:
        await bridge.stop()
        await client.close()
        state.end()
```

**Step 4: Update `build_pipeline` to allow bridge w/o client**

In `supervoice/src/supervoice/pipeline/builder.py`, change the `AgentBridgeProcessor` construction in non-echo mode to defer client injection:

```python
    bridge = (
        AgentBridgeProcessor(echo_mode=True)
        if config.echo_mode
        else AgentBridgeProcessor.__new__(AgentBridgeProcessor)
    )
    if not config.echo_mode:
        # Bypass __init__ — handler injects client + calls start() later.
        FrameProcessor.__init__(bridge)
        bridge._echo_mode = False
        bridge._client = None
        bridge._turn_id = 0
        bridge._consumer_task = None
        bridge._response_started = False
```

Cleaner alternative: change `AgentBridgeProcessor.__init__` to accept `client=None` and validate in `start()`. Prefer the cleaner alternative — modify `processor.py`:

```python
    def __init__(
        self,
        echo_mode: bool = False,
        client: AgentBridgeClient | None = None,
    ) -> None:
        super().__init__()
        self._echo_mode = echo_mode
        self._client = client
        self._turn_id = 0
        self._consumer_task = None
        self._response_started = False

    def attach_client(self, client: AgentBridgeClient) -> None:
        self._client = client

    async def start(self) -> None:
        if not self._echo_mode and self._client is None:
            raise RuntimeError("bridge in WSS mode but no client attached")
        if self._client is not None:
            self._consumer_task = asyncio.create_task(self._consume_bridge())
```

And in `handler.py`, replace `bridge._client = client` with `bridge.attach_client(client)`.

Revert the messy `__new__` patch in `builder.py`.

**Step 5: Run all tests**

Run: `uv run pytest -v`
Expected: all green.

**Step 6: Wire `/call` endpoint to `run_bridge_call`**

In `main.py`, replace `run_echo_call` with `run_bridge_call(...)` passing `settings.agent_bridge_url`.

**Step 7: Commit**

```bash
git add supervoice/src/supervoice/session/handler.py supervoice/src/supervoice/bridge/processor.py supervoice/src/supervoice/pipeline/builder.py supervoice/src/supervoice/main.py supervoice/tests/test_handler_bridge_mode.py
git commit -m "feat(supervoice): real bridge mode end-to-end"
```

**End of Phase 2.** Text-only boundary works; a real Agent Bridge can be plugged in.

---

## Phase 3 — Voice profiles + polish (Week 3)

### Task 18: Voice profile catalog

**Files:**
- Create: `supervoice/src/supervoice/voice_profile/__init__.py`
- Create: `supervoice/src/supervoice/voice_profile/catalog.py`
- Create: `supervoice/src/supervoice/voice_profile/profiles.yaml`
- Create: `supervoice/tests/test_voice_profile.py`

**Step 1: Write the failing test**

```python
# supervoice/tests/test_voice_profile.py
from supervoice.voice_profile.catalog import VoiceProfileCatalog


def test_load_default_catalog():
    cat = VoiceProfileCatalog.load_default()
    p = cat.get("hi-female")
    assert p.id == "hi-female"
    assert p.language == "hi"
    assert len(p.stt_preference) >= 1
    assert len(p.tts_preference) >= 1


def test_unknown_profile_raises():
    cat = VoiceProfileCatalog.load_default()
    try:
        cat.get("does-not-exist")
    except KeyError:
        return
    raise AssertionError("expected KeyError")


def test_list_profiles():
    cat = VoiceProfileCatalog.load_default()
    ids = {p.id for p in cat.list()}
    assert {"hi-female", "hi-male", "en-female", "en-male"} <= ids
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_voice_profile.py -v`
Expected: ImportError.

**Step 3: Write `profiles.yaml`**

```yaml
# supervoice/src/supervoice/voice_profile/profiles.yaml
profiles:
  - id: hi-female
    language: hi
    persona: warm
    stt_preference:
      - {provider: deepgram, language: hi}
      - {provider: cartesia, language: hi}
    tts_preference:
      - {provider: cartesia, voice_id: sonic-hindi-female}
      - {provider: elevenlabs, voice_id: hi-female-rachel}
  - id: hi-male
    language: hi
    persona: deep
    stt_preference:
      - {provider: deepgram, language: hi}
    tts_preference:
      - {provider: cartesia, voice_id: sonic-hindi-male}
  - id: en-female
    language: en
    persona: warm
    stt_preference:
      - {provider: deepgram, language: en}
    tts_preference:
      - {provider: cartesia, voice_id: sonic-english-female}
      - {provider: elevenlabs, voice_id: rachel}
  - id: en-male
    language: en
    persona: deep
    stt_preference:
      - {provider: deepgram, language: en}
    tts_preference:
      - {provider: cartesia, voice_id: sonic-english-male}
```

**Step 4: Implement catalog**

```python
# supervoice/src/supervoice/voice_profile/catalog.py
from __future__ import annotations

from importlib.resources import files
from pathlib import Path

import yaml
from pydantic import BaseModel


class STTSpec(BaseModel):
    provider: str
    language: str


class TTSSpec(BaseModel):
    provider: str
    voice_id: str


class VoiceProfile(BaseModel):
    id: str
    language: str
    persona: str
    stt_preference: list[STTSpec]
    tts_preference: list[TTSSpec]


class VoiceProfileCatalog(BaseModel):
    profiles: list[VoiceProfile]

    @classmethod
    def load_default(cls) -> "VoiceProfileCatalog":
        text = (
            files("supervoice.voice_profile")
            .joinpath("profiles.yaml")
            .read_text()
        )
        return cls.model_validate(yaml.safe_load(text))

    @classmethod
    def load_from(cls, path: Path) -> "VoiceProfileCatalog":
        return cls.model_validate(yaml.safe_load(path.read_text()))

    def get(self, profile_id: str) -> VoiceProfile:
        for p in self.profiles:
            if p.id == profile_id:
                return p
        raise KeyError(profile_id)

    def list(self) -> list[VoiceProfile]:
        return list(self.profiles)
```

Also update `pyproject.toml` to include the yaml file:

```toml
[tool.hatch.build.targets.wheel]
packages = ["src/supervoice"]
[tool.hatch.build.targets.wheel.shared-data]
"src/supervoice/voice_profile/profiles.yaml" = "supervoice/voice_profile/profiles.yaml"
```

**Step 5: Run, expect pass**

Run: `uv run pytest tests/test_voice_profile.py -v`
Expected: 3 passed.

**Step 6: Commit**

```bash
git add supervoice/src/supervoice/voice_profile/ supervoice/tests/test_voice_profile.py supervoice/pyproject.toml
git commit -m "feat(supervoice): voice profile catalog + 4 default profiles"
```

---

### Task 19: STT/TTS failover chain

**Files:**
- Modify: `supervoice/src/supervoice/speech/stt_factory.py`
- Modify: `supervoice/src/supervoice/speech/tts_factory.py`
- Create: `supervoice/src/supervoice/speech/failover.py`
- Create: `supervoice/tests/test_failover.py`

**Step 1: Write the failing test**

```python
# supervoice/tests/test_failover.py
import pytest
from pydantic import SecretStr

from supervoice.speech.failover import resolve_stt_with_fallback, resolve_tts_with_fallback
from supervoice.voice_profile.catalog import VoiceProfile, STTSpec, TTSSpec


def test_resolve_stt_uses_first_available_provider():
    profile = VoiceProfile(
        id="t",
        language="en",
        persona="warm",
        stt_preference=[
            STTSpec(provider="missing-provider", language="en"),
            STTSpec(provider="deepgram", language="en"),
        ],
        tts_preference=[TTSSpec(provider="cartesia", voice_id="v")],
    )
    api_keys = {"deepgram": SecretStr("dg")}
    stt = resolve_stt_with_fallback(profile, api_keys)
    assert stt.__class__.__name__ == "DeepgramSTTService"


def test_resolve_stt_raises_when_no_provider_available():
    profile = VoiceProfile(
        id="t",
        language="en",
        persona="warm",
        stt_preference=[STTSpec(provider="missing", language="en")],
        tts_preference=[TTSSpec(provider="cartesia", voice_id="v")],
    )
    with pytest.raises(RuntimeError):
        resolve_stt_with_fallback(profile, api_keys={})
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_failover.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/speech/failover.py
from __future__ import annotations

from pydantic import SecretStr
from loguru import logger

from supervoice.voice_profile.catalog import VoiceProfile
from .stt_factory import STTProviderConfig, create_stt
from .tts_factory import TTSProviderConfig, create_tts


def resolve_stt_with_fallback(
    profile: VoiceProfile,
    api_keys: dict[str, SecretStr],
):
    for spec in profile.stt_preference:
        key = api_keys.get(spec.provider)
        if key is None:
            logger.info(
                "stt provider not configured, trying next",
                provider=spec.provider,
            )
            continue
        try:
            return create_stt(
                STTProviderConfig(
                    provider=spec.provider, api_key=key, language=spec.language
                )
            )
        except ValueError as e:
            logger.warning(
                "stt provider unsupported, trying next",
                provider=spec.provider,
                error=str(e),
            )
    raise RuntimeError(
        f"no STT provider available for profile {profile.id}"
    )


def resolve_tts_with_fallback(
    profile: VoiceProfile,
    api_keys: dict[str, SecretStr],
):
    for spec in profile.tts_preference:
        key = api_keys.get(spec.provider)
        if key is None:
            continue
        try:
            return create_tts(
                TTSProviderConfig(
                    provider=spec.provider,
                    api_key=key,
                    voice_id=spec.voice_id,
                )
            )
        except ValueError:
            continue
    raise RuntimeError(
        f"no TTS provider available for profile {profile.id}"
    )
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_failover.py -v`
Expected: 2 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/speech/failover.py supervoice/tests/test_failover.py
git commit -m "feat(supervoice): STT/TTS failover via voice-profile preference"
```

---

### Task 20: Idle monitor (lift from lite_v2)

**Files:**
- Create: `supervoice/src/supervoice/session/idle_monitor.py`
- Create: `supervoice/tests/test_idle_monitor.py`

**Reference:** `/Users/parvbhullar/Drives/Vault/Projects/Unpod/super/super/core/voice/pipecat/lite_v2/events.py:139-223`. Trim handover logic.

**Step 1: Write the failing test**

```python
# supervoice/tests/test_idle_monitor.py
import asyncio
import pytest

from supervoice.session.state import SessionState
from supervoice.session.idle_monitor import IdleMonitor


@pytest.mark.asyncio
async def test_idle_monitor_warns_then_disconnects():
    state = SessionState(session_id="x")
    state.mark_idle()
    warnings: list[int] = []
    disconnects: list[bool] = []

    monitor = IdleMonitor(
        state=state,
        warning_at_s=0.1,
        disconnect_at_s=0.2,
        on_warning=lambda lvl: warnings.append(lvl),
        on_disconnect=lambda: disconnects.append(True),
        poll_interval_s=0.02,
    )
    task = asyncio.create_task(monitor.run())
    await asyncio.sleep(0.35)
    task.cancel()

    assert len(warnings) >= 1
    assert disconnects == [True]


@pytest.mark.asyncio
async def test_idle_monitor_skips_when_processing():
    state = SessionState(session_id="x")
    state.mark_processing()  # never goes idle
    disconnects: list[bool] = []

    monitor = IdleMonitor(
        state=state,
        warning_at_s=0.05,
        disconnect_at_s=0.1,
        on_warning=lambda _: None,
        on_disconnect=lambda: disconnects.append(True),
        poll_interval_s=0.02,
    )
    task = asyncio.create_task(monitor.run())
    await asyncio.sleep(0.2)
    task.cancel()
    assert disconnects == []
```

**Step 2: Run, expect failure**

Run: `uv run pytest tests/test_idle_monitor.py -v`
Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/session/idle_monitor.py
from __future__ import annotations

import asyncio
import time
from typing import Callable

from .state import SessionState


class IdleMonitor:
    def __init__(
        self,
        state: SessionState,
        warning_at_s: float,
        disconnect_at_s: float,
        on_warning: Callable[[int], None],
        on_disconnect: Callable[[], None],
        poll_interval_s: float = 1.0,
    ) -> None:
        self._state = state
        self._warn_at = warning_at_s
        self._disconnect_at = disconnect_at_s
        self._on_warning = on_warning
        self._on_disconnect = on_disconnect
        self._poll = poll_interval_s

    async def run(self) -> None:
        while not self._state.shutdown:
            if self._state.is_processing or self._state.idle_since is None:
                await asyncio.sleep(self._poll)
                continue
            elapsed = time.time() - self._state.idle_since
            if (
                elapsed >= self._warn_at
                and self._state.idle_warning_count == 0
            ):
                self._state.idle_warning_count = 1
                self._on_warning(1)
            if elapsed >= self._disconnect_at:
                self._on_disconnect()
                return
            await asyncio.sleep(self._poll)
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_idle_monitor.py -v`
Expected: 2 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/session/idle_monitor.py supervoice/tests/test_idle_monitor.py
git commit -m "feat(supervoice): idle monitor with warning + disconnect"
```

---

### Task 21: Wire voice profile + idle monitor into handler

**Files:**
- Modify: `supervoice/src/supervoice/session/handler.py`
- Modify: `supervoice/src/supervoice/main.py`
- Create: `supervoice/tests/test_handler_profile.py`

**Step 1: Write the test**

```python
# supervoice/tests/test_handler_profile.py
import pytest
from unittest.mock import AsyncMock, MagicMock

from pydantic import SecretStr

from supervoice.session.handler import run_call_with_profile


@pytest.mark.asyncio
async def test_run_call_with_profile_resolves_providers(mock_bridge):
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    fake_runner = MagicMock()
    fake_runner.run = AsyncMock()

    await run_call_with_profile(
        session_id="abc",
        transport=fake_transport,
        profile_id="en-female",
        api_keys={
            "deepgram": SecretStr("dg"),
            "cartesia": SecretStr("ct"),
        },
        agent_bridge_url=mock_bridge,
        runner_factory=MagicMock(return_value=fake_runner),
    )
    fake_runner.run.assert_awaited()
```

**Step 2: Run, expect failure**

Expected: ImportError.

**Step 3: Implement**

Add to `supervoice/src/supervoice/session/handler.py`:

```python
from pydantic import SecretStr

from supervoice.speech.failover import (
    resolve_stt_with_fallback,
    resolve_tts_with_fallback,
)
from supervoice.voice_profile.catalog import VoiceProfileCatalog


async def run_call_with_profile(
    session_id: str,
    transport: Any,
    profile_id: str,
    api_keys: dict[str, SecretStr],
    agent_bridge_url: str,
    runner_factory: Callable[..., PipelineRunner] = PipelineRunner,
) -> None:
    catalog = VoiceProfileCatalog.load_default()
    profile = catalog.get(profile_id)
    state = SessionState(session_id=session_id, voice_profile_id=profile_id)

    stt_service = resolve_stt_with_fallback(profile, api_keys)
    tts_service = resolve_tts_with_fallback(profile, api_keys)

    client = AgentBridgeClient(url=agent_bridge_url)
    await client.connect()

    bridge = AgentBridgeProcessor(echo_mode=False)
    bridge.attach_client(client)

    from pipecat.pipeline.pipeline import Pipeline
    from supervoice.pipeline.builder import TTSSanitizeFilter

    pipeline = Pipeline(
        [
            transport.input(),
            stt_service,
            bridge,
            TTSSanitizeFilter(),
            tts_service,
            transport.output(),
        ]
    )

    await bridge.start()
    task = PipelineTask(
        pipeline, params=PipelineParams(allow_interruptions=True)
    )
    runner = runner_factory()
    try:
        await runner.run(task)
    finally:
        await bridge.stop()
        await client.close()
        state.end()
```

**Step 4: Update `/call` endpoint**

In `main.py`, accept a profile query param:

```python
@app.websocket("/call")
async def call_endpoint(ws: WebSocket, profile: str = "en-female") -> None:
    settings: Settings = app.state.settings
    await ws.accept()
    connection = SmallWebRTCConnection(ws)
    await connection.initialize()
    transport = create_webrtc_transport(connection)

    api_keys = {
        "deepgram": settings.deepgram_api_key,
        "cartesia": settings.cartesia_api_key,
    }
    if settings.elevenlabs_api_key is not None:
        api_keys["elevenlabs"] = settings.elevenlabs_api_key

    await run_call_with_profile(
        session_id=connection.peer_id or "anon",
        transport=transport,
        profile_id=profile,
        api_keys=api_keys,
        agent_bridge_url=settings.agent_bridge_url,
    )
```

**Step 5: Run all tests**

Run: `uv run pytest -v`
Expected: all green.

**Step 6: Commit**

```bash
git add supervoice/src/supervoice/session/handler.py supervoice/src/supervoice/main.py supervoice/tests/test_handler_profile.py
git commit -m "feat(supervoice): voice-profile-driven call wiring"
```

---

### Task 22: Per-call metrics

**Files:**
- Create: `supervoice/src/supervoice/observability/__init__.py`
- Create: `supervoice/src/supervoice/observability/metrics.py`
- Create: `supervoice/tests/test_metrics.py`

**Step 1: Write the failing test**

```python
# supervoice/tests/test_metrics.py
import time
from supervoice.observability.metrics import CallMetrics


def test_records_ttfa():
    m = CallMetrics(session_id="x")
    m.mark_user_turn_end()
    time.sleep(0.05)
    m.mark_first_agent_audio()
    assert m.ttfa_ms is not None
    assert 30 < m.ttfa_ms < 200


def test_records_asr_final_latency():
    m = CallMetrics(session_id="x")
    m.mark_user_audio_end()
    time.sleep(0.03)
    m.mark_asr_final()
    assert 20 < m.asr_final_ms < 150


def test_snapshot_contains_session_id():
    m = CallMetrics(session_id="abc")
    snap = m.snapshot()
    assert snap["session_id"] == "abc"
```

**Step 2: Run, expect failure**

Expected: ImportError.

**Step 3: Implement**

```python
# supervoice/src/supervoice/observability/metrics.py
from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any


@dataclass
class CallMetrics:
    session_id: str
    _user_turn_end_t: float | None = None
    _user_audio_end_t: float | None = None
    _asr_final_t: float | None = None
    _first_agent_audio_t: float | None = None
    extras: dict[str, Any] = field(default_factory=dict)

    def mark_user_audio_end(self) -> None:
        self._user_audio_end_t = time.monotonic()

    def mark_asr_final(self) -> None:
        self._asr_final_t = time.monotonic()

    def mark_user_turn_end(self) -> None:
        self._user_turn_end_t = time.monotonic()

    def mark_first_agent_audio(self) -> None:
        self._first_agent_audio_t = time.monotonic()

    @property
    def ttfa_ms(self) -> float | None:
        if self._user_turn_end_t is None or self._first_agent_audio_t is None:
            return None
        return (self._first_agent_audio_t - self._user_turn_end_t) * 1000.0

    @property
    def asr_final_ms(self) -> float | None:
        if self._user_audio_end_t is None or self._asr_final_t is None:
            return None
        return (self._asr_final_t - self._user_audio_end_t) * 1000.0

    def snapshot(self) -> dict[str, Any]:
        return {
            "session_id": self.session_id,
            "ttfa_ms": self.ttfa_ms,
            "asr_final_ms": self.asr_final_ms,
            **self.extras,
        }
```

**Step 4: Run, expect pass**

Run: `uv run pytest tests/test_metrics.py -v`
Expected: 3 passed.

**Step 5: Commit**

```bash
git add supervoice/src/supervoice/observability/ supervoice/tests/test_metrics.py
git commit -m "feat(supervoice): per-call metrics (ttfa, asr-final)"
```

---

### Task 23: End-to-end smoke test

**Files:**
- Create: `supervoice/tests/test_e2e_smoke.py`

A test that exercises the full pipeline path with mocked transport but real STT/TTS classes (not network calls — the test asserts wiring, not external services).

**Step 1: Write the test**

```python
# supervoice/tests/test_e2e_smoke.py
import pytest
from unittest.mock import AsyncMock, MagicMock
from pydantic import SecretStr

from supervoice.pipeline.builder import build_pipeline, PipelineConfig
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


def test_pipeline_assembly_for_all_default_profiles():
    """Smoke test: build the pipeline for each profile w/o crashing."""
    from supervoice.voice_profile.catalog import VoiceProfileCatalog
    from supervoice.speech.failover import (
        resolve_stt_with_fallback,
        resolve_tts_with_fallback,
    )

    catalog = VoiceProfileCatalog.load_default()
    api_keys = {
        "deepgram": SecretStr("dg"),
        "cartesia": SecretStr("ct"),
        "elevenlabs": SecretStr("el"),
    }
    for profile in catalog.list():
        stt = resolve_stt_with_fallback(profile, api_keys)
        tts = resolve_tts_with_fallback(profile, api_keys)
        # Verify the providers instantiated without raising.
        assert stt is not None
        assert tts is not None


@pytest.mark.asyncio
async def test_full_pipeline_constructs_with_bridge(mock_bridge):
    from supervoice.bridge.client import AgentBridgeClient

    client = AgentBridgeClient(url=mock_bridge)
    await client.connect()

    transport = MagicMock()
    transport.input = MagicMock(return_value=MagicMock())
    transport.output = MagicMock(return_value=MagicMock())

    cfg = PipelineConfig(
        stt=STTProviderConfig(
            provider="deepgram", api_key=SecretStr("x"), language="en"
        ),
        tts=TTSProviderConfig(
            provider="cartesia", api_key=SecretStr("x"), voice_id="v"
        ),
        transport=transport,
        echo_mode=False,
    )
    pipeline, bridge = build_pipeline(cfg)
    bridge.attach_client(client)
    await bridge.start()

    names = [p.__class__.__name__ for p in pipeline._processors]
    assert "AgentBridgeProcessor" in names
    assert "TTSSanitizeFilter" in names

    await bridge.stop()
    await client.close()
```

**Step 2: Run**

Run: `uv run pytest tests/test_e2e_smoke.py -v`
Expected: 2 passed.

**Step 3: Commit**

```bash
git add supervoice/tests/test_e2e_smoke.py
git commit -m "test(supervoice): E2E smoke for all profiles"
```

---

### Task 24: README + run script

**Files:**
- Modify: `supervoice/README.md`
- Create: `supervoice/scripts/run.sh`
- Create: `supervoice/scripts/test.sh`

**Step 1: README**

```markdown
# supervoice

Speech pipeline with text-only Agent Bridge boundary.

## Architecture

```
audio_in → [VAD → EOU → STT] → user.text ──► AgentBridge (WSS)
                                                    │
audio_out ◄── [TTS pool ◄─ sanitize] ◄── agent.text ┘
```

LLM lives in the remote Agent Bridge. Supervoice never holds LLM state.

## Setup

```bash
uv sync
cp .env.example .env  # fill in keys
```

## Run

```bash
./scripts/run.sh
```

## Test

```bash
./scripts/test.sh
```

## Voice profiles

See `src/supervoice/voice_profile/profiles.yaml`. V1 ships: hi-female, hi-male, en-female, en-male.

Select per-call via WebSocket query: `ws://host:8080/call?profile=hi-female`.
```

**Step 2: Scripts**

```bash
# supervoice/scripts/run.sh
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
uv run uvicorn supervoice.main:app --host 0.0.0.0 --port 8080 --reload
```

```bash
# supervoice/scripts/test.sh
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
uv run ruff check .
uv run pyrefly check
uv run pytest -v
```

```bash
chmod +x supervoice/scripts/*.sh
```

**Step 3: Commit**

```bash
git add supervoice/README.md supervoice/scripts/
git commit -m "docs(supervoice): README + run/test scripts"
```

---

### Task 25: Final quality gate

**Step 1: Lint**

Run: `cd supervoice && uv run ruff check . --fix`
Expected: no errors.

**Step 2: Format**

Run: `uv run ruff format .`
Expected: clean.

**Step 3: Type check**

Run: `uv run pyrefly init` (first time only)
Run: `uv run pyrefly check`
Expected: no errors. Fix any reported issues per CLAUDE.md guidelines.

**Step 4: Full test pass**

Run: `uv run pytest -v --tb=short`
Expected: all tests pass.

**Step 5: Manual E2E**

1. Start a mock Agent Bridge server (use `tests/fixtures/mock_bridge.py` as a standalone script — wrap with `asyncio.run`).
2. Run `./scripts/run.sh`.
3. Open Pipecat's WebRTC test client at `http://localhost:8080` (or use Pipecat's `small_webrtc.client_html`).
4. Connect with `?profile=en-female`.
5. Speak: "Hello, this is a test."
6. Expect: TTS reads back something containing your transcript (mock bridge echoes).

**Step 6: Commit**

```bash
git add -u
git commit -m "chore(supervoice): final lint + format pass for v1"
```

**End of Phase 3.** Supervoice v1 is shippable to one design partner.

---

## Post-v1 trigger points (not in scope)

Document these in a follow-up issue, do not implement:

1. **Smart-Turn ONNX latency > 200ms p99 measured** → port `TurnDetector` to Rust+PyO3 crate
2. **First on-prem deal demands single-binary** → same trigger
3. **Mid-call language switching demanded** → second STT lane + provider re-route logic
4. **SDK consumer (developer-facing)** → separate repo, consumes Agent Bridge protocol from `supervoice/src/supervoice/bridge/protocol.py`

## Reference skills

- @superpowers:executing-plans — for task-by-task execution
- @superpowers:test-driven-development — every task is TDD
- @superpowers:verification-before-completion — before each commit
- @python-development:python-testing-patterns — pytest-asyncio patterns
- @python-development:uv-package-manager — uv commands
